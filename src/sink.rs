//! Local RTMP sink — accepts the proxy's published stream so you can
//! validate the full OBS → InstantClone → "Twitch" pipeline without ever
//! touching a real streaming platform.
//!
//! Usage:
//!     instantclone sink [--port N] [--file out.flv] [--verbose]
//!
//! Then point InstantClone's egress URL at `rtmp://127.0.0.1:N/live` and
//! use any stream key. Frames flow:
//!
//!     OBS ─→ InstantClone (proxy, :1935) ─→ this sink (:N)
//!                                              │
//!                                              ├─ live stats to stdout
//!                                              └─ optional FLV recording
//!                                                 (playable in VLC/mpv)
//!
//! The sink is intentionally tiny — handshake, AMF replies, tag receive,
//! optional FLV write. It deliberately reuses our own `rtmp::*` modules
//! so any wire-format bug shows up here just as it would against Twitch.

use crate::h264;
use crate::rtmp::amf0::{self, Amf0};
use crate::rtmp::chunk::{ChunkReader, ChunkWriter, Message};
use crate::rtmp::handshake;
use bytes::BytesMut;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{split, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

pub fn run_cli(args: &[String]) -> io::Result<()> {
    let mut port: u16 = 1936;
    let mut web_port: u16 = 1937;        // 0 = disable web player
    let mut file: Option<String> = None;
    let mut verbose = false;
    let mut max_mb: Option<u64> = None;
    let mut temp = false;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => {
                port = it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--port needs a number"); std::process::exit(2);
                });
            }
            "--web-port" => {
                web_port = it.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--web-port needs a number"); std::process::exit(2);
                });
            }
            "--file" => {
                file = it.next().cloned().or_else(|| {
                    eprintln!("--file needs a path"); std::process::exit(2);
                });
            }
            "--max-mb" => {
                max_mb = it.next().and_then(|s| s.parse().ok()).or_else(|| {
                    eprintln!("--max-mb needs a number"); std::process::exit(2);
                });
            }
            "--temp" => temp = true,
            "--verbose" | "-v" => verbose = true,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => {
                eprintln!("unknown sink option: {}", other);
                print_help();
                std::process::exit(2);
            }
        }
    }

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move { run_sink(port, web_port, file, verbose, max_mb, temp).await })
}

fn print_help() {
    eprintln!(
"InstantClone — RTMP sink (for end-to-end local testing)

USAGE:
    instantclone sink [OPTIONS]

OPTIONS:
    --port N        RTMP listen port (default 1936)
    --web-port N    HTTP port for the in-browser live player
                    (default 1937; set to 0 to disable)
    --file PATH     Also record the received stream as an FLV file
                    (playable in VLC, mpv, ffmpeg, etc.)
    --max-mb N      Cap the recording size (default: unlimited).
                    Recording stops cleanly when the cap is reached;
                    streaming to the platform continues unaffected.
    --temp          Delete the recording on Ctrl+C exit (useful when
                    you're just smoke-testing the pipeline and don't
                    want gigabytes piling up on disk).
    --verbose, -v   Log every received tag (very noisy)
    --help, -h      Show this message

To use this with the proxy:
    1. Run `instantclone sink --file recording.flv`
    2. Run `instantclone` in another terminal
    3. In the web UI, set platform → \"Custom\" and URL to
       rtmp://127.0.0.1:1936/live/test
    4. Point OBS at rtmp://127.0.0.1:1935/live");
}

async fn run_sink(
    port: u16,
    web_port: u16,
    out_path: Option<String>,
    verbose: bool,
    max_mb: Option<u64>,
    temp: bool,
) -> io::Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    let live = LiveStream::new();

    println!();
    println!("┌─────────────────────────────────────────────────────────┐");
    println!("│  InstantClone — local RTMP sink                         │");
    println!("│                                                         │");
    println!("│  RTMP:       rtmp://127.0.0.1:{:<5}/live                 │", port);
    if web_port > 0 {
        println!("│  Web player: http://127.0.0.1:{:<5}/                     │", web_port);
    } else {
        println!("│  Web player: disabled (--web-port 0)                    │");
    }
    if let Some(ref p) = out_path {
        println!("│  Recording:  {:<42} │", truncate(p, 42));
        match max_mb {
            Some(n) => println!("│  Cap:        {:<42} │", format!("{} MB", n)),
            None    => println!("│  Cap:        unlimited (use --max-mb N to cap)          │"),
        }
        if temp {
            println!("│  Cleanup:    delete on Ctrl+C (--temp)                  │");
        }
    } else {
        println!("│  Recording:  (off — use --file PATH to enable)          │");
    }
    println!("│                                                         │");
    println!("│  In InstantClone's web UI:                              │");
    println!("│    Platform → Custom                                    │");
    println!("│    URL     → rtmp://127.0.0.1:{}/live/test               │", port);
    println!("│    Key     → (anything)                                 │");
    println!("└─────────────────────────────────────────────────────────┘");
    println!();

    if out_path.is_some() && max_mb.is_none() && !temp {
        eprintln!("[sink] WARNING: recording is uncapped. At 8 Mbps that's ~3.6 GB/hour.");
        eprintln!("[sink]          Use `--max-mb 1000` to cap, or `--temp` to delete on exit.");
    }

    // Spawn the live-preview HTTP server unless disabled.
    if web_port > 0 {
        let live2 = live.clone();
        tokio::spawn(async move {
            if let Err(e) = run_web(web_port, live2).await {
                eprintln!("[sink-web] error: {}", e);
            }
        });
    }

    let accept_loop = async {
        loop {
            let (sock, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(e) => { eprintln!("[sink] accept failed: {}", e); break; }
            };
            let _ = sock.set_nodelay(true);
            let out = out_path.clone();
            let live_for_session = live.clone();
            tokio::spawn(async move {
                println!("[sink] {} → connected", peer);
                match handle_one(sock, out, verbose, max_mb, live_for_session).await {
                    Ok(()) => println!("[sink] {} → closed cleanly", peer),
                    Err(e) => println!("[sink] {} → {}", peer, e),
                }
            });
        }
    };

    tokio::select! {
        _ = accept_loop => {}
        _ = tokio::signal::ctrl_c() => {
            println!();
            println!("[sink] Ctrl+C received, shutting down.");
            if let Some(ref p) = out_path {
                let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                let mb = size as f64 / 1024.0 / 1024.0;
                if temp {
                    match std::fs::remove_file(p) {
                        Ok(_) => println!("[sink] removed {} ({:.1} MB) — --temp was set", p, mb),
                        Err(e) => println!("[sink] could not remove {}: {}", p, e),
                    }
                } else {
                    println!("[sink] recording saved: {} ({:.1} MB)", p, mb);
                    println!("[sink] (use --temp next time if you don't want it kept)");
                }
            }
        }
    }
    Ok(())
}

