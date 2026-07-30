//! Outbound RTMP publisher: connect to Twitch/YouTube/Restream, negotiate
//! through `connect` → `releaseStream` → `FCPublish` → `createStream` →
//! `publish`, then expose a simple `send_audio`/`send_video`/`send_data`
//! API that the controller drives.
//!
//! URL form: rtmp://host[:port]/app/stream_key (default port 1935), or
//! rtmps://host[:port]/app/stream_key for TLS (default port 443). Kick's
//! ingest is RTMPS-only, so the egress socket transparently upgrades to
//! TLS when the URL scheme is `rtmps://` - see `EgressStream`.

use crate::rtmp::amf0::{self, Amf0};
use crate::rtmp::chunk::{ChunkReader, ChunkWriter, Message};
use crate::rtmp::handshake;
use bytes::BytesMut;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{split, AsyncRead, AsyncWrite, ReadBuf, ReadHalf, WriteHalf};
use tokio::net::TcpStream;

pub struct EgressUrl {
    pub host: String,
    pub port: u16,
    pub app: String,
    pub stream_key: String,
    pub tc_url: String,
    /// True when the URL scheme was `rtmps://` - the socket must be
    /// wrapped in TLS before the RTMP handshake. Kick requires this.
    pub tls: bool,
}

impl EgressUrl {
    pub fn parse(url: &str) -> io::Result<Self> {
        // Accept both schemes. Default port follows the scheme: 1935 for
        // plain RTMP, 443 for RTMPS (the port Kick and other TLS ingests
        // publish on). An explicit `:port` in the URL always wins.
        let (tls, rest, default_port) = if let Some(r) = url.strip_prefix("rtmps://") {
            (true, r, 443u16)
        } else if let Some(r) = url.strip_prefix("rtmp://") {
            (false, r, 1935u16)
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not an rtmp:// or rtmps:// url",
            ));
        };
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
            None => (host_part.to_string(), default_port),
        };
        let last_slash = path
            .rfind('/')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing stream key"))?;
        let app = path[..last_slash].to_string();
        let stream_key = path[last_slash + 1..].to_string();
        let scheme = if tls { "rtmps" } else { "rtmp" };
        let tc_url = format!("{}://{}:{}/{}", scheme, host, port, app);
        Ok(Self {
            host,
            port,
            app,
            stream_key,
            tc_url,
            tls,
        })
    }
}

/// Egress socket that is either a plain TCP stream or a TLS stream over
/// TCP. One concrete type keeps `EgressClient` / `EgressSink` free of a
/// generic IO parameter (so the controller's egress supervisor never has
/// to name it), while still letting a single destination pick RTMP or
/// RTMPS at connect time. Both variants are `Unpin`, so the delegating
/// poll impls below can use `Pin::new` on the inner stream directly. The
/// TLS variant is boxed because a `TlsStream` is far larger than a bare
/// `TcpStream` - boxing keeps the enum small (and clippy quiet) so the
/// common plain-RTMP path doesn't carry the TLS struct's footprint.
pub enum EgressStream {
    Plain(TcpStream),
    Tls(Box<tokio_native_tls::TlsStream<TcpStream>>),
}

impl AsyncRead for EgressStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            EgressStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            EgressStream::Tls(s) => Pin::new(&mut **s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for EgressStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            EgressStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            EgressStream::Tls(s) => Pin::new(&mut **s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            EgressStream::Plain(s) => Pin::new(s).poll_flush(cx),
            EgressStream::Tls(s) => Pin::new(&mut **s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            EgressStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            EgressStream::Tls(s) => Pin::new(&mut **s).poll_shutdown(cx),
        }
    }
}

pub struct EgressClient {
    reader: ChunkReader<ReadHalf<EgressStream>>,
    writer: ChunkWriter<WriteHalf<EgressStream>>,
    stream_id: u32,
}

