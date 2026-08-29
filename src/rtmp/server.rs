//! Ingest: accept an RTMP publish session from OBS and feed every audio,
//! video, and data message into the controller's writer side.
//!
//! Only one active publisher is meaningful at a time (one streamer). We
//! still accept many TCP connections but the controller will reject a
//! second `publish` until the first goes away. That guard is what makes
//! the IPv4 and IPv6 listeners safe to run side by side: whichever one
//! the encoder happens to arrive on takes the slot, and the other is
//! refused exactly as a second OBS on the same listener would be.

use crate::controller::Controller;
use crate::h264;
use crate::rtmp::amf0::{self, Amf0};
use crate::rtmp::chunk::{ChunkReader, ChunkWriter, Message};
use crate::rtmp::handshake;
use bytes::BytesMut;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use tokio::io::split;
use tokio::net::TcpListener;

/// Bind one ingest address. Kept separate from `serve` so the supervisor
/// can tell a bind failure (permanent for the IPv6 leg on a machine with
/// IPv6 disabled) from a serve failure (worth retrying), instead of
/// respawning a hopeless listener once a second forever.
///
/// IPv6 addresses go through `TcpSocket` rather than `TcpListener::bind`
/// so `IPV6_V6ONLY` can be set before the bind - see `tcp::set_v6_only`
/// for why the two sockets must not overlap.
pub async fn bind(addr: &str) -> io::Result<TcpListener> {
    let listener = if addr.starts_with('[') {
        bind_v6_only(addr)?
    } else {
        TcpListener::bind(addr).await?
    };
    // Keep this listener out of a restart/self-update child (else the ingest
    // port stays bound after we exit and the new instance can't reclaim it).
    crate::self_update::dont_inherit(&listener);
    eprintln!("[ingest] listening on {}", addr);
    Ok(listener)
}

/// Bind a `[::1]` / `[::]` address with `IPV6_V6ONLY` forced on.
///
/// `SO_REUSEADDR` mirrors what `TcpListener::bind` does per platform (std
/// sets it on unix, not on Windows), so a hot-rebind behaves the same on
/// both legs. Anything else would let the IPv6 leg fail to reclaim the
/// port on a restart where IPv4 succeeded.
fn bind_v6_only(addr: &str) -> io::Result<TcpListener> {
    let parsed: std::net::SocketAddr = addr.parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a socket address: {addr}"),
        )
    })?;
    let sock = tokio::net::TcpSocket::new_v6()?;
    crate::rtmp::tcp::set_v6_only(&sock)?;
    #[cfg(unix)]
    sock.set_reuseaddr(true)?;
    sock.bind(parsed)?;
    // Matches the backlog tokio uses inside `TcpListener::bind`.
    sock.listen(1024)
}

pub async fn serve(listener: TcpListener, ctrl: Arc<Controller>) -> io::Result<()> {
    loop {
        let (sock, peer) = listener.accept().await?;
        sock.set_nodelay(true)?;
        // Mirror egress: aggressive TCP keepalive so a hung OBS process
        // (no clean FIN) is detected within ~30 s instead of riding the
        // OS default (~2 h on Windows). Without this, the dashboard
        // reports "OBS alive" indefinitely after a crash.
        let _ = crate::rtmp::tcp::set_aggressive_keepalive(&sock);
        let ctrl = ctrl.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(sock, ctrl, peer).await {
                eprintln!("[ingest] {} closed: {}", peer, e);
            }
        });
    }
}

/// RAII guard: while held, this publisher owns the `ingest_alive` flag.
/// Drop releases it so the dashboard correctly reflects the OBS state
/// regardless of how `handle` returns (clean EOF, network error, panic).
struct PublishGuard {
    ctrl: Arc<Controller>,
    active: bool,
}
impl Drop for PublishGuard {
    fn drop(&mut self) {
        if self.active {
            self.ctrl.mark_ingest_dead();
        }
    }
}

