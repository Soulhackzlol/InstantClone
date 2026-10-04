//! One stream, from the moment OBS starts sending to the moment it ends
//! for good, read from the controller's events: when it started, how often
//! OBS crashed, and how each destination held up.
//!
//! A crash OBS comes back from is the same stream: crash protection kept
//! the destinations up, so viewers saw one stream. It ends when the
//! reconnect screen runs out or is ended, when OBS stops on purpose, or
//! when OBS drops without crash protection and isn't back within
//! `END_GRACE`: OBS reconnects on its own after a network blip, and that
//! blip must not become a second stream with its own reports and pings.
//!
//! Destinations are tracked by name, the way events name them. What this
//! keeps feeds the smart destination events (`destination_recovered`,
//! `_still_down`, `_unstable`, `_steady`) and the end-of-stream report.

use super::event::{Event, EventKind};
use super::host::{fmt_duration, fmt_span};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How long OBS has to come back after dropping unprotected before the
/// stream counts as over. OBS's own reconnect tries for longer, but a
/// stream that is gone a minute has ended for its viewers.
pub const END_GRACE: Duration = Duration::from_secs(60);
/// Drops remembered per destination per stream.
const MAX_DROPS: usize = 1000;

#[derive(Default)]
pub struct Session {
    /// When the stream started: (monotonic, Unix ms). None between streams.
    started: Option<(Instant, u64)>,
    /// When OBS dropped without crash protection: the stream ends
    /// `END_GRACE` later unless OBS comes back first.
    dropped_at: Option<Instant>,
    crashes: u32,
    /// OBS dropping without crash protection and coming back: how often,
    /// and for how long in all.
    obs_drops: u32,
    obs_down: Duration,
    destinations: BTreeMap<String, Health>,
}

/// How one destination is doing in this stream.
#[derive(Clone, Debug, Default)]
pub struct Health {
    pub platform: String,
    pub reason: String,
    pub down_since: Option<Instant>,
    pub live_since: Option<Instant>,
    /// The latest drops this stream, oldest first (at most `MAX_DROPS`).
    pub drops: Vec<Instant>,
    /// Every drop this stream, however many: what reports and alerts count.
    pub drop_count: usize,
    pub down_total: Duration,
}

impl Health {
    pub fn drops_within(&self, window: Duration, now: Instant) -> usize {
        self.drops
            .iter()
            .filter(|t| now.duration_since(**t) <= window)
            .count()
    }
}

impl Session {
    pub fn is_on(&self) -> bool {
        self.started.is_some()
    }

    /// Unix ms the stream started at, 0 between streams.
    pub fn started_ms(&self) -> u64 {
        self.started.map_or(0, |(_, ms)| ms)
    }

    pub fn uptime(&self, now: Instant) -> Duration {
        self.started
            .map_or(Duration::ZERO, |(at, _)| now.duration_since(at))
    }

    pub fn destinations(&self) -> &BTreeMap<String, Health> {
        &self.destinations
    }

    /// Whether the stream is waiting to see if OBS comes back.
    pub fn is_ending(&self) -> bool {
        self.dropped_at.is_some()
    }

    /// The stream ending once OBS stayed away `END_GRACE`: its report, as
    /// of when OBS dropped. Call every tick while `is_ending`.
    pub fn expire(&mut self, now: Instant, timeline: (usize, String)) -> Vec<Event> {
        match self.dropped_at {
            Some(at) if now.duration_since(at) >= END_GRACE => self.end(at, timeline),
            _ => Vec::new(),
        }
    }

