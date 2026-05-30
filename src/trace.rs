//! Wire-level egress trace. Append-only file at `./instantclone-trace.log`
//! that records every event in the publish path: handshake, AMF0 command
//! send/receive, sequence headers, every tag's wire bytes for the first
//! N frames, every IDR, every cut/reanchor event, and periodic stats.
//!
//! Goal: be able to compare what InstantClone sends to a platform with
//! what OBS sends directly, byte-for-byte and tick-for-tick. The dashboard
//! log buffer (512 entries) is way too small for that; this writes
//! straight to disk with no rate limit.
//!
//! Init is opt-out via the `INSTANTCLONE_NO_TRACE` env var. On by default
//! during the beta so the user can ship traces without flipping a flag.
//! Writes are protected by a mutex; the hot path is a single locked
//! `writeln!` per event, which at typical ~30 fps is ~3000 lines / s on
//! a busy stream — well under disk-write contention thresholds.

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

struct TraceState {
    file: Mutex<BufWriter<std::fs::File>>,
    start: Instant,
}

static STATE: OnceLock<TraceState> = OnceLock::new();

/// Initialise the trace writer. Idempotent; subsequent calls are no-ops.
/// `INSTANTCLONE_NO_TRACE=1` disables tracing entirely (zero overhead —
/// the static stays unset and every log call is a single `OnceLock::get`
/// that returns `None`).
pub fn init(path: impl AsRef<Path>) {
    if std::env::var("INSTANTCLONE_NO_TRACE").ok().as_deref() == Some("1") {
        return;
    }
    let path = path.as_ref();
    let file = match OpenOptions::new().create(true).append(true).open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[trace] failed to open {}: {}", path.display(), e);
            return;
        }
    };
    let _ = STATE.set(TraceState {
        file: Mutex::new(BufWriter::with_capacity(64 * 1024, file)),
        start: Instant::now(),
    });
    log("session", &format!("trace started at {}", path.display()));
}

/// Write one trace line. `category` is a short tag (≤20 chars) used to
/// grep the file later (`grep VIDEO_TAG`, `grep CUT`, etc.). `msg` is the
/// human-readable detail.
pub fn log(category: &str, msg: &str) {
    let Some(s) = STATE.get() else {
        return;
    };
    let ms = s.start.elapsed().as_secs_f64() * 1000.0;
    let mut f = s.file.lock().unwrap();
    let _ = writeln!(f, "T+{:>11.3}ms  {:<22}  {}", ms, category, msg);
}

/// Convenience: hex-dump up to `max` bytes for quick wire inspection.
/// Long payloads get truncated with a `…+N more` suffix so the line stays
/// grep-friendly.
pub fn hex_prefix(bytes: &[u8], max: usize) -> String {
    let take = bytes.len().min(max);
    let mut s = String::with_capacity(take * 3);
    for (i, b) in bytes[..take].iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{:02x}", b));
    }
    if bytes.len() > max {
        s.push_str(&format!("  (+{} more)", bytes.len() - max));
    }
    s
}

/// Flush the buffer. Called on shutdown so the last few thousand events
/// don't get lost in the BufWriter when the process exits.
pub fn flush() {
    if let Some(s) = STATE.get() {
        let mut f = s.file.lock().unwrap();
        let _ = f.flush();
    }
}
