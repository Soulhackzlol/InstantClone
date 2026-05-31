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
//! Off by default for end users — `main` calls `set_enabled` with
//! whichever value `settings.tracing_enabled` carries, and that
//! defaults to `false` on a fresh install. Streamers reporting a bug
//! flip it on in System → Advanced diagnostics, reproduce, and send
//! the file. `INSTANTCLONE_NO_TRACE=1` is a separate hard-kill that
//! disables the whole subsystem regardless of the runtime atomic.
//!
//! Writes are protected by a mutex; the hot path is a single locked
//! `writeln!` per event, which at typical ~30 fps is ~3000 lines / s on
//! a busy stream — well under disk-write contention thresholds. The
//! file still grows fast enough (≈ 6 MB / 10 min at 8 Mbps) that an
//! always-on default is the wrong call for a typical streamer; reserve
//! the cost for users who are actively diagnosing.

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

struct TraceState {
    file: Mutex<BufWriter<std::fs::File>>,
    start: Instant,
}

static STATE: OnceLock<TraceState> = OnceLock::new();

/// Runtime on/off switch — flipped by the settings UI via `set_enabled`.
/// Default false so a freshly installed app stays quiet; main.rs picks
/// up the persisted `settings.tracing_enabled` shortly after start and
/// flips this if the user opted in.
///
/// `Relaxed` is fine here: a flip taking a few microseconds to be
/// observed by the hot path is invisible — the worst case is a
/// handful of extra lines after disable, or a handful of dropped
/// lines after enable. No memory ordering invariant rides on this.
static ENABLED: AtomicBool = AtomicBool::new(false);

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
/// human-readable detail. Becomes a single atomic load when tracing is
/// disabled — cheap enough to call from any hot path.
pub fn log(category: &str, msg: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(s) = STATE.get() else {
        return;
    };
    let ms = s.start.elapsed().as_secs_f64() * 1000.0;
    let mut f = s.file.lock().unwrap();
    let _ = writeln!(f, "T+{:>11.3}ms  {:<22}  {}", ms, category, msg);
}

/// Flip tracing on/off at runtime. Called by the settings POST handler
/// so a user can toggle the System-tab checkbox without restarting the
/// app. The file stays open either way — re-enabling resumes appending
/// to the same file.
pub fn set_enabled(on: bool) {
    let was = ENABLED.swap(on, Ordering::Relaxed);
    if was != on {
        log(
            "session",
            if on {
                "tracing enabled (via UI)"
            } else {
                "tracing disabled (via UI)"
            },
        );
        if !on {
            flush();
        }
    }
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
