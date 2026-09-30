//! The integrations engine: matches what happens to the integrations that
//! care, and runs them.
//!
//! It lives on its own OS thread with its own single-threaded runtime. The
//! stream pipeline never shares a thread with it, so a slow webhook, a
//! chat flood or a long `wait` cannot delay a single video frame. Events
//! come in through a bounded channel whose `try_send` works from any thread
//! (the controller, the tray, hotkey and MIDI callbacks); if the engine ever
//! falls behind, events are dropped and counted, never queued without end.
//!
//! Limits that keep a runaway integration harmless:
//! - each integration runs at most `PER_INTEGRATION` copies at once, all of
//!   them at most `GLOBAL_RUNS`;
//! - `cooldown_ms` spaces runs of the same trigger; chat commands also have
//!   a per-viewer cooldown;
//! - the runner caps waits, nesting and message sizes.

use super::effects::RealHost;
use super::event::{Event, EventKind};
use super::model::{DiscordChannel, Integration, MatchMode, PhoneConnection, Roles, Trigger};
use super::runner::{self, RunContext, RunEnv, RunStatus, StepLog};
use super::store::Store;
use super::twitch::irc::ChatMessage;
use super::twitch::{unix_ms, Twitch, Which};
use crate::config::Settings;
use crate::controller::Controller;
use crate::json::{self, Value};
use crate::sync::Mutex;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch, Semaphore};

const QUEUE: usize = 512;
const PER_INTEGRATION: usize = 4;
const GLOBAL_RUNS: usize = 32;
const ACTIVITY_KEEP: usize = 150;
const TICK: Duration = Duration::from_millis(250);

enum Input {
    Event(Event),
    Hook {
        token: String,
        body: String,
        query: String,
    },
    Test {
        integration: Box<Integration>,
        handler: usize,
        reply: oneshot::Sender<Record>,
    },
    Login(Which),
    CancelLogin,
    Logout(Which),
}

/// One run, for the activity log.
#[derive(Clone, Debug)]
pub struct Record {
    pub at_ms: u64,
    pub integration_id: String,
    pub name: String,
    pub trigger: String,
    /// `ok`, `stopped`, `failed` or `busy`.
    pub status: &'static str,
    pub test: bool,
    pub steps: Vec<StepLog>,
}

