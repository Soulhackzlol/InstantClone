//! The chat activity trigger's view of chat: the last few minutes of
//! messages, and how a trigger's rules read them right now.
//!
//! "Busier than normal" compares against this channel's own chat, so one
//! setting works for twenty viewers and for twenty thousand: normal is the
//! average speed over the last ten minutes, leaving out the window being
//! judged, and never below `MIN_NORMAL` so three lines in a quiet chat
//! don't count as a hype moment.

use super::model::{ChatActivity, RuleKind};
use super::twitch::irc::ChatMessage;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How far back "normal" looks.
const HISTORY: Duration = Duration::from_secs(600);
/// "Normal" needs this much chat history before it means anything.
const MIN_HISTORY: Duration = Duration::from_secs(60);
/// Messages kept at most: a very busy chat keeps less than ten minutes.
const MAX_KEPT: usize = 6000;
/// The least "normal" can be, in messages per window.
const MIN_NORMAL: f64 = 2.0;
/// A share of messages needs a few messages to be a share at all.
const MIN_FOR_SHARE: usize = 3;
/// Characters of a message kept for word matching.
const MAX_TEXT: usize = 200;

struct Msg {
    at: Instant,
    login: String,
    is_sub: bool,
    is_vip: bool,
    is_mod: bool,
    is_broadcaster: bool,
    text: String,
}

/// Recent chat, shared by every chat activity trigger.
#[derive(Default)]
pub struct ChatWindow {
    msgs: VecDeque<Msg>,
    /// When chat was first seen: until `MIN_HISTORY` later, "normal" is
    /// unknown.
    since: Option<Instant>,
}

/// How chat reads against one trigger's rules.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    pub messages: usize,
    /// Messages per window on a normal stretch.
    pub normal: f64,
    /// `messages` over `normal`.
    pub busier: f64,
    pub chatters: usize,
    /// Percent of messages containing one of a words rule's words: the
    /// highest of the rules, for `{word_share}`.
    pub word_share: f64,
    /// Per rule, in order: its own share (0 for rules that aren't words).
    pub shares: Vec<f64>,
    /// The most used word of any words rule, or empty.
    pub top_word: String,
    /// Whether "normal" is known yet.
    pub ready: bool,
    /// One per rule, in order.
    pub passing: Vec<bool>,
    pub fires: bool,
}

impl ChatWindow {
    pub fn push(&mut self, msg: &ChatMessage, now: Instant) {
        self.since.get_or_insert(now);
        if self.msgs.len() >= MAX_KEPT {
            self.msgs.pop_front();
        }
        self.msgs.push_back(Msg {
            at: now,
            login: msg.user_login.clone(),
            is_sub: msg.is_sub,
            is_vip: msg.is_vip,
            is_mod: msg.is_mod,
            is_broadcaster: msg.is_broadcaster,
            text: msg.text.to_lowercase().chars().take(MAX_TEXT).collect(),
        });
        self.trim(now);
    }

    fn trim(&mut self, now: Instant) {
        while self
            .msgs
            .front()
            .is_some_and(|m| now.duration_since(m.at) > HISTORY)
        {
            self.msgs.pop_front();
        }
    }

