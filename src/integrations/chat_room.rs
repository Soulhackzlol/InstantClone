//! What runs need from chat while they are going: who talked lately (to
//! reward the people who are there), and the next message a "Wait for
//! chat" step is waiting for.
//!
//! The engine tells it every chat message; steps ask it.

use crate::sync::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// How long a chatter counts as there, at most.
pub const MAX_ACTIVE: Duration = Duration::from_secs(60 * 60);
/// Steps waiting for chat at once. Past this a new wait fails at once.
const MAX_WAITERS: usize = 64;

/// A chat message a waiting step was handed.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub user: String,
    pub user_login: String,
    pub message: String,
}

struct Waiter {
    /// What the message must start with, in lowercase; empty for anything.
    starts_with: String,
    /// The only login it listens to; None for anyone.
    from: Option<String>,
    reply: oneshot::Sender<Heard>,
}

#[derive(Default)]
pub struct ChatRoom {
    last_spoke: Mutex<HashMap<String, Instant>>,
    waiters: Mutex<Vec<Waiter>>,
}

impl ChatRoom {
    /// A chat message arrived: note who spoke, and hand it to every step
    /// that was waiting for it.
    pub fn heard(&self, heard: &Heard, now: Instant) {
        self.last_spoke.lock().insert(heard.user_login.clone(), now);
        let text = heard.message.trim().to_lowercase();
        let mut waiters = self.waiters.lock();
        let (wanted, rest): (Vec<Waiter>, Vec<Waiter>) = waiters.drain(..).partition(|w| {
            text.starts_with(&w.starts_with)
                && w.from
                    .as_ref()
                    .is_none_or(|login| *login == heard.user_login)
        });
        // A waiter whose step gave up (its wait ran out) is dropped here.
        *waiters = rest.into_iter().filter(|w| !w.reply.is_closed()).collect();
        for waiter in wanted {
            let _ = waiter.reply.send(heard.clone());
        }
    }

    /// Wait for the next message that starts with `starts_with` (anything
    /// when empty), from `from` or from anyone.
    pub fn wait(
        &self,
        starts_with: &str,
        from: Option<String>,
    ) -> Result<oneshot::Receiver<Heard>, String> {
        let mut waiters = self.waiters.lock();
        waiters.retain(|w| !w.reply.is_closed());
        if waiters.len() >= MAX_WAITERS {
            return Err("too many steps are waiting for chat already".to_string());
        }
        let (reply, answer) = oneshot::channel();
        waiters.push(Waiter {
            starts_with: starts_with.trim().to_lowercase(),
            from,
            reply,
        });
        Ok(answer)
    }

    /// The logins that spoke within `within`, in name order.
    pub fn active(&self, within: Duration, now: Instant) -> Vec<String> {
        let mut logins: Vec<String> = self
            .last_spoke
            .lock()
            .iter()
            .filter(|(_, at)| now.saturating_duration_since(**at) <= within)
            .map(|(login, _)| login.clone())
            .collect();
        logins.sort();
        logins
    }

    /// Forget who spoke too long ago to count.
    pub fn prune(&self, now: Instant) {
        self.last_spoke
            .lock()
            .retain(|_, at| now.saturating_duration_since(*at) <= MAX_ACTIVE);
        self.waiters.lock().retain(|w| !w.reply.is_closed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(login: &str, message: &str) -> Heard {
        Heard {
            user: login.to_uppercase(),
            user_login: login.to_string(),
            message: message.to_string(),
        }
    }

    #[test]
    fn a_wait_gets_the_first_message_that_fits() {
        let room = ChatRoom::default();
        let mut anyone = room.wait("!grab", None).unwrap();
        let mut only_bo = room.wait("!accept", Some("bo".to_string())).unwrap();
        room.heard(&said("ana", "hello"), Instant::now());
        room.heard(&said("ana", "!accept"), Instant::now());
        assert!(anyone.try_recv().is_err());
        assert!(only_bo.try_recv().is_err(), "ana isn't who it waits for");
        room.heard(&said("ana", "!GRAB it"), Instant::now());
        room.heard(&said("bo", "!accept"), Instant::now());
        assert_eq!(anyone.try_recv().unwrap().user_login, "ana");
        assert_eq!(only_bo.try_recv().unwrap().user_login, "bo");
    }

    #[test]
    fn only_recent_chatters_are_active() {
        let room = ChatRoom::default();
        let start = Instant::now();
        room.heard(&said("old", "hi"), start);
        room.heard(&said("new", "hi"), start + Duration::from_secs(500));
        let now = start + Duration::from_secs(600);
        assert_eq!(room.active(Duration::from_secs(300), now), ["new"]);
        assert_eq!(room.active(Duration::from_secs(900), now), ["new", "old"]);
    }
}
