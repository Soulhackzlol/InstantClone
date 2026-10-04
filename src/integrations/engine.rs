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
//! - each integration has at most `PER_INTEGRATION` runs doing something at
//!   once (`GLOBAL_RUNS` for all of them), and at most `PER_INTEGRATION_RUNS`
//!   started and not finished (`GLOBAL_STARTED`): a run waiting for the
//!   delay gives its place back, so presses during a long delay queue up;
//! - `cooldown_ms` spaces runs of the same trigger; chat commands also have
//!   a per-viewer cooldown;
//! - quiet hours keep an integration silent for part of the day;
//! - the runner caps waits, nesting and message sizes.

use super::activity::ChatWindow;
use super::alerts::Board;
use super::effects::RealHost;
use super::event::{Event, EventKind};
use super::model::{
    pad_matches, ChatActivity, DiscordChannel, Integration, MatchMode, PhoneConnection, Roles,
    Trigger,
};
use super::obsws::{self, SceneNews};
use super::runner::{self, RunContext, RunEnv, RunStatus, StepLog};
use super::session::Session;
use super::store::Store;
use super::timeline::{Kind as LineKind, Timeline};
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
const PER_INTEGRATION_RUNS: usize = 16;
const GLOBAL_STARTED: usize = 64;
const ACTIVITY_KEEP: usize = 150;
const TICK: Duration = Duration::from_millis(250);
/// How often per-viewer and per-trigger bookkeeping is trimmed. A busy
/// chat adds an entry per viewer per command; without trimming, a long
/// stream would only ever grow them.
const PRUNE_EVERY: Duration = Duration::from_secs(60);
/// Runs counted for `{uses}` are written at most this often: a busy chat
/// command would otherwise rewrite the data file on every use.
const USES_SAVE_EVERY: Duration = Duration::from_secs(5);
/// Runs each card's history strip shows.
const RECENT_KEEP: usize = 12;
/// A destination stuck in a connect-then-drop loop (a key the platform
/// accepts, then refuses) would otherwise alert on every round. Each
/// destination gets at most one "live" and one "dropped" per window.
const FLAP_WINDOW: Duration = Duration::from_secs(300);
/// A chat activity trigger stays quiet this long after a stream starts:
/// the first seconds of chat ("hi!", "hello") are a burst by nature.
pub const ACTIVITY_WARMUP: Duration = Duration::from_secs(30);
/// How often chat activity triggers judge chat at most.
const ACTIVITY_READ_EVERY: Duration = Duration::from_millis(250);
/// How long the dashboard keeps the OBS connection open after asking for
/// the scene list, so picking a scene works before anything is saved.
const OBS_FOR_DASHBOARD: Duration = Duration::from_secs(60);

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
        run: TestRun,
        reply: oneshot::Sender<Record>,
    },
    #[cfg_attr(not(windows), allow(dead_code))]
    Shortcut(Shortcut, String),
    Scene(SceneNews),
    Login(Which),
    CancelLogin,
    Logout(Which),
}

/// How a test runs: values to use instead of the samples, and whether it
/// may send anything.
#[derive(Clone, Debug, Default)]
pub struct TestRun {
    pub values: BTreeMap<String, String>,
    pub dry: bool,
}

/// What a shortcut press came from. Only Windows has hotkeys and MIDI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Shortcut {
    Hotkey,
    Midi,
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
    /// The latest runs' outcomes, oldest first.
    recent: VecDeque<&'static str>,
}

/// The engine's public face: shared with the controller and the web layer.
pub struct Handle {
    tx: mpsc::Sender<Input>,
    activity: Mutex<VecDeque<Record>>,
    stats: Mutex<HashMap<String, Stat>>,
    pub twitch: Arc<Twitch>,
    /// What "Show on stream" steps put on the alerts browser source.
    pub alerts: Arc<Board>,
    store: Arc<Store>,
    /// Integration hotkeys Windows wouldn't register (another app holds
    /// them, or there are too many), for the dashboard to point out.
    refused_keys: Mutex<Vec<String>>,
    dropped: AtomicU64,
    /// The last minutes of chat, read by chat activity triggers and by the
    /// dashboard's live meter while one is being tuned.
    chat: Arc<Mutex<ChatWindow>>,
    /// The OBS WebSocket connection, as scene triggers and the dashboard
    /// see it.
    obs: Arc<Mutex<obsws::Status>>,
    /// Until when (Unix ms) the dashboard wants OBS connected.
    obs_wanted_until: AtomicU64,
    /// Why chat activity triggers can't fire right now, or empty: see
    /// `Engine::activity_gate`. For the dashboard's live meter.
    activity_held: Mutex<&'static str>,
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
        if self
            .tx
            .try_send(Input::Hook { token, body, query })
            .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A global hotkey or MIDI pad was pressed; `signature` is the combo
    /// (`Ctrl+Alt+K`) or the pad (`note:1:36@Device`). Never blocks.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn shortcut(&self, from: Shortcut, signature: &str) {
        let input = Input::Shortcut(from, signature.to_string());
        if self.tx.try_send(input).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn set_refused_keys(&self, combos: Vec<String>) {
        *self.refused_keys.lock() = combos;
    }

    pub fn refused_keys(&self) -> Vec<String> {
        self.refused_keys.lock().clone()
    }

    /// Write what is kept in memory between saves (the `{uses}` counts).
    /// Call before the app exits.
    pub fn flush(&self) {
        let _ = self.store.save();
    }

