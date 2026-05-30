//! Outbound RTMP publisher: connect to Twitch/YouTube/Restream, negotiate
//! through `connect` → `releaseStream` → `FCPublish` → `createStream` →
//! `publish`, then expose a simple `send_audio`/`send_video`/`send_data`
//! API that the controller drives.
//!
//! URL form: rtmp://host[:port]/app/stream_key
//! Default port is 1935.

use crate::rtmp::amf0::{self, Amf0};
use crate::rtmp::chunk::{ChunkReader, ChunkWriter, Message};
use crate::rtmp::handshake;
use bytes::BytesMut;
use std::collections::HashMap;
use std::io;
use tokio::io::{split, ReadHalf, WriteHalf};
use tokio::net::TcpStream;

pub struct EgressUrl {
    pub host: String,
    pub port: u16,
    pub app: String,
    pub stream_key: String,
    pub tc_url: String,
}

impl EgressUrl {
    pub fn parse(url: &str) -> io::Result<Self> {
        let rest = url
            .strip_prefix("rtmp://")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an rtmp:// url"))?;
        let slash = rest
            .find('/')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing app path"))?;
        let host_part = &rest[..slash];
        let path = &rest[slash + 1..];
        let (host, port) = match host_part.find(':') {
            Some(c) => (
                host_part[..c].to_string(),
                host_part[c + 1..]
                    .parse()
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad port"))?,
            ),
            None => (host_part.to_string(), 1935u16),
        };
        let last_slash = path
            .rfind('/')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing stream key"))?;
        let app = path[..last_slash].to_string();
        let stream_key = path[last_slash + 1..].to_string();
        let tc_url = format!("rtmp://{}:{}/{}", host, port, app);
        Ok(Self {
            host,
            port,
            app,
            stream_key,
            tc_url,
        })
    }
}

pub struct EgressClient {
    reader: ChunkReader<ReadHalf<TcpStream>>,
    writer: ChunkWriter<WriteHalf<TcpStream>>,
    stream_id: u32,
}