    /// How chat reads against `rules` at `now`. The window is kept within
    /// what can be judged: the dashboard's meter sends rules nobody
    /// validated yet.
    pub fn read(&self, rules: &ChatActivity, now: Instant) -> Reading {
        let window =
            Duration::from_millis(rules.window_ms.clamp(1_000, HISTORY.as_millis() as u64));
        let counts = |m: &Msg| {
            m.is_broadcaster
                || rules.roles.everyone
                || (rules.roles.mods && m.is_mod)
                || (rules.roles.vips && m.is_vip)
                || (rules.roles.subs && m.is_sub)
        };
        let recent: Vec<&Msg> = self
            .msgs
            .iter()
            .filter(|m| now.duration_since(m.at) <= window && counts(m))
            .collect();
        let older = self
            .msgs
            .iter()
            .filter(|m| now.duration_since(m.at) > window && counts(m))
            .count();
        let history = self
            .since
            .map_or(Duration::ZERO, |s| now.duration_since(s))
            .min(HISTORY);
        let ready = history >= MIN_HISTORY + window;
        let normal = if ready {
            let windows = (history - window).as_secs_f64() / window.as_secs_f64();
            (older as f64 / windows).max(MIN_NORMAL)
        } else {
            0.0
        };
        let messages = recent.len();
        let mut chatters: Vec<&str> = recent.iter().map(|m| m.login.as_str()).collect();
        chatters.sort_unstable();
        chatters.dedup();
        // Each words rule counts its own words; the top word is chat's
        // favourite out of all of them.
        let shares: Vec<f64> = rules
            .rules
            .iter()
            .map(|r| match r.kind {
                RuleKind::Words => word_stats(&recent, &word_list(&r.words)).0,
                _ => 0.0,
            })
            .collect();
        let all_words = word_list(
            &rules
                .rules
                .iter()
                .filter(|r| r.kind == RuleKind::Words)
                .map(|r| r.words.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
        let top_word = word_stats(&recent, &all_words).1;
        let word_share = shares.iter().copied().fold(0.0, f64::max);
        let busier = if normal > 0.0 {
            messages as f64 / normal
        } else {
            0.0
        };
        let passing: Vec<bool> = rules
            .rules
            .iter()
            .zip(&shares)
            .map(|(r, share)| match r.kind {
                RuleKind::Messages => messages as f64 >= r.value,
                RuleKind::Busier => ready && busier >= r.value,
                RuleKind::Chatters => chatters.len() as f64 >= r.value,
                RuleKind::Words => *share >= r.value,
            })
            .collect();
        let fires = !passing.is_empty()
            && if rules.match_all {
                passing.iter().all(|p| *p)
            } else {
                passing.iter().any(|p| *p)
            };
        Reading {
            messages,
            normal,
            busier,
            chatters: chatters.len(),
            word_share,
            shares,
            top_word,
            ready,
            passing,
            fires,
        }
    }
}

/// The comma separated words of a words rule, lowercase, each once.
fn word_list(words: &str) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    for word in words.split(',').map(|w| w.trim().to_lowercase()) {
        if !word.is_empty() && !list.contains(&word) {
            list.push(word);
        }
    }
    list
}

/// The share of messages holding any of `words`, and the most used one.
fn word_stats(recent: &[&Msg], words: &[String]) -> (f64, String) {
    if words.is_empty() || recent.len() < MIN_FOR_SHARE {
        return (0.0, String::new());
    }
    let mut hits = vec![0usize; words.len()];
    let mut matching = 0;
    for m in recent {
        let tokens: Vec<&str> = m
            .text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        let mut any = false;
        for (i, w) in words.iter().enumerate() {
            if says(&m.text, &tokens, w) {
                hits[i] += 1;
                any = true;
            }
        }
        matching += usize::from(any);
    }
    // Reversed, so a tie goes to the word listed first.
    let top = hits
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, n)| **n > 0)
        .max_by_key(|(_, n)| **n)
        .map(|(i, _)| words[i].clone())
        .unwrap_or_default();
    (matching as f64 * 100.0 / recent.len() as f64, top)
}

/// Whether a message says `word`. A phrase ("let's go") is looked for as
/// written; a single word must be a whole word, or start one when it has
/// three letters or more ("pog" in "poggers", "clip" in "clipped"), so
/// "w" never matches "wow".
fn says(text: &str, tokens: &[&str], word: &str) -> bool {
    if word.contains(char::is_whitespace) {
        return text.contains(word);
    }
    let prefix_ok = word.chars().count() >= 3;
    tokens
        .iter()
        .any(|t| *t == word || (prefix_ok && t.starts_with(word)))
}

#[cfg(test)]
mod tests {
    use super::super::model::{ActivityRule, Roles};
    use super::*;

    fn msg(user: &str, text: &str) -> ChatMessage {
        ChatMessage {
            user_login: user.into(),
            text: text.into(),
            ..ChatMessage::default()
        }
    }

    fn rules(kind: RuleKind, value: f64, words: &str) -> ChatActivity {
        ChatActivity {
            window_ms: 10_000,
            match_all: true,
            rules: vec![ActivityRule {
                kind,
                value,
                words: words.into(),
            }],
            roles: Roles::EVERYONE,
            only_live: true,
        }
    }

    /// A steady chat for `minutes`, one message every 10 s, then a burst.
    fn chat_then_burst(minutes: u64, burst: usize) -> (ChatWindow, Instant) {
        let mut w = ChatWindow::default();
        let t0 = Instant::now();
        for s in (0..minutes * 60).step_by(10) {
            w.push(&msg("regular", "hello"), t0 + Duration::from_secs(s));
        }
        let now = t0 + Duration::from_secs(minutes * 60);
        for n in 0..burst {
            w.push(&msg(&format!("v{n}"), "CLIP IT pog"), now);
        }
        (w, now)
    }

