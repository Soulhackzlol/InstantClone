//! Goals and timers: numbers a stream works toward and clocks that run
//! down, drawn by overlay widgets.
//!
//! A goal counts one thing (follows, subs, bits, or only what steps and the
//! dashboard add) toward a target. A timer counts down, and can gain time
//! from subs, bits and follows: a subathon. What they are is a setting;
//! where they stand is kept in the integrations' data file, so a restart
//! mid-stream loses nothing.
//!
//! One `Tracker` is shared by the engine (events), the steps, the dashboard
//! and the overlay feed. Whatever moves a goal, reaching its target is
//! noticed here and handed to the engine as an event at its next tick; a
//! timer running out is too.

use super::event::{Event, EventKind};
use crate::json::{self, Value};
use crate::sync::Mutex;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const MAX_GOALS: usize = 16;
pub const MAX_TIMERS: usize = 8;
const MAX_ID: usize = 32;
const MAX_NAME: usize = 40;
/// The most a goal counts to.
pub const MAX_TARGET: u64 = 1_000_000_000;
/// The longest a timer runs: 30 days.
pub const MAX_TIMER_S: u64 = 30 * 24 * 3600;
/// What a tier 2 and a tier 3 sub add to a timer, against a tier 1: about
/// what they cost.
const TIER_WEIGHTS: [u64; 3] = [1, 2, 5];
/// A stream "starting" this soon after one was last seen live is that one
/// carrying on: InstantClone restarted mid-stream (a crash, an update), and
/// OBS connecting again isn't a new stream.
const SAME_STREAM_MS: u64 = 10 * 60 * 1000;
/// How often a live stream is noted for that, in the data file.
const LIVE_NOTE_EVERY_MS: u64 = 60 * 1000;
/// A timer that ran out longer ago than this ran out while the app was
/// closed: it ends quietly, instead of "ending" as the app starts.
const ENDED_NEWS_WITHIN_MS: u64 = 60 * 1000;

/// What a goal counts by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Counts {
    Follows,
    /// New subs, resubs and each gifted sub.
    Subs,
    Bits,
    /// Nothing: only steps and the dashboard move it.
    Own,
}