impl EgressClient {
    pub async fn connect(url: &EgressUrl) -> io::Result<Self> {
        crate::trace::log(
            "EGRESS_DIAL",
            &format!("host={} port={} app={}", url.host, url.port, url.app),
        );
        let mut sock = TcpStream::connect((url.host.as_str(), url.port)).await?;
        sock.set_nodelay(true)?;
        // Aggressive TCP keepalive (Windows-only — best-effort no-op on
        // other platforms). Twitch's edge sometimes silently drops idle
        // sessions; without keepalive probes we'd not notice until the
        // first frame post-pause failed with EPIPE. 30 s probe + 10 s
        // retry interval gives us reliable disconnect detection well
        // before the user's viewers see a frozen stream.
        let _ = crate::rtmp::tcp::set_aggressive_keepalive(&sock);
        crate::trace::log("TCP_CONNECTED", &format!("host={}", url.host));
        handshake::perform_client(&mut sock).await?;
        crate::trace::log("RTMP_HANDSHAKE_OK", &format!("host={}", url.host));

        let (rd, wr) = split(sock);
        let mut reader = ChunkReader::new(rd);
        let mut writer = ChunkWriter::new(wr);
        writer.send_set_chunk_size(4096).await?;
        crate::trace::log("CHUNK_SIZE_SET", "size=4096");

        // --- connect ---
        // Match what OBS's libobs RTMP plugin (and librtmp underneath it)
        // sends. Two reasons this matters for Twitch specifically:
        //
        // 1. flashVer = "FMLE/3.0 (compatible; FMSc/1.0)" puts us in the
        //    same preferred-publisher lane OBS gets.
        // 2. The audioCodecs / videoCodecs bitmaps are how a publisher
        //    declares "I will send AAC + H.264" at the protocol level.
        //    Twitch's transcoder uses these flags to decide which
        //    ladder lane to allocate. WITHOUT them, Twitch sees a
        //    publisher that hasn't promised modern codecs and routes
        //    the stream to a conservative "Source-only" lane — which is
        //    exactly the symptom users were hitting at high bitrate.
        //    Bit values mirror librtmp's defaults exactly:
        //       audioCodecs    = 3191  (incl. SUPPORT_SND_AAC bit 10)
        //       videoCodecs    = 252   (incl. SUPPORT_VID_H264 bit 7)
        //       videoFunction  = 1     (SUPPORT_VID_CLIENT_SEEK)
        //       objectEncoding = 0     (AMF0)
        //       capabilities   = 239   (Flash player default)
        //       fpad           = false (no proxy in front of us)
        let mut props = HashMap::new();
        props.insert("app".to_string(), Amf0::String(url.app.clone()));
        props.insert("type".to_string(), Amf0::String("nonprivate".into()));
        props.insert(
            "flashVer".to_string(),
            Amf0::String("FMLE/3.0 (compatible; FMSc/1.0)".into()),
        );
        props.insert("tcUrl".to_string(), Amf0::String(url.tc_url.clone()));
        props.insert("fpad".to_string(), Amf0::Boolean(false));
        props.insert("capabilities".to_string(), Amf0::Number(239.0));
        props.insert("audioCodecs".to_string(), Amf0::Number(3191.0));
        props.insert("videoCodecs".to_string(), Amf0::Number(252.0));
        props.insert("videoFunction".to_string(), Amf0::Number(1.0));
        props.insert("objectEncoding".to_string(), Amf0::Number(0.0));
        let mut buf = BytesMut::new();
        amf0::enc_string(&mut buf, "connect");
        amf0::enc_number(&mut buf, 1.0);
        amf0::enc_value(&mut buf, &Amf0::Object(props));
        writer.write_message(3, 0, 20, 0, &buf).await?;
        writer.flush().await?;
        crate::trace::log(
            "AMF_OUT",
            &format!(
                "cmd=connect tcUrl={} flashVer=\"FMLE/3.0 (compatible; FMSc/1.0)\" \
                 audioCodecs=3191 videoCodecs=252 videoFunction=1 objectEncoding=0",
                url.tc_url
            ),
        );
        let connect_resp = await_command_status(&mut reader, "_result").await?;
        crate::trace::log(
            "AMF_IN",
            &format!(
                "cmd=_result(connect) details={}",
                summarize_status(&connect_resp)
            ),
        );

        // --- releaseStream ---
        send_cmd(
            &mut writer,
            "releaseStream",
            2.0,
            &[Amf0::String(url.stream_key.clone())],
        )
        .await?;
        crate::trace::log("AMF_OUT", "cmd=releaseStream key_redacted=true");
        // --- FCPublish ---
        send_cmd(
            &mut writer,
            "FCPublish",
            3.0,
            &[Amf0::String(url.stream_key.clone())],
        )
        .await?;
        crate::trace::log("AMF_OUT", "cmd=FCPublish key_redacted=true");

        // --- createStream ---
        send_cmd(&mut writer, "createStream", 4.0, &[]).await?;
        crate::trace::log("AMF_OUT", "cmd=createStream");
        let create_resp = await_command_status(&mut reader, "_result").await?;
        let stream_id = create_resp.get(3).and_then(|v| v.as_f64()).unwrap_or(1.0) as u32;
        crate::trace::log(
            "AMF_IN",
            &format!("cmd=_result(createStream) stream_id={}", stream_id),
        );

        // --- publish ---
        let mut buf = BytesMut::new();
        amf0::enc_string(&mut buf, "publish");
        amf0::enc_number(&mut buf, 5.0);
        amf0::enc_null(&mut buf);
        amf0::enc_string(&mut buf, &url.stream_key);
        amf0::enc_string(&mut buf, "live");
        writer.write_message(4, 0, 20, stream_id, &buf).await?;
        writer.flush().await?;
        crate::trace::log("AMF_OUT", "cmd=publish key_redacted=true type=live");
        let publish_resp = await_command_status(&mut reader, "onStatus").await?;
        crate::trace::log(
            "AMF_IN",
            &format!(
                "cmd=onStatus(publish) details={}",
                summarize_status(&publish_resp)
            ),
        );

        Ok(Self {
            reader,
            writer,
            stream_id,
        })
    }

    /// Spawn a background drain of server-to-client messages (acks, ping,
    /// onStatus). The resulting `EgressSink` owns an `AbortHandle` for
    /// that drain — when the sink is dropped (egress disconnects /
    /// reconnects), the drain task is aborted, preventing zombie tasks
    /// from accumulating on every reconnect.
    pub fn spawn_reader_drain(mut self) -> EgressSink {
        let drain = tokio::spawn(async move {
            loop {
                if self.reader.read_message().await.is_err() {
                    return;
                }
            }
        });
        EgressSink {
            writer: self.writer,
            stream_id: self.stream_id,
            _drain_abort: drain.abort_handle(),
        }
    }
}

pub struct EgressSink {
    writer: ChunkWriter<WriteHalf<TcpStream>>,
    pub stream_id: u32,
    // Aborts the paired drain task when the sink is dropped. Held only
    // for its Drop side-effect; never read.
    _drain_abort: tokio::task::AbortHandle,
}

impl Drop for EgressSink {
    fn drop(&mut self) {
        self._drain_abort.abort();
    }
}