async fn handle_one(
    mut sock: TcpStream,
    out_path: Option<String>,
    verbose: bool,
    max_mb: Option<u64>,
    live: Arc<LiveStream>,
) -> io::Result<()> {
    handshake::perform_server(&mut sock).await?;
    let (rd, wr) = split(sock);
    let mut reader = ChunkReader::new(rd);
    let mut writer = ChunkWriter::new(wr);
    writer.send_set_chunk_size(4096).await?;

    let mut flv = out_path
        .as_deref()
        .map(|p| FlvWriter::create(p, max_mb))
        .transpose()?;
    let mut stats = SinkStats::new(verbose);

    loop {
        let msg = reader.read_message().await?;
        match msg.type_id {
            20 => handle_command(&mut writer, &msg).await?,
            17 => { /* AMF3 — ignore */ }
            18 /* AMF0 onMetaData / @setDataFrame */ => {
                if let Some(f) = flv.as_mut() {
                    f.write_tag(18, msg.timestamp, &msg.payload)?;
                }
                // Cache + broadcast for the web player. Each tag is
                // pre-framed once into an Arc so every connected client
                // receives a cheap pointer clone, not a copy.
                let bytes = Arc::new(flv_tag_bytes(18, msg.timestamp, &msg.payload));
                *live.metadata.lock().unwrap() = Some(bytes.clone());
                let _ = live.tx.send(bytes);
                println!("[sink] metadata received ({} bytes)", msg.payload.len());
            }
            8 /* audio */ => {
                if let Some(f) = flv.as_mut() {
                    f.write_tag(8, msg.timestamp, &msg.payload)?;
                }
                let bytes = Arc::new(flv_tag_bytes(8, msg.timestamp, &msg.payload));
                if h264::is_aac_seq_header(&msg.payload) {
                    *live.seq_audio.lock().unwrap() = Some(bytes.clone());
                }
                let _ = live.tx.send(bytes);
                stats.note_audio(msg.timestamp, msg.payload.len());
            }
            9 /* video */ => {
                let info = h264::classify_video_tag(&msg.payload);
                if let Some(f) = flv.as_mut() {
                    f.write_tag(9, msg.timestamp, &msg.payload)?;
                }
                let bytes = Arc::new(flv_tag_bytes(9, msg.timestamp, &msg.payload));
                if info.is_seq_header {
                    *live.seq_video.lock().unwrap() = Some(bytes.clone());
                }
                let _ = live.tx.send(bytes);
                stats.note_video(msg.timestamp, msg.payload.len(), info.is_idr, info.is_seq_header);
            }
            _ => { /* ignore other types */ }
        }
        stats.maybe_print();
    }
}

