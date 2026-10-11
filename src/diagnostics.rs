//! What InstantClone hands over when something goes wrong: the crash log a
//! panic leaves beside the settings, and the plain-text report behind the
//! Logs tab's "Copy diagnostics" (`GET /diagnostics`).
//!
//! Both quote settings and log lines, and both end up pasted into a bug
//! report or a Discord channel, so both go through one scrub on the way out:
//! every secret the settings hold is cut out, and every web address is cut
//! back to its host.
//!
//! Release builds abort on a panic and have no console, so before this the
//! only trace of a crash was the 1 500-line log, which lives in memory and
//! dies with the process.

use crate::config::Settings;
use crate::controller::Controller;
use crate::integrations::clock::iso_utc;
use crate::integrations::model::Trigger;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The crash log's file name, beside the settings file.
pub const CRASH_LOG_NAME: &str = "instantclone-crash.log";
/// Past this the next report starts the file over. A report is ~30 KB, so
/// this keeps the last several crashes and never grows without bound.
const CRASH_LOG_MAX_BYTES: u64 = 256 * 1024;
/// Log lines a crash report keeps: the last minutes before it.
const CRASH_LOG_LINES: usize = 200;
/// Starts every report; the crash time follows it, then ` ====`.
const CRASH_HEADER: &str = "==== InstantClone closed unexpectedly at ";
/// Appended once the next start has told the user, so it says so only once.
const SEEN_MARK: &str = "---- reported on the next start";
/// What a secret becomes.
const REDACTED: &str = "[redacted]";

/// The secrets of the settings in force, kept apart from the settings
/// channel so the panic hook never has to borrow it: a panic inside a
/// settings update would otherwise wait forever on the lock it holds, and a
/// process that should abort would hang instead.
static SECRETS: crate::sync::Mutex<Vec<String>> = crate::sync::Mutex::new(Vec::new());

/// Refresh what the crash log scrubs. Called at startup and on every
/// settings change.
pub fn remember_secrets(settings: &Settings) {
    *SECRETS.lock() = secrets_of(settings);
}

/// Every secret value the settings hold: stream keys, custom URLs (which
/// can carry a key), the ingest key, the dock token, the password hash,
/// Discord webhooks, the phone topic and the integrations' hook tokens.
/// Longest first, so a secret that contains another goes whole.
fn secrets_of(s: &Settings) -> Vec<String> {
    let mut out = Vec::new();
    let mut add = |value: &str| {
        let value = value.trim();
        if !value.is_empty() {
            out.push(value.to_string());
        }
    };
    for value in [
        &s.ingest_key,
        &s.stream_key,
        &s.custom_egress_url,
        &s.dashboard_password_hash,
        &s.dock_token,
        &s.discord_webhook_url,
        &s.phone.topic,
    ] {
        add(value);
    }
    for d in &s.destinations {
        add(&d.stream_key);
        add(&d.custom_egress_url);
    }
    for channel in &s.discord_channels {
        add(&channel.url);
    }
    for handler in s.integrations.iter().flat_map(|i| &i.handlers) {
        if let Trigger::Webhook { token } | Trigger::Shortcut { token, .. } = &handler.trigger {
            add(token);
        }
    }
    out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    out.dedup();
    out
}

/// `text` with every secret cut out and every web address cut to its host.
fn scrub(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), REDACTED);
        }
    }
    crate::integrations::runner::redact_urls(&out)
}

/// Now, in Unix milliseconds.
fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A log line with the wall-clock time it was logged in front. Log lines
/// carry the process clock (`[+  12.345s]`), which says nothing to someone
/// matching them against an OBS log or a viewer's "at 9:14 it froze".
fn with_wall_clock(line: &str, now_unix_ms: u64, now_process_ms: u64) -> String {
    let logged_at_s = line
        .strip_prefix("[+")
        .and_then(|rest| rest.split_once("s]"))
        .and_then(|(secs, _)| secs.trim().parse::<f64>().ok());
    match logged_at_s {
        Some(secs) => {
            let started = now_unix_ms.saturating_sub(now_process_ms);
            let at_ms = started + (secs * 1000.0) as u64;
            format!("{} {line}", iso_utc(at_ms / 1000))
        }
        None => line.to_string(),
    }
}