impl Counts {
    pub fn id(self) -> &'static str {
        match self {
            Counts::Follows => "follows",
            Counts::Subs => "subs",
            Counts::Bits => "bits",
            Counts::Own => "own",
        }
    }

    fn from_id(id: &str) -> Counts {
        match id {
            "follows" => Counts::Follows,
            "subs" => Counts::Subs,
            "bits" => Counts::Bits,
            _ => Counts::Own,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Goal {
    /// What steps and widgets know it by (`{goal.subs}`).
    pub id: String,
    pub name: String,
    pub counts: Counts,
    pub target: u64,
    /// Start again from 0 with every stream.
    pub each_stream: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Timer {
    pub id: String,
    pub name: String,
    /// What it starts from, and goes back to on a reset.
    pub start_s: u64,
    /// Seconds each sub, each 100 bits and each follow add; 0 for none.
    pub per_sub_s: u64,
    pub per_100_bits_s: u64,
    pub per_follow_s: u64,
    /// Tier 2 and 3 subs add more (`TIER_WEIGHTS`).
    pub tiers: bool,
    /// The most it can hold, however much is added; 0 for no limit.
    pub max_s: u64,
}

/// `a-z`, `0-9` and `_`: safe in a template name, a config key and a page.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn name_of(v: &Value) -> String {
    let flat = super::template::one_line(v.str_or("name", ""));
    flat.trim().chars().take(MAX_NAME).collect()
}

impl Goal {
    /// A saved or sent goal, or `None` when it has no usable id or name.
    pub fn from_json(v: &Value) -> Option<Goal> {
        let goal = Goal {
            id: v.str_or("id", "").to_string(),
            name: name_of(v),
            counts: Counts::from_id(v.str_or("counts", "")),
            target: v.u64_or("target", 0).min(MAX_TARGET),
            each_stream: v.bool_or("each_stream", false),
        };
        (valid_id(&goal.id) && !goal.name.is_empty() && goal.target > 0).then_some(goal)
    }

    pub fn to_json(&self) -> Value {
        json::obj([
            ("id", json::str(&self.id)),
            ("name", json::str(&self.name)),
            ("counts", json::str(self.counts.id())),
            ("target", Value::Num(self.target as f64)),
            ("each_stream", Value::Bool(self.each_stream)),
        ])
    }
}

impl Timer {
    pub fn from_json(v: &Value) -> Option<Timer> {
        let seconds = |key: &str| v.u64_or(key, 0).min(MAX_TIMER_S);
        let timer = Timer {
            id: v.str_or("id", "").to_string(),
            name: name_of(v),
            start_s: seconds("start_s"),
            per_sub_s: seconds("per_sub_s"),
            per_100_bits_s: seconds("per_100_bits_s"),
            per_follow_s: seconds("per_follow_s"),
            tiers: v.bool_or("tiers", true),
            max_s: seconds("max_s"),
        };
        (valid_id(&timer.id) && !timer.name.is_empty() && timer.start_s > 0).then_some(timer)
    }

    pub fn to_json(&self) -> Value {
        let n = |s: u64| Value::Num(s as f64);
        json::obj([
            ("id", json::str(&self.id)),
            ("name", json::str(&self.name)),
            ("start_s", n(self.start_s)),
            ("per_sub_s", n(self.per_sub_s)),
            ("per_100_bits_s", n(self.per_100_bits_s)),
            ("per_follow_s", n(self.per_follow_s)),
            ("tiers", Value::Bool(self.tiers)),
            ("max_s", n(self.max_s)),
        ])
    }

    fn start_ms(&self) -> u64 {
        self.start_s * 1000
    }

    /// `left_ms` kept within what the timer may hold.
    fn capped(&self, left_ms: u64) -> u64 {
        let most = if self.max_s > 0 {
            self.max_s
        } else {
            MAX_TIMER_S
        };
        left_ms.min(most * 1000)
    }
}

/// The goals and timers the dashboard sent, checked: what it says is wrong
/// is what the streamer reads.
pub fn parse_setup(req: &Value) -> Result<(Vec<Goal>, Vec<Timer>), String> {
    let list = |key: &str| req.get(key).and_then(Value::as_array).unwrap_or(&[]);
    let goals: Vec<Goal> = list("goals").iter().filter_map(Goal::from_json).collect();
    let timers: Vec<Timer> = list("timers").iter().filter_map(Timer::from_json).collect();
    if goals.len() != list("goals").len() || timers.len() != list("timers").len() {
        return Err("each one needs a name and a number above 0".to_string());
    }
    if goals.len() > MAX_GOALS || timers.len() > MAX_TIMERS {
        return Err(format!(
            "that's more than {MAX_GOALS} goals or {MAX_TIMERS} timers"
        ));
    }
    let mut ids: Vec<&str> = goals.iter().map(|g| g.id.as_str()).collect();
    let goal_ids = ids.len();
    ids.sort_unstable();
    ids.dedup();
    let mut timer_ids: Vec<&str> = timers.iter().map(|t| t.id.as_str()).collect();
    timer_ids.sort_unstable();
    timer_ids.dedup();
    if ids.len() != goal_ids || timer_ids.len() != timers.len() {
        return Err("two of them have the same short name".to_string());
    }
    Ok((goals, timers))
}

/// Where a timer stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Clock {
    Paused { left_ms: u64 },
    Running { ends_at_ms: u64 },
}

impl Clock {
    fn left_ms(self, now_ms: u64) -> u64 {
        match self {
            Clock::Paused { left_ms } => left_ms,
            Clock::Running { ends_at_ms } => ends_at_ms.saturating_sub(now_ms),
        }
    }

    /// The same clock holding `left_ms`.
    fn holding(self, left_ms: u64, now_ms: u64) -> Clock {
        match self {
            Clock::Paused { .. } => Clock::Paused { left_ms },
            Clock::Running { .. } => Clock::Running {
                ends_at_ms: now_ms + left_ms,
            },
        }
    }
}

pub enum GoalChange {
    Add(i64),
    Set(u64),
    Reset,
}

pub enum TimerChange {
    /// Run from where it stands, or from the top once it has run out.
    Start,
    Pause,
    /// Back to its starting time, stopped.
    Reset,
    /// Seconds more, or fewer.
    Add(i64),
    /// Seconds left, from now.
    Set(u64),
}

#[derive(Default)]
struct Inner {
    goals: Vec<Goal>,
    timers: Vec<Timer>,
    values: BTreeMap<String, u64>,
    clocks: BTreeMap<String, Clock>,
    /// When a stream was last seen live (0: none since the last one ended).
    live_at_ms: u64,
    /// Goals reached and timers run out since the engine last asked.
    news: Vec<Event>,
}

impl Inner {
    fn clock(&self, timer: &Timer) -> Clock {
        let fresh = Clock::Paused {
            left_ms: timer.start_ms(),
        };
        self.clocks.get(&timer.id).copied().unwrap_or(fresh)
    }

    /// Give goal `at` a new value; crossing its target is news.
    fn set_value(&mut self, at: usize, value: u64) {
        let goal = &self.goals[at];
        let value = value.min(MAX_TARGET);
        let before = self.values.insert(goal.id.clone(), value).unwrap_or(0);
        if before < goal.target && value >= goal.target {
            let reached = Event::new(EventKind::GoalReached)
                .with("goal", goal.name.clone())
                .with("goal_id", goal.id.clone())
                .with("value", value.to_string())
                .with("target", goal.target.to_string());
            self.news.push(reached);
        }
    }
}

pub struct Tracker {
    inner: Mutex<Inner>,
    /// The data file's "something worth saving changed" flag.
    dirty: Arc<AtomicBool>,
}

impl Tracker {
    /// `saved` is what `state_json` wrote the last time the app ran.
    pub fn new(saved: Option<&Value>, dirty: Arc<AtomicBool>) -> Tracker {
        let mut inner = Inner::default();
        let fields = |key: &str| match saved.and_then(|v| v.get(key)) {
            Some(Value::Obj(fields)) => fields.iter().collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        for (id, value) in fields("goals") {
            if let Some(n) = value.as_f64().filter(|n| *n >= 0.0) {
                inner.values.insert(id.clone(), (n as u64).min(MAX_TARGET));
            }
        }
        for (id, clock) in fields("timers") {
            let at = clock.u64_or("at_ms", 0);
            let clock = if clock.bool_or("running", false) {
                Clock::Running { ends_at_ms: at }
            } else {
                Clock::Paused { left_ms: at }
            };
            inner.clocks.insert(id.clone(), clock);
        }
        inner.live_at_ms = saved.map_or(0, |v| v.u64_or("live_at_ms", 0));
        Tracker {
            inner: Mutex::new(inner),
            dirty,
        }
    }

    /// Where every goal and timer stands, for the data file.
    pub fn state_json(&self) -> Value {
        let inner = self.inner.lock();
        let goals = inner
            .values
            .iter()
            .map(|(id, n)| (id.clone(), Value::Num(*n as f64)));
        let timers = inner.clocks.iter().map(|(id, clock)| {
            let (running, at) = match *clock {
                Clock::Running { ends_at_ms } => (true, ends_at_ms),
                Clock::Paused { left_ms } => (false, left_ms),
            };
            let kept = json::obj([
                ("running", Value::Bool(running)),
                ("at_ms", Value::Num(at as f64)),
            ]);
            (id.clone(), kept)
        });
        json::obj([
            ("goals", Value::Obj(goals.collect())),
            ("timers", Value::Obj(timers.collect())),
            ("live_at_ms", Value::Num(inner.live_at_ms as f64)),
        ])
    }

    fn changed(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// The goals and timers that exist now. One that is gone forgets where
    /// it stood.
    pub fn configure(&self, goals: &[Goal], timers: &[Timer]) {
        let mut inner = self.inner.lock();
        if inner.goals == goals && inner.timers == timers {
            return;
        }
        inner.goals = goals.to_vec();
        inner.timers = timers.to_vec();
        let Inner {
            goals,
            timers,
            values,
            clocks,
            ..
        } = &mut *inner;
        values.retain(|id, _| goals.iter().any(|g| g.id == *id));
        clocks.retain(|id, _| timers.iter().any(|t| t.id == *id));
        self.changed();
    }

    /// The Twitch events a goal counts or a timer gains time from: the
    /// engine listens for these even when no integration does.
    pub fn wanted_events(&self) -> Vec<EventKind> {
        let inner = self.inner.lock();
        let counted = |counts: Counts| {
            inner.goals.iter().any(|g| g.counts == counts)
                || inner.timers.iter().any(|t| match counts {
                    Counts::Follows => t.per_follow_s > 0,
                    Counts::Subs => t.per_sub_s > 0,
                    Counts::Bits => t.per_100_bits_s > 0,
                    Counts::Own => false,
                })
        };
        let mut wanted = Vec::new();
        if counted(Counts::Follows) {
            wanted.push(EventKind::TwitchFollow);
        }
        if counted(Counts::Subs) {
            wanted.extend([EventKind::TwitchSub, EventKind::TwitchResub]);
        }
        if counted(Counts::Bits) {
            wanted.push(EventKind::TwitchCheer);
        }
        wanted
    }

    /// Count a follow, a sub or bits toward the goals that count them, and
    /// give running timers the time they add.
    pub fn observe(&self, event: &Event, now_ms: u64) {
        let number = |name: &str| event.var(name).and_then(|v| v.parse::<u64>().ok());
        let (counts, amount) = match event.kind {
            EventKind::TwitchFollow => (Counts::Follows, 1),
            EventKind::TwitchSub | EventKind::TwitchResub => (Counts::Subs, 1),
            EventKind::TwitchCheer => (Counts::Bits, number("bits").unwrap_or(0)),
            _ => return,
        };
        let tier = number("tier").unwrap_or(1).clamp(1, 3) as usize;
        let mut inner = self.inner.lock();
        // Most events move nothing: those leave the data file alone.
        let mut moved = false;
        for at in 0..inner.goals.len() {
            if inner.goals[at].counts == counts {
                let now = inner.values.get(&inner.goals[at].id).copied().unwrap_or(0);
                inner.set_value(at, now.saturating_add(amount));
                moved = true;
            }
        }
        for at in 0..inner.timers.len() {
            let timer = &inner.timers[at];
            let seconds = match counts {
                Counts::Follows => timer.per_follow_s,
                Counts::Subs if timer.tiers => timer.per_sub_s * TIER_WEIGHTS[tier - 1],
                Counts::Subs => timer.per_sub_s,
                Counts::Bits => amount.saturating_mul(timer.per_100_bits_s) / 100,
                Counts::Own => 0,
            };
            let clock = inner.clock(timer);
            let left = clock.left_ms(now_ms);
            // One that ran out is over: a late sub doesn't restart it.
            if seconds == 0 || left == 0 {
                continue;
            }
            let left = timer.capped(left.saturating_add(seconds.saturating_mul(1000)));
            let id = timer.id.clone();
            inner.clocks.insert(id, clock.holding(left, now_ms));
            moved = true;
        }
        if moved {
            self.changed();
        }
    }

    /// A step or the dashboard moving a goal; its new value.
    pub fn change_goal(&self, id: &str, change: GoalChange) -> Result<u64, String> {
        let mut inner = self.inner.lock();
        let Some(at) = inner.goals.iter().position(|g| g.id == id) else {
            return Err(format!("there is no goal called {id}"));
        };
        let now = inner.values.get(id).copied().unwrap_or(0);
        let value = match change {
            GoalChange::Add(by) if by < 0 => now.saturating_sub(by.unsigned_abs()),
            GoalChange::Add(by) => now.saturating_add(by.unsigned_abs()),
            GoalChange::Set(value) => value,
            GoalChange::Reset => 0,
        };
        inner.set_value(at, value);
        self.changed();
        Ok(value.min(MAX_TARGET))
    }

    /// A step or the dashboard driving a timer; the milliseconds it holds.
    pub fn change_timer(&self, id: &str, change: TimerChange, now_ms: u64) -> Result<u64, String> {
        let mut inner = self.inner.lock();
        let Some(timer) = inner.timers.iter().find(|t| t.id == id).cloned() else {
            return Err(format!("there is no timer called {id}"));
        };
        let clock = inner.clock(&timer);
        let left = clock.left_ms(now_ms);
        let next = match change {
            // One that ran out starts over from the top.
            TimerChange::Start if left == 0 => Clock::Running {
                ends_at_ms: now_ms + timer.start_ms(),
            },
            TimerChange::Start => Clock::Running {
                ends_at_ms: now_ms + left,
            },
            TimerChange::Pause => Clock::Paused { left_ms: left },
            TimerChange::Reset => Clock::Paused {
                left_ms: timer.start_ms(),
            },
            TimerChange::Add(by) => {
                let by_ms = by.unsigned_abs().saturating_mul(1000);
                let left = if by < 0 {
                    left.saturating_sub(by_ms)
                } else {
                    left.saturating_add(by_ms)
                };
                clock.holding(timer.capped(left), now_ms)
            }
            TimerChange::Set(seconds) => {
                clock.holding(timer.capped(seconds.saturating_mul(1000)), now_ms)
            }
        };
        inner.clocks.insert(timer.id, next);
        self.changed();
        Ok(next.left_ms(now_ms))
    }

    /// A new stream: the goals that count per stream start over. Unless it
    /// is the last one carrying on, after InstantClone restarted mid-stream.
    pub fn stream_started(&self, now_ms: u64) {
        let mut inner = self.inner.lock();
        let Inner {
            goals,
            values,
            live_at_ms,
            ..
        } = &mut *inner;
        let carries_on = *live_at_ms > 0 && now_ms.saturating_sub(*live_at_ms) <= SAME_STREAM_MS;
        if !carries_on {
            for goal in goals.iter().filter(|g| g.each_stream) {
                values.remove(&goal.id);
            }
        }
        *live_at_ms = now_ms;
        self.changed();
    }

    /// Each tick while a stream is live: noted (once a minute, and only with
    /// a goal that counts per stream) so a restart can tell the same stream
    /// from a new one.
    pub fn still_live(&self, now_ms: u64) {
        let mut inner = self.inner.lock();
        let per_stream = inner.goals.iter().any(|g| g.each_stream);
        if per_stream && now_ms.saturating_sub(inner.live_at_ms) >= LIVE_NOTE_EVERY_MS {
            inner.live_at_ms = now_ms;
            self.changed();
        }
    }

    /// The stream ended: the next one is a new one, however soon.
    pub fn stream_ended(&self) {
        let mut inner = self.inner.lock();
        if inner.live_at_ms != 0 {
            inner.live_at_ms = 0;
            self.changed();
        }
    }

    /// Goals reached and timers run out since the last call, as events.
    pub fn take_news(&self, now_ms: u64) -> Vec<Event> {
        let mut inner = self.inner.lock();
        let over: Vec<Timer> = inner
            .timers
            .iter()
            .filter(|t| matches!(inner.clocks.get(&t.id), Some(Clock::Running { ends_at_ms }) if *ends_at_ms <= now_ms))
            .cloned()
            .collect();
        for timer in over {
            let ended = inner
                .clocks
                .insert(timer.id.clone(), Clock::Paused { left_ms: 0 });
            let just_now = matches!(ended, Some(Clock::Running { ends_at_ms })
                if now_ms.saturating_sub(ends_at_ms) <= ENDED_NEWS_WITHIN_MS);
            if just_now {
                let ended = Event::new(EventKind::TimerEnded)
                    .with("timer", timer.name)
                    .with("timer_id", timer.id);
                inner.news.push(ended);
            }
            self.changed();
        }
        std::mem::take(&mut inner.news)
    }

    /// What a template name under `goal.` says: `subs` is the count, and
    /// `subs.target`, `subs.left`, `subs.percent` and `subs.name` the rest.
    pub fn goal_text(&self, name: &str) -> Option<String> {
        let (id, part) = name.split_once('.').unwrap_or((name, ""));
        let inner = self.inner.lock();
        let goal = inner.goals.iter().find(|g| g.id == id)?;
        let value = inner.values.get(id).copied().unwrap_or(0);
        Some(match part {
            "" => value.to_string(),
            "target" => goal.target.to_string(),
            "left" => goal.target.saturating_sub(value).to_string(),
            "percent" => (value.min(goal.target) * 100 / goal.target).to_string(),
            "name" => goal.name.clone(),
            _ => return None,
        })
    }

    /// What a template name under `timer.` says: `subathon` is the time
    /// left as a clock, and `subathon.seconds`, `subathon.running` and
    /// `subathon.name` the rest.
    pub fn timer_text(&self, name: &str, now_ms: u64) -> Option<String> {
        let (id, part) = name.split_once('.').unwrap_or((name, ""));
        let inner = self.inner.lock();
        let timer = inner.timers.iter().find(|t| t.id == id)?;
        let clock = inner.clock(timer);
        let left = clock.left_ms(now_ms);
        Some(match part {
            "" => clock_text(left),
            "seconds" => left.div_ceil(1000).to_string(),
            "running" => {
                let running = matches!(clock, Clock::Running { .. }) && left > 0;
                if running { "yes" } else { "no" }.to_string()
            }
            "name" => timer.name.clone(),
            _ => return None,
        })
    }

    /// Everything, for the dashboard: what each is and where it stands.
    /// A running timer says when it ends, not how long is left, so this
    /// only changes when something happens.
    pub fn to_json(&self) -> Value {
        let inner = self.inner.lock();
        let goals = inner.goals.iter().map(|goal| {
            let value = inner.values.get(&goal.id).copied().unwrap_or(0);
            with_fields(goal.to_json(), [("value", Value::Num(value as f64))])
        });
        let timers = inner
            .timers
            .iter()
            .map(|timer| with_fields(timer.to_json(), clock_fields(inner.clock(timer))));
        json::obj([
            ("goals", Value::Arr(goals.collect())),
            ("timers", Value::Arr(timers.collect())),
        ])
    }

    /// What an overlay draws, as two JSON arrays: each goal's name, count
    /// and target, and each timer's name and clock. It is on stream anyway;
    /// how they are set up stays out of it.
    pub fn overlay_json(&self) -> (String, String) {
        let inner = self.inner.lock();
        let goals = inner.goals.iter().map(|goal| {
            let value = inner.values.get(&goal.id).copied().unwrap_or(0);
            json::obj([
                ("id", json::str(&goal.id)),
                ("name", json::str(&goal.name)),
                ("value", Value::Num(value as f64)),
                ("target", Value::Num(goal.target as f64)),
            ])
        });
        let timers = inner.timers.iter().map(|timer| {
            let named = json::obj([
                ("id", json::str(&timer.id)),
                ("name", json::str(&timer.name)),
            ]);
            with_fields(named, clock_fields(inner.clock(timer)))
        });
        (
            Value::Arr(goals.collect()).to_json(),
            Value::Arr(timers.collect()).to_json(),
        )
    }
}

/// A clock as pages read it: `ends_at_ms` while it runs (Unix time),
/// `left_ms` while it doesn't.
fn clock_fields(clock: Clock) -> [(&'static str, Value); 3] {
    let (running, ends_at, left) = match clock {
        Clock::Running { ends_at_ms } => (true, ends_at_ms, 0),
        Clock::Paused { left_ms } => (false, 0, left_ms),
    };
    [
        ("running", Value::Bool(running)),
        ("ends_at_ms", Value::Num(ends_at as f64)),
        ("left_ms", Value::Num(left as f64)),
    ]
}

fn with_fields<const N: usize>(base: Value, more: [(&'static str, Value); N]) -> Value {
    let Value::Obj(mut fields) = base else {
        return base;
    };
    fields.extend(more.into_iter().map(|(k, v)| (k.to_string(), v)));
    Value::Obj(fields)
}

/// Milliseconds as a countdown reads: `4:05`, or `1:02:09` from an hour up.
/// Rounded up, so it says 0:00 only once it is over.
pub fn clock_text(left_ms: u64) -> String {
    super::host::fmt_clock(left_ms.div_ceil(1000).saturating_mul(1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(id: &str, counts: Counts, target: u64) -> Goal {
        Goal {
            id: id.to_string(),
            name: id.to_uppercase(),
            counts,
            target,
            each_stream: false,
        }
    }

    fn subathon() -> Timer {
        Timer {
            id: "subathon".to_string(),
            name: "Subathon".to_string(),
            start_s: 3600,
            per_sub_s: 300,
            per_100_bits_s: 60,
            per_follow_s: 0,
            tiers: true,
            max_s: 0,
        }
    }

    fn tracker(goals: &[Goal], timers: &[Timer]) -> Tracker {
        let tracker = Tracker::new(None, Default::default());
        tracker.configure(goals, timers);
        tracker
    }

    fn sub(tier: &str) -> Event {
        Event::new(EventKind::TwitchSub).with("tier", tier)
    }

    fn kinds(news: &[Event]) -> Vec<EventKind> {
        news.iter().map(|e| e.kind).collect()
    }

    #[test]
    fn a_goal_counts_its_own_kind_and_is_reached_once() {
        let t = tracker(
            &[
                goal("subs", Counts::Subs, 2),
                goal("bits", Counts::Bits, 500),
            ],
            &[],
        );
        t.observe(&sub("1"), 0);
        t.observe(&Event::new(EventKind::TwitchFollow), 0);
        t.observe(&Event::new(EventKind::TwitchCheer).with("bits", "120"), 0);
        assert_eq!(t.goal_text("subs").unwrap(), "1");
        assert_eq!(t.goal_text("bits").unwrap(), "120");
        let heard = [
            EventKind::TwitchSub,
            EventKind::TwitchResub,
            EventKind::TwitchCheer,
        ];
        assert_eq!(t.wanted_events(), heard);
        assert!(t.take_news(0).is_empty());

        t.observe(&Event::new(EventKind::TwitchResub), 0);
        let news = t.take_news(0);
        assert_eq!(kinds(&news), [EventKind::GoalReached]);
        assert_eq!(news[0].var("goal"), Some("SUBS"));
        assert_eq!(news[0].var("target"), Some("2"));
        // Past the target it keeps counting, and says nothing more.
        t.observe(&sub("1"), 0);
        assert!(t.take_news(0).is_empty());
        assert_eq!(t.goal_text("subs.left").unwrap(), "0");
        assert_eq!(t.goal_text("subs.percent").unwrap(), "100");
    }

    #[test]
    fn steps_and_the_dashboard_move_a_goal() {
        let t = tracker(&[goal("pot", Counts::Own, 10)], &[]);
        assert_eq!(t.change_goal("pot", GoalChange::Add(4)), Ok(4));
        assert_eq!(t.change_goal("pot", GoalChange::Add(-9)), Ok(0));
        assert_eq!(t.change_goal("pot", GoalChange::Set(10)), Ok(10));
        assert_eq!(kinds(&t.take_news(0)), [EventKind::GoalReached]);
        assert_eq!(t.change_goal("pot", GoalChange::Reset), Ok(0));
        assert!(t.change_goal("nope", GoalChange::Reset).is_err());
        // A sub is not what this one counts.
        t.observe(&sub("1"), 0);
        assert_eq!(t.goal_text("pot").unwrap(), "0");
    }

    #[test]
    fn a_timer_runs_gains_time_and_ends_once() {
        let t = tracker(&[], &[subathon()]);
        assert_eq!(t.timer_text("subathon", 0).unwrap(), "1:00:00");
        t.change_timer("subathon", TimerChange::Start, 1_000)
            .unwrap();
        // A tier 1 sub is 5 minutes, a tier 3 five times that, 250 bits 2:30.
        t.observe(&sub("1"), 1_000);
        t.observe(&sub("3"), 1_000);
        t.observe(
            &Event::new(EventKind::TwitchCheer).with("bits", "250"),
            1_000,
        );
        assert_eq!(t.timer_text("subathon.seconds", 1_000).unwrap(), "5550");
        assert_eq!(t.timer_text("subathon.running", 1_000).unwrap(), "yes");

        // Paused, it holds what it had, and a sub still adds to it.
        let left = t.change_timer("subathon", TimerChange::Pause, 51_000);
        assert_eq!(left, Ok(5_500_000));
        t.observe(&sub("1"), 900_000);
        assert_eq!(t.timer_text("subathon", 900_000).unwrap(), "1:36:40");

        t.change_timer("subathon", TimerChange::Set(2), 0).unwrap();
        t.change_timer("subathon", TimerChange::Start, 0).unwrap();
        assert!(t.take_news(1_999).is_empty());
        let news = t.take_news(2_000);
        assert_eq!(kinds(&news), [EventKind::TimerEnded]);
        assert_eq!(news[0].var("timer"), Some("Subathon"));
        assert!(t.take_news(9_000).is_empty());
        // Over is over: a late sub doesn't bring it back.
        t.observe(&sub("1"), 9_000);
        assert_eq!(t.timer_text("subathon", 9_000).unwrap(), "0:00");
        assert_eq!(t.timer_text("subathon.running", 9_000).unwrap(), "no");
        assert_eq!(
            t.change_timer("subathon", TimerChange::Start, 9_000),
            Ok(3_600_000)
        );
        assert_eq!(t.timer_text("subathon.running", 9_500).unwrap(), "yes");
        assert_eq!(
            t.change_timer("subathon", TimerChange::Reset, 9_500),
            Ok(3_600_000)
        );
        assert_eq!(t.timer_text("subathon.running", 9_500).unwrap(), "no");
    }

    #[test]
    fn a_timer_never_holds_more_than_its_limit() {
        let mut capped = subathon();
        capped.max_s = 3700;
        capped.tiers = false;
        let t = tracker(&[], &[capped]);
        t.observe(&sub("3"), 0);
        assert_eq!(t.timer_text("subathon.seconds", 0).unwrap(), "3700");
        assert_eq!(
            t.change_timer("subathon", TimerChange::Add(-4000), 0),
            Ok(0)
        );
        assert_eq!(
            t.change_timer("subathon", TimerChange::Add(9_999_999), 0),
            Ok(3_700_000)
        );
    }

    #[test]
    fn where_things_stand_survives_a_restart() {
        let dirty: Arc<AtomicBool> = Default::default();
        let setup = (
            [
                goal("subs", Counts::Subs, 50),
                goal("daily", Counts::Follows, 5),
            ],
            [subathon()],
        );
        let t = Tracker::new(None, dirty.clone());
        t.configure(&setup.0, &setup.1);
        t.observe(&sub("1"), 0);
        t.observe(&Event::new(EventKind::TwitchFollow), 0);
        t.change_timer("subathon", TimerChange::Start, 5_000)
            .unwrap();
        assert!(dirty.load(Ordering::Relaxed));

        let saved = json::parse(&t.state_json().to_json()).unwrap();
        let back = Tracker::new(Some(&saved), Default::default());
        back.configure(&setup.0, &setup.1);
        assert_eq!(back.goal_text("subs").unwrap(), "1");
        assert_eq!(back.timer_text("subathon.seconds", 6_000).unwrap(), "3899");

        // A goal that counts per stream starts over; one that is gone
        // forgets where it stood.
        let mut daily = setup.0[1].clone();
        daily.each_stream = true;
        back.configure(&[daily.clone()], &[]);
        back.stream_started(7_000);
        assert_eq!(back.goal_text("daily").unwrap(), "0");
        assert!(back.goal_text("subs").is_none());
        back.configure(&[daily, setup.0[0].clone()], &setup.1);
        assert_eq!(back.goal_text("subs").unwrap(), "0");
        assert_eq!(back.timer_text("subathon", 0).unwrap(), "1:00:00");
    }

    #[test]
    fn a_restart_mid_stream_carries_on_and_a_new_stream_starts_over() {
        const MIN: u64 = 60_000;
        let mut daily = goal("daily", Counts::Follows, 5);
        daily.each_stream = true;
        let restart = |t: &Tracker| {
            let saved = json::parse(&t.state_json().to_json()).unwrap();
            let back = Tracker::new(Some(&saved), Default::default());
            back.configure(&[daily.clone()], &[subathon()]);
            back
        };
        let t = restart(&Tracker::new(None, Default::default()));
        t.stream_started(0);
        t.observe(&Event::new(EventKind::TwitchFollow), MIN);
        t.still_live(30 * MIN);

        // Back 3 minutes after it was last seen live: the same stream.
        let back = restart(&t);
        back.stream_started(33 * MIN);
        assert_eq!(back.goal_text("daily").unwrap(), "1");
        // Ended, then a new one, however soon: from 0.
        back.stream_ended();
        let back = restart(&back);
        back.stream_started(34 * MIN);
        assert_eq!(back.goal_text("daily").unwrap(), "0");
        // Live long ago, never ended (the app was closed): a new one too.
        back.observe(&Event::new(EventKind::TwitchFollow), 35 * MIN);
        let back = restart(&back);
        back.stream_started(300 * MIN);
        assert_eq!(back.goal_text("daily").unwrap(), "0");
    }

    #[test]
    fn a_timer_that_ran_out_while_the_app_was_closed_ends_quietly() {
        let t = tracker(&[], &[subathon()]);
        t.change_timer("subathon", TimerChange::Start, 0).unwrap();
        let next_day = 24 * 3600 * 1000;
        assert!(t.take_news(next_day).is_empty());
        assert_eq!(t.timer_text("subathon", next_day).unwrap(), "0:00");
    }

    #[test]
    fn an_event_nothing_counts_leaves_the_data_file_alone() {
        let dirty: Arc<AtomicBool> = Default::default();
        let t = Tracker::new(None, dirty.clone());
        t.configure(&[goal("subs", Counts::Subs, 50)], &[subathon()]);
        dirty.store(false, Ordering::Relaxed);
        t.observe(&Event::new(EventKind::TwitchFollow), 0);
        assert!(!dirty.load(Ordering::Relaxed));
        // More bits than any real cheer fill the timer to its limit, no more.
        let bits = u64::MAX.to_string();
        t.observe(&Event::new(EventKind::TwitchCheer).with("bits", bits), 0);
        assert!(dirty.load(Ordering::Relaxed));
        let most = MAX_TIMER_S.to_string();
        assert_eq!(t.timer_text("subathon.seconds", 0).unwrap(), most);
    }

    #[test]
    fn a_running_clock_reads_the_same_until_something_happens() {
        let t = tracker(&[goal("subs", Counts::Subs, 50)], &[subathon()]);
        t.change_timer("subathon", TimerChange::Start, 1_000)
            .unwrap();
        let (goals, timers) = t.overlay_json();
        assert_eq!(
            goals,
            r#"[{"id":"subs","name":"SUBS","value":0,"target":50}]"#
        );
        let clock = json::parse(&timers).unwrap();
        let clock = &clock.as_array().unwrap()[0];
        assert_eq!(clock.u64_or("ends_at_ms", 0), 3_601_000);
        assert!(clock.bool_or("running", false));
        // How it is set up is the dashboard's business, not the stream's.
        assert!(!timers.contains("per_sub_s"));
        assert_eq!(t.overlay_json().1, timers);
        assert_eq!(t.to_json().to_json(), t.to_json().to_json());
    }

    #[test]
    fn a_setup_is_checked_before_it_is_kept() {
        let ok = json::parse(
            r#"{"goals":[{"id":"subs","name":" Sub goal ","counts":"subs","target":50}],
                "timers":[{"id":"t1","name":"Subathon","start_s":3600,"per_sub_s":300}]}"#,
        )
        .unwrap();
        let (goals, timers) = parse_setup(&ok).unwrap();
        assert_eq!(
            (goals[0].name.as_str(), goals[0].counts),
            ("Sub goal", Counts::Subs)
        );
        assert!(timers[0].tiers && timers[0].max_s == 0);
        assert_eq!(Goal::from_json(&goals[0].to_json()).unwrap(), goals[0]);
        assert_eq!(Timer::from_json(&timers[0].to_json()).unwrap(), timers[0]);

        for bad in [
            r#"{"goals":[{"id":"Sub Goal","name":"x","target":5}]}"#,
            r#"{"goals":[{"id":"a","name":"","target":5}]}"#,
            r#"{"goals":[{"id":"a","name":"x","target":0}]}"#,
            r#"{"timers":[{"id":"a","name":"x","start_s":0}]}"#,
            r#"{"goals":[{"id":"a","name":"x","target":5},{"id":"a","name":"y","target":5}]}"#,
        ] {
            assert!(parse_setup(&json::parse(bad).unwrap()).is_err(), "{bad}");
        }
        assert_eq!(parse_setup(&Value::Null).unwrap(), (vec![], vec![]));
    }

    #[test]
    fn a_countdown_reads_like_a_clock() {
        assert_eq!(clock_text(0), "0:00");
        assert_eq!(clock_text(1), "0:01");
        assert_eq!(clock_text(245_000), "4:05");
        assert_eq!(clock_text(3_729_000), "1:02:09");
    }
}
