//! What can happen in InstantClone that an integration can react to.
//!
//! Each kind has a stable id (saved in integrations, so never renamed), a
//! label and group for the dashboard, and the variables it carries with a
//! sample value each. The samples fill previews and test runs, so a test
//! message reads like the real one.

use crate::json::{self, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    ObsConnected,
    ObsDisconnected,
    EbDetected,
    HoldOpened,
    ObsBack,
    HoldExpired,
    HoldEnded,
    DestinationLive,
    DestinationDropped,
    AllDestinationsDown,
    DelayOn,
    DelayOff,
    DelayChanged,
    DelayArmed,
    DelayReady,
    DelayDisarmed,
    // Made by the engine from the events above (see `engine::Session`).
    StreamStarted,
    StreamEnded,
    OnAirChanged,
    TimelineUpdated,
    DestinationRecovered,
    DestinationStillDown,
    DestinationUnstable,
    DestinationSteady,
}

/// One variable an event carries: its name and a sample value.
pub type VarSpec = (&'static str, &'static str);

/// A setting of the trigger rather than a value to match: (name, default,
/// label). Saved among the trigger's filters, read by the engine.
pub type ParamSpec = (&'static str, &'static str, &'static str);

/// The most any trigger setting can be. See `EventKind::param`.
const MAX_PARAM: f64 = 1_000_000.0;

impl EventKind {
    pub const ALL: [EventKind; 24] = [
        EventKind::ObsConnected,
        EventKind::ObsDisconnected,
        EventKind::EbDetected,
        EventKind::StreamStarted,
        EventKind::StreamEnded,
        EventKind::OnAirChanged,
        EventKind::TimelineUpdated,
        EventKind::HoldOpened,
        EventKind::ObsBack,
        EventKind::HoldExpired,
        EventKind::HoldEnded,
        EventKind::DestinationLive,
        EventKind::DestinationDropped,
        EventKind::DestinationRecovered,
        EventKind::DestinationStillDown,
        EventKind::DestinationUnstable,
        EventKind::DestinationSteady,
        EventKind::AllDestinationsDown,
        // The delay's life, in order: armed, ready, on air, changed, off.
        EventKind::DelayArmed,
        EventKind::DelayReady,
        EventKind::DelayOn,
        EventKind::DelayChanged,
        EventKind::DelayOff,
        EventKind::DelayDisarmed,
    ];

    pub fn id(self) -> &'static str {
        match self {
            EventKind::ObsConnected => "obs_connected",
            EventKind::ObsDisconnected => "obs_disconnected",
            EventKind::EbDetected => "eb_detected",
            EventKind::HoldOpened => "hold_opened",
            EventKind::ObsBack => "obs_back",
            EventKind::HoldExpired => "hold_expired",
            EventKind::HoldEnded => "hold_ended",
            EventKind::DestinationLive => "destination_live",
            EventKind::DestinationDropped => "destination_dropped",
            EventKind::AllDestinationsDown => "all_destinations_down",
            EventKind::DelayOn => "delay_on",
            EventKind::DelayOff => "delay_off",
            EventKind::DelayChanged => "delay_changed",
            EventKind::DelayArmed => "delay_armed",
            EventKind::DelayReady => "delay_ready",
            EventKind::DelayDisarmed => "delay_disarmed",
            EventKind::StreamStarted => "stream_started",
            EventKind::StreamEnded => "stream_ended",
            EventKind::OnAirChanged => "onair_changed",
            EventKind::TimelineUpdated => "timeline_updated",
            EventKind::DestinationRecovered => "destination_recovered",
            EventKind::DestinationStillDown => "destination_still_down",
            EventKind::DestinationUnstable => "destination_unstable",
            EventKind::DestinationSteady => "destination_steady",
        }
    }

    pub fn from_id(id: &str) -> Option<EventKind> {
        EventKind::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            EventKind::ObsConnected => "OBS starts streaming",
            EventKind::ObsDisconnected => "OBS stops or drops",
            EventKind::EbDetected => "Enhanced Broadcasting detected",
            EventKind::HoldOpened => "OBS crashes",
            EventKind::ObsBack => "OBS is back",
            EventKind::HoldExpired => "The hold runs out",
            EventKind::HoldEnded => "You end the hold early",
            EventKind::DestinationLive => "A platform goes live",
            EventKind::DestinationDropped => "A platform drops",
            EventKind::AllDestinationsDown => "Every platform is down",
            EventKind::DelayOn => "The delay turns on",
            EventKind::DelayOff => "The delay turns off",
            EventKind::DelayChanged => "The delay changes",
            EventKind::DelayArmed => "You arm a delay",
            EventKind::DelayReady => "The armed delay is ready",
            EventKind::DelayDisarmed => "You cancel an armed delay",
            EventKind::StreamStarted => "A stream starts",
            EventKind::StreamEnded => "A stream ends",
            EventKind::OnAirChanged => "The on-air state changes",
            EventKind::TimelineUpdated => "The timeline gets a line",
            EventKind::DestinationRecovered => "A platform comes back",
            EventKind::DestinationStillDown => "A platform stays down",
            EventKind::DestinationUnstable => "A platform keeps dropping",
            EventKind::DestinationSteady => "A platform is steady again",
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            EventKind::ObsConnected
            | EventKind::ObsDisconnected
            | EventKind::EbDetected
            | EventKind::StreamStarted
            | EventKind::StreamEnded
            | EventKind::OnAirChanged
            | EventKind::TimelineUpdated => "Stream",
            EventKind::HoldOpened
            | EventKind::ObsBack
            | EventKind::HoldExpired
            | EventKind::HoldEnded => "Crash protection",
            EventKind::DestinationLive
            | EventKind::DestinationDropped
            | EventKind::DestinationRecovered
            | EventKind::DestinationStillDown
            | EventKind::DestinationUnstable
            | EventKind::DestinationSteady
            | EventKind::AllDestinationsDown => "Platforms",
            EventKind::DelayOn
            | EventKind::DelayOff
            | EventKind::DelayChanged
            | EventKind::DelayArmed
            | EventKind::DelayReady
            | EventKind::DelayDisarmed => "Delay",
        }
    }

    /// Variables the event itself carries, on top of the always-available
    /// ones in `vars::GLOBAL`.
    pub fn vars(self) -> &'static [VarSpec] {
        match self {
            // `protected`: crash protection took over (a hold opened).
            EventKind::ObsDisconnected => &[("stopped", "no"), ("protected", "no")],
            // `reason`: crash, freeze, or stopped (OBS stopped on purpose and
            // crash protection holds every disconnect).
            // `hold_ends_at`: when the hold runs out, in Unix seconds. Discord
            // shows `<t:{hold_ends_at}:R>` as a live countdown.
            EventKind::HoldOpened => &[
                ("reason", "crash"),
                ("hold", "2m 00s"),
                ("hold_ends_at", "1790000000"),
            ],
            EventKind::ObsBack => &[("down_for", "38 s")],
            EventKind::HoldExpired => &[("hold", "2m 00s")],
            EventKind::HoldEnded => &[("down_for", "1m 12s")],
            EventKind::DestinationLive => &[("destination", "YouTube"), ("platform", "youtube")],
            EventKind::DestinationDropped => &[
                ("destination", "YouTube"),
                ("platform", "youtube"),
                ("reason", "connection timed out"),
            ],
            EventKind::DelayOn => &[("delay", "30s"), ("previous", "0s")],
            EventKind::DelayOff => &[("previous", "30s")],
            EventKind::DelayChanged => &[("delay", "30s"), ("previous", "20s")],
            // Armed: the delay that will go on air once the buffer holds it.
            EventKind::DelayArmed | EventKind::DelayReady => &[("delay", "30s")],
            EventKind::DelayDisarmed => &[("previous", "30s")],
            EventKind::StreamEnded => &[
                ("duration", "3h 12m"),
                ("crashes", "1"),
                ("drops", "2"),
                ("highlights", "4"),
                ("report", "YouTube: 2 drops, down 1m 12s\nKick: steady"),
                ("chapters", "0:00 Start\n12:31 Ranked\n1:04:10 Boss fight"),
            ],
            // `state` is crash, delay, live or off: the most important one
            // that is true, in that order.
            EventKind::OnAirChanged => &[("state", "live"), ("previous", "off")],
            EventKind::TimelineUpdated => &[("line", "⭐ Highlight"), ("kind", "highlight")],
            EventKind::DestinationRecovered => &[
                ("destination", "YouTube"),
                ("platform", "youtube"),
                ("down_for", "1m 12s"),
                ("down_s", "72"),
            ],
            EventKind::DestinationStillDown => &[
                ("destination", "YouTube"),
                ("platform", "youtube"),
                ("reason", "connection timed out"),
                ("down_for", "1m 00s"),
                ("drops", "1"),
            ],
            EventKind::DestinationUnstable => &[
                ("destination", "YouTube"),
                ("platform", "youtube"),
                ("reason", "connection timed out"),
                ("drops", "3"),
                ("within", "10 min"),
            ],
            EventKind::DestinationSteady => &[
                ("destination", "YouTube"),
                ("platform", "youtube"),
                ("drops", "4"),
                ("down_total", "2m 30s"),
            ],
            EventKind::ObsConnected
            | EventKind::EbDetected
            | EventKind::AllDestinationsDown
            | EventKind::StreamStarted => &[],
        }
    }

    /// Settings of a trigger on this event: thresholds the engine applies
    /// per trigger, so two integrations can tune them differently.
    pub fn params(self) -> &'static [ParamSpec] {
        match self {
            EventKind::DestinationStillDown => &[("after_s", "60", "Still down after (seconds)")],
            EventKind::DestinationUnstable => &[
                ("drops", "3", "Drops"),
                ("within_min", "10", "Within (minutes)"),
            ],
            EventKind::DestinationSteady => &[("for_min", "10", "Live again for (minutes)")],
            _ => &[],
        }
    }

    /// A trigger's setting `name`, or its default when unset or not a
    /// positive number. Capped at a million (11 days in seconds): the
    /// engine turns these into durations, and an absurd one typed in the
    /// dashboard must not overflow one.
    pub fn param(self, filters: &std::collections::BTreeMap<String, String>, name: &str) -> f64 {
        let default = self
            .params()
            .iter()
            .find(|(n, ..)| *n == name)
            .and_then(|(_, d, _)| d.parse().ok())
            .unwrap_or(0.0);
        filters
            .get(name)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(default)
            .min(MAX_PARAM)
    }

    pub fn is_param(self, name: &str) -> bool {
        self.params().iter().any(|(n, ..)| *n == name)
    }
}

