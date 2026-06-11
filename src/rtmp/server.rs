//! Ingest: accept an RTMP publish session from OBS and feed every audio,
//! video, and data message into the controller's writer side.
//!
//! Only one active publisher is meaningful at a time (one streamer). We
//! still accept many TCP connections but the controller will reject a
//! second `publish` until the first goes away.

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

pub async fn run(addr: String, ctrl: Arc<Controller>) -> io::Result<()> {
    let listener = TcpListener::bind(&addr).await?;
    // Keep this listener out of a restart/self-update child (else the ingest
    // port stays bound after we exit and the new instance can't reclaim it).
    crate::self_update::dont_inherit(&listener);
    eprintln!("[ingest] listening on {}", addr);
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
            if let Err(e) = handle(sock, ctrl).await {
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

async fn handle(mut sock: tokio::net::TcpStream, ctrl: Arc<Controller>) -> io::Result<()> {
    handshake::perform_server(&mut sock).await?;

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
        match msg.type_id {
            20 /* AMF0 command */ => {
                handle_command(&mut writer, &ctrl, &msg, &mut guard).await?;
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
            match ctrl.begin_publish(&stream_key).await {
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