    /// Run one handler now with sample values (`values` override them),
    /// messages marked `[TEST]`; a `dry` test sends nothing at all.
    pub async fn test(
        &self,
        integration: Integration,
        handler: usize,
        run: TestRun,
    ) -> Option<Record> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .try_send(Input::Test {
                integration: Box::new(integration),
                handler,
                run,
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
        // "busy" and "cooldown" aren't runs: they must not clear a failing
        // streak or fill the history dots, and a flood of them is one line
        // in the log.
        let busy = matches!(record.status, "busy" | "cooldown");
        if busy {
            let mut activity = self.activity.lock();
            // Among the last few, so two integrations turned away in turn
            // can't push real runs out of the log.
            if let Some(last) =
                activity.iter_mut().rev().take(8).find(|r| {
                    r.status == record.status && r.integration_id == record.integration_id
                })
            {
                last.at_ms = record.at_ms;
                return;
            }
        }
        {
            let mut stats = self.stats.lock();
            let stat = stats.entry(record.integration_id.clone()).or_default();
            if !record.test && !busy {
                stat.last_ms = record.at_ms;
                stat.last_status = record.status;
                stat.runs += 1;
                stat.failing = if record.status == "failed" {
                    stat.failing + 1
                } else {
                    0
                };
                if stat.recent.len() >= RECENT_KEEP {
                    stat.recent.pop_front();
                }
                stat.recent.push_back(record.status);
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
                            (
                                "recent",
                                Value::Arr(s.recent.iter().map(|r| json::str(*r)).collect()),
                            ),
                        ]),
                    )
                })
                .collect(),
        )
    }

    /// Forget the stats of integrations that no longer exist.
    fn retain_stats(&self, alive: &[&str]) {
        self.stats
            .lock()
            .retain(|id, _| alive.contains(&id.as_str()));
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// How chat reads right now against `rules`: the live meter of a chat
    /// activity trigger being tuned, saved or not.
    pub fn chat_meter(&self, rules: &ChatActivity) -> Value {
        let r = self.chat.lock().read(rules, Instant::now());
        // Not streaming and the first seconds only hold back a trigger that
        // only runs while streaming; the reconnect screen holds back all.
        let held = match *self.activity_held.lock() {
            "offline" | "warmup" if !rules.only_live => "",
            held => held,
        };
        json::obj([
            ("held", json::str(held)),
            (
                "shares",
                Value::Arr(r.shares.iter().map(|s| Value::Num(s.round())).collect()),
            ),
            ("messages", Value::Num(r.messages as f64)),
            ("normal", Value::Num((r.normal * 10.0).round() / 10.0)),
            ("busier", Value::Num((r.busier * 10.0).round() / 10.0)),
            ("chatters", Value::Num(r.chatters as f64)),
            ("word_share", Value::Num(r.word_share.round())),
            ("top_word", json::str(&r.top_word)),
            ("ready", Value::Bool(r.ready)),
            (
                "passing",
                Value::Arr(r.passing.iter().map(|p| Value::Bool(*p)).collect()),
            ),
            ("fires", Value::Bool(r.fires)),
        ])
    }

    /// The OBS connection and its scenes. Asking keeps OBS connected for a
    /// minute, so the scene list fills in while a trigger is being set up.
    pub fn obs_status(&self) -> Value {
        self.obs_wanted_until.store(
            unix_ms() + OBS_FOR_DASHBOARD.as_millis() as u64,
            Ordering::Relaxed,
        );
        self.obs.lock().to_json()
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
        &settings.borrow().twitch_client_id,
    ));
    let alerts = Arc::new(Board::new());
    let obs = Arc::new(Mutex::new(obsws::Status::default()));
    let handle = Arc::new(Handle {
        tx,
        activity: Mutex::new(VecDeque::new()),
        stats: Mutex::new(HashMap::new()),
        twitch: twitch.clone(),
        alerts: alerts.clone(),
        store: store.clone(),
        refused_keys: Mutex::new(Vec::new()),
        dropped: AtomicU64::new(0),
        chat: Arc::new(Mutex::new(ChatWindow::default())),
        obs: obs.clone(),
        obs_wanted_until: AtomicU64::new(0),
        activity_held: Mutex::new("offline"),
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
                let (obs_tx, obs_commands) = mpsc::channel(16);
                let host = Arc::new(RealHost {
                    ctrl: ctrl.clone(),
                    twitch: twitch.clone(),
                    default_delay_ms: Default::default(),
                    programs: Default::default(),
                    alerts,
                    stream_started_ms: AtomicU64::new(0),
                    onair: Mutex::new("off"),
                    obs: obs_tx,
                });
                // OBS's scene news reaches the engine like any other input.
                let (want_obs, wanted) = watch::channel(false);
                let (news_tx, mut news_rx) = mpsc::channel(64);
                tokio::spawn(obsws::run(wanted, obs, news_tx, obs_commands));
                let scene_tx = engine_handle.tx.clone();
                tokio::spawn(async move {
                    while let Some(news) = news_rx.recv().await {
                        if scene_tx.send(Input::Scene(news)).await.is_err() {
                            return;
                        }
                    }
                });
                let mut engine = Engine::new(engine_handle, host, store, ctrl, want_obs);
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
    /// Messages from Shared Chat partners count too (`twitch_shared_chat`).
    shared_chat: bool,
    last_run: HashMap<String, Instant>,
    viewer_last: HashMap<String, Instant>,
    timers: HashMap<String, Instant>,
    /// Per integration: (started, doing something). See the module comment.
    limits: HashMap<String, (Arc<Semaphore>, Arc<Semaphore>)>,
    global: (Arc<Semaphore>, Arc<Semaphore>),
    delay_seen: Option<u32>,
    /// The delay's phase and armed delay at the last tick: see `watch_phase`.
    phase_seen: Option<(&'static str, u32)>,
    flap: FlapGuard,
    discord_messages: DiscordMessages,
    discord_turns: Arc<runner::DiscordTurns>,
    /// `{uses}` changed since the data file was last written.
    uses_dirty: bool,
    uses_saved: Instant,
    session: Session,
    timeline: Arc<Mutex<Timeline>>,
    /// The on-air state at the last tick.
    onair_seen: Option<&'static str>,
    /// The on-air triggers that heard the state last (`id#idx`): one that
    /// is new hears it at once, so a light is never out of date.
    onair_told: Vec<String>,
    /// Per trigger and destination (`id#idx#name`): the drop a "stays
    /// down" already fired for, and the drops a "steady again" counted.
    still_down_fired: HashMap<String, usize>,
    steady_fired: HashMap<String, usize>,
    /// Each destination's drop count when "keeps dropping" last looked.
    drops_seen: HashMap<String, usize>,
    /// When each chat activity trigger last fired. Once fired, it waits for
    /// chat to calm down (its rules no longer met) before it can again.
    activity_fired: HashMap<String, Instant>,
    activity_calm: HashMap<String, bool>,
    /// When chat was last judged for activity triggers: a raid's hundreds
    /// of messages a second are judged a few times a second, not each.
    activity_read: Option<Instant>,
    scene_now: String,
    /// The scene on air when the OBS connection was lost: the same scene
    /// once it's back is no switch.
    scene_lost: String,
    /// Scene switches waiting out a trigger's settle time, per trigger.
    scene_pending: HashMap<String, PendingScene>,
    want_obs: watch::Sender<bool>,
}

/// The last Discord message each integration posted, by key (see
/// `RunEnv::discord_messages`).
type DiscordMessages = Arc<Mutex<HashMap<String, String>>>;

/// A switch to a scene a trigger wants, not yet settled.
struct PendingScene {
    since: Instant,
    scene: String,
    previous: String,
}

impl Engine {
    fn new(
        handle: Arc<Handle>,
        host: Arc<RealHost>,
        store: Arc<Store>,
        ctrl: Arc<Controller>,
        want_obs: watch::Sender<bool>,
    ) -> Engine {
        let discord_messages = Arc::new(Mutex::new(HashMap::new()));
        let discord_turns = Arc::new(runner::DiscordTurns::default());
        let timeline = Arc::new(Mutex::new(Timeline::default()));
        let env = Arc::new(make_env(
            Vec::new(),
            PhoneConnection::default(),
            &store,
            (&discord_messages, &discord_turns),
            (&timeline, &handle),
        ));
        Engine {
            handle,
            host,
            store,
            ctrl,
            integrations: Vec::new(),
            env,
            shared_chat: false,
            last_run: HashMap::new(),
            viewer_last: HashMap::new(),
            timers: HashMap::new(),
            limits: HashMap::new(),
            global: (
                Arc::new(Semaphore::new(GLOBAL_STARTED)),
                Arc::new(Semaphore::new(GLOBAL_RUNS)),
            ),
            delay_seen: None,
            phase_seen: None,
            flap: FlapGuard::default(),
            discord_messages,
            discord_turns,
            uses_dirty: false,
            uses_saved: Instant::now(),
            session: Session::default(),
            timeline,
            onair_seen: None,
            onair_told: Vec::new(),
            still_down_fired: HashMap::new(),
            steady_fired: HashMap::new(),
            drops_seen: HashMap::new(),
            activity_fired: HashMap::new(),
            activity_calm: HashMap::new(),
            activity_read: None,
            scene_now: String::new(),
            scene_lost: String::new(),
            scene_pending: HashMap::new(),
            want_obs,
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
        let mut last_prune = Instant::now();
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
                    // Already followed when they happened (`event`
                    // observes before the guard): only the integrations
                    // hear them late.
                    for event in self.flap.release(Instant::now()) {
                        self.dispatch(&event);
                    }
                    self.watch_phase();
                    self.watch_delay();
                    self.watch_onair();
                    self.watch_session();
                    self.watch_destinations();
                    self.update_activity_gate();
                    self.settle_scenes();
                    self.update_obs_want();
                    self.run_timers();
                    self.save_uses_if_due();
                    if last_prune.elapsed() >= PRUNE_EVERY {
                        last_prune = Instant::now();
                        self.prune();
                    }
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
            (&self.discord_messages, &self.discord_turns),
            (&self.timeline, &self.handle),
        ));
        self.host
            .default_delay_ms
            .store(s.auto_arm_delay_ms.max(1000), Ordering::Relaxed);
        self.host.twitch.use_client_id(&s.twitch_client_id);
        self.shared_chat = s.twitch_shared_chat;
        // Forget per-trigger state for integrations that are gone.
        let alive: Vec<&str> = self.integrations.iter().map(|i| i.id.as_str()).collect();
        let keep = |key: &String| alive.iter().any(|id| key.starts_with(&format!("{id}#")));
        self.last_run.retain(|k, _| keep(k));
        self.viewer_last.retain(|k, _| keep(k));
        self.timers.retain(|k, _| keep(k));
        self.still_down_fired.retain(|k, _| keep(k));
        self.steady_fired.retain(|k, _| keep(k));
        self.activity_fired.retain(|k, _| keep(k));
        self.activity_calm.retain(|k, _| keep(k));
        self.scene_pending.retain(|k, _| keep(k));
        self.limits.retain(|id, _| alive.contains(&id.as_str()));
        self.handle.retain_stats(&alive);
    }

    /// Drop bookkeeping that can no longer affect anything: viewer and
    /// trigger cooldowns that have run out, and old flap records.
    fn prune(&mut self) {
        let (mut longest_viewer, mut longest_run) = (Duration::ZERO, Duration::ZERO);
        for i in &self.integrations {
            longest_run = longest_run.max(Duration::from_millis(i.cooldown_ms));
            for h in &i.handlers {
                if let Trigger::ChatCommand {
                    user_cooldown_ms, ..
                } = h.trigger
                {
                    longest_viewer = longest_viewer.max(Duration::from_millis(user_cooldown_ms));
                }
            }
        }
        self.viewer_last.retain(|_, t| t.elapsed() < longest_viewer);
        self.last_run.retain(|_, t| t.elapsed() < longest_run);
        self.flap.prune(Instant::now());
        // Here rather than on every settings change: a delete undone within
        // the minute keeps its count and its Discord message.
        let alive: Vec<&str> = self.integrations.iter().map(|i| i.id.as_str()).collect();
        {
            let mut uses = self.store.uses.lock();
            let before = uses.len();
            uses.retain(|id, _| alive.contains(&id.as_str()));
            self.uses_dirty |= uses.len() != before;
        }
        // Keys are `[test:]integration:channel`.
        self.discord_messages.lock().retain(|key, _| {
            let id = key
                .trim_start_matches("test:")
                .split(':')
                .next()
                .unwrap_or("");
            alive.contains(&id)
        });
    }

    fn save_uses_if_due(&mut self) {
        self.uses_dirty |= self.store.counters_dirty.swap(false, Ordering::Relaxed);
        if self.uses_dirty && self.uses_saved.elapsed() >= USES_SAVE_EVERY {
            self.uses_saved = Instant::now();
            // A failed write stays due, for the next round.
            self.uses_dirty = self.store.save().is_err();
        }
    }

    fn input(&mut self, input: Input) {
        match input {
            Input::Event(event) => self.event(event),
            Input::Hook { token, body, query } => self.hook(&token, body, query),
            Input::Test {
                integration,
                handler,
                run,
                reply,
            } => self.test(*integration, handler, run, reply),
            Input::Shortcut(from, signature) => self.shortcut(from, &signature),
            Input::Scene(news) => self.scene(news),
            Input::Login(which) => self.handle.twitch.begin_login(which),
            Input::CancelLogin => self.handle.twitch.cancel_login(),
            Input::Logout(which) => self.handle.twitch.logout(which),
        }
    }

    fn event(&mut self, event: Event) {
        // The session sees every event, flapping or not: smart destination
        // alerts are built on the drops the flap guard holds back.
        let made = self.observe(&event);
        if !self.flap.suppress(&event, Instant::now()) {
            self.dispatch(&event);
        }
        if event.kind == EventKind::DestinationDropped {
            self.check_unstable(&event);
        }
        for next in made {
            self.event(next);
        }
    }

    /// Run every integration waiting for `event`.
    fn dispatch(&mut self, event: &Event) {
        let matches: Vec<(Arc<Integration>, usize)> = self
            .enabled_handlers()
            .filter(|(_, _, h)| match &h.trigger {
                Trigger::Event { kind, filters } => {
                    *kind == event.kind && filters_match(filters, event)
                }
                _ => false,
            })
            .map(|(i, idx, _)| (i, idx))
            .collect();
        for (integration, idx) in matches {
            self.start_run(
                integration,
                idx,
                event_vars(event),
                None,
                event.kind.label().to_string(),
            );
        }
    }

    /// Follow the stream's life. Returns the events it makes, after
    /// setting up for them: a new stream gets a fresh timeline.
    fn observe(&mut self, event: &Event) -> Vec<Event> {
        let ending = matches!(
            event.kind,
            EventKind::ObsDisconnected | EventKind::HoldExpired | EventKind::HoldEnded
        );
        let timeline = if ending {
            self.timeline_summary()
        } else {
            (0, String::new())
        };
        let made = self
            .session
            .observe(event, Instant::now(), unix_ms(), timeline);
        self.follow_session(&made);
        made
    }

    /// What the end-of-stream report says about the timeline.
    fn timeline_summary(&self) -> (usize, String) {
        let t = self.timeline.lock();
        (t.count(LineKind::Highlight), t.chapters())
    }

    /// Set up for the stream events the session just made: a new stream
    /// gets a fresh timeline, fresh Discord messages, and its scene looked
    /// at.
    fn follow_session(&mut self, made: &[Event]) {
        for e in made {
            match e.kind {
                EventKind::StreamStarted => {
                    self.timeline.lock().clear();
                    self.still_down_fired.clear();
                    self.steady_fired.clear();
                    self.drops_seen.clear();
                    forget_subject_messages(&mut self.discord_messages.lock());
                    self.host
                        .stream_started_ms
                        .store(self.session.started_ms(), Ordering::Relaxed);
                    let scene = self.scene_now.clone();
                    if !scene.is_empty() {
                        self.scene_entered(&scene, "", true);
                    }
                }
                EventKind::StreamEnded => self.host.stream_started_ms.store(0, Ordering::Relaxed),
                _ => {}
            }
        }
    }

    /// Each tick: a stream whose OBS dropped unprotected and stayed away
    /// ends (see `session::END_GRACE`).
    fn watch_session(&mut self) {
        if !self.session.is_ending() {
            return;
        }
        let timeline = self.timeline_summary();
        let made = self.session.expire(Instant::now(), timeline);
        self.follow_session(&made);
        for event in made {
            self.event(event);
        }
    }

    /// A destination dropped: tell the "keeps dropping" triggers whose
    /// threshold it reached, each with its own count and window.
    fn check_unstable(&mut self, dropped: &Event) {
        let name = dropped.var("destination").unwrap_or("");
        let Some(health) = self.session.destinations().get(name).cloned() else {
            return;
        };
        // A retry failing while it is still down isn't a new drop.
        if self.drops_seen.insert(name.to_string(), health.drop_count) == Some(health.drop_count) {
            return;
        }
        let now = Instant::now();
        let kind = EventKind::DestinationUnstable;
        let due: Vec<(Arc<Integration>, usize, Event)> = self
            .enabled_handlers()
            .filter_map(|(i, idx, h)| {
                let Trigger::Event { kind: k, filters } = &h.trigger else {
                    return None;
                };
                if *k != kind {
                    return None;
                }
                let within_min = kind.param(filters, "within_min");
                let window = Duration::from_secs_f64(within_min * 60.0);
                let drops = health.drops_within(window, now);
                let event = Event::new(kind)
                    .with("destination", name)
                    .with("platform", health.platform.clone())
                    .with("reason", health.reason.clone())
                    .with("drops", drops.to_string())
                    .with("within", format!("{} min", within_min.round()));
                let reached = drops as f64 >= kind.param(filters, "drops");
                (reached && filters_match(filters, &event)).then_some((i, idx, event))
            })
            .collect();
        for (integration, idx, event) in due {
            self.start_run(
                integration,
                idx,
                event_vars(&event),
                None,
                kind.label().to_string(),
            );
        }
    }

    /// Each tick: destinations that stayed down long enough, or have been
    /// steady long enough after dropping, for the triggers that wait for it.
    fn watch_destinations(&mut self) {
        if !self.session.is_on() {
            return;
        }
        let now = Instant::now();
        let mut due: Vec<(Arc<Integration>, usize, Event, String, usize)> = Vec::new();
        for (i, idx, h) in self.enabled_handlers() {
            let Trigger::Event { kind, filters } = &h.trigger else {
                continue;
            };
            for (name, health) in self.session.destinations() {
                let key = format!("{}#{idx}#{name}", i.id);
                let drops = health.drop_count;
                let event = match kind {
                    EventKind::DestinationStillDown => {
                        let Some(since) = health.down_since else {
                            continue;
                        };
                        let down = now.duration_since(since);
                        let after = Duration::from_secs_f64(kind.param(filters, "after_s"));
                        if down < after || self.still_down_fired.get(&key) == Some(&drops) {
                            continue;
                        }
                        Event::new(*kind)
                            .with("reason", health.reason.clone())
                            .with(
                                "down_for",
                                super::host::fmt_duration(down.as_millis() as u64),
                            )
                    }
                    EventKind::DestinationSteady => {
                        let Some(since) = health.live_since else {
                            continue;
                        };
                        let after = Duration::from_secs_f64(kind.param(filters, "for_min") * 60.0);
                        let counted = self.steady_fired.get(&key).copied().unwrap_or(0);
                        if drops == 0 || drops <= counted || now.duration_since(since) < after {
                            continue;
                        }
                        Event::new(*kind).with(
                            "down_total",
                            super::host::fmt_duration(health.down_total.as_millis() as u64),
                        )
                    }
                    _ => continue,
                };
                let event = event
                    .with("destination", name.clone())
                    .with("platform", health.platform.clone())
                    .with("drops", drops.to_string());
                // Marked whether or not a filter lets it through: either way
                // this outage has been looked at.
                due.push((i.clone(), idx, event, key, drops));
            }
        }
        for (integration, idx, event, key, drops) in due {
            let map = if event.kind == EventKind::DestinationStillDown {
                &mut self.still_down_fired
            } else {
                &mut self.steady_fired
            };
            map.insert(key, drops);
            let Trigger::Event { filters, .. } = &integration.handlers[idx].trigger else {
                continue;
            };
            if filters_match(filters, &event) {
                let label = event.kind.label().to_string();
                self.start_run(integration.clone(), idx, event_vars(&event), None, label);
            }
        }
    }

    /// Each tick: the on-air state, and an event when it changes. On-air
    /// triggers that haven't heard it yet (the app just started, or one was
    /// just switched on) hear it at once, with `previous` the same as
    /// `state`: a light by the door is never out of date.
    fn watch_onair(&mut self) {
        let stopped_on_purpose = self
            .ctrl
            .hold_status()
            .is_some_and(|h| h.reason == crate::crash_hold::HoldReason::Stopped);
        let now = if self.ctrl.hold_active() && !stopped_on_purpose {
            "crash"
        } else if self.ctrl.hold_active() && self.ctrl.target_delay_ms() > 0 {
            // Stopped on purpose, held by crash protection: still on air.
            "delay"
        } else if self.ctrl.hold_active() {
            "live"
        } else if self.session.is_ending() {
            // OBS dropped without crash protection and may be back any
            // second: not "off", nobody should walk in.
            "crash"
        } else if self.ctrl.ingest_alive() && self.ctrl.target_delay_ms() > 0 {
            "delay"
        } else if self.ctrl.ingest_alive() {
            "live"
        } else {
            "off"
        };
        *self.host.onair.lock() = now;
        let before = self.onair_seen.replace(now);
        if before.is_some_and(|b| b != now) {
            self.onair_told = self.onair_keys();
            self.event(
                Event::new(EventKind::OnAirChanged)
                    .with("state", now)
                    .with("previous", before.unwrap_or(now)),
            );
            return;
        }
        let keys = self.onair_keys();
        let newcomers: Vec<(Arc<Integration>, usize)> = self
            .enabled_handlers()
            .filter(|(i, idx, h)| {
                matches!(
                    &h.trigger,
                    Trigger::Event {
                        kind: EventKind::OnAirChanged,
                        ..
                    }
                ) && !self.onair_told.contains(&format!("{}#{idx}", i.id))
            })
            .map(|(i, idx, _)| (i, idx))
            .collect();
        // Gone ones are forgotten; a new one is told once a run started.
        self.onair_told.retain(|k| keys.contains(k));
        let event = Event::new(EventKind::OnAirChanged)
            .with("state", now)
            .with("previous", now);
        for (integration, idx) in newcomers {
            let Trigger::Event { filters, .. } = &integration.handlers[idx].trigger else {
                continue;
            };
            let key = format!("{}#{idx}", integration.id);
            let label = event.kind.label().to_string();
            let told = !filters_match(filters, &event)
                || self.start_run(integration.clone(), idx, event_vars(&event), None, label);
            if told {
                self.onair_told.push(key);
            }
        }
    }

    /// The switched-on on-air triggers, as `id#idx`.
    fn onair_keys(&self) -> Vec<String> {
        self.enabled_handlers()
            .filter(|(_, _, h)| {
                matches!(
                    &h.trigger,
                    Trigger::Event {
                        kind: EventKind::OnAirChanged,
                        ..
                    }
                )
            })
            .map(|(i, idx, _)| format!("{}#{idx}", i.id))
            .collect()
    }

    fn chat(&mut self, msg: ChatMessage) {
        // A Shared Chat partner's viewers only count when the streamer said
        // so: otherwise anyone there could use this channel's commands.
        if msg.from_shared_chat && !self.shared_chat {
            return;
        }
        // Our own bot talking must never trigger us. (Unless the "bot" is
        // the streamer's own account: then these are their own commands.)
        let is_own_bot = {
            let accounts = self.store.accounts.lock();
            let main = accounts
                .main
                .as_ref()
                .map(|a| a.login.as_str())
                .unwrap_or("");
            accounts.bot.as_ref().is_some_and(|b| {
                !b.login.eq_ignore_ascii_case(main) && b.login.eq_ignore_ascii_case(&msg.user_login)
            })
        };
        if is_own_bot {
            return;
        }
        self.handle.chat.lock().push(&msg, Instant::now());
        self.check_activity(&msg);
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
            let key = format!("{}#{idx}#{}", integration.id, msg.user_login);
            let vars = chat_vars(&msg, &args);
            let trigger = if label == "chat" {
                "Chat message".to_string()
            } else {
                format!("{label} in chat")
            };
            // A command that didn't run (quiet hours, busy) doesn't make the
            // viewer wait before trying again.
            if self.start_run(integration, idx, vars, Some(msg.id.clone()), trigger) {
                self.viewer_last.insert(key, Instant::now());
            }
        }
    }

    /// Why chat activity triggers that only run while streaming can't fire
    /// now, or empty; "hold" holds back every one. For the meter.
    fn activity_gate(&self, now: Instant) -> &'static str {
        if self.ctrl.hold_active() {
            "hold"
        } else if !self.ctrl.ingest_alive() || !self.session.is_on() {
            "offline"
        } else if self.session.uptime(now) < ACTIVITY_WARMUP {
            "warmup"
        } else {
            ""
        }
    }

    fn update_activity_gate(&mut self) {
        *self.handle.activity_held.lock() = self.activity_gate(Instant::now());
    }

    /// After each chat message: chat activity triggers whose rules chat now
    /// meets. Each fires once per burst: then it waits for chat to calm
    /// down (and at least one window) before it can fire again. Never
    /// during the reconnect screen, and (when it only runs live) not in the
    /// first seconds of a stream, when everyone says hi at once.
    fn check_activity(&mut self, latest: &ChatMessage) {
        let now = Instant::now();
        if self
            .activity_read
            .is_some_and(|at| now.duration_since(at) < ACTIVITY_READ_EVERY)
        {
            return;
        }
        self.activity_read = Some(now);
        let gate = self.activity_gate(now);
        if gate == "hold" {
            return;
        }
        let streaming = gate.is_empty();
        let candidates: Vec<(Arc<Integration>, usize, ChatActivity)> = self
            .enabled_handlers()
            .filter_map(|(i, idx, h)| match &h.trigger {
                Trigger::ChatActivity(a) if streaming || !a.only_live => Some((i, idx, a.clone())),
                _ => None,
            })
            .collect();
        for (integration, idx, rules) in candidates {
            let key = format!("{}#{idx}", integration.id);
            let window = Duration::from_millis(rules.window_ms);
            let reading = self.handle.chat.lock().read(&rules, now);
            if !reading.fires {
                self.activity_calm.insert(key, true);
                continue;
            }
            let spaced = self
                .activity_fired
                .get(&key)
                .is_none_or(|t| now.duration_since(*t) >= window);
            let calmed = self.activity_calm.get(&key).copied().unwrap_or(true);
            if !spaced || !calmed {
                continue;
            }
            let vars = BTreeMap::from([
                ("messages".to_string(), reading.messages.to_string()),
                ("normal".to_string(), format!("{:.1}", reading.normal)),
                ("busier".to_string(), format!("{:.1}", reading.busier)),
                ("chatters".to_string(), reading.chatters.to_string()),
                (
                    "word_share".to_string(),
                    format!("{:.0}", reading.word_share),
                ),
                ("top_word".to_string(), reading.top_word.clone()),
                ("user".to_string(), latest.display_name.clone()),
                ("message".to_string(), latest.text.clone()),
            ]);
            if self.start_run(integration, idx, vars, None, "Chat got busy".to_string()) {
                self.activity_fired.insert(key.clone(), now);
                self.activity_calm.insert(key, false);
            }
        }
    }

    /// News from OBS about its scenes. A switch to a scene a trigger wants
    /// starts that trigger's settle time; it fires once the scene stayed.
    /// The scene on air when OBS connects counts as a switch to it when a
    /// stream is already on (the app or OBS restarted mid-stream).
    fn scene(&mut self, news: SceneNews) {
        let (name, switched) = match news {
            SceneNews::Current(name) => (name, false),
            SceneNews::Switched(name) => (name, true),
        };
        let previous = std::mem::replace(&mut self.scene_now, name.clone());
        if previous == name {
            return;
        }
        if name.is_empty() {
            self.scene_lost = previous;
            return;
        }
        let lost = std::mem::take(&mut self.scene_lost);
        if switched {
            self.scene_entered(&name, &previous, false);
        } else if previous.is_empty() && self.session.is_on() && name != lost {
            self.scene_entered(&name, "", true);
        }
    }

    /// `name` went on air after `previous` (empty: not known). `starting`:
    /// a stream started on it, rather than OBS switching to it; a trigger
    /// that only acts when coming from a certain scene then doesn't.
    fn scene_entered(&mut self, name: &str, previous: &str, starting: bool) {
        let now = Instant::now();
        let streaming = self.ctrl.ingest_alive() || starting;
        let keys: Vec<(String, bool)> = self
            .enabled_handlers()
            .filter_map(|(i, idx, h)| match &h.trigger {
                Trigger::Scene {
                    scene,
                    from,
                    except,
                    only_live,
                    ..
                } => {
                    let from_ok = if from.trim().is_empty() {
                        true
                    } else {
                        !starting && matches_any(previous, from)
                    };
                    let excepted = !except.trim().is_empty() && matches_any(name, except);
                    let wanted = matches_any(name, scene)
                        && !excepted
                        && from_ok
                        && (streaming || !only_live);
                    Some((format!("{}#{idx}", i.id), wanted))
                }
                _ => None,
            })
            .collect();
        for (key, wanted) in keys {
            if wanted {
                let pending = PendingScene {
                    since: now,
                    scene: name.to_string(),
                    previous: previous.to_string(),
                };
                self.scene_pending.insert(key, pending);
            } else {
                // Moved on before it settled: that switch never happened.
                self.scene_pending.remove(&key);
            }
        }
    }

    /// Each tick: scene switches that have settled. The reconnect screen
    /// cancels them: a crash isn't a scene change.
    fn settle_scenes(&mut self) {
        if self.scene_pending.is_empty() {
            return;
        }
        if self.ctrl.hold_active() {
            self.scene_pending.clear();
            return;
        }
        let due: Vec<(Arc<Integration>, usize, String)> = self
            .enabled_handlers()
            .filter_map(|(i, idx, h)| {
                let Trigger::Scene { settle_ms, .. } = &h.trigger else {
                    return None;
                };
                let key = format!("{}#{idx}", i.id);
                let pending = self.scene_pending.get(&key)?;
                let settled = pending.since.elapsed() >= Duration::from_millis(*settle_ms);
                (settled && pending.scene == self.scene_now).then_some((i, idx, key))
            })
            .collect();
        for (integration, idx, key) in due {
            let Some(pending) = self.scene_pending.remove(&key) else {
                continue;
            };
            let vars = BTreeMap::from([
                ("scene".to_string(), pending.scene.clone()),
                ("previous_scene".to_string(), pending.previous),
            ]);
            self.start_run(
                integration,
                idx,
                vars,
                None,
                format!("Scene {}", pending.scene),
            );
        }
        // Whatever is left over for a scene no longer on air is stale.
        let now_scene = self.scene_now.clone();
        self.scene_pending.retain(|_, p| p.scene == now_scene);
    }

    /// Keep OBS connected while a scene trigger or an OBS step is on, or
    /// the dashboard is picking a scene.
    fn update_obs_want(&mut self) {
        let uses_obs = |h: &super::model::Handler| {
            let mut found = matches!(h.trigger, Trigger::Scene { .. });
            super::model::visit_steps(&h.steps, &mut |s| {
                found |= s.enabled && s.kind == super::model::StepKind::Obs
            });
            found
        };
        let wanted = self.enabled_handlers().any(|(_, _, h)| uses_obs(h))
            || unix_ms() < self.handle.obs_wanted_until.load(Ordering::Relaxed);
        if *self.want_obs.borrow() != wanted {
            self.want_obs.send_replace(wanted);
        }
    }

    fn shortcut(&mut self, from: Shortcut, signature: &str) {
        let live = self.ctrl.ingest_alive();
        let matches: Vec<(Arc<Integration>, usize)> = self
            .enabled_handlers()
            .filter(|(_, _, h)| match (&h.trigger, from) {
                (
                    Trigger::Shortcut {
                        only_live: true, ..
                    },
                    _,
                ) if !live => false,
                (Trigger::Shortcut { hotkey, .. }, Shortcut::Hotkey) => {
                    !hotkey.is_empty() && hotkey == signature
                }
                (Trigger::Shortcut { midi, .. }, Shortcut::Midi) => pad_matches(midi, signature),
                _ => false,
            })
            .map(|(i, idx, _)| (i, idx))
            .collect();
        let (label, source) = match from {
            Shortcut::Hotkey => (format!("Hotkey {signature}"), "hotkey"),
            Shortcut::Midi => ("MIDI pad".to_string(), "midi"),
        };
        for (integration, idx) in matches {
            let vars = BTreeMap::from([("source".to_string(), source.to_string())]);
            self.start_run(integration, idx, vars, None, label.clone());
        }
    }

    /// A call to a secret address: a web call trigger, or a button's
    /// Stream Deck address.
    fn hook(&mut self, token: &str, body: String, query: String) {
        let same = |t: &str| {
            !t.is_empty() && crate::crypto::constant_time_eq(t.as_bytes(), token.as_bytes())
        };
        let live = self.ctrl.ingest_alive();
        let matches: Vec<(Arc<Integration>, usize, &'static str)> = self
            .enabled_handlers()
            .filter_map(|(i, idx, h)| match &h.trigger {
                Trigger::Webhook { token: t } if same(t) => Some((i, idx, "Web call")),
                Trigger::Shortcut {
                    token: t,
                    only_live,
                    ..
                } if same(t) && (live || !only_live) => Some((i, idx, "Stream Deck or app")),
                _ => None,
            })
            .collect();
        for (integration, idx, label) in matches {
            let source = if label == "Web call" {
                "web"
            } else {
                "stream_deck"
            };
            let vars = BTreeMap::from([
                ("body".to_string(), body.clone()),
                ("query".to_string(), query.clone()),
                ("source".to_string(), source.to_string()),
            ]);
            self.start_run(integration, idx, vars, None, label.to_string());
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
        // A timer switched off forgets its last run, so switching it back on
        // waits a full period instead of firing at once.
        let running: Vec<String> = timers
            .iter()
            .map(|(i, idx, ..)| format!("{}#{idx}", i.id))
            .collect();
        self.timers.retain(|key, _| running.contains(key));
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

    /// Turn arming a delay into events: armed, ready, cancelled. Going on
    /// air and off it are `watch_delay`'s.
    fn watch_phase(&mut self) {
        let now = (self.ctrl.phase(), self.ctrl.armed_delay_ms());
        let Some(before) = self.phase_seen.replace(now) else {
            return;
        };
        for event in phase_events(before, now) {
            self.event(event);
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
        let mut event = Event::new(kind).with("previous", previous);
        // The delay just set: the applied one only catches up a moment later
        // (after the buffer jump), so `{delay}` would still read the old one.
        if now > 0 {
            event = event.with("delay", fmt(now));
        }
        self.event(event);
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

    /// Start a run unless quiet hours, the cooldown or the run limits say
    /// no. True when it started: only then does it count toward cooldowns.
    fn start_run(
        &mut self,
        integration: Arc<Integration>,
        idx: usize,
        mut vars: BTreeMap<String, String>,
        reply_to: Option<String>,
        trigger: String,
    ) -> bool {
        if let Some(quiet) = integration.quiet {
            let now = super::clock::now();
            if quiet.contains((now.hour * 60 + now.minute) as u16) {
                return false;
            }
        }
        let key = format!("{}#{idx}", integration.id);
        if integration.cooldown_ms > 0
            && self
                .last_run
                .get(&key)
                .is_some_and(|t| t.elapsed() < Duration::from_millis(integration.cooldown_ms))
        {
            // A button pressed too soon says so: the press wasn't lost.
            // Chat, much busier, doesn't fill the log with them.
            if matches!(
                integration.handlers[idx].trigger,
                Trigger::Shortcut { .. } | Trigger::ChatActivity(_)
            ) {
                self.turned_away(&integration, trigger, "cooldown");
            }
            return false;
        }
        let (started_limit, active_limit) = self
            .limits
            .entry(integration.id.clone())
            .or_insert_with(|| {
                (
                    Arc::new(Semaphore::new(PER_INTEGRATION_RUNS)),
                    Arc::new(Semaphore::new(PER_INTEGRATION)),
                )
            })
            .clone();
        let places = vec![active_limit.clone(), self.global.1.clone()];
        let permits = (
            started_limit.try_acquire_owned(),
            self.global.0.clone().try_acquire_owned(),
            active_limit.try_acquire_owned(),
            self.global.1.clone().try_acquire_owned(),
        );
        let (Ok(started), Ok(started_all), Ok(active), Ok(active_all)) = permits else {
            self.turned_away(&integration, trigger, "busy");
            return false;
        };
        self.last_run.insert(key, Instant::now());
        let uses = {
            let mut uses = self.store.uses.lock();
            let count = uses.entry(integration.id.clone()).or_insert(0);
            *count += 1;
            *count
        };
        self.uses_dirty = true;
        vars.insert("uses".to_string(), uses.to_string());
        let ctx = RunContext {
            integration_id: integration.id.clone(),
            vars,
            reply_to,
            test: false,
            dry: false,
            gate: Some(runner::Gate::new(places, vec![active, active_all])),
        };
        let handle = self.handle.clone();
        let ctrl = self.ctrl.clone();
        let host: Arc<dyn super::host::Host> = self.host.clone();
        let env = self.env.clone();
        tokio::spawn(async move {
            let _started = (started, started_all);
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
        true
    }

    /// A run that didn't start, in the activity log: `busy` (too many at
    /// once) or `cooldown` (too soon after the last).
    fn turned_away(&self, integration: &Integration, trigger: String, status: &'static str) {
        self.handle.push(Record {
            at_ms: unix_ms(),
            integration_id: integration.id.clone(),
            name: integration.name.clone(),
            trigger,
            status,
            test: false,
            steps: Vec::new(),
        });
    }

    fn test(
        &mut self,
        integration: Integration,
        idx: usize,
        run: TestRun,
        reply: oneshot::Sender<Record>,
    ) {
        let Some(handler) = integration.handlers.get(idx).cloned() else {
            return;
        };
        let (mut vars, trigger) = sample_vars(&handler.trigger);
        // The values the dashboard was given to test with.
        for (name, value) in run.values.into_iter().take(MAX_TEST_VALUES) {
            if !name.is_empty() && name.len() <= 64 {
                vars.insert(name, value.chars().take(1000).collect());
            }
        }
        // What it would read on its next real run.
        let next = self
            .store
            .uses
            .lock()
            .get(&integration.id)
            .copied()
            .unwrap_or(0)
            + 1;
        vars.insert("uses".to_string(), next.to_string());
        let ctx = RunContext {
            integration_id: integration.id.clone(),
            vars,
            reply_to: None,
            test: true,
            dry: run.dry,
            gate: None,
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

/// The events a change of the delay's phase (`Controller::phase`) makes,
/// from `(phase, armed ms)` before to after. "Ready" is the armed delay
/// becoming ready to go on, never the delay coming back to ready after a
/// cut; a delay switched on as soon as it was ready was ready too.
fn phase_events(before: (&str, u32), now: (&str, u32)) -> Vec<Event> {
    let fmt = super::host::fmt_delay;
    let ((was, was_armed), (is, armed)) = (before, now);
    let mut events = Vec::new();
    if was == is {
        return events;
    }
    if was == "idle" {
        events.push(Event::new(EventKind::DelayArmed).with("delay", fmt(armed)));
    }
    let became_ready = is == "ready" || (was == "preparing" && is == "active");
    if became_ready && matches!(was, "idle" | "preparing") {
        events.push(Event::new(EventKind::DelayReady).with("delay", fmt(armed)));
    }
    if is == "idle" && matches!(was, "preparing" | "ready") {
        events.push(Event::new(EventKind::DelayDisarmed).with("previous", fmt(was_armed)));
    }
    events
}

/// See `FLAP_WINDOW`. What it holds back isn't lost: a destination whose
/// last held event differs from what was last said about it gets that event
/// once the window ends, so "live" is never the final word on a dead one.
#[derive(Default)]
struct FlapGuard {
    /// When each event (kind and destination) last went out.
    last: HashMap<String, Instant>,
    /// What each destination was last reported as: live or dropped.
    reported: HashMap<String, EventKind>,
    /// Each destination's newest held-back event.
    held: HashMap<String, Event>,
}

impl FlapGuard {
    fn prune(&mut self, now: Instant) {
        self.last
            .retain(|_, t| now.duration_since(*t) < FLAP_WINDOW);
    }

    /// The window key of an event the guard watches, and its destination.
    fn key(event: &Event) -> Option<(String, Option<String>)> {
        match event.kind {
            EventKind::DestinationLive | EventKind::DestinationDropped => {
                let destination = event.var("destination").unwrap_or("").to_string();
                Some((
                    format!("{}:{destination}", event.kind.id()),
                    Some(destination),
                ))
            }
            EventKind::AllDestinationsDown => Some((event.kind.id().to_string(), None)),
            _ => None,
        }
    }

    fn in_window(&self, key: &str, now: Instant) -> bool {
        self.last
            .get(key)
            .is_some_and(|t| now.duration_since(*t) < FLAP_WINDOW)
    }

    fn suppress(&mut self, event: &Event, now: Instant) -> bool {
        let Some((key, destination)) = Self::key(event) else {
            return false;
        };
        if self.in_window(&key, now) {
            if let Some(destination) = destination {
                self.held.insert(destination, event.clone());
            }
            return true;
        }
        self.last.insert(key, now);
        if let Some(destination) = destination {
            self.reported.insert(destination.clone(), event.kind);
            self.held.remove(&destination);
        }
        false
    }

    /// Held events whose window is over and that still change what was
    /// last reported: the destination's real state, said late, not never.
    fn release(&mut self, now: Instant) -> Vec<Event> {
        let due: Vec<String> = self
            .held
            .iter()
            .filter(|(_, event)| {
                Self::key(event).is_some_and(|(key, _)| !self.in_window(&key, now))
            })
            .map(|(destination, _)| destination.clone())
            .collect();
        due.into_iter()
            .filter_map(|destination| {
                let event = self.held.remove(&destination)?;
                (self.reported.get(&destination) != Some(&event.kind)).then_some(event)
            })
            .collect()
    }
}

fn status_id(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Ok => "ok",
        RunStatus::Stopped => "stopped",
        RunStatus::Failed => "failed",
    }
}

/// Forget which Discord message each "one message per subject" is about
/// (keys `[test:]integration:channel:subject`): a "down" card still open
/// when the last stream ended must not be edited from the next one, far up
/// the channel. Messages kept per channel (a delay status, say) stay.
fn forget_subject_messages(messages: &mut HashMap<String, String>) {
    messages.retain(|key, _| key.trim_start_matches("test:").matches(':').count() < 2);
}

fn make_env(
    discord: Vec<DiscordChannel>,
    phone: PhoneConnection,
    store: &Arc<Store>,
    (discord_messages, discord_turns): (&DiscordMessages, &Arc<runner::DiscordTurns>),
    (timeline, handle): (&Arc<Mutex<Timeline>>, &Arc<Handle>),
) -> RunEnv {
    let saver = store.clone();
    // Weak: the env lives inside the engine the handle owns a channel to.
    let events = Arc::downgrade(handle);
    RunEnv {
        discord,
        phone,
        counters: store.counters.clone(),
        // Written with the run counts a moment later (`save_uses_if_due`):
        // a chat command counting every message must not rewrite the file
        // each time, on the engine's own thread.
        counters_changed: Box::new(move || {
            saver.counters_dirty.store(true, Ordering::Relaxed);
        }),
        discord_messages: discord_messages.clone(),
        discord_turns: discord_turns.clone(),
        timeline: timeline.clone(),
        emit: Box::new(move |event| {
            if let Some(handle) = events.upgrade() {
                handle.emit(event);
            }
        }),
    }
}

fn event_vars(event: &Event) -> BTreeMap<String, String> {
    event
        .vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// Whether an event passes a trigger's filters. A trigger's settings
/// (thresholds like `after_s`) live among them but aren't filters. A
/// filter can list several values (`YouTube, Kick`) and use wildcards
/// (`*timed out*`); case doesn't matter.
fn filters_match(filters: &BTreeMap<String, String>, event: &Event) -> bool {
    filters.iter().all(|(name, want)| {
        want.trim().is_empty()
            || event.kind.is_param(name)
            || event.var(name).is_some_and(|have| matches_any(have, want))
    })
}

/// Whether `text` matches any of the comma separated `patterns`, each
/// with `*` and `?` wildcards, ignoring case.
pub fn matches_any(text: &str, patterns: &str) -> bool {
    patterns
        .split(',')
        .any(|p| text_matches(text, p, MatchMode::Wildcard))
}

/// A trigger's filters as sample values, where a filter names one value:
/// a test of a "Kick only" alert should read Kick, not YouTube.
fn filter_samples(filters: &BTreeMap<String, String>, vars: &mut BTreeMap<String, String>) {
    for (name, want) in filters {
        let want = want.trim();
        let plain = !want.is_empty() && !want.contains([',', '*', '?']);
        if plain && vars.contains_key(name) {
            vars.insert(name.clone(), want.to_string());
        }
    }
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
        // Who a command is about: `!so @ana` names ana, `!so` alone the viewer.
        (
            "target".to_string(),
            args.first()
                .map(|a| a.trim_start_matches('@').to_string())
                .filter(|a| !a.is_empty())
                .unwrap_or_else(|| msg.display_name.clone()),
        ),
    ]);
    for (i, arg) in args.iter().take(9).enumerate() {
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
        Trigger::Event { kind, filters } => {
            let mut vars: BTreeMap<String, String> = Event::sample(*kind)
                .vars
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            filter_samples(filters, &mut vars);
            // A countdown in a test message should count down from now.
            if let Some(ends) = vars.get_mut("hold_ends_at") {
                *ends = (unix_ms() / 1000 + 120).to_string();
            }
            (vars, format!("Test: {}", kind.label()))
        }
        Trigger::ChatCommand { command, .. } => (
            owned(&[
                ("user", "TestViewer"),
                ("user_login", "testviewer"),
                ("user_role", "mod"),
                ("target", "TestViewer"),
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
        Trigger::Shortcut { .. } => (
            owned(&[("body", ""), ("query", ""), ("source", "hotkey")]),
            "Test: button".to_string(),
        ),
        Trigger::ChatActivity(_) => (owned(ACTIVITY_SAMPLES), "Test: chat got busy".to_string()),
        Trigger::Scene { scene, .. } => (
            owned(&[
                ("scene", scene.trim_matches('*')),
                ("previous_scene", "Just chatting"),
            ]),
            "Test: scene".to_string(),
        ),
    }
}

/// What a chat activity trigger hands its steps, with sample values.
pub const ACTIVITY_SAMPLES: &[(&str, &str)] = &[
    ("messages", "24"),
    ("normal", "4.0"),
    ("busier", "6.0"),
    ("chatters", "15"),
    ("word_share", "58"),
    ("top_word", "pog"),
    ("user", "Ana"),
    ("message", "CLIP THAT"),
];

/// Most values a test may bring.
const MAX_TEST_VALUES: usize = 64;

/// What a scene trigger hands its steps, with sample values.
pub const SCENE_SAMPLES: &[(&str, &str)] =
    &[("scene", "Game"), ("previous_scene", "Just chatting")];

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
    fn a_flapping_destination_alerts_once_per_window() {
        let mut guard = FlapGuard::default();
        let t0 = Instant::now();
        let drop = |d: &str| Event::new(EventKind::DestinationDropped).with("destination", d);
        assert!(!guard.suppress(&drop("YouTube"), t0));
        assert!(guard.suppress(&drop("YouTube"), t0 + Duration::from_secs(30)));
        assert!(
            !guard.suppress(&drop("Kick"), t0 + Duration::from_secs(30)),
            "another destination still alerts"
        );
        assert!(!guard.suppress(&drop("YouTube"), t0 + FLAP_WINDOW + Duration::from_secs(1)));
        assert!(
            !guard.suppress(&Event::new(EventKind::HoldOpened), t0),
            "other events pass"
        );
        assert!(!guard.suppress(&Event::new(EventKind::HoldOpened), t0));
    }

    #[test]
    fn arming_a_delay_says_armed_then_ready_or_cancelled() {
        let kinds = |before, now| -> Vec<EventKind> {
            phase_events(before, now).iter().map(|e| e.kind).collect()
        };
        use EventKind::*;
        assert_eq!(kinds(("idle", 0), ("preparing", 30_000)), [DelayArmed]);
        assert_eq!(
            kinds(("preparing", 30_000), ("ready", 30_000)),
            [DelayReady]
        );
        assert_eq!(
            kinds(("idle", 0), ("ready", 30_000)),
            [DelayArmed, DelayReady],
            "buffer already full"
        );
        assert_eq!(
            kinds(("preparing", 30_000), ("active", 30_000)),
            [DelayReady],
            "switched on as it got ready"
        );
        assert_eq!(kinds(("ready", 30_000), ("idle", 0)), [DelayDisarmed]);
        assert!(
            kinds(("active", 30_000), ("ready", 30_000)).is_empty(),
            "a cut isn't ready news"
        );
        assert!(
            kinds(("active", 30_000), ("idle", 0)).is_empty(),
            "turning off is watch_delay's"
        );
        let armed = &phase_events(("idle", 0), ("preparing", 30_000))[0];
        assert_eq!(armed.var("delay"), Some("30s"));
    }

    #[test]
    fn a_drop_held_back_is_still_said_once_the_window_ends() {
        let mut guard = FlapGuard::default();
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let ev = |kind| Event::new(kind).with("destination", "YouTube");
        assert!(!guard.suppress(&ev(EventKind::DestinationDropped), at(0)));
        assert!(!guard.suppress(&ev(EventKind::DestinationLive), at(10)));
        assert!(guard.suppress(&ev(EventKind::DestinationDropped), at(180)));
        assert!(guard.release(at(200)).is_empty(), "still inside the window");
        let late = guard.release(at(301));
        assert_eq!(late.len(), 1);
        assert_eq!(late[0].kind, EventKind::DestinationDropped);
        assert!(guard.release(at(302)).is_empty(), "said once");
    }

    #[test]
    fn a_flap_that_ends_live_says_nothing_more() {
        let mut guard = FlapGuard::default();
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let ev = |kind| Event::new(kind).with("destination", "YouTube");
        assert!(!guard.suppress(&ev(EventKind::DestinationDropped), at(0)));
        assert!(!guard.suppress(&ev(EventKind::DestinationLive), at(10)));
        assert!(guard.suppress(&ev(EventKind::DestinationDropped), at(20)));
        assert!(guard.suppress(&ev(EventKind::DestinationLive), at(30)));
        assert!(guard.release(at(400)).is_empty(), "live was already said");
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
        assert_eq!(v["target"], "45");
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
        assert!(filters_match(&want("crash, freeze"), &crash), "a list");
        let drop = Event::new(EventKind::DestinationDropped).with("reason", "connection timed out");
        assert!(filters_match(&want("*timed out*"), &drop), "a wildcard");
        assert!(!filters_match(&want("crash"), &drop));
    }

    #[test]
    fn tests_read_like_the_filtered_event() {
        let filters = BTreeMap::from([
            ("destination".to_string(), "Kick".to_string()),
            ("reason".to_string(), "*timeout*".to_string()),
        ]);
        let trigger = Trigger::Event {
            kind: EventKind::DestinationDropped,
            filters,
        };
        let (vars, _) = sample_vars(&trigger);
        assert_eq!(vars["destination"], "Kick");
        assert_eq!(
            vars["reason"], "connection timed out",
            "a pattern isn't a value"
        );
    }

    #[test]
    fn a_new_stream_forgets_per_subject_messages_only() {
        let mut messages = HashMap::from([
            ("i1:ch".to_string(), "1".to_string()),
            ("i1:ch:YouTube".to_string(), "2".to_string()),
            ("test:i1:ch".to_string(), "3".to_string()),
            ("test:i1:ch:flap Kick".to_string(), "4".to_string()),
        ]);
        forget_subject_messages(&mut messages);
        let mut kept: Vec<_> = messages.into_keys().collect();
        kept.sort();
        assert_eq!(kept, ["i1:ch", "test:i1:ch"]);
    }
}