async fn handle(
    mut sock: tokio::net::TcpStream,
    ctrl: Arc<Controller>,
    peer: std::net::SocketAddr,
) -> io::Result<()> {
    handshake::perform_server(&mut sock).await?;

    // Keys the ingest-key rate limiter; the port pre-flight already resolved a
    // stable bind, so the connecting peer's IP is the right client identity.
    let peer_ip = peer.ip().to_string();

    let (rd, wr) = split(sock);
    let mut reader = ChunkReader::new(rd);
    let mut writer = ChunkWriter::new(wr);

    // Negotiate a larger chunk size up-front; 4096 is the de-facto standard.
    writer.send_set_chunk_size(4096).await?;

    let mut guard = PublishGuard {
        ctrl: ctrl.clone(),
        active: false,
    };

    loop {
        let msg = reader.read_message().await?;
        // librtmp's window-ack rule: fire BYTES_READ_REPORT once we've
        // received more than `window_ack_size/10` bytes since our last
        // ack. Strict RTMP relays (nginx-rtmp under tight config, SRS,
        // some CDN ingest edges) drop the publish if this is missing -
        // OBS streams keep working because OBS doesn't gate its send
        // on receiving acks, but a downstream relay re-publishing
        // through us would. Free side-effect: gives the publisher a
        // healthy flow-control signal even when our buffer drains
        // unevenly.
        if let Some(seq) = reader.take_pending_ack() {
            writer.send_ack(seq).await?;
            writer.flush().await?;
        }
        // Media and metadata only count from a connection that actually
        // completed `publish`. The stream key - and with it the ingest key -
        // is checked in `begin_publish` and nowhere else, so dispatching a
        // tag before that means a peer can skip the command entirely and
        // still land frames in the ring: its timestamps interleave with the
        // real publisher's on a different origin, and its onMetaData replaces
        // the cached one that is replayed on every cut and reconnect. A
        // conforming client always publishes first, so this costs nothing.
        if ignore_before_publish(guard.active, msg.type_id) {
            ctrl.log(format!(
                "[ingest] ignored a type-{} message from {peer_ip} that never published",
                msg.type_id
            ));
            continue;
        }
        match msg.type_id {
            20 /* AMF0 command */ => {
                handle_command(&mut writer, &ctrl, &msg, &mut guard, &peer_ip).await?;
            }
            18 /* AMF0 data - onMetaData et al */ => {
                ctrl.on_metadata(msg.payload.to_vec());
            }
            8 /* audio */ => {
                let info = h264::classify_audio_tag(&msg.payload);
                ctrl.note_audio_codec(info.codec);
                if info.is_multitrack { ctrl.note_multitrack_audio(); }
                // Audio multi-track (VOD audio) is forwarded bit-faithfully
                // - Twitch consumes the second track for VOD audio.
                ctrl.on_tag(8, msg.timestamp, &msg.payload, false, info.is_seq_header);
            }
            9 /* video */ => {
                let info = h264::classify_video_tag(&msg.payload);
                ctrl.note_video_codec(info.codec);
                if info.is_metadata {
                    // Enhanced-RTMP PacketTypeMetadata (=4) - typically a
                    // mid-stream HDR `colorInfo` update. Surface in the
                    // wire trace so a future investigation into stale
                    // colour rendering on a destination has a thread to
                    // pull on. We still forward it bit-faithfully below.
                    crate::trace::log(
                        "ENHANCED_METADATA",
                        &format!("codec={} bytes={}", info.codec.label(), msg.payload.len()),
                    );
                }
                if info.is_multitrack {
                    ctrl.note_multitrack_video();
                }
                // Store the raw payload - including any Enhanced Broadcasting
                // multi-track wrapper - and let each egress pump decide what
                // to do with it. Twitch destinations pass the multi-track tag
                // through bit-faithfully (it's what unlocks the transcoded
                // ladder for non-Affiliate accounts via simulcast). Every
                // other platform doesn't support multi-track video and gets
                // a single-track flatten applied just before sending. The
                // IDR / seq-header flags are taken from the multi-track tag's
                // outer header (FrameType for IDR, inner PacketType for seq
                // header) - both spec-required to be track-aligned, so the
                // outer-header signal is correct for cut detection regardless
                // of which destination's flatten path the bytes end up on.
                ctrl.on_tag(9, msg.timestamp, &msg.payload, info.is_idr, info.is_seq_header);
            }
            4 if msg.payload.len() >= 6
                && u16::from_be_bytes([msg.payload[0], msg.payload[1]]) == 6 =>
            {
                // User Control Message, event type 6 = Ping Request.
                // Layout: u16 event type + 4-byte sender timestamp.
                // OBS pings us periodically as its server to verify
                // we're consuming its publish; we echo the timestamp
                // back as event type 7 (Ping Response). Without this,
                // OBS's TCP keepalive would eventually fire, but the
                // explicit RTMP-layer reply is what OBS expects and
                // matches every other RTMP server's behaviour.
                let ts = u32::from_be_bytes([
                    msg.payload[2], msg.payload[3], msg.payload[4], msg.payload[5],
                ]);
                use bytes::{BufMut, BytesMut};
                let mut buf = BytesMut::with_capacity(6);
                buf.put_u16(7); // Ping Response
                buf.put_u32(ts);
                // User Control Messages go on CSID 2, msg type 4.
                let _ = writer.write_message(2, 0, 4, 0, &buf).await;
                let _ = writer.flush().await;
            }
            _ => { /* ignore other message types (incl. non-ping user-control) */ }
        }
    }
}