impl EgressClient {
    pub async fn connect(url: &EgressUrl) -> io::Result<Self> {
        crate::trace::log(
            "EGRESS_DIAL",
            &format!("host={} port={} app={}", url.host, url.port, url.app),
        );
        let tcp = TcpStream::connect((url.host.as_str(), url.port)).await?;
        tcp.set_nodelay(true)?;
        // Aggressive TCP keepalive (Windows-only - best-effort no-op on
        // other platforms). Twitch's edge sometimes silently drops idle
        // sessions; without keepalive probes we'd not notice until the
        // first frame post-pause failed with EPIPE. 30 s probe + 10 s
        // retry interval gives us reliable disconnect detection well
        // before the user's viewers see a frozen stream. Set on the raw
        // TCP socket before any TLS wrap - the options live on the fd.
        let _ = crate::rtmp::tcp::set_aggressive_keepalive(&tcp);
        // Larger SO_SNDBUF for high-bitrate streamers. Windows default is
        // ~64 KB which is enough at ≤ 6 Mbps but starts blocking writes
        // at 10+ Mbps when the egress receiver (Twitch / YouTube) is
        // momentarily slow to ACK. 1 MB holds ~800 ms of stream at
        // 10 Mbps - plenty of slack without pinning huge per-connection
        // memory. Best-effort: errors are logged-only because not every
        // platform / firewall stack honours setsockopt(SO_SNDBUF).
        let _ = crate::rtmp::tcp::set_send_buffer(&tcp, 1024 * 1024);
        crate::trace::log(
            "TCP_CONNECTED",
            &format!("host={} tls={}", url.host, url.tls),
        );
        // Upgrade to TLS for rtmps:// endpoints (Kick). SNI uses the
        // hostname from the URL. Reuses the same native-tls (schannel on
        // Windows) backend already linked for the HTTPS webhook client.
        let mut sock = if url.tls {
            let connector = native_tls::TlsConnector::new()
                .map_err(|e| io::Error::other(format!("TLS init failed: {e}")))?;
            let connector = tokio_native_tls::TlsConnector::from(connector);
            let tls = connector
                .connect(url.host.as_str(), tcp)
                .await
                .map_err(|e| io::Error::other(format!("TLS handshake failed: {e}")))?;
            crate::trace::log("TLS_HANDSHAKE_OK", &format!("host={}", url.host));
            EgressStream::Tls(Box::new(tls))
        } else {
            EgressStream::Plain(tcp)
        };
        handshake::perform_client(&mut sock).await?;
        crate::trace::log("RTMP_HANDSHAKE_OK", &format!("host={}", url.host));

        let (rd, wr) = split(sock);
        let mut reader = ChunkReader::new(rd);
        let mut writer = ChunkWriter::new(wr);
        // Larger chunk size (~60 KB) matches what OBS / librtmp use by
        // default for high-bitrate publishers. At 10 Mbps and a 4 KB
        // chunk size we'd emit ~305 chunk-stream headers per second; at
        // 60 KB it's ~20/sec. The protocol overhead saving is tiny
        // (couple hundred bytes/sec), but the fewer chunk boundaries
        // also reduce per-syscall churn on the Tokio writer.
        const CHUNK_SIZE: u32 = 60_000;
        writer.send_set_chunk_size(CHUNK_SIZE).await?;
        crate::trace::log("CHUNK_SIZE_SET", &format!("size={}", CHUNK_SIZE));

        // --- connect ---
        // Match what OBS's libobs RTMP plugin (and librtmp underneath it)
        // sends. Three reasons this matters for Twitch specifically:
        //
        // 1. flashVer = "FMLE/3.0 (compatible; FMSc/1.0)" puts us in the
        //    same preferred-publisher lane OBS gets.
        // 2. The audioCodecs / videoCodecs bitmaps declare "I will send
        //    AAC + H.264" at the protocol level. Twitch's transcoder
        //    uses these flags to decide which ladder lane to allocate.
        //    WITHOUT them, Twitch routes the stream to a conservative
        //    Source-only lane above ~6 Mbps. Bit values mirror librtmp:
        //       audioCodecs    = 3191  (incl. SUPPORT_SND_AAC bit 10)
        //       videoCodecs    = 252   (incl. SUPPORT_VID_H264 bit 7)
        //       videoFunction  = 1     (SUPPORT_VID_CLIENT_SEEK)
        //       objectEncoding = 0     (AMF0)
        //       capabilities   = 239   (Flash player default)
        //       fpad           = false (no proxy in front of us)
        // 3. The Enhanced RTMP `fourCcList` declares non-legacy codecs
        //    (HEVC, AV1, VP9, Opus, AC-3, FLAC) so HEVC / AV1 streams
        //    via OBS's Enhanced Broadcasting get routed to the modern
        //    codec lane. The legacy bitmaps above don't have bits for
        //    HEVC / AV1 - those codecs only exist in Enhanced RTMP.
        //    Written directly as AMF0 bytes because the strict-array
        //    type doesn't fit our HashMap-based Object builder.
        const FOURCC_LIST: &[&str] = &[
            "avc1", "hvc1", "av01", "vp09", // video
            "mp4a", "Opus", "ac-3", "ec-3", "fLaC", // audio
        ];
        let mut buf = BytesMut::new();
        amf0::enc_string(&mut buf, "connect");
        amf0::enc_number(&mut buf, 1.0);
        amf0::enc_object_begin(&mut buf);
        amf0::enc_object_key(&mut buf, "app");
        amf0::enc_string(&mut buf, &url.app);
        amf0::enc_object_key(&mut buf, "type");
        amf0::enc_string(&mut buf, "nonprivate");
        amf0::enc_object_key(&mut buf, "flashVer");
        amf0::enc_string(&mut buf, "FMLE/3.0 (compatible; FMSc/1.0)");
        amf0::enc_object_key(&mut buf, "tcUrl");
        amf0::enc_string(&mut buf, &url.tc_url);
        amf0::enc_object_key(&mut buf, "fpad");
        amf0::enc_value(&mut buf, &Amf0::Boolean(false));
        amf0::enc_object_key(&mut buf, "capabilities");
        amf0::enc_number(&mut buf, 239.0);
        amf0::enc_object_key(&mut buf, "audioCodecs");
        amf0::enc_number(&mut buf, 3191.0);
        amf0::enc_object_key(&mut buf, "videoCodecs");
        amf0::enc_number(&mut buf, 252.0);
        amf0::enc_object_key(&mut buf, "videoFunction");
        amf0::enc_number(&mut buf, 1.0);
        amf0::enc_object_key(&mut buf, "objectEncoding");
        amf0::enc_number(&mut buf, 0.0);
        amf0::enc_object_key(&mut buf, "fourCcList");
        amf0::enc_strict_array_str(&mut buf, FOURCC_LIST);
        amf0::enc_object_end(&mut buf);
        writer.write_message(3, 0, 20, 0, &buf).await?;
        writer.flush().await?;
        crate::trace::log(
            "AMF_OUT",
            &format!(
                "cmd=connect tcUrl={} flashVer=\"FMLE/3.0 (compatible; FMSc/1.0)\" \
                 audioCodecs=3191 videoCodecs=252 videoFunction=1 objectEncoding=0 \
                 fourCcList=[{}]",
                url.tc_url,
                FOURCC_LIST.join(",")
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
    /// that drain - when the sink is dropped (egress disconnects /
    /// reconnects), the drain task is aborted, preventing zombie tasks
    /// from accumulating on every reconnect.
    ///
    /// The drain doesn't just discard messages now: when it sees a User
    /// Control Message with event type 6 (RTMP Ping Request), it pushes
    /// the server's 4-byte timestamp onto a channel that `pump_dest`
    /// drains at the top of every loop iteration and replies to via
    /// `EgressSink::send_ping_response`. Without this, a server that
    /// pings us mid-stream to verify liveness (Twitch / YouTube edges
    /// do this on long sessions) would conclude we're dead after ~30 s
    /// of silence and drop the publish.
    pub fn spawn_reader_drain(mut self) -> EgressSink {
        let (ping_tx, ping_rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        // Acknowledgement (BYTES_READ_REPORT) queue. The drain task
        // owns the reader's byte counter; the sink owns the writer.
        // After every message the drain checks `take_pending_ack` and
        // forwards the seq number through here. Same pattern as pings
        // - keeps the writer's side of the socket single-owner.
        let (ack_tx, ack_rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let drain = tokio::spawn(async move {
            loop {
                let Ok(msg) = self.reader.read_message().await else {
                    return;
                };
                // User Control Message - type 4. Layout:
                //   bytes 0..2: event type (u16 BE)
                //   bytes 2..:  event-specific payload
                // Event type 6 = Ping Request (4-byte server ts
                // follows). The matching response is event type 7
                // with the same 4-byte ts echoed back.
                if msg.type_id == 4 && msg.payload.len() >= 6 {
                    let event = u16::from_be_bytes([msg.payload[0], msg.payload[1]]);
                    if event == 6 {
                        let ts = u32::from_be_bytes([
                            msg.payload[2],
                            msg.payload[3],
                            msg.payload[4],
                            msg.payload[5],
                        ]);
                        // Best-effort: if the receiver dropped (egress
                        // shutting down), silently ignore.
                        let _ = ping_tx.send(ts);
                    }
                }
                // Send back an Acknowledgement (msg type 3) whenever the
                // reader says we've crossed the peer's window/10
                // threshold. Egress traffic from the server to us is
                // small (acks + pings + onStatus), so this fires rarely
                // - but every well-behaved RTMP peer does it and so
                // should we.
                if let Some(seq) = self.reader.take_pending_ack() {
                    let _ = ack_tx.send(seq);
                }
            }
        });
        EgressSink {
            writer: self.writer,
            stream_id: self.stream_id,
            ping_rx,
            ack_rx,
            _drain_abort: drain.abort_handle(),
        }
    }
}

pub struct EgressSink {
    writer: ChunkWriter<WriteHalf<EgressStream>>,
    pub stream_id: u32,
    /// Pings the drain task has captured from the server, queued for the
    /// pump loop to acknowledge. Unbounded because pings arrive at most
    /// every few seconds and the queue is drained on every pump tick -
    /// even with no traffic on the egress, the queue never grows beyond
    /// a handful of entries.
    ping_rx: tokio::sync::mpsc::UnboundedReceiver<u32>,
    /// BYTES_READ_REPORT sequence numbers queued by the drain task -
    /// see `spawn_reader_drain`. Drained by `drain_pings` on every
    /// pump tick.
    ack_rx: tokio::sync::mpsc::UnboundedReceiver<u32>,
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

    /// Drain any pending Ping Requests and Acknowledgements that the
    /// reader-drain task has queued for us, and write the matching
    /// responses on the wire. Called by `pump_dest` at the top of every
    /// loop iteration; cheap when idle (two non-blocking `try_recv`s),
    /// one short RTMP message per pending event when work is needed.
    /// Errors abort the loop so the supervisor can reconnect.
    pub async fn drain_pings(&mut self) -> io::Result<()> {
        use bytes::BufMut;
        let mut wrote = false;
        while let Ok(ts) = self.ping_rx.try_recv() {
            let mut buf = BytesMut::with_capacity(6);
            buf.put_u16(7); // event type: Ping Response
            buf.put_u32(ts); // echo server's timestamp verbatim
                             // User Control Messages go on CSID 2, message type 4,
                             // timestamp 0, stream id 0 (control stream).
            self.writer.write_message(2, 0, 4, 0, &buf).await?;
            wrote = true;
            crate::trace::log("PING_REPLY", &format!("ts={} (echoed server ping)", ts));
        }
        // Acknowledgement (BYTES_READ_REPORT) drain: the reader queues
        // a sequence number every time it crosses the peer's window/10
        // threshold. Send each one through the writer (CSID 2, msg
        // type 3, 4-byte BE count).
        while let Ok(seq) = self.ack_rx.try_recv() {
            self.writer.send_ack(seq).await?;
            wrote = true;
            crate::trace::log("ACK_SENT", &format!("seq={}", seq));
        }
        if wrote {
            self.writer.flush().await?;
        }
        Ok(())
    }

    /// Clean shutdown matching OBS's exact teardown sequence: send
    /// `FCUnpublish` (paired with the `FCPublish` from connect), then
    /// `deleteStream`. Twitch's session bookkeeping uses this pair to
    /// mark the publish as ended on purpose rather than as a flaky
    /// disconnect that counts against your account / triggers re-auth.
    /// Best-effort: errors swallowed because we're tearing down anyway.
    pub async fn send_delete_stream(&mut self) -> io::Result<()> {
        // FCUnpublish carries the stream key, but we never see it at
        // this layer - the `EgressSink` only knows the stream_id. The
        // stream key is in the URL the EgressClient was built from, but
        // not retained here. Twitch accepts an empty-string key on
        // FCUnpublish (it knows which stream you mean from the
        // session); other servers may not, but the worst case is a
        // silently-ignored unpublish, which still degrades cleanly to
        // the deleteStream below.
        crate::trace::log("AMF_OUT", "cmd=FCUnpublish key_redacted=true");
        let mut buf = BytesMut::new();
        amf0::enc_string(&mut buf, "FCUnpublish");
        amf0::enc_number(&mut buf, 0.0);
        amf0::enc_null(&mut buf);
        amf0::enc_string(&mut buf, "");
        let _ = self.writer.write_message(3, 0, 20, 0, &buf).await;

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
    // Hard cap so a wedged Twitch / YouTube edge that goes silent
    // mid-handshake can't hang the egress task forever. 15 s is well
    // beyond a healthy round-trip (typical: 30-200 ms) while staying
    // short enough that the supervisor's reconnect backoff doesn't
    // pile up. Without this, a publish that Twitch silently drops
    // (which happened repeatedly in the user's 8 k bitrate test -
    // 7 reconnect cycles before the first onStatus arrived) wedges
    // here until the underlying TCP error happens to surface.
    let work = async {
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
    };
    match tokio::time::timeout(std::time::Duration::from_secs(15), work).await {
        Ok(res) => res,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "no `{}` response from server within 15 s - likely silent disconnect or auth delay",
                expect
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_rtmp_defaults_to_1935_no_tls() {
        let u = EgressUrl::parse("rtmp://live.twitch.tv/app/key123").unwrap();
        assert_eq!(u.host, "live.twitch.tv");
        assert_eq!(u.port, 1935);
        assert_eq!(u.app, "app");
        assert_eq!(u.stream_key, "key123");
        assert!(!u.tls);
        assert_eq!(u.tc_url, "rtmp://live.twitch.tv:1935/app");
    }

    #[test]
    fn parse_rtmps_defaults_to_443_with_tls() {
        // Kick's dashboard form: rtmps host on 443, /app path.
        let u = EgressUrl::parse(
            "rtmps://fa723fc1b171.global-contribute.live-video.net/app/sk_test_abc",
        )
        .unwrap();
        assert_eq!(u.host, "fa723fc1b171.global-contribute.live-video.net");
        assert_eq!(u.port, 443);
        assert_eq!(u.app, "app");
        assert_eq!(u.stream_key, "sk_test_abc");
        assert!(u.tls);
        assert_eq!(
            u.tc_url,
            "rtmps://fa723fc1b171.global-contribute.live-video.net:443/app"
        );
    }

    #[test]
    fn parse_rtmps_honours_explicit_port() {
        let u = EgressUrl::parse("rtmps://host.example/app/k").unwrap();
        assert_eq!(u.port, 443);
        let u = EgressUrl::parse("rtmps://host.example:1936/app/k").unwrap();
        assert_eq!(u.port, 1936);
        assert!(u.tls);
    }

    #[test]
    fn parse_rejects_unknown_scheme() {
        assert!(EgressUrl::parse("http://host/app/k").is_err());
        assert!(EgressUrl::parse("host/app/k").is_err());
    }
}