async fn handle_command<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut ChunkWriter<W>,
    msg: &Message,
) -> io::Result<()> {
    let values = amf0::decode_all(&msg.payload)?;
    let name = values.first().and_then(|v| v.as_str()).unwrap_or("").to_string();
    let txn = values.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);

    match name.as_str() {
        "connect" => {
            // Window Ack Size + Set Peer Bandwidth then `_result`. Twitch
            // does the same dance; we mirror it so the proxy sees exactly
            // what it'd see in production.
            let mut buf = BytesMut::new();
            buf.extend_from_slice(&5_000_000u32.to_be_bytes());
            writer.write_message(2, 0, 5, 0, &buf).await?;
            let mut buf = [0u8; 5];
            buf[..4].copy_from_slice(&5_000_000u32.to_be_bytes());
            buf[4] = 2;
            writer.write_message(2, 0, 6, 0, &buf).await?;

            let mut props = HashMap::new();
            props.insert("fmsVer".to_string(), Amf0::String("FMS/3,0,1,123".into()));
            props.insert("capabilities".to_string(), Amf0::Number(31.0));
            let mut info = HashMap::new();
            info.insert("level".to_string(), Amf0::String("status".into()));
            info.insert("code".to_string(), Amf0::String("NetConnection.Connect.Success".into()));
            info.insert("description".to_string(), Amf0::String("Connection succeeded.".into()));
            info.insert("objectEncoding".to_string(), Amf0::Number(0.0));
            let mut buf = BytesMut::new();
            amf0::enc_string(&mut buf, "_result");
            amf0::enc_number(&mut buf, txn);
            amf0::enc_value(&mut buf, &Amf0::Object(props));
            amf0::enc_value(&mut buf, &Amf0::Object(info));
            writer.write_message(3, 0, 20, 0, &buf).await?;
            writer.flush().await?;
        }
        "releaseStream" | "FCPublish" | "FCUnpublish" | "deleteStream" => {
            let mut buf = BytesMut::new();
            amf0::enc_string(&mut buf, "_result");
            amf0::enc_number(&mut buf, txn);
            amf0::enc_null(&mut buf);
            amf0::enc_null(&mut buf);
            writer.write_message(3, 0, 20, 0, &buf).await?;
            writer.flush().await?;
        }
        "createStream" => {
            let mut buf = BytesMut::new();
            amf0::enc_string(&mut buf, "_result");
            amf0::enc_number(&mut buf, txn);
            amf0::enc_null(&mut buf);
            amf0::enc_number(&mut buf, 1.0); // stream id
            writer.write_message(3, 0, 20, 0, &buf).await?;
            writer.flush().await?;
        }
        "publish" => {
            let key = values.get(3).and_then(|v| v.as_str()).unwrap_or("");
            println!("[sink] publish accepted (stream key: {:?})", key);
            let mut info = HashMap::new();
            info.insert("level".to_string(), Amf0::String("status".into()));
            info.insert("code".to_string(), Amf0::String("NetStream.Publish.Start".into()));
            info.insert("description".to_string(), Amf0::String("Sink is recording".into()));
            let mut buf = BytesMut::new();
            amf0::enc_string(&mut buf, "onStatus");
            amf0::enc_number(&mut buf, 0.0);
            amf0::enc_null(&mut buf);
            amf0::enc_value(&mut buf, &Amf0::Object(info));
            writer.write_message(5, 0, 20, msg.stream_id, &buf).await?;
            writer.flush().await?;
        }
        _ => { /* ignore unknown commands */ }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Stats — one summary line per second
// ---------------------------------------------------------------------
struct SinkStats {
    verbose: bool,
    started: Instant,
    window_start: Instant,
    bytes: u64,
    video_frames: u32,
    audio_frames: u32,
    idrs: u32,
    last_video_ts: Option<u32>,
    last_audio_ts: Option<u32>,
    first_ts: Option<u32>,
}

impl SinkStats {
    fn new(verbose: bool) -> Self {
        let now = Instant::now();
        Self {
            verbose,
            started: now,
            window_start: now,
            bytes: 0,
            video_frames: 0,
            audio_frames: 0,
            idrs: 0,
            last_video_ts: None,
            last_audio_ts: None,
            first_ts: None,
        }
    }

    fn note_video(&mut self, ts: u32, len: usize, is_idr: bool, is_seq: bool) {
        self.bytes += len as u64;
        if !is_seq { self.video_frames += 1; }
        if is_idr { self.idrs += 1; }
        if self.first_ts.is_none() { self.first_ts = Some(ts); }
        if self.verbose {
            let marker = if is_seq { "VS" } else if is_idr { "VI" } else { "Vp" };
            println!("  {} ts={:>8} sz={:>6}", marker, ts, len);
        }
        self.last_video_ts = Some(ts);
    }

    fn note_audio(&mut self, ts: u32, len: usize) {
        self.bytes += len as u64;
        self.audio_frames += 1;
        if self.first_ts.is_none() { self.first_ts = Some(ts); }
        if self.verbose {
            println!("  A  ts={:>8} sz={:>6}", ts, len);
        }
        self.last_audio_ts = Some(ts);
    }

    fn maybe_print(&mut self) {
        let elapsed = self.window_start.elapsed();
        if elapsed < Duration::from_secs(1) { return; }
        let kbps = (self.bytes * 8) / elapsed.as_millis().max(1) as u64;
        let v_ts = self.last_video_ts.unwrap_or(0);
        let a_ts = self.last_audio_ts.unwrap_or(0);
        let av_skew = (v_ts as i64) - (a_ts as i64);
        let wall = self.started.elapsed().as_secs_f64();
        println!(
            "[sink] wall={:>6.1}s  in={:>5} kbps  video={} ({} IDR)  audio={}  ts_v={}ms  A/V skew={}ms",
            wall, kbps, self.video_frames, self.idrs, self.audio_frames, v_ts, av_skew
        );
        self.window_start = Instant::now();
        self.bytes = 0;
        self.video_frames = 0;
        self.audio_frames = 0;
        self.idrs = 0;
    }
}

// ---------------------------------------------------------------------
// FLV writer — minimal valid-FLV emission for playback in VLC/mpv/ffmpeg.
// Spec ref: Adobe FLV file format spec v10 (2008).
// ---------------------------------------------------------------------
struct FlvWriter {
    w: BufWriter<File>,
    max_bytes: Option<u64>,   // hard cap; further writes are silently dropped past it
    bytes_written: u64,
    cap_warned: bool,
}

impl FlvWriter {
    fn create(path: &str, max_mb: Option<u64>) -> io::Result<Self> {
        let f = File::create(path)?;
        let mut w = BufWriter::with_capacity(256 * 1024, f);
        // 9-byte FLV header + PreviousTagSize0
        w.write_all(&[b'F', b'L', b'V', 1, 0x05, 0, 0, 0, 9])?;
        w.write_all(&[0, 0, 0, 0])?;
        Ok(Self {
            w,
            max_bytes: max_mb.map(|m| m * 1024 * 1024),
            bytes_written: 13,
            cap_warned: false,
        })
    }

    fn write_tag(&mut self, tag_type: u8, timestamp_ms: u32, payload: &[u8]) -> io::Result<()> {
        let len = payload.len() as u32;
        let tag_bytes = 11u64 + len as u64 + 4;
        // Hard cap: stop writing when we would exceed the requested size.
        // We never truncate a partial tag — refuse cleanly at the boundary.
        if let Some(cap) = self.max_bytes {
            if self.bytes_written + tag_bytes > cap {
                if !self.cap_warned {
                    eprintln!("[sink] recording capped at {} MB — further tags dropped from FLV",
                        cap / 1024 / 1024);
                    let _ = self.w.flush();
                    self.cap_warned = true;
                }
                return Ok(());
            }
        }
        let mut hdr = [0u8; 11];
        hdr[0] = tag_type;
        hdr[1] = ((len >> 16) & 0xFF) as u8;
        hdr[2] = ((len >> 8) & 0xFF) as u8;
        hdr[3] = (len & 0xFF) as u8;
        hdr[4] = ((timestamp_ms >> 16) & 0xFF) as u8;
        hdr[5] = ((timestamp_ms >> 8) & 0xFF) as u8;
        hdr[6] = (timestamp_ms & 0xFF) as u8;
        hdr[7] = ((timestamp_ms >> 24) & 0xFF) as u8;
        self.w.write_all(&hdr)?;
        self.w.write_all(payload)?;
        let prev = 11u32 + len;
        self.w.write_all(&prev.to_be_bytes())?;
        self.bytes_written += tag_bytes;
        // Flush after each tag so a Ctrl+C doesn't lose the latest frame
        // and VLC can tail the file during live record.
        if len > 0 { let _ = self.w.flush(); }
        Ok(())
    }
}

impl Drop for FlvWriter {
    fn drop(&mut self) {
        let _ = self.w.flush();
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { return s.to_string(); }
    let mut out = String::with_capacity(max);
    out.push_str(&s[..max.saturating_sub(1)]);
    out.push('…');
    out
}

// ---------------------------------------------------------------------
// Live web player — serves an HTML page plus an HTTP-FLV stream of the
// tags we're receiving in real time. Browser plays with flv.js (loaded
// from a CDN; ~150 KB, no binary bloat).
//
// Design:
//   * Each incoming RTMP tag is pre-framed as FLV bytes once (Arc'd) and
//     broadcast to all connected web clients.
//   * Sequence headers + onMetaData are cached so new viewers joining
//     mid-stream get the bootstrap data before the live tag stream
//     starts flowing into their pipe.
//   * Chunked transfer encoding keeps the HTTP connection open
//     indefinitely; flv.js doesn't care about Content-Length.
// ---------------------------------------------------------------------

/// Shared state for the live web preview. Cheap to clone (Arc-ed).
pub struct LiveStream {
    tx: broadcast::Sender<Arc<Vec<u8>>>,
    seq_video: std::sync::Mutex<Option<Arc<Vec<u8>>>>,
    seq_audio: std::sync::Mutex<Option<Arc<Vec<u8>>>>,
    metadata:  std::sync::Mutex<Option<Arc<Vec<u8>>>>,
}

impl LiveStream {
    fn new() -> Arc<Self> {
        // 1024-frame buffer: at 60 fps a slow client has ~17 s to catch
        // up before falling out of the broadcast queue. Beyond that they
        // get Lagged and we close their connection.
        let (tx, _) = broadcast::channel(1024);
        Arc::new(Self {
            tx,
            seq_video: std::sync::Mutex::new(None),
            seq_audio: std::sync::Mutex::new(None),
            metadata:  std::sync::Mutex::new(None),
        })
    }
}

/// Serialize one FLV tag (header + payload + previousTagSize tail) into a
/// single Vec — the exact bytes that go on the wire for HTTP-FLV.
fn flv_tag_bytes(tag_type: u8, ts: u32, payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut out = Vec::with_capacity(11 + payload.len() + 4);
    out.push(tag_type);
    out.push(((len >> 16) & 0xFF) as u8);
    out.push(((len >> 8) & 0xFF) as u8);
    out.push((len & 0xFF) as u8);
    out.push(((ts >> 16) & 0xFF) as u8);
    out.push(((ts >> 8) & 0xFF) as u8);
    out.push((ts & 0xFF) as u8);
    out.push(((ts >> 24) & 0xFF) as u8);
    out.extend_from_slice(&[0, 0, 0]); // stream id always 0 for FLV
    out.extend_from_slice(payload);
    out.extend_from_slice(&(11u32 + len).to_be_bytes());
    out
}

async fn run_web(port: u16, live: Arc<LiveStream>) -> io::Result<()> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    println!("[sink-web] 🎬 open http://127.0.0.1:{}/ in your browser to watch live", port);
    loop {
        let (sock, _) = listener.accept().await?;
        let _ = sock.set_nodelay(true);
        let live = live.clone();
        tokio::spawn(async move {
            let _ = handle_web_request(sock, live).await;
        });
    }
}

async fn handle_web_request(mut sock: TcpStream, live: Arc<LiveStream>) -> io::Result<()> {
    // Read the request line + headers (up to 4 KB — plenty for any GET).
    let mut buf = [0u8; 4096];
    let mut used = 0;
    loop {
        let n = sock.read(&mut buf[used..]).await?;
        if n == 0 { return Ok(()); }
        used += n;
        if buf[..used].windows(4).any(|w| w == b"\r\n\r\n") { break; }
        if used == buf.len() { return Ok(()); }
    }
    let req = std::str::from_utf8(&buf[..used]).unwrap_or("");
    let path = req.split_whitespace().nth(1).unwrap_or("/");

    match path {
        "/" => serve_player_page(&mut sock).await,
        "/live.flv" => serve_live_flv(&mut sock, live).await,
        _ => {
            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            Ok(())
        }
    }
}

async fn serve_player_page(sock: &mut TcpStream) -> io::Result<()> {
    let body = PLAYER_HTML.as_bytes();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body).await?;
    Ok(())
}

