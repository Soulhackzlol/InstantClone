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
use std::time::Duration;
use tokio::io::split;
use tokio::net::TcpListener;

/// How long a new connection has to finish the RTMP handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a connection that hasn't published may go without a complete
/// message. OBS publishes within a second of connecting; this only frees
/// sockets that connect and then sit. A publishing connection has no such
/// limit: a frozen OBS is crash protection's to handle.
const UNPUBLISHED_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Largest AMF0 command accepted. OBS's connect / publish commands are a
/// few hundred bytes. A 16 MB command of 1-byte values would decode into
/// millions of values (~56 bytes each in memory) before any stream key is
/// checked.
const MAX_COMMAND_BYTES: usize = 64 * 1024;

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
    /// The publisher token `begin_publish` gave this connection.
    token: u64,
}
impl PublishGuard {
    /// This connection published and is still the current publisher (a
    /// newer one takes over from a hung OBS, see `Controller::begin_publish`).
    fn is_current(&self) -> bool {
        self.active && self.ctrl.publisher_token() == self.token
    }
}
impl Drop for PublishGuard {
    fn drop(&mut self) {
        if self.active {
            self.ctrl.end_publish(self.token);
        }
    }
}

async fn handle(
    mut sock: tokio::net::TcpStream,
    ctrl: Arc<Controller>,
    peer: std::net::SocketAddr,
) -> io::Result<()> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake::perform_server(&mut sock))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "handshake timed out"))??;

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
        token: 0,
    };

    // One warning per connection for media sent without publishing.
    let mut warned_unpublished = false;
    loop {
        let msg = if guard.active {
            reader.read_message().await?
        } else {
            tokio::time::timeout(UNPUBLISHED_IDLE_TIMEOUT, reader.read_message())
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "idle without publishing"))??
        };
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
        // A newer publisher took over from this one (it had stopped sending
        // video): nothing more it sends may reach the buffer.
        if guard.active && !guard.is_current() {
            return Err(io::Error::other("replaced by a newer publisher"));
        }
        if ignore_before_publish(guard.active, msg.type_id) {
            // Once per connection, not once per message. Reaching this needs
            // only TCP plus the handshake, and a peer can manufacture a
            // complete message per wire byte - which would evict the whole
            // bounded log ring at line rate, destroying the very evidence
            // someone would use to work out what was happening.
            if !warned_unpublished {
                warned_unpublished = true;
                ctrl.log(format!(
                    "[ingest] {peer_ip} is sending media without publishing; ignoring it"
                ));
            }
            continue;
        }
        match msg.type_id {
            20 /* AMF0 command */ => {
                if msg.payload.len() > MAX_COMMAND_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "oversized AMF0 command",
                    ));
                }
                handle_command(&mut writer, &ctrl, &msg, &mut guard, &peer_ip).await?;
            }
            18 /* AMF0 data - onMetaData et al */ => {
                ctrl.on_metadata(msg.payload.to_vec());
            }
            8 /* audio */ => {
                let info = h264::classify_audio_tag(&msg.payload);
                ctrl.note_audio_codec(info.codec);
                if info.is_multitrack { ctrl.note_multitrack_audio(&msg.payload); }
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
        "releaseStream" | "FCPublish" => {
            // No-op acks. Most clients don't care about the response body.
            send_simple_result(writer, txn_id).await?;
        }
        "FCUnpublish" | "deleteStream" => {
            // OBS sends both from RTMP_Close on every deliberate stop, and
            // never on a crash. Remember it so the disconnect that follows
            // counts as a stop (crash protection stays out of the way).
            if guard.is_current() {
                ctrl.note_unpublish();
            }
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
                Ok(token) => {
                    guard.active = true;
                    guard.token = token;
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
    use super::*;
    use crate::buffer::DiskRing;
    use crate::rtmp::client::{EgressClient, EgressUrl};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

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

    static UNIQ: AtomicU32 = AtomicU32::new(0);

    /// A real ingest listener on a loopback port, serving a Controller over
    /// its own temp ring. Stops the listener and deletes the ring on drop.
    struct Ingest {
        ctrl: Option<Arc<Controller>>,
        port: u16,
        path: std::path::PathBuf,
        server: tokio::task::JoinHandle<io::Result<()>>,
    }

    impl Ingest {
        fn ctrl(&self) -> &Arc<Controller> {
            self.ctrl.as_ref().expect("alive until drop")
        }
    }

    impl Drop for Ingest {
        fn drop(&mut self) {
            self.server.abort();
            // Best-effort: a connection task still winding down may hold
            // the ring open a moment longer, which Windows won't delete.
            drop(self.ctrl.take());
            let _ = std::fs::remove_file(&self.path);
        }
    }

    async fn ingest() -> Ingest {
        let n = UNIQ.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("ic-test-ingest-{}-{}.buf", std::process::id(), n));
        let _ = std::fs::remove_file(&path);
        let ring = Arc::new(DiskRing::create(&path, 1 << 20).expect("ring create"));
        let ctrl = Arc::new(Controller::new(ring, 0));
        let listener = bind("127.0.0.1:0").await.expect("loopback bind");
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(serve(listener, ctrl.clone()));
        Ingest {
            ctrl: Some(ctrl),
            port,
            path,
            server,
        }
    }

    /// Poll `done` until it holds, failing the test after 5 s. The ingest
    /// runs on its own task, so its effects land shortly after the bytes.
    async fn eventually(what: &str, done: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn read_tag(ctrl: &Controller, seq: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        ctrl.ring
            .try_read_seq(seq, &mut buf)
            .expect("no io error")
            .expect("tag present");
        buf
    }

    /// End to end through `serve` and `handle`, driven by the crate's own
    /// egress client: handshake, connect, createStream, publish, then media.
    /// Every unit test of the pieces can pass while this wiring is broken
    /// (a reply the client rejects, a tag routed to the wrong kind), and
    /// this is the one that would notice.
    #[tokio::test]
    async fn a_publisher_reaches_the_ring_through_the_real_ingest_path() {
        let ingest = ingest().await;
        let ctrl = ingest.ctrl().clone();
        let url = EgressUrl::parse(&format!("rtmp://127.0.0.1:{}/live/key", ingest.port)).unwrap();
        let client = tokio::time::timeout(Duration::from_secs(5), EgressClient::connect(&url))
            .await
            .expect("handshake and publish finish")
            .expect("the ingest accepts the publish");
        assert!(ctrl.ingest_alive(), "publish claimed the slot");

        let mut sink = client.spawn_reader_drain();
        // Legacy AVC keyframe whose NAL walk finds an IDR, then AAC.
        let video = [0x17, 0x01, 0, 0, 0, 0, 0, 0, 5, 0x65, 1, 2, 3, 4];
        let audio = [0xAF, 0x01, 0x21, 0x10];
        sink.send_video(40, &video).await.unwrap();
        sink.send_audio(41, &audio).await.unwrap();
        sink.flush().await.unwrap();

        eventually("both tags indexed", || ctrl.ring.latest_seq() == Some(1)).await;
        assert_eq!(read_tag(&ctrl, 0), video);
        assert_eq!(read_tag(&ctrl, 1), audio);
        let idr = ctrl.ring.newest_idr().expect("the keyframe is a cut point");
        assert_eq!((idr.seq, idr.ts_ms, idr.kind), (0, 40, 9));

        // Closing the connection releases the publisher slot.
        drop(sink);
        eventually("slot released", || !ctrl.ingest_alive()).await;
    }

    /// The pre-publish gate, through the real connection handler: a peer
    /// that sends a frame without publishing gets nothing into the ring.
    /// The `connect` sent afterwards is answered only once the frame before
    /// it was handled, so its `_result` proves the frame was seen and
    /// dropped rather than still in flight.
    #[tokio::test]
    async fn media_before_publish_never_reaches_the_ring() {
        let ingest = ingest().await;
        let ctrl = ingest.ctrl().clone();
        let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", ingest.port))
            .await
            .unwrap();
        handshake::perform_client(&mut sock).await.unwrap();
        let (rd, wr) = split(sock);
        let mut reader = ChunkReader::new(rd);
        let mut writer = ChunkWriter::new(wr);

        let frame = [0x17, 0x01, 0, 0, 0, 0, 0, 0, 5, 0x65, 1, 2, 3, 4];
        writer.write_message(6, 0, 9, 1, &frame).await.unwrap();
        let mut connect = BytesMut::new();
        amf0::enc_string(&mut connect, "connect");
        amf0::enc_number(&mut connect, 1.0);
        amf0::enc_object(&mut connect, &[("app", &Amf0::String("live".into()))]);
        writer.write_message(3, 0, 20, 0, &connect).await.unwrap();
        writer.flush().await.unwrap();

        let reply = tokio::time::timeout(Duration::from_secs(5), reader.read_message())
            .await
            .expect("the ingest answers connect")
            .unwrap();
        let values = amf0::decode_all(&reply.payload).unwrap();
        assert_eq!(values[0].as_str(), Some("_result"));
        assert_eq!(
            ctrl.ring.latest_seq(),
            None,
            "the unpublished frame was dropped"
        );
        assert!(!ctrl.ingest_alive());
    }

    /// Whether this machine can bind IPv6 at all. A machine or container
    /// with IPv6 switched off cannot run the IPv6 tests; the product
    /// degrades to IPv4 there by design (`supervise_ingest_leg` drops the
    /// optional leg), so skipping is the honest outcome rather than a red
    /// suite on a Docker image without IPv6.
    async fn ipv6_loopback_or_skip() -> Option<TcpListener> {
        match bind("[::1]:0").await {
            Ok(l) => Some(l),
            Err(e) => {
                eprintln!("skipping IPv6 test, [::1] is unbindable here: {e}");
                None
            }
        }
    }

    /// The wildcard legs have to hold one port simultaneously. Without
    /// `IPV6_V6ONLY` this passes on Windows (the option defaults on) and
    /// fails on Linux with EADDRINUSE, because `net.ipv6.bindv6only`
    /// defaults off there and `[::]` swallows v4-mapped addresses. This is
    /// the test that keeps the two-socket layout portable. IPv6 support is
    /// probed on a different address first, so that exact EADDRINUSE is a
    /// failure here and not mistaken for "no IPv6" and skipped.
    #[tokio::test]
    async fn wildcard_v4_and_v6_listeners_share_one_port() {
        if ipv6_loopback_or_skip().await.is_none() {
            return;
        }
        let v4 = bind("0.0.0.0:0").await.expect("IPv4 wildcard bind");
        let port = v4.local_addr().unwrap().port();
        let v6 = bind(&format!("[::]:{port}"))
            .await
            .expect("[::] must share the port IPv4 holds (is IPV6_V6ONLY set?)");
        assert!(v6.local_addr().unwrap().is_ipv6());
    }

    /// The default layout, plus the property `Controller::begin_publish`
    /// depends on: a v6-only socket reports a local peer as `::1`, which
    /// `is_loopback` accepts. A dual-stack socket would hand us
    /// `::ffff:127.0.0.1` instead, where `is_loopback` is false and a
    /// publisher sitting at the machine would be rate-limited as remote.
    #[tokio::test]
    async fn ipv6_loopback_listener_accepts_and_reports_a_loopback_peer() {
        let Some(v6) = ipv6_loopback_or_skip().await else {
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