impl Record {
    fn to_json(&self) -> Value {
        json::obj([
            ("at_ms", Value::Num(self.at_ms as f64)),
            ("integration_id", json::str(&self.integration_id)),
            ("name", json::str(&self.name)),
            ("trigger", json::str(&self.trigger)),
            ("status", json::str(self.status)),
            ("test", Value::Bool(self.test)),
            (
                "steps",
                Value::Arr(
                    self.steps
                        .iter()
                        .map(|s| {
                            json::obj([
                                ("label", json::str(&s.label)),
                                ("status", json::str(s.status.id())),
                                ("at_ms", Value::Num(s.at_ms as f64)),
                                ("detail", json::str(&s.detail)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

#[derive(Clone, Default)]
struct Stat {
    last_ms: u64,
    last_status: &'static str,
    /// Failures in a row; 0 after a good run.
    failing: u32,
    runs: u64,
}

/// The engine's public face: shared with the controller and the web layer.
pub struct Handle {
    tx: mpsc::Sender<Input>,
    activity: Mutex<VecDeque<Record>>,
    stats: Mutex<HashMap<String, Stat>>,
    pub twitch: Arc<Twitch>,
    dropped: AtomicU64,
}

impl Handle {
    /// Report something that happened. Never blocks; safe from any thread.
    pub fn emit(&self, event: Event) {
        if self.tx.try_send(Input::Event(event)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A call to `/hooks/<token>`.
    pub fn hook(&self, token: String, body: String, query: String) {
        let _ = self.tx.try_send(Input::Hook { token, body, query });
    }

    /// Run one handler now with sample values, messages marked `[TEST]`.
    pub async fn test(&self, integration: Integration, handler: usize) -> Option<Record> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .try_send(Input::Test {
                integration: Box::new(integration),
                handler,
                reply,
            })
            .ok()?;
        tokio::time::timeout(Duration::from_secs(60), rx)
            .await
            .ok()?
            .ok()
    }

    pub fn twitch_login(&self, which: Which) {
        let _ = self.tx.try_send(Input::Login(which));
    }

    pub fn twitch_cancel_login(&self) {
        let _ = self.tx.try_send(Input::CancelLogin);
    }

    pub fn twitch_logout(&self, which: Which) {
        let _ = self.tx.try_send(Input::Logout(which));
    }

    fn push(&self, record: Record) {
        {
            let mut stats = self.stats.lock();
            let stat = stats.entry(record.integration_id.clone()).or_default();
            if !record.test {
                stat.last_ms = record.at_ms;
                stat.last_status = record.status;
                stat.runs += 1;
                stat.failing = if record.status == "failed" {
                    stat.failing + 1
                } else {
                    0
                };
            }
        }
        let mut activity = self.activity.lock();
        if activity.len() >= ACTIVITY_KEEP {
            activity.pop_front();
        }
        activity.push_back(record);
    }

    /// Recent runs, newest first.
    pub fn activity_json(&self) -> Value {
        Value::Arr(
            self.activity
                .lock()
                .iter()
                .rev()
                .map(Record::to_json)
                .collect(),
        )
    }

    /// Per-integration last run, keyed by integration id.
    pub fn stats_json(&self) -> Value {
        Value::Obj(
            self.stats
                .lock()
                .iter()
                .map(|(id, s)| {
                    (
                        id.clone(),
                        json::obj([
                            ("last_ms", Value::Num(s.last_ms as f64)),
                            ("last_status", json::str(s.last_status)),
                            ("failing", Value::Num(s.failing as f64)),
                            ("runs", Value::Num(s.runs as f64)),
                        ]),
                    )
                })
                .collect(),
        )
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Start the engine thread. `data_dir` holds the integrations data file.
pub fn start(
    ctrl: Arc<Controller>,
    settings: watch::Receiver<Settings>,
    data_dir: PathBuf,
) -> std::io::Result<Arc<Handle>> {
    let (tx, rx) = mpsc::channel(QUEUE);
    let (chat_tx, chat_rx) = mpsc::channel(256);
    let store = Arc::new(Store::open(&data_dir));
    let log_ctrl = ctrl.clone();
    let twitch = Arc::new(Twitch::new(
        store.clone(),
        chat_tx,
        Box::new(move |line| log_ctrl.log(line)),
    ));
    let handle = Arc::new(Handle {
        tx,
        activity: Mutex::new(VecDeque::new()),
        stats: Mutex::new(HashMap::new()),
        twitch: twitch.clone(),
        dropped: AtomicU64::new(0),
    });
    let engine_handle = handle.clone();
    std::thread::Builder::new()
        .name("instantclone-integrations".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    ctrl.log(format!("[integrations] could not start: {e}"));
                    return;
                }
            };
            runtime.block_on(async move {
                twitch.start();
                let host = Arc::new(RealHost {
                    ctrl: ctrl.clone(),
                    twitch: twitch.clone(),
                    default_delay_ms: Default::default(),
                });
                let mut engine = Engine::new(engine_handle, host, store, ctrl);
                engine.run(rx, chat_rx, settings).await;
            });
        })?;
    Ok(handle)
}

struct Engine {
    handle: Arc<Handle>,
    host: Arc<RealHost>,
    store: Arc<Store>,
    ctrl: Arc<Controller>,
    integrations: Vec<Arc<Integration>>,
    env: Arc<RunEnv>,
    last_run: HashMap<String, Instant>,
    viewer_last: HashMap<String, Instant>,
    timers: HashMap<String, Instant>,
    limits: HashMap<String, Arc<Semaphore>>,
    global: Arc<Semaphore>,
    delay_seen: Option<u32>,
}

impl Engine {
    fn new(
        handle: Arc<Handle>,
        host: Arc<RealHost>,
        store: Arc<Store>,
        ctrl: Arc<Controller>,
    ) -> Engine {
        let env = Arc::new(make_env(Vec::new(), PhoneConnection::default(), &store));
        Engine {
            handle,
            host,
            store,
            ctrl,
            integrations: Vec::new(),
            env,
            last_run: HashMap::new(),
            viewer_last: HashMap::new(),
            timers: HashMap::new(),
            limits: HashMap::new(),
            global: Arc::new(Semaphore::new(GLOBAL_RUNS)),
            delay_seen: None,
        }
    }

    async fn run(
        &mut self,
        mut rx: mpsc::Receiver<Input>,
        mut chat_rx: mpsc::Receiver<ChatMessage>,
        mut settings: watch::Receiver<Settings>,
    ) {
        self.apply_settings(&settings.borrow_and_update());
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                input = rx.recv() => match input {
                    Some(input) => self.input(input),
                    None => return,
                },
                Some(msg) = chat_rx.recv() => self.chat(msg),
                changed = settings.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let snapshot = settings.borrow_and_update().clone();
                    self.apply_settings(&snapshot);
                }
                _ = tick.tick() => {
                    self.watch_delay();
                    self.run_timers();
                }
            }
        }
    }

    fn apply_settings(&mut self, s: &Settings) {
        self.integrations = s.integrations.iter().cloned().map(Arc::new).collect();
        self.env = Arc::new(make_env(
            s.discord_channels.clone(),
            s.phone.clone(),
            &self.store,
        ));
        self.host
            .default_delay_ms
            .store(s.auto_arm_delay_ms.max(1000), Ordering::Relaxed);
        // Forget per-trigger state for integrations that are gone.
        let alive: Vec<&str> = self.integrations.iter().map(|i| i.id.as_str()).collect();
        let keep = |key: &String| alive.iter().any(|id| key.starts_with(&format!("{id}#")));
        self.last_run.retain(|k, _| keep(k));
        self.viewer_last.retain(|k, _| keep(k));
        self.timers.retain(|k, _| keep(k));
        self.limits.retain(|id, _| alive.contains(&id.as_str()));
    }

    fn input(&mut self, input: Input) {
        match input {
            Input::Event(event) => self.event(event),
            Input::Hook { token, body, query } => self.hook(&token, body, query),
            Input::Test {
                integration,
                handler,
                reply,
            } => self.test(*integration, handler, reply),
            Input::Login(which) => self.handle.twitch.begin_login(which),
            Input::CancelLogin => self.handle.twitch.cancel_login(),
            Input::Logout(which) => self.handle.twitch.logout(which),
        }
    }

    fn event(&mut self, event: Event) {
        let matches: Vec<(Arc<Integration>, usize)> = self
            .enabled_handlers()
            .filter(|(_, _, h)| match &h.trigger {
                Trigger::Event { kind, filters } => {
                    *kind == event.kind && filters_match(filters, &event)
                }
                _ => false,
            })
            .map(|(i, idx, _)| (i, idx))
            .collect();
        for (integration, idx) in matches {
            let vars = event
                .vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect();
            self.start_run(integration, idx, vars, None, event.kind.label().to_string());
        }
    }

    fn chat(&mut self, msg: ChatMessage) {
        // Our own bot talking must never trigger us.
        let bot = self
            .store
            .accounts
            .lock()
            .bot
            .as_ref()
            .map(|b| b.login.clone());
        if bot.is_some_and(|b| b.eq_ignore_ascii_case(&msg.user_login)) {
            return;
        }
        let text = msg.text.trim();
        let mut words = text.split_whitespace();
        let first = words.next().unwrap_or("").to_lowercase();
        let args: Vec<&str> = words.collect();
        let candidates: Vec<(Arc<Integration>, usize, String)> = self
            .enabled_handlers()
            .filter_map(|(i, idx, h)| match &h.trigger {
                Trigger::ChatCommand {
                    command,
                    aliases,
                    roles,
                    user_cooldown_ms,
                } if (*command == first || aliases.contains(&first)) && allowed(*roles, &msg) => {
                    let key = format!("{}#{idx}#{}", i.id, msg.user_login);
                    let cooled = self
                        .viewer_last
                        .get(&key)
                        .is_some_and(|t| t.elapsed() < Duration::from_millis(*user_cooldown_ms));
                    (!cooled).then(|| (i.clone(), idx, first.clone()))
                }
                Trigger::ChatMessage {
                    pattern,
                    mode,
                    roles,
                } if allowed(*roles, &msg) && text_matches(text, pattern, *mode) => {
                    Some((i.clone(), idx, "chat".to_string()))
                }
                _ => None,
            })
            .collect();
        for (integration, idx, label) in candidates {
            self.viewer_last.insert(
                format!("{}#{idx}#{}", integration.id, msg.user_login),
                Instant::now(),
            );
            let vars = chat_vars(&msg, &args);
            let trigger = if label == "chat" {
                "Chat message".to_string()
            } else {
                format!("{label} in chat")
            };
            self.start_run(integration, idx, vars, Some(msg.id.clone()), trigger);
        }
    }

    fn hook(&mut self, token: &str, body: String, query: String) {
        let matches: Vec<(Arc<Integration>, usize)> = self
            .enabled_handlers()
            .filter(|(_, _, h)| {
                matches!(&h.trigger, Trigger::Webhook { token: t }
                    if crate::crypto::constant_time_eq(t.as_bytes(), token.as_bytes()))
            })
            .map(|(i, idx, _)| (i, idx))
            .collect();
        for (integration, idx) in matches {
            let vars = BTreeMap::from([
                ("body".to_string(), body.clone()),
                ("query".to_string(), query.clone()),
            ]);
            self.start_run(integration, idx, vars, None, "Web call".to_string());
        }
    }

    fn run_timers(&mut self) {
        let live = self.ctrl.ingest_alive();
        let timers: Vec<(Arc<Integration>, usize, u64, bool)> = self
            .enabled_handlers()
            .filter_map(|(i, idx, h)| match h.trigger {
                Trigger::Timer {
                    every_ms,
                    only_live,
                } => Some((i, idx, every_ms, only_live)),
                _ => None,
            })
            .collect();
        for (integration, idx, every_ms, only_live) in timers {
            let key = format!("{}#{idx}", integration.id);
            // A new timer waits one full period before its first run.
            let last = *self.timers.entry(key.clone()).or_insert_with(Instant::now);
            if last.elapsed() < Duration::from_millis(every_ms) {
                continue;
            }
            self.timers.insert(key, Instant::now());
            if live || !only_live {
                self.start_run(integration, idx, BTreeMap::new(), None, "Timer".to_string());
            }
        }
    }

    /// Turn changes in the applied delay into delay events.
    fn watch_delay(&mut self) {
        let now = self.ctrl.target_delay_ms();
        let Some(before) = self.delay_seen.replace(now) else {
            return;
        };
        let fmt = super::host::fmt_delay;
        let kind = match (before, now) {
            (0, n) if n > 0 => EventKind::DelayOn,
            (b, 0) if b > 0 => EventKind::DelayOff,
            (b, n) if b != n => EventKind::DelayChanged,
            _ => return,
        };
        let previous = if before == 0 {
            "0s".to_string()
        } else {
            fmt(before)
        };
        self.event(Event::new(kind).with("previous", previous));
    }

    fn enabled_handlers(
        &self,
    ) -> impl Iterator<Item = (Arc<Integration>, usize, &super::model::Handler)> + '_ {
        self.integrations
            .iter()
            .filter(|i| i.enabled)
            .flat_map(|i| {
                i.handlers
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| h.enabled)
                    .map(move |(idx, h)| (i.clone(), idx, h))
            })
    }

    fn start_run(
        &mut self,
        integration: Arc<Integration>,
        idx: usize,
        vars: BTreeMap<String, String>,
        reply_to: Option<String>,
        trigger: String,
    ) {
        let key = format!("{}#{idx}", integration.id);
        if integration.cooldown_ms > 0
            && self
                .last_run
                .get(&key)
                .is_some_and(|t| t.elapsed() < Duration::from_millis(integration.cooldown_ms))
        {
            return;
        }
        self.last_run.insert(key, Instant::now());
        let limit = self
            .limits
            .entry(integration.id.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(PER_INTEGRATION)))
            .clone();
        let permits = (
            limit.try_acquire_owned(),
            self.global.clone().try_acquire_owned(),
        );
        let (Ok(own), Ok(global)) = permits else {
            self.handle.push(Record {
                at_ms: unix_ms(),
                integration_id: integration.id.clone(),
                name: integration.name.clone(),
                trigger,
                status: "busy",
                test: false,
                steps: Vec::new(),
            });
            return;
        };
        let ctx = RunContext {
            vars,
            reply_to,
            test: false,
        };
        let handle = self.handle.clone();
        let ctrl = self.ctrl.clone();
        let host: Arc<dyn super::host::Host> = self.host.clone();
        let env = self.env.clone();
        tokio::spawn(async move {
            let _permits = (own, global);
            let at_ms = unix_ms();
            let (status, steps) =
                runner::run(&integration.handlers[idx].steps, ctx, host, env).await;
            let status = status_id(&status);
            if status == "failed" {
                if let Some(step) = steps
                    .iter()
                    .find(|s| s.status == runner::StepStatus::Failed)
                {
                    ctrl.log(format!(
                        "[integrations] {}: {} failed: {}",
                        integration.name, step.label, step.detail
                    ));
                }
            }
            handle.push(Record {
                at_ms,
                integration_id: integration.id.clone(),
                name: integration.name.clone(),
                trigger,
                status,
                test: false,
                steps,
            });
        });
    }

    fn test(&mut self, integration: Integration, idx: usize, reply: oneshot::Sender<Record>) {
        let Some(handler) = integration.handlers.get(idx).cloned() else {
            return;
        };
        let (vars, trigger) = sample_vars(&handler.trigger);
        let ctx = RunContext {
            vars,
            reply_to: None,
            test: true,
        };
        let host: Arc<dyn super::host::Host> = self.host.clone();
        let env = self.env.clone();
        let handle = self.handle.clone();
        tokio::spawn(async move {
            let at_ms = unix_ms();
            let (status, steps) = runner::run(&handler.steps, ctx, host, env).await;
            let record = Record {
                at_ms,
                integration_id: integration.id.clone(),
                name: integration.name.clone(),
                trigger,
                status: status_id(&status),
                test: true,
                steps,
            };
            handle.push(record.clone());
            let _ = reply.send(record);
        });
    }
}

fn status_id(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Ok => "ok",
        RunStatus::Stopped => "stopped",
        RunStatus::Failed => "failed",
    }
}

fn make_env(discord: Vec<DiscordChannel>, phone: PhoneConnection, store: &Arc<Store>) -> RunEnv {
    let saver = store.clone();
    RunEnv {
        discord,
        phone,
        counters: store.counters.clone(),
        counters_changed: Box::new(move || {
            let _ = saver.save();
        }),
    }
}

fn filters_match(filters: &BTreeMap<String, String>, event: &Event) -> bool {
    filters.iter().all(|(name, want)| {
        want.trim().is_empty()
            || event
                .var(name)
                .is_some_and(|have| have.trim().eq_ignore_ascii_case(want.trim()))
    })
}

fn allowed(roles: Roles, msg: &ChatMessage) -> bool {
    msg.is_broadcaster
        || roles.everyone
        || (roles.mods && msg.is_mod)
        || (roles.vips && msg.is_vip)
        || (roles.subs && msg.is_sub)
}

fn role_of(msg: &ChatMessage) -> &'static str {
    if msg.is_broadcaster {
        "broadcaster"
    } else if msg.is_mod {
        "mod"
    } else if msg.is_vip {
        "vip"
    } else if msg.is_sub {
        "sub"
    } else {
        "viewer"
    }
}

fn chat_vars(msg: &ChatMessage, args: &[&str]) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::from([
        ("user".to_string(), msg.display_name.clone()),
        ("user_login".to_string(), msg.user_login.clone()),
        ("user_role".to_string(), role_of(msg).to_string()),
        ("message".to_string(), msg.text.clone()),
        ("args".to_string(), args.join(" ")),
    ]);
    for (i, arg) in args.iter().take(3).enumerate() {
        vars.insert(format!("arg{}", i + 1), arg.to_string());
    }
    vars
}

/// Trigger variables with sample values, for test runs.
fn sample_vars(trigger: &Trigger) -> (BTreeMap<String, String>, String) {
    let owned = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>()
    };
    match trigger {
        Trigger::Event { kind, .. } => (
            Event::sample(*kind)
                .vars
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            format!("Test: {}", kind.label()),
        ),
        Trigger::ChatCommand { command, .. } => (
            owned(&[
                ("user", "TestViewer"),
                ("user_login", "testviewer"),
                ("user_role", "mod"),
                ("message", &format!("{command} 30")),
                ("args", "30"),
                ("arg1", "30"),
            ]),
            format!("Test: {command}"),
        ),
        Trigger::ChatMessage { pattern, .. } => (
            owned(&[
                ("user", "TestViewer"),
                ("user_login", "testviewer"),
                ("user_role", "viewer"),
                ("message", pattern.as_str()),
            ]),
            "Test: chat message".to_string(),
        ),
        Trigger::Timer { .. } => (BTreeMap::new(), "Test: timer".to_string()),
        Trigger::Webhook { .. } => (
            owned(&[("body", r#"{"test":true}"#), ("query", "")]),
            "Test: web call".to_string(),
        ),
    }
}

/// Match chat text against a pattern, ignoring case.
pub fn text_matches(text: &str, pattern: &str, mode: MatchMode) -> bool {
    let text = text.to_lowercase();
    let pattern = pattern.trim().to_lowercase();
    if pattern.is_empty() {
        return false;
    }
    match mode {
        MatchMode::Contains => text.contains(&pattern),
        MatchMode::StartsWith => text.starts_with(&pattern),
        MatchMode::Exact => text.trim() == pattern,
        MatchMode::Wildcard => wildcard(text.trim(), &pattern),
    }
}

/// `*` = any run of characters, `?` = one character. Iterative, so a
/// hostile pattern cannot blow up the way naive backtracking would.
fn wildcard(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut ti, mut pi) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            ti += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(text: &str) -> ChatMessage {
        ChatMessage {
            text: text.into(),
            user_login: "ana".into(),
            display_name: "Ana".into(),
            ..ChatMessage::default()
        }
    }

    #[test]
    fn wildcards_match_like_file_globs() {
        assert!(wildcard("gg wp everyone", "gg*"));
        assert!(wildcard("clip it", "clip ??"));
        assert!(wildcard("abc", "*b*"));
        assert!(!wildcard("abc", "a?"));
        assert!(wildcard(&"a".repeat(200), &"*a".repeat(50)));
    }

    #[test]
    fn matching_ignores_case() {
        assert!(text_matches("POG champ", "pog", MatchMode::StartsWith));
        assert!(text_matches(" GG ", "gg", MatchMode::Exact));
        assert!(!text_matches("gg", "", MatchMode::Contains));
    }

    #[test]
    fn roles_gate_commands_but_never_the_broadcaster() {
        let mut m = msg("!cut");
        assert!(!allowed(Roles::MODS, &m));
        m.is_mod = true;
        assert!(allowed(Roles::MODS, &m));
        let mut owner = msg("!cut");
        owner.is_broadcaster = true;
        assert!(allowed(Roles::MODS, &owner));
    }

    #[test]
    fn chat_vars_split_arguments() {
        let v = chat_vars(&msg("!setdelay 45 now"), &["45", "now"]);
        assert_eq!(v["arg1"], "45");
        assert_eq!(v["arg2"], "now");
        assert_eq!(v["args"], "45 now");
        assert_eq!(v["user_role"], "viewer");
    }

    #[test]
    fn filters_compare_event_values() {
        let crash = Event::new(EventKind::HoldOpened).with("reason", "crash");
        let want = |v: &str| BTreeMap::from([("reason".to_string(), v.to_string())]);
        assert!(filters_match(&want("Crash"), &crash));
        assert!(!filters_match(&want("freeze"), &crash));
        assert!(filters_match(&want(""), &crash));
        assert!(filters_match(&BTreeMap::new(), &crash));
    }
}
