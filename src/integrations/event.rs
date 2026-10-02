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
}

/// One variable an event carries: its name and a sample value.
pub type VarSpec = (&'static str, &'static str);

impl EventKind {
    pub const ALL: [EventKind; 16] = [
        EventKind::ObsConnected,
        EventKind::ObsDisconnected,
        EventKind::EbDetected,
        EventKind::HoldOpened,
        EventKind::ObsBack,
        EventKind::HoldExpired,
        EventKind::HoldEnded,
        EventKind::DestinationLive,
        EventKind::DestinationDropped,
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
            EventKind::DestinationLive => "A destination goes live",
            EventKind::DestinationDropped => "A destination drops",
            EventKind::AllDestinationsDown => "Every destination is down",
            EventKind::DelayOn => "The delay turns on",
            EventKind::DelayOff => "The delay turns off",
            EventKind::DelayChanged => "The delay changes",
            EventKind::DelayArmed => "You arm a delay",
            EventKind::DelayReady => "The armed delay is ready",
            EventKind::DelayDisarmed => "You cancel an armed delay",
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            EventKind::ObsConnected | EventKind::ObsDisconnected | EventKind::EbDetected => {
                "Stream"
            }
            EventKind::HoldOpened
            | EventKind::ObsBack
            | EventKind::HoldExpired
            | EventKind::HoldEnded => "Crash protection",
            EventKind::DestinationLive
            | EventKind::DestinationDropped
            | EventKind::AllDestinationsDown => "Destinations",
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
            EventKind::ObsConnected | EventKind::EbDetected | EventKind::AllDestinationsDown => &[],
        }
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
    fn catalog_is_valid_json() {
        assert!(crate::config::is_valid_json(&catalog_json().to_json()));
    }
}
