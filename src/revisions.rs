//! A change counter for each kind of data the dashboard and the dock draw
//! from their own endpoint, sent as `rev` in every state frame.
//!
//! The live numbers (delay, bitrate, health) already arrive in the state
//! feed. Everything else (settings, destinations, profiles, OBS setup...) is
//! fetched once by the page that shows it, so a change made in another
//! window, from the tray, or outside the app (Streamlabs installed, OBS
//! opened) stayed invisible until F5. Now a page fetches a kind of data
//! again exactly when its counter moves, and never polls for it.
//!
//! Two sources move the counters:
//!   * the settings: `follow_settings` compares each saved version with the
//!     one before and bumps only the kinds that differ;
//!   * the world outside: `follow_outside` samples what OBS and Streamlabs
//!     look like, every few seconds and only while a page is watching.
//!
//! A change the settings don't hold (an overlay file, a log line) calls
//! `bump` itself.

use crate::config::Settings;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// A kind of data a page fetches on its own. The names are the keys of
/// `rev` in the state feed, which the pages read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    /// Every setting no other topic owns.
    Config,
    Destinations,
    Profiles,
    /// Integrations and what they use: values, Discord channels, the phone,
    /// the alerts look, goals and timers.
    Integrations,
    /// The overlay files.
    Overlays,
    /// Each OBS on this PC: registered, open, its version.
    Obs,
    Streamlabs,
    /// The dashboard's log: a line was added, or it was cleared.
    Logs,
}

const TOPICS: [Topic; 8] = [
    Topic::Config,
    Topic::Destinations,
    Topic::Profiles,
    Topic::Integrations,
    Topic::Overlays,
    Topic::Obs,
    Topic::Streamlabs,
    Topic::Logs,
];

impl Topic {
    fn name(self) -> &'static str {
        match self {
            Topic::Config => "config",
            Topic::Destinations => "destinations",
            Topic::Profiles => "profiles",
            Topic::Integrations => "integrations",
            Topic::Overlays => "overlays",
            Topic::Obs => "obs",
            Topic::Streamlabs => "streamlabs",
            Topic::Logs => "logs",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

static REVS: [AtomicU64; TOPICS.len()] = [const { AtomicU64::new(0) }; TOPICS.len()];

/// Pages fetch `topic` again.
pub fn bump(topic: Topic) {
    REVS[topic.index()].fetch_add(1, Ordering::Relaxed);
}

/// Where this run's counters start, in ms since 1970: a page still open
/// across a restart sees every one differ and fetches it all again, even a
/// topic that moved the same number of times in both runs.
fn boot_offset() -> u64 {
    static OFFSET: OnceLock<u64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
    })
}

/// The counters as the state feed's `rev` object.
pub fn json() -> String {
    let fields: Vec<String> = TOPICS
        .iter()
        .map(|t| {
            format!(
                r#""{}":{}"#,
                t.name(),
                boot_offset() + REVS[t.index()].load(Ordering::Relaxed)
            )
        })
        .collect();
    format!("{{{}}}", fields.join(","))
}

// ---- who is watching ----

/// State feeds open right now (dashboards and docks).
static FEEDS: AtomicUsize = AtomicUsize::new(0);
/// When a page last polled the state instead, in ms since `epoch()`.
static LAST_POLL_MS: AtomicU64 = AtomicU64::new(0);
/// A polling page counts as watching for this long after its last poll.
const POLL_COUNTS_FOR: Duration = Duration::from_secs(5);