/// The commit this binary was built from (set by the release workflow).
fn build_label() -> String {
    let version = crate::update_check::current_version();
    match option_env!("INSTANTCLONE_BUILD_COMMIT").filter(|c| !c.is_empty()) {
        Some(commit) => format!("{version} (commit {commit})"),
        None => format!("{version} (local build)"),
    }
}

/// The crash log beside the settings file at `config_path`.
pub fn crash_log_path(config_path: &Path) -> PathBuf {
    crate::config::beside_config(config_path, Path::new(CRASH_LOG_NAME))
}

/// Write a crash report before the process goes down. Release builds
/// abort on a panic, so this runs just before the abort, on whatever thread
/// panicked. It only ever `try_lock`s: that thread may hold the log or the
/// secrets, and waiting on them would turn an abort into a hang.
pub fn install_panic_hook(ctrl: Arc<Controller>, crash_log: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_string());
        let location = info
            .location()
            .map_or_else(String::new, |l| format!("{}:{}", l.file(), l.line()));
        let thread = std::thread::current();
        let lines: Option<Vec<String>> = ctrl.logs.try_lock().map(|q| {
            let skip = q.len().saturating_sub(CRASH_LOG_LINES);
            q.iter().skip(skip).cloned().collect()
        });
        let secrets = SECRETS.try_lock().map(|s| s.clone());
        let report = crash_report(
            &Crash {
                message: &message,
                location: &location,
                thread: thread.name().unwrap_or("unnamed"),
                lines: lines.as_deref(),
                secrets: secrets.as_deref(),
            },
            unix_now_ms(),
            crate::controller::process_now_ms(),
        );
        append_bounded(&crash_log, &report);
        previous(info);
    }));
}

/// What the panic hook knows about a crash.
struct Crash<'a> {
    message: &'a str,
    location: &'a str,
    thread: &'a str,
    /// The last log lines, None when the panicking thread held the log.
    lines: Option<&'a [String]>,
    /// None when they couldn't be read: the log lines are left out then,
    /// since there is nothing to scrub them with.
    secrets: Option<&'a [String]>,
}