async fn handle_command<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut ChunkWriter<W>,
    ctrl: &Arc<Controller>,
    msg: &Message,
    guard: &mut PublishGuard,
    peer_ip: &str,
) -> io::Result<()> {
    let values = amf0::decode_all(&msg.payload)?;
    let name = values
        .first()
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let txn_id = values.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);

    match name.as_str() {
        "connect" => {
            // Acknowledge bandwidth + Window ack size (some clients require this).
            send_window_ack_size(writer, 5_000_000).await?;
            send_set_peer_bandwidth(writer, 5_000_000, 2).await?;
            // _result with capabilities/version etc.
            let mut props = HashMap::new();
            props.insert("fmsVer".to_string(), Amf0::String("FMS/3,0,1,123".into()));
            props.insert("capabilities".to_string(), Amf0::Number(31.0));
            let mut info = HashMap::new();
            info.insert("level".to_string(), Amf0::String("status".into()));
            info.insert(
                "code".to_string(),
                Amf0::String("NetConnection.Connect.Success".into()),
            );
            info.insert(
                "description".to_string(),
                Amf0::String("Connection succeeded.".into()),
            );
            info.insert("objectEncoding".to_string(), Amf0::Number(0.0));
            send_command_result(writer, txn_id, Amf0::Object(props), Amf0::Object(info)).await?;
        }
        "releaseStream" | "FCPublish" | "FCUnpublish" | "deleteStream" => {
            // No-op acks. Most clients don't care about the response body.
            send_simple_result(writer, txn_id).await?;
        }
        "createStream" => {
            // Stream id 1. We always use the same.
            let mut buf = BytesMut::new();
            amf0::enc_string(&mut buf, "_result");
            amf0::enc_number(&mut buf, txn_id);
            amf0::enc_null(&mut buf);
            amf0::enc_number(&mut buf, 1.0);
            writer.write_message(3, 0, 20, 0, &buf).await?;
            writer.flush().await?;
        }
        "publish" => {
            let stream_key = values
                .get(3)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            match ctrl.begin_publish(&stream_key, peer_ip).await {
                Ok(_token) => {
                    guard.active = true;
                    // onStatus NetStream.Publish.Start
                    let mut info = HashMap::new();
                    info.insert("level".to_string(), Amf0::String("status".into()));
                    info.insert(
                        "code".to_string(),
                        Amf0::String("NetStream.Publish.Start".into()),
                    );
                    info.insert(
                        "description".to_string(),
                        Amf0::String("Start publishing".into()),
                    );
                    let mut buf = BytesMut::new();
                    amf0::enc_string(&mut buf, "onStatus");
                    amf0::enc_number(&mut buf, 0.0);
                    amf0::enc_null(&mut buf);
                    amf0::enc_value(&mut buf, &Amf0::Object(info));
                    writer.write_message(5, 0, 20, msg.stream_id, &buf).await?;
                    writer.flush().await?;
                }
                Err(e) => {
                    // Tell the client the slot is taken so it doesn't sit
                    // there hopefully sending video into nothing.
                    let mut info = HashMap::new();
                    info.insert("level".to_string(), Amf0::String("error".into()));
                    info.insert(
                        "code".to_string(),
                        Amf0::String("NetStream.Publish.BadName".into()),
                    );
                    info.insert("description".to_string(), Amf0::String(e.to_string()));
                    let mut buf = BytesMut::new();
                    amf0::enc_string(&mut buf, "onStatus");
                    amf0::enc_number(&mut buf, 0.0);
                    amf0::enc_null(&mut buf);
                    amf0::enc_value(&mut buf, &Amf0::Object(info));
                    let _ = writer.write_message(5, 0, 20, msg.stream_id, &buf).await;
                    let _ = writer.flush().await;
                    return Err(e);
                }
            }
        }
        _ => {
            // Unknown command - silently ack so the client doesn't error.
            if txn_id != 0.0 {
                send_simple_result(writer, txn_id).await?;
            }
        }
    }
    Ok(())
}

async fn send_command_result<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut ChunkWriter<W>,
    txn_id: f64,
    props: Amf0,
    info: Amf0,
) -> io::Result<()> {
    let mut buf = BytesMut::new();
    amf0::enc_string(&mut buf, "_result");
    amf0::enc_number(&mut buf, txn_id);
    amf0::enc_value(&mut buf, &props);
    amf0::enc_value(&mut buf, &info);
    writer.write_message(3, 0, 20, 0, &buf).await?;
    writer.flush().await
}