fn epoch() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    u64::try_from(epoch().elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Held by an open state feed: while any is, the outside is sampled.
pub struct Watching(());

impl Drop for Watching {
    fn drop(&mut self) {
        FEEDS.fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn watching() -> Watching {
    FEEDS.fetch_add(1, Ordering::Relaxed);
    Watching(())
}

/// A page polled the state (no feed): it watches for a few seconds.
pub fn note_poll() {
    // Stored +1 so a poll in the first millisecond still reads as one.
    LAST_POLL_MS.store(now_ms() + 1, Ordering::Relaxed);
}

fn is_watched() -> bool {
    if FEEDS.load(Ordering::Relaxed) > 0 {
        return true;
    }
    let last = LAST_POLL_MS.load(Ordering::Relaxed);
    last != 0 && now_ms().saturating_sub(last) < POLL_COUNTS_FOR.as_millis() as u64
}

// ---- the settings ----

/// Bump the topics each new version of the settings changed.
pub async fn follow_settings(mut settings: watch::Receiver<Settings>) {
    let mut before = settings.borrow_and_update().clone();
    while settings.changed().await.is_ok() {
        let after = settings.borrow_and_update().clone();
        for topic in changed_topics(&before, &after) {
            bump(topic);
        }
        before = after;
    }
}

/// The topics whose data differs between two versions of the settings.
fn changed_topics(before: &Settings, after: &Settings) -> Vec<Topic> {
    let mut changed = Vec::new();
    if before.destinations != after.destinations {
        changed.push(Topic::Destinations);
    }
    if before.profiles != after.profiles {
        changed.push(Topic::Profiles);
    }
    if integrations_changed(before, after) {
        changed.push(Topic::Integrations);
    }
    if before.overlays_dir != after.overlays_dir {
        changed.push(Topic::Overlays);
    }
    if config_changed(before, after) {
        changed.push(Topic::Config);
    }
    changed
}

/// Whether what the integrations tab draws from the settings changed.
fn integrations_changed(before: &Settings, after: &Settings) -> bool {
    before.integrations != after.integrations
        || before.user_values != after.user_values
        || before.discord_channels != after.discord_channels
        || before.phone != after.phone
        || before.alerts_look != after.alerts_look
        || before.goals != after.goals
        || before.timers != after.timers
        || before.twitch_client_id != after.twitch_client_id
        || before.twitch_shared_chat != after.twitch_shared_chat
}

/// Whether anything changed outside the parts the other topics own, the
/// delay the state feed already carries and the dock layouts (each dock
/// keeps its own in step). Compared as a whole, not field by field, so a
/// setting added later counts here without anyone listing it.
fn config_changed(before: &Settings, after: &Settings) -> bool {
    let mut rest = after.clone();
    rest.destinations.clone_from(&before.destinations);
    rest.profiles.clone_from(&before.profiles);
    rest.docks.clone_from(&before.docks);
    rest.integrations.clone_from(&before.integrations);
    rest.user_values.clone_from(&before.user_values);
    rest.discord_channels.clone_from(&before.discord_channels);
    rest.phone.clone_from(&before.phone);
    rest.alerts_look.clone_from(&before.alerts_look);
    rest.goals.clone_from(&before.goals);
    rest.timers.clone_from(&before.timers);
    rest.target_delay_ms = before.target_delay_ms;
    rest.armed_delay_ms = before.armed_delay_ms;
    rest != *before
}

// ---- the world outside ----

/// How often the outside is looked at while a page is watching. Each look
/// reads a few small files and the process list, which alone takes about 20 ms
/// on a busy PC: 1% of a core every 2 s, for something that rarely changes.
const OUTSIDE_EVERY: Duration = Duration::from_secs(4);

/// Bump a topic when its sample changes. `sample` answers each outside topic
/// with what its endpoint would answer now; it runs on a blocking thread.
pub async fn follow_outside<F>(sample: F)
where
    F: Fn() -> Vec<(Topic, String)> + Send + Sync + 'static,
{
    let sample = Arc::new(sample);
    let mut seen = Seen::default();
    // The baseline comes before any page, so a page never misses a change
    // made between loading its data and the first look.
    let first = sample.clone();
    if let Ok(now) = tokio::task::spawn_blocking(move || first()).await {
        seen.changed(now);
    }
    loop {
        tokio::time::sleep(OUTSIDE_EVERY).await;
        if !is_watched() {
            continue;
        }
        let sample = sample.clone();
        let Ok(now) = tokio::task::spawn_blocking(move || sample()).await else {
            continue;
        };
        for topic in seen.changed(now) {
            bump(topic);
        }
    }
}

/// The last sample of each outside topic.
#[derive(Default)]
struct Seen([Option<String>; TOPICS.len()]);

impl Seen {
    /// The topics whose sample differs from the last one. A first sample
    /// only sets the baseline.
    fn changed(&mut self, now: Vec<(Topic, String)>) -> Vec<Topic> {
        let mut changed = Vec::new();
        for (topic, value) in now {
            let slot = &mut self.0[topic.index()];
            if slot.as_ref().is_some_and(|last| *last != value) {
                changed.push(topic);
            }
            *slot = Some(value);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Settings {
        Settings::defaults()
    }

    #[test]
    fn json_lists_every_topic_once() {
        let text = json();
        for topic in TOPICS {
            assert_eq!(
                text.matches(&format!(r#""{}":"#, topic.name())).count(),
                1,
                "{text}"
            );
        }
        assert!(crate::json::parse(&text).is_ok(), "{text}");
    }

    /// A page open across a restart compares against these: a fresh run has
    /// to differ even in a topic that never moved.
    #[test]
    fn counters_start_where_no_earlier_run_could_have_reached() {
        let first = crate::json::parse(&json()).unwrap();
        let config = first
            .get("config")
            .and_then(crate::json::Value::as_f64)
            .unwrap();
        assert!(config >= boot_offset() as f64 && boot_offset() > 1_000_000_000_000);
    }

    #[test]
    fn bump_moves_only_its_topic() {
        let read = |t: Topic| REVS[t.index()].load(Ordering::Relaxed);
        let before = read(Topic::Profiles);
        bump(Topic::Profiles);
        assert_eq!(read(Topic::Profiles), before + 1);
    }

    #[test]
    fn same_settings_change_nothing() {
        assert!(changed_topics(&base(), &base()).is_empty());
    }

    #[test]
    fn a_profile_change_is_only_profiles() {
        let mut after = base();
        after.profiles.push(crate::config::DelayProfile {
            name: "Scrim".into(),
            delay_ms: 20_000,
        });
        assert_eq!(changed_topics(&base(), &after), vec![Topic::Profiles]);
    }

    #[test]
    fn a_plain_setting_is_config() {
        let mut after = base();
        after.buffer_mb += 1;
        assert_eq!(changed_topics(&base(), &after), vec![Topic::Config]);
    }

    #[test]
    fn arming_a_delay_changes_nothing_the_pages_fetch() {
        // The state feed carries the delay: refetching the config for it
        // would reload the System tab on every arm.
        let mut after = base();
        after.armed_delay_ms += 5_000;
        after.target_delay_ms += 5_000;
        assert!(changed_topics(&base(), &after).is_empty());
    }

    #[test]
    fn integrations_parts_are_integrations() {
        let mut after = base();
        after.twitch_shared_chat = !after.twitch_shared_chat;
        let changed = changed_topics(&base(), &after);
        assert!(changed.contains(&Topic::Integrations), "{changed:?}");
    }

    #[test]
    fn a_new_overlays_folder_is_config_and_overlays() {
        let mut after = base();
        after.overlays_dir = "elsewhere".into();
        assert_eq!(
            changed_topics(&base(), &after),
            vec![Topic::Overlays, Topic::Config]
        );
    }

    #[test]
    fn first_sample_is_a_baseline() {
        let mut seen = Seen::default();
        assert!(seen.changed(vec![(Topic::Obs, "a".into())]).is_empty());
        assert!(seen.changed(vec![(Topic::Obs, "a".into())]).is_empty());
        assert_eq!(
            seen.changed(vec![(Topic::Obs, "b".into())]),
            vec![Topic::Obs]
        );
    }

    #[test]
    fn samples_are_kept_per_topic() {
        let mut seen = Seen::default();
        seen.changed(vec![
            (Topic::Obs, "a".into()),
            (Topic::Streamlabs, "x".into()),
        ]);
        let changed = seen.changed(vec![
            (Topic::Obs, "a".into()),
            (Topic::Streamlabs, "y".into()),
        ]);
        assert_eq!(changed, vec![Topic::Streamlabs]);
    }
}