fn crash_report(crash: &Crash, now_unix_ms: u64, now_process_ms: u64) -> String {
    let secrets = crash.secrets.unwrap_or(&[]);
    let mut out = format!(
        "{CRASH_HEADER}{} ====\nVersion: {}\nOS: {} {}\nThread: {}\nPanic: {}\nAt: {}\n",
        iso_utc(now_unix_ms / 1000),
        build_label(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        crash.thread,
        crash.message,
        crash.location,
    );
    match (crash.lines, crash.secrets) {
        (Some(lines), Some(_)) => {
            out.push_str(&format!("Last {} log lines:\n", lines.len()));
            for line in lines {
                out.push_str(&with_wall_clock(line, now_unix_ms, now_process_ms));
                out.push('\n');
            }
        }
        (None, _) => out.push_str("(the log was busy at the moment of the crash)\n"),
        (Some(_), None) => out.push_str("(log lines left out: the secrets to hide were busy)\n"),
    }
    out.push('\n');
    scrub(&out, secrets)
}

/// Append `report` to the crash log, starting the file over once it has
/// grown past `CRASH_LOG_MAX_BYTES`. Best effort: a crash log that can't be
/// written must not stop the process going down.
fn append_bounded(path: &Path, report: &str) {
    let too_big = std::fs::metadata(path).is_ok_and(|m| m.len() > CRASH_LOG_MAX_BYTES);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(true)
        .truncate(too_big)
        .open(path);
    if let Ok(mut file) = file {
        let _ = file.write_all(report.as_bytes());
        let _ = file.flush();
    }
}

/// If the last run crashed and nobody has been told, say so once in the
/// log, pointing at the crash log, and mark it told.
pub fn announce_previous_crash(crash_log: &Path, ctrl: &Controller) {
    let Ok(text) = std::fs::read_to_string(crash_log) else {
        return;
    };
    let Some(when) = unreported_crash(&text) else {
        return;
    };
    let shown = std::path::absolute(crash_log).unwrap_or_else(|_| crash_log.to_path_buf());
    ctrl.log(format!(
        "[crash] InstantClone closed unexpectedly at {when} (UTC). What it was doing is in {} - \
         attach that file if you report it.",
        shown.display()
    ));
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(crash_log) {
        let _ = writeln!(file, "{SEEN_MARK}, {}", iso_utc(unix_now_ms() / 1000));
    }
}

/// The time of the newest crash in the log, unless a start already
/// reported it.
fn unreported_crash(text: &str) -> Option<&str> {
    let start = text.rfind(CRASH_HEADER)? + CRASH_HEADER.len();
    let rest = &text[start..];
    if rest.contains(SEEN_MARK) {
        return None;
    }
    rest.split_once(" ====").map(|(when, _)| when)
}

/// When the newest crash in the log happened, reported or not, for the
/// diagnostics header.
fn last_crash(crash_log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(crash_log).ok()?;
    let start = text.rfind(CRASH_HEADER)? + CRASH_HEADER.len();
    let (when, _) = text[start..].split_once(" ====")?;
    Some(when.to_string())
}

/// `1h 02m 03s`.
fn hms(secs: u64) -> String {
    format!("{}h {:02}m {:02}s", secs / 3600, secs / 60 % 60, secs % 60)
}

/// The plain-text report behind "Copy diagnostics": who we are, the
/// settings (already redacted for display), what the delay and every
/// destination are doing, and the log with wall-clock times. Scrubbed of
/// every configured secret as the last step, so nothing a section printed
/// can slip through.
pub fn report(ctrl: &Controller, settings: &Settings, config_path: &Path) -> String {
    let now_unix_ms = unix_now_ms();
    let now_process_ms = crate::controller::process_now_ms();
    let uptime = ctrl.uptime_secs();
    let mut out = String::with_capacity(64 * 1024);
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };

    line("InstantClone diagnostics".into());
    line(format!("Version: {}", build_label()));
    line(format!(
        "OS: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    line(format!(
        "Started: {} UTC (up {})",
        iso_utc((now_unix_ms / 1000).saturating_sub(uptime)),
        hms(uptime)
    ));
    line(format!("Generated: {} UTC", iso_utc(now_unix_ms / 1000)));
    line(format!(
        "Last crash: {}",
        last_crash(&crash_log_path(config_path)).map_or_else(
            || "none on record".to_string(),
            |when| format!("{when}, see {CRASH_LOG_NAME}")
        )
    ));

    line(String::new());
    line("== State ==".into());
    line(format!(
        "Delay: {} | armed {} ms, on air {} ms (target {} ms)",
        ctrl.phase(),
        ctrl.armed_delay_ms(),
        ctrl.current_delay_ms(),
        ctrl.target_delay_ms()
    ));
    // The ring is made once at startup: a size saved since then waits for a
    // restart, so report the one running and name the saved one apart.
    let running_mb = ctrl.ring.capacity() / (1024 * 1024);
    let saved = if running_mb == settings.buffer_mb {
        String::new()
    } else {
        format!(
            " (saved {} MB, applies after a restart)",
            settings.buffer_mb
        )
    };
    line(format!(
        "Buffer: {running_mb} MB{saved}, {} ms filled of {} ms wanted, holds about {} ms at {} kbps",
        ctrl.buffer_fill_ms(),
        ctrl.target_buffer_ms(),
        ctrl.delay_capacity_ms(),
        ctrl.capacity_kbps()
    ));
    if let Some(short) = ctrl.capacity_short() {
        line(format!(
            "Capacity: runs at {} ms. {}",
            short.holds_ms, short.message
        ));
    }
    line(format!(
        "OBS: {} | {} / {}, {} kbps in ({} video), {} disconnects",
        if ctrl.ingest_alive() {
            "streaming"
        } else {
            "not streaming"
        },
        ctrl.video_codec().label(),
        ctrl.audio_codec().label(),
        ctrl.bitrate_kbps(),
        ctrl.video_bitrate_kbps(),
        ctrl.ingest_disconnects()
    ));
    if let Some(hold) = ctrl.hold_status() {
        line(format!(
            "Crash protection: holding ({:?}), {} s left of {} s",
            hold.reason,
            hold.remaining.as_secs(),
            hold.total.as_secs()
        ));
    }
    if let Some(tail) = ctrl.tail_left() {
        line(format!(
            "Delay tail: {} ms still airing after OBS left",
            tail.as_millis()
        ));
    }
    let conflicts = ctrl.hotkey_conflicts();
    if !conflicts.is_empty() {
        line(format!(
            "Hotkeys another app holds: {}",
            conflicts.join(", ")
        ));
    }
    let snapshot = ctrl.destination_snapshot();
    line(format!("Destinations: {}", settings.destinations.len()));
    for d in &settings.destinations {
        let live = snapshot.iter().find(|t| t.0 == d.id);
        let (alive, kbps, reconnects) = live.map_or((false, 0, 0), |t| (t.1, t.3, t.7));
        line(format!(
            "  - {} [{}{}] {} | {}, {} kbps, {} reconnects",
            d.name,
            d.platform,
            if d.wants_vertical() { ", vertical" } else { "" },
            if d.enabled { "on" } else { "off" },
            if alive { "connected" } else { "not connected" },
            kbps,
            reconnects
        ));
    }

    line(String::new());
    line("== Settings ==".into());
    line(settings.to_json(crate::autostart::is_enabled(), false));

    let lines: Vec<String> = ctrl.logs.lock().iter().cloned().collect();
    line(String::new());
    line(format!("== Log ({} lines) ==", lines.len()));
    for entry in &lines {
        line(with_wall_clock(entry, now_unix_ms, now_process_ms));
    }
    scrub(&out, &secrets_of(settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Destination;

    fn destination(platform: &str, key: &str, url: &str) -> Destination {
        Destination {
            id: platform.into(),
            name: platform.into(),
            enabled: true,
            platform: platform.into(),
            stream_key: key.into(),
            custom_egress_url: url.into(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        }
    }

    fn settings_with_secrets() -> Settings {
        let mut s = Settings::defaults();
        s.ingest_key = "ingest-secret-1".into();
        s.dock_token = "0123abcd4567ef89".into();
        s.phone.topic = "my-private-topic".into();
        s.destinations
            .push(destination("twitch", "live_123456_abcdefSECRET", ""));
        s.destinations.push(destination(
            "custom",
            "",
            "rtmp://edge.example/app/customKEY987",
        ));
        s.discord_channels
            .push(crate::integrations::model::DiscordChannel {
                id: "d1".into(),
                name: "alerts".into(),
                url: "https://discord.com/api/webhooks/1/webhookTOKENabc".into(),
            });
        s
    }

    /// Each place a secret is kept is on the list. The scrub test below
    /// can't show this: a URL is cut down to its host anyway, so a webhook
    /// would vanish from its text even if nobody listed it.
    #[test]
    fn every_secret_source_is_listed() {
        let mut s = Settings::defaults();
        s.ingest_key = "s-ingest".into();
        s.stream_key = "s-stream".into();
        s.custom_egress_url = "s-egress".into();
        s.dashboard_password_hash = "s-hash".into();
        s.dock_token = "s-dock".into();
        s.discord_webhook_url = "s-webhook".into();
        s.phone.topic = "s-topic".into();
        s.destinations
            .push(destination("custom", "s-dest-key", "s-dest-url"));
        s.discord_channels
            .push(crate::integrations::model::DiscordChannel {
                id: "d".into(),
                name: "d".into(),
                url: "s-channel".into(),
            });
        let listed = secrets_of(&s);
        for placeholder in [
            "s-ingest",
            "s-stream",
            "s-egress",
            "s-hash",
            "s-dock",
            "s-webhook",
            "s-topic",
            "s-dest-key",
            "s-dest-url",
            "s-channel",
        ] {
            assert!(
                listed.iter().any(|l| l == placeholder),
                "{placeholder} is not listed"
            );
        }
    }

    #[test]
    fn every_configured_secret_is_cut_out() {
        let s = settings_with_secrets();
        let secrets = secrets_of(&s);
        let text = "key live_123456_abcdefSECRET, ingest ingest-secret-1, dock \
                    0123abcd4567ef89, topic my-private-topic, url \
                    rtmp://edge.example/app/customKEY987 and \
                    https://discord.com/api/webhooks/1/webhookTOKENabc ok";
        let clean = scrub(text, &secrets);
        for leaked in [
            "abcdefSECRET",
            "ingest-secret-1",
            "0123abcd4567ef89",
            "my-private-topic",
            "customKEY987",
            "webhookTOKENabc",
        ] {
            assert!(!clean.contains(leaked), "{leaked} in {clean}");
        }
        assert!(clean.contains("key [redacted]"), "{clean}");
        assert!(clean.ends_with(" ok"), "{clean}");
    }

    /// A key that contains another key goes whole, not half-redacted.
    #[test]
    fn longer_secrets_are_cut_before_the_ones_they_contain() {
        let mut s = Settings::defaults();
        s.ingest_key = "abcd".into();
        s.dock_token = "abcdefgh".into();
        assert_eq!(
            secrets_of(&s),
            vec!["abcdefgh".to_string(), "abcd".to_string()]
        );
        assert_eq!(scrub("t=abcdefgh", &secrets_of(&s)), "t=[redacted]");
    }

    #[test]
    fn log_lines_get_the_wall_clock_time_they_were_logged() {
        // The process started at 2026-10-07T10:00:00Z and has run 60 s.
        let now_unix_ms = 1_791_367_200_000 + 60_000;
        let line = "[+  12.345s] [Main] connected";
        assert_eq!(
            with_wall_clock(line, now_unix_ms, 60_000),
            "2026-10-07T10:00:12Z [+  12.345s] [Main] connected"
        );
        assert_eq!(with_wall_clock("no stamp", now_unix_ms, 60_000), "no stamp");
    }

    #[test]
    fn a_crash_report_is_scrubbed_and_says_when() {
        let s = settings_with_secrets();
        let secrets = secrets_of(&s);
        let lines = vec!["[+   1.000s] [Main] key live_123456_abcdefSECRET refused".to_string()];
        let report = crash_report(
            &Crash {
                message: "index out of bounds in ingest-secret-1",
                location: "src/controller.rs:42",
                thread: "main",
                lines: Some(&lines),
                secrets: Some(&secrets),
            },
            1_791_367_200_000,
            5_000,
        );
        assert!(report
            .starts_with("==== InstantClone closed unexpectedly at 2026-10-07T10:00:00Z ===="));
        assert!(report.contains("At: src/controller.rs:42"));
        assert!(report.contains("Last 1 log lines:"));
        assert!(!report.contains("abcdefSECRET") && !report.contains("ingest-secret-1"));
        assert!(!report.contains('\u{2014}'), "no em dashes");

        // Without the secrets the log lines stay out rather than go raw.
        let blind = crash_report(
            &Crash {
                lines: Some(&lines),
                secrets: None,
                message: "boom",
                location: "",
                thread: "tray",
            },
            1_791_367_200_000,
            5_000,
        );
        assert!(!blind.contains("abcdefSECRET"));
        assert!(blind.contains("log lines left out"));
    }

    /// The next start says so once: the mark it leaves silences the start
    /// after, until a new crash is appended.
    #[test]
    fn a_crash_is_reported_once_on_the_next_start() {
        let first = format!("{CRASH_HEADER}2026-10-07T10:00:00Z ====\nPanic: x\n\n");
        assert_eq!(unreported_crash(&first), Some("2026-10-07T10:00:00Z"));
        let seen = format!("{first}{SEEN_MARK}, 2026-10-07T10:05:00Z\n");
        assert_eq!(unreported_crash(&seen), None);
        let again = format!("{seen}{CRASH_HEADER}2026-10-08T09:00:00Z ====\nPanic: y\n\n");
        assert_eq!(unreported_crash(&again), Some("2026-10-08T09:00:00Z"));
        assert_eq!(unreported_crash("nothing here"), None);
    }

    #[test]
    fn the_crash_log_stays_bounded() {
        let scratch = crate::test_support::TestDir::new("ic-crash-log");
        let path = scratch.join("crash.log");
        let big = "x".repeat(CRASH_LOG_MAX_BYTES as usize + 10);
        append_bounded(&path, &big);
        append_bounded(&path, "fresh report\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh report\n");
        append_bounded(&path, "second\n");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fresh report\nsecond\n"
        );
    }
}