async fn send_simple_result<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut ChunkWriter<W>,
    txn_id: f64,
) -> io::Result<()> {
    let mut buf = BytesMut::new();
    amf0::enc_string(&mut buf, "_result");
    amf0::enc_number(&mut buf, txn_id);
    amf0::enc_null(&mut buf);
    amf0::enc_null(&mut buf);
    writer.write_message(3, 0, 20, 0, &buf).await?;
    writer.flush().await
}

async fn send_window_ack_size<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut ChunkWriter<W>,
    size: u32,
) -> io::Result<()> {
    writer.write_message(2, 0, 5, 0, &size.to_be_bytes()).await
}

async fn send_set_peer_bandwidth<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut ChunkWriter<W>,
    size: u32,
    limit_type: u8,
) -> io::Result<()> {
    let mut buf = [0u8; 5];
    buf[..4].copy_from_slice(&size.to_be_bytes());
    buf[4] = limit_type;
    writer.write_message(2, 0, 6, 0, &buf).await
}

/// Whether a message must be dropped because this connection never
/// completed `publish`.
///
/// Audio, video and metadata are the three that carry a stream into the
/// ring. Commands are exempt - `publish` itself is one, so gating those
/// would make the state unreachable.
fn ignore_before_publish(published: bool, type_id: u8) -> bool {
    !published && matches!(type_id, 8 | 9 | 18)
}

#[cfg(test)]
mod tests {

    /// The stream key - and the ingest key with it - is checked inside
    /// `begin_publish` and nowhere else. A peer that connects and then sends
    /// video without ever publishing would otherwise land tags in the ring
    /// on its own timeline, and replace the cached onMetaData that is
    /// replayed on every cut and reconnect, with nothing in the log to say
    /// a second publisher existed.
    #[test]
    fn media_is_ignored_until_the_connection_has_published() {
        for type_id in [8u8, 9, 18] {
            assert!(
                ignore_before_publish(false, type_id),
                "type {type_id} must not reach the controller before publish"
            );
            assert!(
                !ignore_before_publish(true, type_id),
                "type {type_id} must flow once published"
            );
        }
        // Commands are how a connection publishes in the first place.
        for type_id in [20u8, 17, 4, 5, 6] {
            assert!(
                !ignore_before_publish(false, type_id),
                "type {type_id} is not media and must still be handled"
            );
        }
    }
    use super::*;

    /// A machine or container with IPv6 switched off cannot run these.
    /// The product degrades to IPv4 there by design (`supervise_ingest_leg`
    /// drops the optional leg), so skipping is the honest outcome rather
    /// than a red suite on a Docker image without IPv6.
    async fn bind_v6_or_skip(addr: &str) -> Option<TcpListener> {
        match bind(addr).await {
            Ok(l) => Some(l),
            Err(e) => {
                eprintln!("skipping IPv6 test, {addr} is unbindable here: {e}");
                None
            }
        }
    }

    /// The wildcard legs have to hold one port simultaneously. Without
    /// `IPV6_V6ONLY` this passes on Windows (the option defaults on) and
    /// fails on Linux with EADDRINUSE, because `net.ipv6.bindv6only`
    /// defaults off there and `[::]` swallows v4-mapped addresses. This is
    /// the test that keeps the two-socket layout portable.
    #[tokio::test]
    async fn wildcard_v4_and_v6_listeners_share_one_port() {
        let v4 = bind("0.0.0.0:0").await.expect("IPv4 wildcard bind");
        let port = v4.local_addr().unwrap().port();
        let Some(v6) = bind_v6_or_skip(&format!("[::]:{port}")).await else {
            return;
        };
        assert!(v6.local_addr().unwrap().is_ipv6());
    }

    /// The default layout, plus the property `Controller::begin_publish`
    /// depends on: a v6-only socket reports a local peer as `::1`, which
    /// `is_loopback` accepts. A dual-stack socket would hand us
    /// `::ffff:127.0.0.1` instead, where `is_loopback` is false and a
    /// publisher sitting at the machine would be rate-limited as remote.
    #[tokio::test]
    async fn ipv6_loopback_listener_accepts_and_reports_a_loopback_peer() {
        let Some(v6) = bind_v6_or_skip("[::1]:0").await else {
            return;
        };
        let addr = v6.local_addr().unwrap();
        assert!(addr.is_ipv6());

        let client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect over IPv6");
        let (accepted, peer) = v6.accept().await.expect("accept");
        assert!(
            peer.ip().is_loopback(),
            "peer must classify as loopback, got {peer}"
        );
        drop((client, accepted));
    }

    /// A bad address must surface as an error the supervisor can act on,
    /// not a panic inside the bind path.
    #[tokio::test]
    async fn malformed_ipv6_address_is_an_error_not_a_panic() {
        assert!(bind("[not-an-address]:1935").await.is_err());
    }
}