    #[test]
    fn busier_compares_with_the_channels_own_pace() {
        let (w, now) = chat_then_burst(5, 12);
        let r = w.read(&rules(RuleKind::Busier, 3.0, ""), now);
        assert!(r.ready);
        assert_eq!(r.normal, MIN_NORMAL, "one a window is below the floor");
        assert!(r.busier >= 6.0, "{r:?}");
        assert!(r.fires);
    }

    #[test]
    fn busier_waits_for_enough_history() {
        let (w, now) = chat_then_burst(0, 30);
        let r = w.read(&rules(RuleKind::Busier, 2.0, ""), now);
        assert!(!r.ready);
        assert!(!r.fires);
    }

    #[test]
    fn words_count_a_share_of_messages_and_name_the_top_one() {
        let (w, now) = chat_then_burst(2, 6);
        let r = w.read(&rules(RuleKind::Words, 50.0, "clip, pog, lul"), now);
        assert_eq!(r.messages, 7, "the burst plus one regular line");
        assert!(r.word_share > 80.0, "{r:?}");
        assert_eq!(r.top_word, "clip");
        assert!(r.fires);
    }

    #[test]
    fn short_words_match_whole_words_only() {
        let tokens = |t: &'static str| t.split(' ').collect::<Vec<_>>();
        assert!(says("w", &tokens("w"), "w"));
        assert!(!says("wow what", &tokens("wow what"), "w"));
        assert!(says("poggers", &tokens("poggers"), "pog"));
        assert!(says("lets go boys", &tokens("lets go boys"), "lets go"));
        assert!(!says("clap", &tokens("clap"), "clip"));
    }

    #[test]
    fn any_window_the_meter_sends_is_safe() {
        let (w, now) = chat_then_burst(1, 3);
        for window_ms in [0, u64::MAX] {
            let mut r = rules(RuleKind::Busier, 2.0, "");
            r.window_ms = window_ms;
            let _ = w.read(&r, now);
        }
    }

    #[test]
    fn each_words_rule_counts_its_own_words() {
        let (w, now) = chat_then_burst(2, 6);
        let mut both = rules(RuleKind::Words, 50.0, "clip");
        both.rules.push(ActivityRule {
            kind: RuleKind::Words,
            value: 50.0,
            words: "lul".into(),
        });
        let r = w.read(&both, now);
        assert_eq!(r.passing, vec![true, false], "nobody said lul");
        assert!(!r.fires, "all rules must pass");
        assert_eq!(r.top_word, "clip");
    }

    #[test]
    fn one_message_is_never_a_share() {
        let mut w = ChatWindow::default();
        let now = Instant::now();
        w.push(&msg("a", "clip"), now);
        let r = w.read(&rules(RuleKind::Words, 10.0, "clip"), now);
        assert_eq!(r.word_share, 0.0);
        assert!(!r.fires);
    }

    #[test]
    fn chatters_are_people_not_lines() {
        let mut w = ChatWindow::default();
        let now = Instant::now();
        for _ in 0..20 {
            w.push(&msg("spammer", "pog"), now);
        }
        let r = w.read(&rules(RuleKind::Chatters, 3.0, ""), now);
        assert_eq!((r.messages, r.chatters), (20, 1));
        assert!(!r.fires);
    }

    #[test]
    fn any_fires_on_one_rule_all_needs_every_rule() {
        let (w, now) = chat_then_burst(2, 5);
        let mut both = rules(RuleKind::Messages, 3.0, "");
        both.rules.push(ActivityRule {
            kind: RuleKind::Chatters,
            value: 50.0,
            words: String::new(),
        });
        assert!(!w.read(&both, now).fires);
        both.match_all = false;
        let r = w.read(&both, now);
        assert_eq!(r.passing, vec![true, false]);
        assert!(r.fires);
    }

    #[test]
    fn only_the_chosen_roles_count() {
        let mut w = ChatWindow::default();
        let now = Instant::now();
        let mut m = msg("mod1", "x");
        m.is_mod = true;
        w.push(&m, now);
        w.push(&msg("viewer", "x"), now);
        let mut mods = rules(RuleKind::Messages, 2.0, "");
        mods.roles = Roles::MODS;
        assert_eq!(w.read(&mods, now).messages, 1);
    }
}
