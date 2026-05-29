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
        let mut sock = TcpStream::connect((url.host.as_str(), url.port)).await?;
        sock.set_nodelay(true)?;
        // Aggressive TCP keepalive (Windows-only — best-effort no-op on
        // other platforms). Twitch's edge sometimes silently drops idle
        // sessions; without keepalive probes we'd not notice until the
        // first frame post-pause failed with EPIPE. 30 s probe + 10 s
        // retry interval gives us reliable disconnect detection well
        // before the user's viewers see a frozen stream.
        let _ = crate::rtmp::tcp::set_aggressive_keepalive(&sock);
        handshake::perform_client(&mut sock).await?;

        let (rd, wr) = split(sock);
        let mut reader = ChunkReader::new(rd);
        let mut writer = ChunkWriter::new(wr);
        writer.send_set_chunk_size(4096).await?;

        // --- connect ---
        let mut props = HashMap::new();
        props.insert("app".to_string(), Amf0::String(url.app.clone()));
        props.insert("type".to_string(), Amf0::String("nonprivate".into()));
        props.insert(
            "flashVer".to_string(),
            Amf0::String("FMLE/3.0 (compatible; InstantClone)".into()),
        );
        props.insert("tcUrl".to_string(), Amf0::String(url.tc_url.clone()));
        let mut buf = BytesMut::new();
        amf0::enc_string(&mut buf, "connect");
        amf0::enc_number(&mut buf, 1.0);
        amf0::enc_value(&mut buf, &Amf0::Object(props));
        writer.write_message(3, 0, 20, 0, &buf).await?;
        writer.flush().await?;
        await_command_status(&mut reader, "_result").await?;

        // --- releaseStream ---
        send_cmd(
            &mut writer,
            "releaseStream",
            2.0,
            &[Amf0::String(url.stream_key.clone())],
        )
        .await?;
        // --- FCPublish ---
        send_cmd(
            &mut writer,
            "FCPublish",
            3.0,
            &[Amf0::String(url.stream_key.clone())],
        )
        .await?;

        // --- createStream ---
        send_cmd(&mut writer, "createStream", 4.0, &[]).await?;
        let create_resp = await_command_status(&mut reader, "_result").await?;
        let stream_id = create_resp.get(3).and_then(|v| v.as_f64()).unwrap_or(1.0) as u32;

        // --- publish ---
        let mut buf = BytesMut::new();
        amf0::enc_string(&mut buf, "publish");
        amf0::enc_number(&mut buf, 5.0);
        amf0::enc_null(&mut buf);
        amf0::enc_string(&mut buf, &url.stream_key);
        amf0::enc_string(&mut buf, "live");
        writer.write_message(4, 0, 20, stream_id, &buf).await?;
        writer.flush().await?;
        await_command_status(&mut reader, "onStatus").await?;

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
        let mut buf = BytesMut::new();
        amf0::enc_string(&mut buf, "deleteStream");
        amf0::enc_number(&mut buf, 0.0);
        amf0::enc_null(&mut buf);
        amf0::enc_number(&mut buf, self.stream_id as f64);
        self.writer.write_message(3, 0, 20, 0, &buf).await?;
        self.writer.flush().await
    }
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
