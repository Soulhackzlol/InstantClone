//! What "Show on stream" steps put on the alerts browser source.
//!
//! The board keeps the last few alerts with an increasing id. The page in
//! OBS (`/alerts`) reads them over its own event stream and plays each new
//! id once, in order, so two alerts close together queue instead of
//! overlapping. Only what a step chose to show is here: it is on stream
//! anyway, which is why the page needs no login.

use crate::json::{self, Value};
use crate::sync::Mutex;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Alerts kept for a page that (re)connects. Older ones are never replayed.
const KEEP: usize = 8;
/// How long an alert stays on the board: a page that reconnects after this
/// no longer plays it.
const FRESH: Duration = Duration::from_secs(30);
pub const MIN_SECONDS: u64 = 2;
pub const MAX_SECONDS: u64 = 30;
const MAX_TITLE: usize = 80;
const MAX_TEXT: usize = 300;

struct Alert {
    id: u64,
    title: String,
    text: String,
    ms: u64,
    at: Instant,
}

pub struct Board {
    /// Changes with every start of the app, so a page that outlives a
    /// restart knows the ids started over.
    boot: String,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    last_id: u64,
    alerts: VecDeque<Alert>,
}

impl Board {
    pub fn new() -> Board {
        Board {
            boot: crate::crypto::random_token()[..8].to_string(),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Put an alert up for `seconds` (clamped to what reads well).
    pub fn show(&self, title: &str, text: &str, seconds: u64) {
        let mut inner = self.inner.lock();
        inner.last_id += 1;
        let alert = Alert {
            id: inner.last_id,
            title: one_line(title, MAX_TITLE),
            text: one_line(text, MAX_TEXT),
            ms: seconds.clamp(MIN_SECONDS, MAX_SECONDS) * 1000,
            at: Instant::now(),
        };
        if inner.alerts.len() >= KEEP {
            inner.alerts.pop_front();
        }
        inner.alerts.push_back(alert);
    }

    /// The fresh alerts, oldest first. Unchanged between alerts, so the
    /// page's event stream only sends when something new is up.
    pub fn to_json(&self) -> String {
        let mut inner = self.inner.lock();
        inner.alerts.retain(|a| a.at.elapsed() < FRESH);
        let alerts = Value::Arr(
            inner
                .alerts
                .iter()
                .map(|a| {
                    json::obj([
                        ("id", Value::Num(a.id as f64)),
                        ("title", json::str(&a.title)),
                        ("text", json::str(&a.text)),
                        ("ms", Value::Num(a.ms as f64)),
                    ])
                })
                .collect(),
        );
        json::obj([("boot", json::str(&self.boot)), ("alerts", alerts)]).to_json()
    }
}

fn one_line(text: &str, max: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    flat.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alerts_queue_with_rising_ids_and_sane_lengths() {
        let board = Board::new();
        board.show("Clip!", "by Ana\r\nnice", 1);
        board.show("", &"x".repeat(900), 99);
        let doc = json::parse(&board.to_json()).unwrap();
        assert_eq!(doc.str_or("boot", "").len(), 8);
        let list = doc.get("alerts").and_then(Value::as_array).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].str_or("text", ""), "by Ana  nice");
        assert_eq!(list[0].u64_or("ms", 0), MIN_SECONDS * 1000);
        assert_eq!(list[1].u64_or("ms", 0), MAX_SECONDS * 1000);
        assert_eq!(list[1].str_or("text", "").len(), MAX_TEXT);
        assert!(list[1].u64_or("id", 0) > list[0].u64_or("id", 0));
    }

    #[test]
    fn only_the_last_few_are_kept() {
        let board = Board::new();
        for n in 0..20 {
            board.show("", &n.to_string(), 5);
        }
        let doc = json::parse(&board.to_json()).unwrap();
        let list = doc.get("alerts").and_then(Value::as_array).unwrap();
        assert_eq!(list.len(), KEEP);
        assert_eq!(list[0].str_or("text", ""), "12");
    }
}