    /// Follow one controller event. Returns the events it makes: a stream
    /// starting or ending, a destination coming back. `highlights` and
    /// `chapters` are the timeline's, for the end-of-stream report.
    pub fn observe(
        &mut self,
        event: &Event,
        now: Instant,
        unix_ms: u64,
        timeline: (usize, String),
    ) -> Vec<Event> {
        match event.kind {
            EventKind::ObsConnected if self.started.is_none() => {
                self.started = Some((now, unix_ms));
                vec![Event::new(EventKind::StreamStarted)]
            }
            // Back within the grace: the same stream carries on, and the
            // report still says OBS was gone.
            EventKind::ObsConnected => {
                if let Some(at) = self.dropped_at.take() {
                    self.obs_drops += 1;
                    self.obs_down += now.duration_since(at);
                }
                Vec::new()
            }
            // Stopping on purpose with "hold on every disconnect" opens a
            // hold too, but that isn't a crash.
            EventKind::HoldOpened if self.started.is_some() => {
                if event.var("reason") != Some("stopped") {
                    self.crashes += 1;
                }
                Vec::new()
            }
            EventKind::ObsDisconnected if event.var("protected") == Some("no") => {
                if event.var("stopped") == Some("yes") {
                    self.end(now, timeline)
                } else {
                    self.dropped_at.get_or_insert(now);
                    Vec::new()
                }
            }
            EventKind::HoldExpired | EventKind::HoldEnded => self.end(now, timeline),
            // Between streams nothing is watched: a late event from the last
            // one must not open the next one's report.
            EventKind::DestinationDropped if self.started.is_some() => {
                self.dropped(event, now);
                Vec::new()
            }
            EventKind::DestinationLive if self.started.is_some() => self.came_back(event, now),
            _ => Vec::new(),
        }
    }

    fn dropped(&mut self, event: &Event, now: Instant) {
        let health = self.health(event);
        health.reason = event.var("reason").unwrap_or("").to_string();
        // A drop while already down (a retry failing) is the same outage.
        if health.down_since.is_none() {
            health.down_since = Some(now);
            // A destination flapping all stream long keeps its latest drops.
            if health.drops.len() >= MAX_DROPS {
                health.drops.remove(0);
            }
            health.drops.push(now);
            health.drop_count += 1;
        }
        health.live_since = None;
    }

    fn came_back(&mut self, event: &Event, now: Instant) -> Vec<Event> {
        let health = self.health(event);
        health.live_since = Some(now);
        let Some(since) = health.down_since.take() else {
            return Vec::new();
        };
        let down = now.duration_since(since);
        health.down_total += down;
        let platform = health.platform.clone();
        vec![Event::new(EventKind::DestinationRecovered)
            .with("destination", event.var("destination").unwrap_or(""))
            .with("platform", platform)
            .with("down_for", fmt_duration(down.as_millis() as u64))
            .with("down_s", down.as_secs().to_string())]
    }

    fn health(&mut self, event: &Event) -> &mut Health {
        let name = event.var("destination").unwrap_or("").to_string();
        let health = self.destinations.entry(name).or_default();
        if let Some(platform) = event.var("platform") {
            health.platform = platform.to_string();
        }
        health
    }

    /// The stream ended at `now`: the report, and a clean slate for the
    /// next one.
    fn end(&mut self, now: Instant, (highlights, chapters): (usize, String)) -> Vec<Event> {
        self.dropped_at = None;
        let Some((started, _)) = self.started.take() else {
            return Vec::new();
        };
        let drops: usize = self.destinations.values().map(|h| h.drop_count).sum();
        let mut lines: Vec<String> = Vec::new();
        if self.obs_drops > 0 {
            lines.push(format!(
                "OBS: dropped {} without crash protection, gone {}",
                if self.obs_drops == 1 {
                    "once".to_string()
                } else {
                    format!("{} times", self.obs_drops)
                },
                fmt_duration(self.obs_down.as_millis() as u64)
            ));
        }
        lines.extend(
            self.destinations
                .iter()
                .map(|(name, h)| report_line(name, h, now)),
        );
        let report = lines.join("\n");
        let event = Event::new(EventKind::StreamEnded)
            .with(
                "duration",
                fmt_span(now.duration_since(started).as_millis() as u64),
            )
            .with("crashes", (self.crashes + self.obs_drops).to_string())
            .with("drops", drops.to_string())
            .with("highlights", highlights.to_string())
            .with("report", report)
            .with("chapters", chapters);
        self.crashes = 0;
        self.obs_drops = 0;
        self.obs_down = Duration::ZERO;
        self.destinations.clear();
        vec![event]
    }
}