/// Something that happened, with its values.
#[derive(Clone, Debug)]
pub struct Event {
    pub kind: EventKind,
    pub vars: Vec<(&'static str, String)>,
}

impl Event {
    pub fn new(kind: EventKind) -> Self {
        Self {
            kind,
            vars: Vec::new(),
        }
    }

    pub fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.vars.push((name, value.into()));
        self
    }

    /// The event filled with its sample values, for test runs.
    pub fn sample(kind: EventKind) -> Self {
        Self {
            kind,
            vars: kind
                .vars()
                .iter()
                .map(|(n, v)| (*n, v.to_string()))
                .collect(),
        }
    }

    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The event catalog for the dashboard: ids, labels, groups and variables.
pub fn catalog_json() -> Value {
    Value::Arr(
        EventKind::ALL
            .iter()
            .map(|k| {
                json::obj([
                    ("id", json::str(k.id())),
                    ("label", json::str(k.label())),
                    ("group", json::str(k.group())),
                    ("vars", vars_json(k.vars())),
                    (
                        "params",
                        Value::Arr(
                            k.params()
                                .iter()
                                .map(|(name, default, label)| {
                                    json::obj([
                                        ("name", json::str(*name)),
                                        ("default", json::str(*default)),
                                        ("label", json::str(*label)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect(),
    )
}

pub fn vars_json(vars: &[VarSpec]) -> Value {
    Value::Arr(
        vars.iter()
            .map(|(name, sample)| {
                json::obj([("name", json::str(*name)), ("sample", json::str(*sample))])
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn ids_round_trip_and_are_unique() {
        for kind in EventKind::ALL {
            assert_eq!(EventKind::from_id(kind.id()), Some(kind));
        }
        let mut ids: Vec<_> = EventKind::ALL.iter().map(|k| k.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), EventKind::ALL.len());
    }

    #[test]
    fn samples_carry_every_declared_var() {
        let e = Event::sample(EventKind::DestinationDropped);
        assert_eq!(e.var("destination"), Some("YouTube"));
        assert_eq!(e.var("reason"), Some("connection timed out"));
    }

    #[test]
    fn trigger_settings_fall_back_and_stay_bounded() {
        let kind = EventKind::DestinationStillDown;
        let set = |v: &str| BTreeMap::from([("after_s".to_string(), v.to_string())]);
        assert_eq!(kind.param(&BTreeMap::new(), "after_s"), 60.0);
        assert_eq!(kind.param(&set("90"), "after_s"), 90.0);
        assert_eq!(kind.param(&set("-5"), "after_s"), 60.0);
        assert_eq!(kind.param(&set("soon"), "after_s"), 60.0);
        assert_eq!(kind.param(&set("1e30"), "after_s"), MAX_PARAM);
        // What the engine does with it can't overflow a Duration.
        let _ = std::time::Duration::from_secs_f64(kind.param(&set("inf"), "after_s") * 60.0);
        assert!(kind.is_param("after_s") && !kind.is_param("destination"));
    }

    #[test]
    fn catalog_is_valid_json() {
        assert!(crate::config::is_valid_json(&catalog_json().to_json()));
    }
}
