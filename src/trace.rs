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
//! Off by default for end users - `main` calls `set_enabled` with
//! whichever value `settings.tracing_enabled` carries, and that
//! defaults to `false` on a fresh install. Streamers reporting a bug
//! flip it on in System → Advanced diagnostics, reproduce, and send
//! the file. `INSTANTCLONE_NO_TRACE=1` is a separate hard-kill that
//! disables the whole subsystem regardless of the runtime atomic.
//!
//! Writes are protected by a mutex; the hot path is a single locked
//! `writeln!` per event, which at typical ~30 fps is ~3000 lines / s on
//! a busy stream - well under disk-write contention thresholds. The
//! file still grows fast enough (≈ 6 MB / 10 min at 8 Mbps) that an
//! always-on default is the wrong call for a typical streamer; reserve
//! the cost for users who are actively diagnosing.

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

/// Hard cap on how much trace we'll write before auto-disabling. The
/// log grows at ~6 MB / 10 min on a busy EB stream when tracing is on,
/// so a streamer who flipped it on for diagnostics and forgot would
/// fill ~36 GB over a 100-hour week. The cap fires once, emits one
/// last line ("trace cap reached - disabling"), and flips ENABLED to
/// false. The user has to manually re-enable from the System tab if
/// they want to keep going, by which point they've also presumably
/// noticed the file size and rotated it.
const MAX_TRACE_BYTES: u64 = 200 * 1024 * 1024; // 200 MB

struct TraceState {
    file: Mutex<BufWriter<std::fs::File>>,
    start: Instant,
    /// Approximate bytes written since process start. We bump this by
    /// the line length on every `log()` call (atomic add, no mutex)
    /// and check against `MAX_TRACE_BYTES`. Approximate because we
    /// don't count the file's pre-existing bytes from earlier sessions,
    /// but the cap fires before either grows beyond 200 MB *this*
    /// session, which is the relevant safety bound.
    bytes_written: AtomicU64,
}

static STATE: OnceLock<TraceState> = OnceLock::new();

/// Runtime on/off switch - flipped by the settings UI via `set_enabled`.
/// Default false so a freshly installed app stays quiet; main.rs picks
/// up the persisted `settings.tracing_enabled` shortly after start and
/// flips this if the user opted in.
///
/// `Relaxed` is fine here: a flip taking a few microseconds to be
/// observed by the hot path is invisible - the worst case is a
/// handful of extra lines after disable, or a handful of dropped
/// lines after enable. No memory ordering invariant rides on this.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Initialise the trace writer. Idempotent; subsequent calls are no-ops.
/// `INSTANTCLONE_NO_TRACE=1` disables tracing entirely (zero overhead -
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
        bytes_written: AtomicU64::new(0),
    });
    log("session", &format!("trace started at {}", path.display()));
}

/// Cheap probe for callers on the per-tag hot path. They want to skip
/// the `format!` that builds the `msg` argument when tracing is off -
/// `log` itself short-circuits but only AFTER the caller has already
/// allocated the message string. At ~300 video tags/s an unconditional
/// `format!` is ~24 KB/s of churn nobody asked for. Reads the same
/// atomic `log` reads; if a flip races the check, the only effect is
/// one missed line at the boundary.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Write one trace line. `category` is a short tag (≤20 chars) used to
/// grep the file later (`grep VIDEO_TAG`, `grep CUT`, etc.). `msg` is the
/// human-readable detail. Becomes a single atomic load when tracing is
/// disabled - cheap enough to call from any hot path.
pub fn log(category: &str, msg: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(s) = STATE.get() else {
        return;
    };
    let ms = s.start.elapsed().as_secs_f64() * 1000.0;
    // Approximate-but-cheap size cap. We bump the counter by an
    // estimated line length (the formatted prefix is ~40 bytes plus
    // the message). When the cap trips we emit one terminal line and
    // flip the runtime atom - subsequent calls short-circuit at the
    // ENABLED check above. The cap-tripping path itself races for the
    // privilege of writing the terminal line via compare_exchange, so
    // we don't spam it across pumps.
    let line_bytes = (msg.len() + category.len() + 40) as u64;
    let prev = s.bytes_written.fetch_add(line_bytes, Ordering::Relaxed);
    if prev + line_bytes > MAX_TRACE_BYTES {
        // Try to be the thread that emits the final notice. swap to
        // false; only the winner sees `true` and writes the line.
        if ENABLED.swap(false, Ordering::Relaxed) {
            let mut f = s.file.lock().unwrap();
            let _ = writeln!(
                f,
                "T+{:>11.3}ms  {:<22}  trace cap reached ({} MB) - disabling. \
                 Re-enable from System -> Advanced diagnostics if you need more.",
                ms,
                "session",
                MAX_TRACE_BYTES / (1024 * 1024)
            );
            let _ = f.flush();
        }
        return;
    }
    let mut f = s.file.lock().unwrap();
    let _ = writeln!(f, "T+{:>11.3}ms  {:<22}  {}", ms, category, msg);
}

/// Flip tracing on/off at runtime. Called by the settings POST handler
/// so a user can toggle the System-tab checkbox without restarting the
/// app. The file stays open either way - re-enabling resumes appending
/// to the same file.
pub fn set_enabled(on: bool) {
    let was = ENABLED.swap(on, Ordering::Relaxed);
    if was != on {
        if on {
            // Reset the cap counter so a user re-enabling after the
            // cap fired (or after they manually rotated the file) gets
            // a fresh budget instead of immediately re-tripping. Lives
            // here rather than in `init` so a stop->start cycle from
            // the UI works exactly like a fresh launch.
            if let Some(s) = STATE.get() {
                s.bytes_written.store(0, Ordering::Relaxed);
            }
            log("session", "tracing enabled (via UI)");
        } else {
            log("session", "tracing disabled (via UI)");
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

#[cfg(test)]
mod tests {
    use super::*;

    // hex_prefix is pure (no global STATE), so these run without init().

    #[test]
    fn hex_prefix_empty_is_blank() {
        assert_eq!(hex_prefix(&[], 16), "");
    }

    #[test]
    fn hex_prefix_formats_lowercase_space_separated() {
        assert_eq!(
            hex_prefix(&[0x00, 0xde, 0xad, 0xbe, 0xef], 16),
            "00 de ad be ef"
        );
    }

    #[test]
    fn hex_prefix_no_suffix_when_within_max() {
        // Exactly `max` bytes: fully shown, no "(+N more)" tail.
        assert_eq!(hex_prefix(&[0x01, 0x02], 2), "01 02");
    }

    #[test]
    fn hex_prefix_truncates_with_remaining_count() {
        // Over `max`: shows the prefix plus a grep-friendly remainder note.
        assert_eq!(hex_prefix(&[1, 2, 3, 4, 5], 2), "01 02  (+3 more)");
    }
}
