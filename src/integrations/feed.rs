//! The OBS dock's events list: what "Add to the dock's events" steps wrote,
//! oldest first, each with the alert its run showed, to play again.
//!
//! Kept in memory for this run of the app: the dock asks for the list when
//! the state stream says it changed (`changes`).

use super::alerts::{self, Card};
use crate::json::{self, Value};
use crate::sync::Mutex;
use std::collections::VecDeque;

/// Events kept: the newest this many.
const KEEP: usize = 100;
const MAX_TEXT: usize = 200;
const MAX_GROUP: usize = 40;

pub struct Feed {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    last_id: u64,
    /// Bumped by every add and clear.
    changes: u64,
    events: VecDeque<Entry>,
}

struct Entry {
    id: u64,
    at_ms: u64,
    group: String,
    text: String,
    /// The alert the same run showed, for Replay.
    card: Option<Card>,
}

impl Feed {
    pub fn new() -> Feed {
        Feed {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Add an event: a line, the group the dock filters it by, and the alert
    /// to replay, if its run showed one.
    pub fn add(&self, group: &str, text: &str, card: Option<Card>) {
        let mut inner = self.inner.lock();
        inner.last_id += 1;
        inner.changes += 1;
        let entry = Entry {
            id: inner.last_id,
            at_ms: super::twitch::unix_ms(),
            group: alerts::one_line(group, MAX_GROUP),
            text: alerts::one_line(text, MAX_TEXT),
            card,
        };
        if inner.events.len() >= KEEP {
            inner.events.pop_front();
        }
        inner.events.push_back(entry);
    }

    /// Changes with every add and clear: the dock fetches the list then.
    pub fn changes(&self) -> u64 {
        self.inner.lock().changes
    }

    /// The alert event `id` showed, to put up again.
    pub fn card(&self, id: u64) -> Option<Card> {
        let inner = self.inner.lock();
        inner.events.iter().find(|e| e.id == id)?.card.clone()
    }

    pub fn to_json(&self) -> Value {
        let inner = self.inner.lock();
        Value::Arr(
            inner
                .events
                .iter()
                .map(|e| {
                    json::obj([
                        ("id", Value::Num(e.id as f64)),
                        ("at_ms", Value::Num(e.at_ms as f64)),
                        ("group", json::str(&e.group)),
                        ("text", json::str(&e.text)),
                        ("replay", Value::Bool(e.card.is_some())),
                    ])
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_keep_their_alert_to_replay_and_the_newest_hundred() {
        let feed = Feed::new();
        feed.add(
            "Bits",
            "Nat cheered\n500 bits",
            Some(Card::plain("Bits", "Nat", 6)),
        );
        feed.add("", "OBS crashed", None);
        let list = feed.to_json();
        let first = &list.as_array().unwrap()[0];
        assert_eq!(first.str_or("text", ""), "Nat cheered 500 bits");
        assert!(first.bool_or("replay", false));
        assert_eq!(feed.card(1).map(|c| c.text), Some("Nat".to_string()));
        assert!(feed.card(2).is_none(), "nothing to replay");
        let before = feed.changes();
        for n in 0..KEEP {
            feed.add("", &n.to_string(), None);
        }
        assert_eq!(feed.to_json().as_array().unwrap().len(), KEEP);
        assert!(feed.card(1).is_none(), "the oldest went");
        assert!(feed.changes() > before);
    }
}