impl EgressSink {
    pub async fn send_metadata(&mut self, payload: &[u8]) -> io::Result<()> {
        crate::trace::log(
            "METADATA_SENT",
            &format!(
                "bytes={} hex={}",
                payload.len(),
                crate::trace::hex_prefix(payload, 64)
            ),
        );
        // AMF0 data on CSID 5, message type 18, timestamp 0.
        self.writer
            .write_message(5, 0, 18, self.stream_id, payload)
            .await?;
        self.writer.flush().await
    }

    pub async fn send_audio(&mut self, timestamp: u32, payload: &[u8]) -> io::Result<()> {
        // CSID 6 conventionally for audio.
        self.writer
            .write_message(6, timestamp, 8, self.stream_id, payload)
            .await
    }

    pub async fn send_video(&mut self, timestamp: u32, payload: &[u8]) -> io::Result<()> {
        // CSID 7 conventionally for video.
        self.writer
            .write_message(7, timestamp, 9, self.stream_id, payload)
            .await
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        self.writer.flush().await
    }

    /// Send AMF0 `deleteStream` then flush. The Twitch session
    /// bookkeeping uses this to mark the publish as a clean shutdown
    /// instead of an unexpected disconnect (which it would otherwise
    /// log + potentially count against your account on flaky internet).
    /// Best-effort; errors are swallowed since we're tearing down anyway.
    pub async fn send_delete_stream(&mut self) -> io::Result<()> {
        crate::trace::log("AMF_OUT", "cmd=deleteStream");
        let mut buf = BytesMut::new();
        amf0::enc_string(&mut buf, "deleteStream");
        amf0::enc_number(&mut buf, 0.0);
        amf0::enc_null(&mut buf);
        amf0::enc_number(&mut buf, self.stream_id as f64);
        self.writer.write_message(3, 0, 20, 0, &buf).await?;
        self.writer.flush().await
    }
}

/// Convert an AMF0 onStatus / _result info object into a one-line summary
/// like `level=status code=NetConnection.Connect.Success description="..."`,
/// suitable for the trace log. Falls back to a count when the payload
/// isn't an object we can introspect.
fn summarize_status(vals: &[amf0::Amf0]) -> String {
    for v in vals {
        if let Some(o) = v.as_object() {
            let level = o.get("level").and_then(|x| x.as_str()).unwrap_or("?");
            let code = o.get("code").and_then(|x| x.as_str()).unwrap_or("?");
            let desc = o.get("description").and_then(|x| x.as_str()).unwrap_or("");
            return format!("level={} code={} description=\"{}\"", level, code, desc);
        }
    }
    format!("non-object response, {} values", vals.len())
}

async fn send_cmd<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut ChunkWriter<W>,
    name: &str,
    txn: f64,
    args: &[Amf0],
) -> io::Result<()> {
    let mut buf = BytesMut::new();
    amf0::enc_string(&mut buf, name);
    amf0::enc_number(&mut buf, txn);
    amf0::enc_null(&mut buf);
    for a in args {
        amf0::enc_value(&mut buf, a);
    }
    writer.write_message(3, 0, 20, 0, &buf).await?;
    writer.flush().await
}

async fn await_command_status<R: tokio::io::AsyncReadExt + Unpin>(
    reader: &mut ChunkReader<R>,
    expect: &str,
) -> io::Result<Vec<Amf0>> {
    loop {
        let msg: Message = reader.read_message().await?;
        if msg.type_id != 20 {
            continue;
        }
        let vals = amf0::decode_all(&msg.payload)?;
        let cmd = vals.first().and_then(|v| v.as_str());

        // Both `_error` and `onStatus { level: "error" }` indicate failure.
        // Surface them as io::Errors so the supervisor can log a useful
        // reason instead of silently looping forever waiting for `_result`.
        if cmd == Some("_error") {
            let desc = vals
                .iter()
                .find_map(|v| v.as_object())
                .and_then(|o| o.get("description").and_then(|v| v.as_str()))
                .unwrap_or("unspecified _error")
                .to_string();
            return Err(io::Error::other(format!("RTMP _error: {}", desc)));
        }
        if cmd == Some("onStatus") {
            if let Some(info) = vals.get(3).and_then(|v| v.as_object()) {
                let level = info.get("level").and_then(|v| v.as_str()).unwrap_or("");
                if level == "error" {
                    let desc = info
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("onStatus level=error")
                        .to_string();
                    let code = info.get("code").and_then(|v| v.as_str()).unwrap_or("");
                    return Err(io::Error::other(format!("RTMP {}: {}", code, desc)));
                }
            }
        }
        if cmd == Some(expect) {
            return Ok(vals);
        }
        // Anything else (Window Ack, onBWDone, onFCPublish, ping …) is
        // ignored and we keep reading until we see what we're waiting for.
    }
}