async fn serve_live_flv(sock: &mut TcpStream, live: Arc<LiveStream>) -> io::Result<()> {
    let head = b"HTTP/1.1 200 OK\r\n\
                 Content-Type: video/x-flv\r\n\
                 Transfer-Encoding: chunked\r\n\
                 Cache-Control: no-store\r\n\
                 Connection: close\r\n\
                 Access-Control-Allow-Origin: *\r\n\r\n";
    sock.write_all(head).await?;

    // Subscribe to the live tag stream BEFORE writing preroll, so we
    // don't miss any tags between bootstrap and live transition.
    let mut rx = live.tx.subscribe();

    // FLV global header (3 byte sig + version + flags + offset + first
    // PreviousTagSize). flv.js needs this once at the top of the stream.
    send_chunked(sock, &[b'F', b'L', b'V', 1, 0x05, 0, 0, 0, 9, 0, 0, 0, 0]).await?;

    // Bootstrap: cached onMetaData + AAC seq header + AVC seq header.
    // Without these flv.js can't initialize its decoder.
    let snapshots = {
        let m = live.metadata.lock().unwrap().clone();
        let a = live.seq_audio.lock().unwrap().clone();
        let v = live.seq_video.lock().unwrap().clone();
        [m, a, v]
    };
    for slot in snapshots.iter().flatten() {
        send_chunked(sock, slot).await?;
    }

    // Live stream — break on any send error (client closed) or lag.
    loop {
        match rx.recv().await {
            Ok(bytes) => {
                if send_chunked(sock, &bytes).await.is_err() { break; }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => break,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }

    // Final 0-length chunk to politely close the chunked stream.
    let _ = sock.write_all(b"0\r\n\r\n").await;
    Ok(())
}

async fn send_chunked(sock: &mut TcpStream, data: &[u8]) -> io::Result<()> {
    // <hex-length>\r\n<bytes>\r\n   per RFC 7230 §4.1
    let header = format!("{:x}\r\n", data.len());
    sock.write_all(header.as_bytes()).await?;
    sock.write_all(data).await?;
    sock.write_all(b"\r\n").await
}

/// HTML player. Uses flv.js from a CDN (~150 KB) plus a hand-rolled
/// player chrome modelled loosely on Twitch's — custom controls, LIVE
/// badge, buffering spinner, and a toggleable "stats for nerds" overlay
/// (press S) that exposes everything you need to diagnose proxy hiccups:
/// receive bitrate, buffer-ahead, dropped frames, stall count, total
/// stall time, FLV stream stats, player state. The default flv.js
/// config stays low-latency on purpose so any proxy-side jitter shows
/// up clearly here rather than being absorbed.
const PLAYER_HTML: &str = r##"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>InstantClone — live preview</title>
<style>
*{box-sizing:border-box;margin:0;padding:0;-webkit-font-smoothing:antialiased}
:root{
  --bg:#0a0a0d; --panel:rgba(18,18,24,.85); --border:rgba(255,255,255,.08);
  --fg:#efefef; --muted:#a3a3ad; --dim:#6a6a73;
  --accent:#9146ff; --good:#34c759; --bad:#ff453a; --warn:#ffae00;
}
html,body{height:100%;width:100%}
body{background:var(--bg);color:var(--fg);
  font-family:-apple-system,Segoe UI,Roboto,sans-serif;
  overflow:hidden;user-select:none}

/* Player container — relatively positioned so overlays anchor cleanly */
.stage{position:relative;width:100%;height:100%;background:#000;cursor:default}
.stage.hide-cursor{cursor:none}
video{width:100%;height:100%;background:#000;outline:none;display:block}

/* LIVE badge — top-left, red dot when receiving, gray when offline */
.live{
  position:absolute;top:14px;left:14px;
  display:inline-flex;align-items:center;gap:7px;
  padding:5px 10px 5px 9px;border-radius:4px;
  background:rgba(0,0,0,.6);backdrop-filter:blur(6px);
  font-size:11px;font-weight:700;letter-spacing:1.2px;
  color:var(--fg);
  pointer-events:none;
}
.live .ldot{width:7px;height:7px;border-radius:50%;background:var(--bad);
  box-shadow:0 0 8px var(--bad);animation:pulse 1.4s ease-in-out infinite}
.live.off{color:var(--dim)}
.live.off .ldot{background:var(--dim);box-shadow:none;animation:none}
@keyframes pulse{0%,100%{opacity:1}50%{opacity:.4}}

/* Buffering spinner — center, only when video is stalled */
.buffering{
  position:absolute;inset:0;display:flex;
  align-items:center;justify-content:center;
  background:rgba(0,0,0,.35);
  opacity:0;pointer-events:none;
  transition:opacity .15s;
}
.buffering.show{opacity:1}
.buffering .ring{
  width:54px;height:54px;
  border:4px solid rgba(255,255,255,.18);
  border-top-color:var(--accent);
  border-radius:50%;
  animation:spin .9s linear infinite;
}
.buffering .label{
  position:absolute;bottom:34%;
  font-size:12px;color:var(--fg);font-weight:500;letter-spacing:.5px;
  opacity:.85;
}
@keyframes spin{to{transform:rotate(360deg)}}

/* Stats overlay — top-right, slides in. Press S to toggle. */
.stats{
  position:absolute;top:14px;right:14px;
  width:280px;max-width:calc(100% - 28px);
  padding:14px 16px;
  background:var(--panel);backdrop-filter:blur(10px);
  border:1px solid var(--border);border-radius:8px;
  font-size:11px;font-family:Menlo,Consolas,monospace;
  color:var(--fg);
  transform:translateX(calc(100% + 28px));opacity:0;
  transition:transform .25s cubic-bezier(.32,.72,0,1),opacity .2s;
  pointer-events:none;
}
.stats.open{transform:translateX(0);opacity:1;pointer-events:auto}
.stats h4{
  font-size:10px;text-transform:uppercase;letter-spacing:1.2px;
  color:var(--muted);font-weight:600;margin-bottom:8px;
  display:flex;justify-content:space-between;align-items:center;
}
.stats h4 .hint{font-weight:400;color:var(--dim);text-transform:none;letter-spacing:0}
.stats .row{
  display:flex;justify-content:space-between;
  padding:3px 0;line-height:1.35;
  font-variant-numeric:tabular-nums;
}
.stats .row .k{color:var(--muted)}
.stats .row .v{color:var(--fg);font-weight:500}
.stats .row .v.bad{color:var(--bad)}
.stats .row .v.warn{color:var(--warn)}
.stats .row .v.good{color:var(--good)}
.stats hr{border:0;border-top:1px solid var(--border);margin:8px 0}

/* Bottom controls — auto-hide after 2s of mouse-still. */
.controls{
  position:absolute;left:0;right:0;bottom:0;
  display:flex;align-items:center;gap:8px;
  padding:10px 14px 12px;
  background:linear-gradient(180deg,transparent,rgba(0,0,0,.7));
  opacity:1;transition:opacity .25s;
}
.stage.idle .controls{opacity:0;pointer-events:none}
.controls button{
  background:transparent;border:0;color:var(--fg);
  width:36px;height:36px;border-radius:6px;
  display:inline-flex;align-items:center;justify-content:center;
  cursor:pointer;transition:background .15s,transform .12s;
  padding:0;
}
.controls button:hover{background:rgba(255,255,255,.12)}
.controls button:active{transform:scale(.92)}
.controls button svg{width:18px;height:18px;color:var(--fg);fill:none;stroke:currentColor;stroke-width:2;stroke-linecap:round;stroke-linejoin:round}
.controls .spacer{flex:1}
.controls .lbl{
  font-size:11px;color:var(--muted);
  display:flex;align-items:center;gap:6px;
  padding:0 8px;font-variant-numeric:tabular-nums;
}
.controls .lbl .ldot{width:6px;height:6px;border-radius:50%;background:var(--bad);
  box-shadow:0 0 6px var(--bad)}
.controls .live-edge{
  padding:0 10px;width:auto;height:32px;font-size:11px;font-weight:600;
  letter-spacing:.5px;border-radius:4px;
  background:var(--bad);color:#fff;
  text-transform:uppercase;
}
.controls .live-edge:hover{background:#d63a30}
.controls .live-edge.hidden{display:none}
.controls .vol{display:flex;align-items:center;gap:6px}
.controls .vol input[type=range]{
  -webkit-appearance:none;width:0;height:3px;background:rgba(255,255,255,.3);
  border-radius:2px;outline:none;cursor:pointer;
  transition:width .2s;
}
.controls .vol:hover input[type=range]{width:72px}
.controls .vol input[type=range]::-webkit-slider-thumb{
  -webkit-appearance:none;width:12px;height:12px;
  border-radius:50%;background:var(--fg);cursor:pointer;
}

/* Record button — empty circle when idle, pulsing red dot when active. */
.controls .rec-btn{position:relative}
.controls .rec-btn .rec-ring{
  width:14px;height:14px;border-radius:50%;
  border:1.8px solid var(--fg);
  transition:.15s;
}
.controls .rec-btn.on .rec-ring{
  border-color:var(--bad);background:var(--bad);
  box-shadow:0 0 10px var(--bad);
  animation:rec-pulse 1.2s ease-in-out infinite;
}
@keyframes rec-pulse{0%,100%{opacity:1}50%{opacity:.55}}
.controls .rec-meta{
  font-size:10px;color:var(--bad);font-weight:600;
  font-variant-numeric:tabular-nums;
  padding-left:4px;letter-spacing:.5px;
}
.controls .rec-meta.hidden{display:none}

/* Toast — bottom-center, transient. */
.toast{
  position:absolute;left:50%;bottom:80px;transform:translateX(-50%);
  background:var(--panel);backdrop-filter:blur(8px);
  border:1px solid var(--border);border-radius:6px;
  padding:9px 16px;font-size:12px;color:var(--fg);
  opacity:0;pointer-events:none;
  transition:opacity .2s;
}
.toast.show{opacity:1}

/* Error banner — for fatal flv.js errors. */
.err{
  position:absolute;top:50%;left:50%;
  transform:translate(-50%,-50%);
  background:var(--panel);border:1px solid var(--border);
  border-radius:8px;padding:20px 24px;
  text-align:center;max-width:420px;
  display:none;
}
.err.show{display:block}
.err h3{color:var(--bad);font-size:14px;margin-bottom:8px}
.err p{font-size:12px;color:var(--muted);line-height:1.5}
.err button{
  margin-top:12px;padding:7px 16px;background:var(--accent);
  color:#fff;border:0;border-radius:5px;font:inherit;cursor:pointer;
  font-size:12px;font-weight:600;
}
</style></head><body>
<div class="stage" id="stage">
  <video id="v" autoplay muted playsinline></video>

  <!-- Twitch-style LIVE badge -->
  <div class="live off" id="live"><span class="ldot"></span><span>LIVE</span></div>

  <!-- Buffering spinner — visible while video is waiting for data -->
  <div class="buffering" id="buffering">
    <div class="ring"></div>
    <div class="label" id="buffering-label">Buffering…</div>
  </div>

  <!-- Stats-for-nerds — press S to toggle -->
  <div class="stats" id="stats">
    <h4>Stream debug <span class="hint">press S to hide</span></h4>
    <div class="row"><span class="k">Resolution</span><span class="v" id="s-res">—</span></div>
    <div class="row"><span class="k">Video codec</span><span class="v" id="s-vcodec">—</span></div>
    <div class="row"><span class="k">Audio codec</span><span class="v" id="s-acodec">—</span></div>
    <div class="row"><span class="k">FPS (reported)</span><span class="v" id="s-fps">—</span></div>
    <hr>
    <div class="row"><span class="k">Receive rate</span><span class="v" id="s-rcv">—</span></div>
    <div class="row"><span class="k">Decoded frames</span><span class="v" id="s-dec">0</span></div>
    <div class="row"><span class="k">Dropped frames</span><span class="v" id="s-drop">0</span></div>
    <hr>
    <div class="row"><span class="k">Buffer ahead</span><span class="v" id="s-buf">0.00 s</span></div>
    <div class="row"><span class="k">Buffered end</span><span class="v" id="s-end">0.00 s</span></div>
    <div class="row"><span class="k">Playhead</span><span class="v" id="s-cur">0.00 s</span></div>
    <div class="row"><span class="k">Ready state</span><span class="v" id="s-state">—</span></div>
    <hr>
    <div class="row"><span class="k">Stalls</span><span class="v" id="s-stalls">0</span></div>
    <div class="row"><span class="k">Last stall</span><span class="v" id="s-last-stall">—</span></div>
    <div class="row"><span class="k">Total stalled</span><span class="v" id="s-stall-total">0.00 s</span></div>
    <div class="row"><span class="k">Stalled now</span><span class="v" id="s-stalling">no</span></div>
    <hr>
    <div class="row"><span class="k">Session uptime</span><span class="v" id="s-up">0 s</span></div>
  </div>

  <!-- Controls bar — auto-hides when idle -->
  <div class="controls">
    <button id="play-pause" title="Play/Pause (Space)">
      <svg id="icon-play" viewBox="0 0 24 24"><polygon points="6 4 20 12 6 20" fill="currentColor" stroke="none"/></svg>
      <svg id="icon-pause" viewBox="0 0 24 24" style="display:none"><rect x="6" y="4" width="4" height="16" fill="currentColor" stroke="none"/><rect x="14" y="4" width="4" height="16" fill="currentColor" stroke="none"/></svg>
    </button>
    <div class="vol">
      <button id="mute" title="Mute (M)">
        <svg id="icon-vol" viewBox="0 0 24 24"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" fill="currentColor" stroke="none"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07M19.07 4.93a10 10 0 0 1 0 14.14"/></svg>
        <svg id="icon-mute" viewBox="0 0 24 24" style="display:none"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" fill="currentColor" stroke="none"/><line x1="23" y1="9" x2="17" y2="15"/><line x1="17" y1="9" x2="23" y2="15"/></svg>
      </button>
      <input id="vol-slider" type="range" min="0" max="100" value="100" title="Volume">
    </div>
    <button class="live-edge hidden" id="live-edge" title="Jump to live edge">Live</button>
    <span class="lbl" id="live-lbl"><span class="ldot"></span><span id="latency">—</span></span>

    <span class="spacer"></span>

    <span class="rec-meta hidden" id="rec-meta">REC <span id="rec-time">0s</span></span>
    <button id="record" class="rec-btn" title="Record trace (R) — downloads .jsonl on stop">
      <span class="rec-ring"></span>
    </button>
    <button id="stats-toggle" title="Stats (S)">
      <svg viewBox="0 0 24 24"><path d="M3 3v18h18"/><path d="M7 14l4-4 4 4 5-5"/></svg>
    </button>
    <button id="reload" title="Reconnect">
      <svg viewBox="0 0 24 24"><polyline points="23 4 23 10 17 10"/><polyline points="1 20 1 14 7 14"/><path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/></svg>
    </button>
    <button id="fullscreen" title="Fullscreen (F)">
      <svg viewBox="0 0 24 24"><path d="M8 3H5a2 2 0 0 0-2 2v3M21 8V5a2 2 0 0 0-2-2h-3M3 16v3a2 2 0 0 0 2 2h3M16 21h3a2 2 0 0 0 2-2v-3"/></svg>
    </button>
  </div>

  <!-- Transient toast (e.g. "Jumped to live") -->
  <div class="toast" id="toast"></div>

  <!-- Fatal error fallback (flv.js dead, etc.) -->
  <div class="err" id="err">
    <h3 id="err-title">Player error</h3>
    <p id="err-body"></p>
    <button onclick="start()">Reload player</button>
  </div>
</div>
<script src="https://cdn.jsdelivr.net/npm/flv.js@1.6.2/dist/flv.min.js"></script>
<script>
const $ = id => document.getElementById(id);
const stage = $('stage'), video = $('v');
let player = null;
let mediaInfo = null;
let flvStats = null;
const stats = {
  startedAt: Date.now(),
  stalls: 0,
  stallStart: null,
  lastStallMs: 0,
  totalStallMs: 0,
};

function showError(title, body){
  $('err-title').textContent = title;
  $('err-body').textContent = body;
  $('err').classList.add('show');
}
function hideError(){ $('err').classList.remove('show'); }

function setLive(on){
  $('live').className = 'live' + (on ? '' : ' off');
}

function toast(msg){
  const t = $('toast');
  t.textContent = msg;
  t.classList.add('show');
  clearTimeout(toast._t);
  toast._t = setTimeout(()=>t.classList.remove('show'), 1800);
}

function showBuffering(label){
  $('buffering-label').textContent = label || 'Buffering…';
  $('buffering').classList.add('show');
}
function hideBuffering(){
  $('buffering').classList.remove('show');
}

function readyStateName(rs){
  return ['nothing','metadata','curr_data','future_data','enough'][rs] || ('rs='+rs);
}

// ── Recording: capture every sample + event to a JSONL log the user
// can download and share for offline analysis. Each line is one JSON
// object with `t` (ms since rec-start), `kind`, and event-specific
// fields. Recording works during normal playback — no perf impact when
// idle (just appending to an array).
const rec = {
  on: false,
  startedAt: 0,
  lines: [],
};

function recPush(obj){
  if(!rec.on) return;
  obj.t = Date.now() - rec.startedAt;
  rec.lines.push(JSON.stringify(obj));
}

function recStart(){
  if(rec.on) return;
  rec.on = true;
  rec.startedAt = Date.now();
  rec.lines = [];
  // Header line — context for whoever reads the trace later.
  rec.lines.push(JSON.stringify({
    t: 0,
    kind: 'header',
    when: new Date().toISOString(),
    ua: navigator.userAgent,
    viewport: { w: window.innerWidth, h: window.innerHeight },
    note: 'InstantClone sink player trace. Sampled every 250ms + event-driven entries.',
  }));
  if(mediaInfo){
    recPush({ kind: 'media_info', info: mediaInfo });
  }
  $('record').classList.add('on');
  $('rec-meta').classList.remove('hidden');
  toast('Recording trace…');
}

function recStop(){
  if(!rec.on) return;
  rec.on = false;
  $('record').classList.remove('on');
  $('rec-meta').classList.add('hidden');
  // Trigger download.
  const blob = new Blob([rec.lines.join('\n') + '\n'], { type: 'application/x-ndjson' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  const ts = new Date().toISOString().replace(/[:.]/g, '-');
  a.href = url;
  a.download = 'instantclone-trace-' + ts + '.jsonl';
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  toast('Saved ' + rec.lines.length + ' entries');
}

$('record').addEventListener('click', () => {
  if(rec.on) recStop(); else recStart();
});

function start(){
  hideError();
  if(!window.flvjs || !flvjs.isSupported()){
    showError('flv.js unavailable', 'The flv.js player script failed to load (offline?) or this browser does not support MediaSource.');
    return;
  }
  if(player){ try{ player.destroy(); }catch(_){} player = null; }
  stats.startedAt = Date.now();
  stats.stalls = 0;
  stats.stallStart = null;
  stats.lastStallMs = 0;
  stats.totalStallMs = 0;
  recPush({ kind: 'player_start' });

  // Default config: aggressive low-latency. We WANT to see proxy
  // hiccups here — bumping these would mask the very bugs the sink
  // exists to surface.
  player = flvjs.createPlayer({
    type: 'flv',
    url: '/live.flv',
    isLive: true,
    hasAudio: true,
    hasVideo: true,
  }, {
    enableStashBuffer: false,
    stashInitialSize: 128,
    autoCleanupSourceBuffer: true,
    autoCleanupMaxBackwardDuration: 30,
    autoCleanupMinBackwardDuration: 10,
  });
  player.attachMediaElement(video);

  player.on(flvjs.Events.MEDIA_INFO, info => {
    mediaInfo = info;
    setLive(true);
    recPush({ kind: 'media_info', info });
  });
  player.on(flvjs.Events.STATISTICS_INFO, s => { flvStats = s; });
  player.on(flvjs.Events.ERROR, (et, ed) => {
    setLive(false);
    showError('Stream error', et + ': ' + ed);
    recPush({ kind: 'error', err_type: et, detail: ed });
  });
  player.on(flvjs.Events.LOADING_COMPLETE, () => {
    setLive(false);
    toast('Stream ended');
    recPush({ kind: 'loading_complete' });
  });

  player.load();
  video.play().catch(() => { /* autoplay blocked — user clicks play */ });
}

// ── Stall tracking via the video element events ─────────────────────
video.addEventListener('waiting', () => {
  if(stats.stallStart == null){
    stats.stalls += 1;
    stats.stallStart = Date.now();
    showBuffering();
    recPush({
      kind: 'stall_start',
      playhead: video.currentTime,
      buffered_end: video.buffered.length ? video.buffered.end(video.buffered.length-1) : 0,
      ready_state: video.readyState,
    });
  }
});
video.addEventListener('playing', () => {
  if(stats.stallStart != null){
    stats.lastStallMs = Date.now() - stats.stallStart;
    stats.totalStallMs += stats.lastStallMs;
    recPush({
      kind: 'stall_end',
      duration_ms: stats.lastStallMs,
      stalls_total: stats.stalls,
      playhead: video.currentTime,
    });
    stats.stallStart = null;
  }
  hideBuffering();
});
video.addEventListener('pause', () => hideBuffering());
video.addEventListener('seeked', () => {
  recPush({ kind: 'seeked', playhead: video.currentTime });
});

// ── Play/Pause + icon swap ──────────────────────────────────────────
function updatePlayPauseIcon(){
  $('icon-play').style.display  = video.paused ? '' : 'none';
  $('icon-pause').style.display = video.paused ? 'none' : '';
}
$('play-pause').addEventListener('click', () => {
  if(video.paused) video.play(); else video.pause();
});
video.addEventListener('play', updatePlayPauseIcon);
video.addEventListener('pause', updatePlayPauseIcon);

// ── Mute / volume ───────────────────────────────────────────────────
function updateMuteIcon(){
  const m = video.muted || video.volume === 0;
  $('icon-vol').style.display  = m ? 'none' : '';
  $('icon-mute').style.display = m ? '' : 'none';
}
$('mute').addEventListener('click', () => {
  video.muted = !video.muted;
  updateMuteIcon();
});
$('vol-slider').addEventListener('input', e => {
  video.volume = e.target.value / 100;
  video.muted = (video.volume === 0);
  updateMuteIcon();
});

// ── Live-edge button ─────────────────────────────────────────────────
function jumpToLive(){
  const b = video.buffered;
  if(b.length === 0) return;
  const target = Math.max(0, b.end(b.length - 1) - 0.05);
  recPush({ kind: 'jump_to_live', from: video.currentTime, to: target });
  video.currentTime = target;
  toast('Jumped to live');
}
$('live-edge').addEventListener('click', jumpToLive);

// ── Fullscreen + reload + stats toggle ───────────────────────────────
$('fullscreen').addEventListener('click', () => {
  if(!document.fullscreenElement) stage.requestFullscreen();
  else document.exitFullscreen();
});
$('reload').addEventListener('click', start);
$('stats-toggle').addEventListener('click', () => $('stats').classList.toggle('open'));

// ── Keyboard ────────────────────────────────────────────────────────
document.addEventListener('keydown', e => {
  if(e.target.tagName === 'INPUT') return;
  if(e.key === ' ') { e.preventDefault(); $('play-pause').click(); }
  else if(e.key === 'm' || e.key === 'M') $('mute').click();
  else if(e.key === 'f' || e.key === 'F') $('fullscreen').click();
  else if(e.key === 's' || e.key === 'S') $('stats-toggle').click();
  else if(e.key === 'l' || e.key === 'L') jumpToLive();
  else if(e.key === 'r' || e.key === 'R') $('record').click();
});

// ── Cursor / controls auto-hide ─────────────────────────────────────
let idleTimer;
function nudge(){
  stage.classList.remove('idle','hide-cursor');
  clearTimeout(idleTimer);
  idleTimer = setTimeout(() => {
    stage.classList.add('idle','hide-cursor');
  }, 2200);
}
stage.addEventListener('mousemove', nudge);
stage.addEventListener('mousedown', nudge);
nudge();

// ── Stats loop ──────────────────────────────────────────────────────
function fmtRate(kBps){
  if(!kBps) return '—';
  if(kBps < 1024) return kBps.toFixed(0) + ' KB/s';
  return (kBps / 1024).toFixed(2) + ' MB/s';
}
function fmtMs(ms){ return ms < 1000 ? ms.toFixed(0) + ' ms' : (ms / 1000).toFixed(2) + ' s'; }
function fmtSecs(s){ return s.toFixed(2) + ' s'; }

let lastDecoded = 0;

function refreshStats(){
  // mediaInfo
  if(mediaInfo){
    $('s-res').textContent    = (mediaInfo.width || '?') + '×' + (mediaInfo.height || '?');
    $('s-vcodec').textContent = mediaInfo.videoCodec || '—';
    $('s-acodec').textContent = mediaInfo.audioCodec || '—';
    $('s-fps').textContent    = (mediaInfo.fps || 0).toFixed(2);
  }

  // flv.js statistics
  if(flvStats){
    $('s-rcv').textContent = fmtRate(flvStats.speed || 0);
  }

  // Playback quality (drops!)
  const q = video.getVideoPlaybackQuality && video.getVideoPlaybackQuality();
  if(q){
    $('s-dec').textContent  = q.totalVideoFrames;
    const drops = q.droppedVideoFrames;
    const dEl = $('s-drop');
    dEl.textContent = drops;
    dEl.className = 'v ' + (drops === 0 ? 'good' : drops < 5 ? 'warn' : 'bad');
    lastDecoded = q.totalVideoFrames;
  }

  // Buffer math
  const b = video.buffered;
  let end = 0;
  if(b.length > 0) end = b.end(b.length - 1);
  const ahead = Math.max(0, end - video.currentTime);
  $('s-buf').textContent = fmtSecs(ahead);
  $('s-buf').className   = 'v ' + (ahead < 0.3 ? 'bad' : ahead < 1.0 ? 'warn' : 'good');
  $('s-end').textContent = fmtSecs(end);
  $('s-cur').textContent = fmtSecs(video.currentTime);
  $('s-state').textContent = readyStateName(video.readyState);

  // Stall counters
  const stalling = stats.stallStart != null;
  $('s-stalls').textContent      = stats.stalls;
  $('s-stalls').className        = 'v ' + (stats.stalls === 0 ? 'good' : 'warn');
  $('s-last-stall').textContent  = stats.lastStallMs ? fmtMs(stats.lastStallMs) : '—';
  $('s-stall-total').textContent = (stats.totalStallMs / 1000).toFixed(2) + ' s';
  $('s-stall-total').className   = 'v ' + (stats.totalStallMs === 0 ? 'good' : 'warn');
  $('s-stalling').textContent    = stalling ? 'YES' : 'no';
  $('s-stalling').className      = 'v ' + (stalling ? 'bad' : 'good');

  // Uptime
  const up = Math.floor((Date.now() - stats.startedAt) / 1000);
  $('s-up').textContent = up < 60 ? up + ' s' :
                           up < 3600 ? Math.floor(up/60) + 'm ' + (up%60) + 's' :
                           Math.floor(up/3600) + 'h ' + Math.floor((up%3600)/60) + 'm';

  // Latency pill on bottom bar
  $('latency').textContent = ahead.toFixed(2) + 's behind';

  // Show "Live" button if user has fallen behind the live edge by >2s
  $('live-edge').classList.toggle('hidden', ahead < 2.5);

  // Update REC timer (mm:ss after the first minute).
  if(rec.on){
    const secs = Math.floor((Date.now() - rec.startedAt) / 1000);
    const rt = $('rec-time');
    rt.textContent = secs < 60 ? secs + 's'
      : Math.floor(secs/60) + ':' + String(secs%60).padStart(2,'0');
    // Append a sample row.
    recPush({
      kind: 'sample',
      playhead: +video.currentTime.toFixed(3),
      buffered_end: +end.toFixed(3),
      buffer_ahead: +ahead.toFixed(3),
      ready_state: video.readyState,
      paused: video.paused,
      decoded: q ? q.totalVideoFrames : null,
      dropped: q ? q.droppedVideoFrames : null,
      stalls_total: stats.stalls,
      stall_total_ms: stats.totalStallMs,
      stalling: stats.stallStart != null,
      receive_kbps: flvStats ? +(flvStats.speed || 0).toFixed(0) : null,
    });
  }
}
setInterval(refreshStats, 250);

// Open stats by default — this player exists for diagnostics.
$('stats').classList.add('open');

start();
</script>
</body></html>
"##;