/// `YouTube: 2 drops, down 1m 12s` or `Kick: steady`.
fn report_line(name: &str, h: &Health, now: Instant) -> String {
    let down = h.down_total
        + h.down_since
            .map_or(Duration::ZERO, |s| now.duration_since(s));
    match h.drop_count {
        0 => format!("{name}: steady"),
        1 => format!(
            "{name}: 1 drop, down {}",
            fmt_duration(down.as_millis() as u64)
        ),
        n => format!(
            "{name}: {n} drops, down {}",
            fmt_duration(down.as_millis() as u64)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest(kind: EventKind, name: &str) -> Event {
        Event::new(kind)
            .with("destination", name)
            .with("platform", "youtube")
            .with("reason", "timed out")
    }

    fn none() -> (usize, String) {
        (0, String::new())
    }

    #[test]
    fn a_stream_starts_once_and_survives_a_crash() {
        let mut s = Session::default();
        let t0 = Instant::now();
        let kinds = |events: Vec<Event>| events.iter().map(|e| e.kind).collect::<Vec<_>>();
        assert_eq!(
            kinds(s.observe(&Event::new(EventKind::ObsConnected), t0, 1000, none())),
            [EventKind::StreamStarted]
        );
        s.observe(&Event::new(EventKind::HoldOpened), t0, 0, none());
        let protected = Event::new(EventKind::ObsDisconnected).with("protected", "yes");
        assert!(s.observe(&protected, t0, 0, none()).is_empty());
        assert!(
            s.observe(&Event::new(EventKind::ObsConnected), t0, 0, none())
                .is_empty(),
            "OBS back from a crash is the same stream"
        );
        assert_eq!(s.started_ms(), 1000);
        let stopped = Event::new(EventKind::ObsDisconnected)
            .with("stopped", "yes")
            .with("protected", "no");
        let end = s.observe(
            &stopped,
            t0 + Duration::from_secs(3600),
            0,
            (2, "0:00 Start".into()),
        );
        assert_eq!(end[0].kind, EventKind::StreamEnded);
        assert_eq!(end[0].var("duration"), Some("1h 00m"));
        assert_eq!(end[0].var("crashes"), Some("1"));
        assert_eq!(end[0].var("highlights"), Some("2"));
        assert!(!s.is_on());
    }

    #[test]
    fn a_blip_without_crash_protection_is_still_one_stream() {
        let mut s = Session::default();
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        s.observe(&Event::new(EventKind::ObsConnected), t0, 0, none());
        let dropped = Event::new(EventKind::ObsDisconnected)
            .with("stopped", "no")
            .with("protected", "no");
        assert!(s.observe(&dropped, at(100), 0, none()).is_empty());
        assert!(s.is_ending());
        assert!(
            s.expire(at(130), none()).is_empty(),
            "still within the grace"
        );
        let back = s.observe(&Event::new(EventKind::ObsConnected), at(140), 0, none());
        assert!(back.is_empty(), "OBS came back: no second stream");
        assert!(!s.is_ending() && s.is_on());
        // Gone for good this time: it ends, as of when OBS dropped.
        s.observe(&dropped, at(200), 0, none());
        let end = s.expire(at(200) + END_GRACE, none());
        assert_eq!(end[0].kind, EventKind::StreamEnded);
        assert_eq!(end[0].var("duration"), Some("3m"));
        assert!(!s.is_on() && !s.is_ending());
    }

    #[test]
    fn an_unprotected_drop_obs_came_back_from_is_in_the_report() {
        let mut s = Session::default();
        let t0 = Instant::now();
        s.observe(&Event::new(EventKind::ObsConnected), t0, 0, none());
        let dropped = Event::new(EventKind::ObsDisconnected)
            .with("stopped", "no")
            .with("protected", "no");
        s.observe(&dropped, t0 + Duration::from_secs(60), 0, none());
        s.observe(
            &Event::new(EventKind::ObsConnected),
            t0 + Duration::from_secs(80),
            0,
            none(),
        );
        let end = s.observe(&Event::new(EventKind::HoldEnded), t0, 0, none());
        assert_eq!(end[0].var("crashes"), Some("1"));
        assert!(end[0].var("report").is_some_and(
            |r| r.starts_with("OBS: dropped once without crash protection, gone 20 s")
        ));
    }

    #[test]
    fn stopping_on_purpose_into_a_hold_is_not_a_crash() {
        let mut s = Session::default();
        let t0 = Instant::now();
        s.observe(&Event::new(EventKind::ObsConnected), t0, 0, none());
        let stopped = Event::new(EventKind::HoldOpened).with("reason", "stopped");
        s.observe(&stopped, t0, 0, none());
        let end = s.observe(&Event::new(EventKind::HoldEnded), t0, 0, none());
        assert_eq!(end[0].var("crashes"), Some("0"));
    }

    #[test]
    fn a_destination_coming_back_says_how_long_it_was_gone() {
        let mut s = Session::default();
        let t0 = Instant::now();
        s.observe(&Event::new(EventKind::ObsConnected), t0, 0, none());
        s.observe(&dest(EventKind::DestinationLive, "YouTube"), t0, 0, none());
        s.observe(
            &dest(EventKind::DestinationDropped, "YouTube"),
            t0,
            0,
            none(),
        );
        // A failed retry while down is the same outage, not a second drop.
        s.observe(
            &dest(EventKind::DestinationDropped, "YouTube"),
            t0,
            0,
            none(),
        );
        let back = s.observe(
            &dest(EventKind::DestinationLive, "YouTube"),
            t0 + Duration::from_secs(72),
            0,
            none(),
        );
        assert_eq!(back[0].kind, EventKind::DestinationRecovered);
        assert_eq!(back[0].var("down_for"), Some("1m 12s"));
        assert_eq!(back[0].var("down_s"), Some("72"));
        assert_eq!(s.destinations()["YouTube"].drop_count, 1);
    }

    #[test]
    fn the_report_names_every_destination() {
        let mut s = Session::default();
        let t0 = Instant::now();
        s.observe(&Event::new(EventKind::ObsConnected), t0, 0, none());
        for (at, kind) in [
            (0, EventKind::DestinationDropped),
            (30, EventKind::DestinationLive),
            (60, EventKind::DestinationDropped),
            (90, EventKind::DestinationLive),
        ] {
            s.observe(
                &dest(kind, "YouTube"),
                t0 + Duration::from_secs(at),
                0,
                none(),
            );
        }
        s.observe(&dest(EventKind::DestinationLive, "Kick"), t0, 0, none());
        let end = s.observe(&Event::new(EventKind::HoldExpired), t0, 0, none());
        assert_eq!(
            end[0].var("report"),
            Some("Kick: steady\nYouTube: 2 drops, down 1m 00s")
        );
        assert_eq!(end[0].var("drops"), Some("2"));
        assert!(s.destinations().is_empty(), "a clean slate for the next");
    }

    #[test]
    fn drops_are_counted_within_a_window() {
        let t0 = Instant::now();
        let h = Health {
            drops: vec![
                t0,
                t0 + Duration::from_secs(300),
                t0 + Duration::from_secs(590),
            ],
            ..Health::default()
        };
        let now = t0 + Duration::from_secs(600);
        assert_eq!(h.drops_within(Duration::from_secs(600), now), 3);
        assert_eq!(h.drops_within(Duration::from_secs(60), now), 1);
    }

    /// A stream day for one kind of streamer: events in order, each `secs`
    /// after the start. Returns every event the session made, with when.
    struct Day {
        session: Session,
        t0: Instant,
        made: Vec<(u64, Event)>,
    }

    impl Day {
        fn new() -> Day {
            Day {
                session: Session::default(),
                t0: Instant::now(),
                made: Vec::new(),
            }
        }

        fn at(&mut self, secs: u64, event: Event) {
            let now = self.t0 + Duration::from_secs(secs);
            let mut out = self.session.expire(now, none());
            out.extend(self.session.observe(&event, now, 0, none()));
            self.made.extend(out.into_iter().map(|e| (secs, e)));
        }

        fn tick(&mut self, secs: u64) {
            let now = self.t0 + Duration::from_secs(secs);
            let out = self.session.expire(now, none());
            self.made.extend(out.into_iter().map(|e| (secs, e)));
        }

        fn kinds(&self) -> Vec<EventKind> {
            self.made.iter().map(|(_, e)| e.kind).collect()
        }

        fn ends(&self) -> Vec<&Event> {
            self.made
                .iter()
                .filter(|(_, e)| e.kind == EventKind::StreamEnded)
                .map(|(_, e)| e)
                .collect()
        }
    }

    fn obs_gone(stopped: bool, protected: bool) -> Event {
        Event::new(EventKind::ObsDisconnected)
            .with("stopped", if stopped { "yes" } else { "no" })
            .with("protected", if protected { "yes" } else { "no" })
    }

    fn hold(reason: &str) -> Event {
        Event::new(EventKind::HoldOpened).with("reason", reason)
    }

    /// Crash protection on, two platforms: OBS crashes once and comes
    /// back, YouTube drops and recovers, then the hold after a stop runs
    /// out. One stream, one crash, the YouTube outage in the report.
    #[test]
    fn day_of_a_careful_streamer() {
        let mut day = Day::new();
        day.at(0, Event::new(EventKind::ObsConnected));
        day.at(600, dest(EventKind::DestinationDropped, "YouTube"));
        day.at(645, dest(EventKind::DestinationLive, "YouTube"));
        day.at(1800, obs_gone(false, true));
        day.at(1800, hold("crash"));
        day.at(1830, Event::new(EventKind::ObsConnected));
        day.at(1830, Event::new(EventKind::ObsBack));
        day.at(3600, obs_gone(true, false));

        use EventKind::*;
        assert_eq!(
            day.kinds(),
            [StreamStarted, DestinationRecovered, StreamEnded],
            "one stream, its YouTube outage said once"
        );
        let end = day.ends()[0];
        assert_eq!(end.var("crashes"), Some("1"));
        assert_eq!(end.var("drops"), Some("1"));
        assert_eq!(end.var("duration"), Some("1h 00m"));
        assert_eq!(end.var("report"), Some("YouTube: 1 drop, down 45 s"));
    }

    /// No crash protection: OBS's connection blips twice and comes back
    /// within the grace (still one stream, both in the report), then it
    /// crashes for good and the stream ends as of that crash.
    #[test]
    fn day_of_a_streamer_without_crash_protection() {
        let mut day = Day::new();
        day.at(0, Event::new(EventKind::ObsConnected));
        day.at(300, obs_gone(false, false));
        day.at(310, Event::new(EventKind::ObsConnected));
        day.at(900, obs_gone(false, false));
        day.at(950, Event::new(EventKind::ObsConnected));
        day.at(1200, obs_gone(false, false));
        day.tick(1259);
        assert!(day.ends().is_empty(), "still within the grace");
        day.tick(1260);

        let ends = day.ends();
        assert_eq!(ends.len(), 1, "one stream");
        assert_eq!(ends[0].var("duration"), Some("20m"), "ends at the crash");
        assert_eq!(ends[0].var("crashes"), Some("2"));
        assert!(ends[0].var("report").is_some_and(
            |r| r.starts_with("OBS: dropped 2 times without crash protection, gone 1m")
        ));
        day.at(1500, Event::new(EventKind::ObsConnected));
        assert_eq!(
            day.kinds().last(),
            Some(&EventKind::StreamStarted),
            "OBS back after the grace is a new stream"
        );
    }

    /// Stop and go: stops in OBS, fixes a setting, starts again 20 s
    /// later. Two streams (an app restart is treated the same way).
    #[test]
    fn day_of_a_stop_and_go_streamer() {
        let mut day = Day::new();
        day.at(0, Event::new(EventKind::ObsConnected));
        day.at(120, obs_gone(true, false));
        day.at(140, Event::new(EventKind::ObsConnected));
        day.at(3000, obs_gone(true, false));

        use EventKind::*;
        assert_eq!(
            day.kinds(),
            [StreamStarted, StreamEnded, StreamStarted, StreamEnded]
        );
        assert!(day.ends().iter().all(|e| e.var("crashes") == Some("0")));
    }

    /// "Hold on every disconnect": a stop on purpose keeps the stream up,
    /// OBS comes back (same stream, no crash), then the second stop's hold
    /// is ended from the dashboard.
    #[test]
    fn day_of_a_streamer_who_holds_every_disconnect() {
        let mut day = Day::new();
        day.at(0, Event::new(EventKind::ObsConnected));
        day.at(600, obs_gone(true, true));
        day.at(600, hold("stopped"));
        day.at(640, Event::new(EventKind::ObsConnected));
        day.at(1200, obs_gone(true, true));
        day.at(1200, hold("stopped"));
        day.at(1260, Event::new(EventKind::HoldEnded));

        let ends = day.ends();
        assert_eq!(ends.len(), 1, "one stream");
        assert_eq!(ends[0].var("crashes"), Some("0"), "stops aren't crashes");
        assert_eq!(ends[0].var("duration"), Some("21m"));
    }

    /// A platform that drops all stream long (a bad Kick edge): every drop
    /// counts, the memory of them stays bounded.
    #[test]
    fn day_of_a_streamer_with_a_flaky_platform() {
        let mut day = Day::new();
        day.at(0, Event::new(EventKind::ObsConnected));
        for n in 0..1_500u64 {
            day.at(1 + n * 4, dest(EventKind::DestinationDropped, "Kick"));
            day.at(3 + n * 4, dest(EventKind::DestinationLive, "Kick"));
        }
        assert_eq!(day.session.destinations()["Kick"].drops.len(), MAX_DROPS);
        day.at(7_000, obs_gone(true, false));

        let end = day.ends()[0];
        assert_eq!(end.var("drops"), Some("1500"));
        assert_eq!(end.var("report"), Some("Kick: 1500 drops, down 50m 00s"));
    }

    /// Late news between streams (a platform drop after the stop, a hold
    /// ending twice) opens nothing and ends nothing.
    #[test]
    fn news_between_streams_is_ignored() {
        let mut day = Day::new();
        day.at(0, Event::new(EventKind::ObsConnected));
        day.at(60, obs_gone(true, false));
        day.at(70, dest(EventKind::DestinationDropped, "YouTube"));
        day.at(80, dest(EventKind::DestinationLive, "YouTube"));
        day.at(90, Event::new(EventKind::HoldEnded));
        day.at(95, Event::new(EventKind::HoldExpired));

        use EventKind::*;
        assert_eq!(day.kinds(), [StreamStarted, StreamEnded]);
        day.at(100, Event::new(EventKind::ObsConnected));
        day.at(200, obs_gone(true, false));
        assert_eq!(
            day.ends()[1].var("report"),
            Some(""),
            "the next report starts clean"
        );
    }
}
