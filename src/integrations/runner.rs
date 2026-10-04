//! Runs one handler's steps: fills in templates, follows checks, waits,
//! and records what each step did for the activity log.
//!
//! A failing step does not end the run. One webhook being down should not
//! keep the Discord message or the chat reply from going out, so the step
//! is logged as failed, its output variables say so (`{response.ok}` is
//! `no`), and the run carries on. Only a `stop` step, or its check failing
//! into nothing, ends a run early.
//!
//! Test runs send messages for real, marked `[TEST]`, so the streamer sees
//! exactly what arrives. They skip waits and never touch the stream, the
//! VOD, the on-stream alerts, OBS, programs or files: those are reported as
//! what would have happened. A dry run sends nothing at all, messages
//! included, and says what each would have been.

use super::clock;
use super::event::{Event, EventKind};
use super::host::{
    fmt_clock, fmt_delay, fmt_duration, DiscordCard, DiscordMessage, Host, HttpRequest, LiveState,
};
use super::model::{DiscordChannel, PhoneConnection, Step, StepKind};
use super::obsws;
use super::template::{self, Vars};
use super::timeline::{self as tl, Timeline};
use crate::json;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Longest single wait a step may ask for.
const MAX_WAIT: Duration = Duration::from_secs(3600);
/// Longest wait a value from a template (chat, a web call) can ask for: a
/// viewer typing a huge number must not hold the integration for an hour.
const MAX_FILLED_WAIT: Duration = Duration::from_secs(60);
/// Discord messages remembered for editing. Keys can come from chat, so
/// they are capped; past this, new ones simply post a new message.
const MAX_REMEMBERED_MESSAGES: usize = 500;
/// How often `wait_delay` re-reads the delay while following it.
const DELAY_POLL: Duration = Duration::from_millis(200);
/// Chat messages are cut to Twitch's limit.
const MAX_CHAT_LEN: usize = 500;
/// Counters are all saved in one file, rewritten on every change.
const MAX_COUNTERS: usize = 5000;
const MAX_COUNTER_NAME: usize = 100;
/// How long a "Show on stream" card stays up when the step names no time.
const OVERLAY_SECONDS: u64 = 6;
/// A card's color when none (or a broken one) is set: the InstantClone blue.
pub const DEFAULT_CARD_COLOR: u32 = 0x5a_c8_fa;
/// What `{timeline}` fits in: a Discord card's description.
const TIMELINE_CHARS: usize = 4000;

/// Connections and persistent values a run can use.
pub struct RunEnv {
    pub discord: Vec<DiscordChannel>,
    pub phone: PhoneConnection,
    pub counters: Arc<crate::sync::Mutex<BTreeMap<String, i64>>>,
    /// Called after a counter changes, to save it.
    pub counters_changed: Box<dyn Fn() + Send + Sync>,
    /// The last Discord message each integration posted per channel (key
    /// `integration:channel`), so a later step can edit it. In memory only:
    /// after a restart the next update simply posts a new message.
    pub discord_messages: Arc<crate::sync::Mutex<HashMap<String, String>>>,
    /// Whose turn it is to post or edit each of those messages.
    pub discord_turns: Arc<DiscordTurns>,
    /// This stream's timeline, shared by every integration.
    pub timeline: Arc<crate::sync::Mutex<Timeline>>,
    /// Report something a step did (a timeline line) as an event of its
    /// own, for other integrations to react to. Never blocks.
    pub emit: Box<dyn Fn(Event) + Send + Sync>,
}

/// Per-run state: the trigger's variables plus what steps produce.
pub struct RunContext {
    /// The integration this run belongs to.
    pub integration_id: String,
    pub vars: BTreeMap<String, String>,
    /// Chat message to reply to, when a chat trigger started the run.
    pub reply_to: Option<String>,
    pub test: bool,
    /// A test that sends nothing at all (see the module comment).
    pub dry: bool,
    /// The run's place in the engine's run limits; None for tests.
    pub gate: Option<Gate>,
}

/// A run's place in the engine's limits on runs going at once. A run that
/// is waiting (a Wait step, or for the delay to air) gives its place back
/// and takes one again to go on, so presses during a long delay queue up
/// instead of being turned away.
pub struct Gate {
    places: Vec<Arc<Semaphore>>,
    held: Vec<OwnedSemaphorePermit>,
}

impl Gate {
    /// `held` are the places taken from `places`, in the same order.
    pub fn new(places: Vec<Arc<Semaphore>>, held: Vec<OwnedSemaphorePermit>) -> Gate {
        Gate { places, held }
    }

    fn give_back(&mut self) {
        self.held.clear();
    }

    async fn take_again(&mut self) {
        if !self.held.is_empty() {
            return;
        }
        for place in &self.places {
            // Closed only when the engine stops; the run then ends anyway.
            if let Ok(permit) = place.clone().acquire_owned().await {
                self.held.push(permit);
            }
        }
    }
}

/// Posting and editing one Discord message, one run at a time and in
/// order. An edit with a newer edit of the same message queued behind it
/// is dropped: the newer one says it all, and Discord's rate limit isn't
/// spent on a state that is already gone.
#[derive(Default)]
pub struct DiscordTurns {
    messages: crate::sync::Mutex<HashMap<String, Arc<Turn>>>,
}

#[derive(Default)]
struct Turn {
    queue: tokio::sync::Mutex<()>,
    /// Edits asked for so far; each edit's number is its place in line.
    edits: AtomicU64,
}

impl DiscordTurns {
    fn turn(&self, key: &str) -> Arc<Turn> {
        self.messages
            .lock()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// Forget a message's line once nobody is in it.
    fn done(&self, key: &str, turn: Arc<Turn>) {
        let mut messages = self.messages.lock();
        // This copy and the map's: nobody else is waiting.
        if Arc::strong_count(&turn) <= 2 {
            messages.remove(key);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepStatus {
    Ok,
    Skipped,
    Failed,
}

impl StepStatus {
    pub fn id(self) -> &'static str {
        match self {
            StepStatus::Ok => "ok",
            StepStatus::Skipped => "skipped",
            StepStatus::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct StepLog {
    pub label: String,
    pub status: StepStatus,
    /// Milliseconds since the run started.
    pub at_ms: u64,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Ok,
    /// A `stop` step ended it on purpose.
    Stopped,
    /// At least one step failed; the rest still ran.
    Failed,
}

enum Flow {
    Continue,
    Stop,
}

struct Runner {
    host: Arc<dyn Host>,
    env: Arc<RunEnv>,
    ctx: RunContext,
    started: Instant,
    log: Vec<StepLog>,
    failed: bool,
}

pub async fn run(
    steps: &[Step],
    ctx: RunContext,
    host: Arc<dyn Host>,
    env: Arc<RunEnv>,
) -> (RunStatus, Vec<StepLog>) {
    let mut runner = Runner {
        host,
        env,
        ctx,
        started: Instant::now(),
        log: Vec::new(),
        failed: false,
    };
    let flow = runner.steps(steps).await;
    let status = match (flow, runner.failed) {
        (_, true) => RunStatus::Failed,
        (Flow::Stop, false) => RunStatus::Stopped,
        (Flow::Continue, false) => RunStatus::Ok,
    };
    (status, runner.log)
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl Runner {
    fn steps<'a>(&'a mut self, steps: &'a [Step]) -> BoxFuture<'a, Flow> {
        Box::pin(async move {
            // Switched-off steps (and what's inside them) don't run.
            for step in steps.iter().filter(|s| s.enabled) {
                if let Flow::Stop = self.step(step).await {
                    return Flow::Stop;
                }
            }
            Flow::Continue
        })
    }

    fn record(&mut self, label: impl Into<String>, status: StepStatus, detail: impl Into<String>) {
        let mut detail = detail.into();
        if status == StepStatus::Failed {
            self.failed = true;
            // Errors can quote the address they failed on, and webhook and
            // API addresses carry their secret in the path or query.
            detail = redact_urls(&detail);
        }
        self.log.push(StepLog {
            label: label.into(),
            status,
            at_ms: self.started.elapsed().as_millis() as u64,
            detail,
        });
    }

    /// Fill in a step parameter's template.
    fn fill(&self, step: &Step, name: &str) -> String {
        self.fill_text(step.param(name))
    }

    fn fill_text(&self, text: &str) -> String {
        let live = self.host.live();
        template::render(text, &self.lookup(&live))
    }

    /// `fill`, escaping inserted values with `escape`.
    fn fill_escaped(&self, step: &Step, name: &str, escape: &dyn Fn(&str) -> String) -> String {
        self.fill_text_escaped(step.param(name), escape)
    }

    fn fill_text_escaped(&self, text: &str, escape: &dyn Fn(&str) -> String) -> String {
        let live = self.host.live();
        template::render_escaped(text, &self.lookup(&live), escape)
    }

    fn lookup<'a>(&'a self, live: &'a LiveState) -> Lookup<'a> {
        Lookup {
            vars: &self.ctx.vars,
            counters: &self.env.counters,
            timeline: &self.env.timeline,
            live,
        }
    }

    fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.ctx.vars.insert(name.into(), value.into());
    }

    /// Run a blocking effect off the async thread.
    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn Host) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let host = self.host.clone();
        tokio::task::spawn_blocking(move || f(host.as_ref()))
            .await
            .unwrap_or_else(|_| Err("the step crashed".to_string()))
    }

    async fn step(&mut self, step: &Step) -> Flow {
        match step.kind {
            StepKind::If => return self.check(step).await,
            StepKind::Stop => {
                self.record("Stop", StepStatus::Ok, "");
                return Flow::Stop;
            }
            StepKind::Wait => self.wait(step).await,
            StepKind::WaitDelay => self.wait_delay(step).await,
            StepKind::Discord => self.discord(step).await,
            StepKind::Chat => self.chat(step).await,
            StepKind::Phone => self.phone(step).await,
            StepKind::Http => self.http(step).await,
            StepKind::DelayAction => self.delay_action(step).await,
            StepKind::Marker => self.marker(step).await,
            StepKind::Clip => self.clip().await,
            StepKind::Program => self.program(step).await,
            StepKind::File => self.file(step).await,
            StepKind::SetVar => {
                let name = self.fill(step, "name");
                let value = self.fill(step, "value");
                self.record(format!("Remember {name}"), StepStatus::Ok, value.clone());
                self.set(name, value);
            }
            StepKind::Counter => self.counter(step),
            StepKind::Overlay => self.overlay(step),
            StepKind::EditText => self.edit_text(step),
            StepKind::Timeline => self.timeline(step),
            StepKind::Obs => self.obs(step).await,
        }
        Flow::Continue
    }

    /// Sleep without holding the run's place in the run limits.
    async fn sleep(&mut self, how_long: Duration) {
        if let Some(gate) = &mut self.ctx.gate {
            gate.give_back();
        }
        tokio::time::sleep(how_long).await;
        if let Some(gate) = &mut self.ctx.gate {
            gate.take_again().await;
        }
    }

    /// In a dry run, record what a step would have sent instead of sending
    /// it. True when it was a dry run.
    fn dry(&mut self, label: impl Into<String>, would: impl Into<String>) -> bool {
        if !self.ctx.dry {
            return false;
        }
        let would = would.into();
        let detail = if would.trim().is_empty() {
            "dry run: nothing was sent".to_string()
        } else {
            format!("dry run, would send: {would}")
        };
        self.record(label, StepStatus::Skipped, detail);
        true
    }

    async fn check(&mut self, step: &Step) -> Flow {
        let left = self.fill(step, "left");
        let right = self.fill(step, "right");
        let op = step.param("op");
        let passed = compare(&left, op, &right);
        self.record(
            "Check",
            StepStatus::Ok,
            format!(
                "\"{left}\" {} \"{right}\": {}",
                op.replace('_', " "),
                if passed { "yes" } else { "no" }
            ),
        );
        let branch = if passed { &step.then } else { &step.otherwise };
        self.steps(branch).await
    }

    async fn wait(&mut self, step: &Step) {
        let ms = self.fill(step, "ms").trim().parse::<u64>().unwrap_or(0);
        let dur = Duration::from_millis(ms).min(wait_cap(step, "ms"));
        if self.ctx.test {
            self.record(
                "Wait",
                StepStatus::Skipped,
                format!(
                    "would wait {} (skipped in tests)",
                    fmt_duration(dur.as_millis() as u64)
                ),
            );
            return;
        }
        self.sleep(dur).await;
        self.record("Wait", StepStatus::Ok, fmt_duration(dur.as_millis() as u64));
    }

    /// Wait until what was happening when the run started has reached
    /// viewers, following the delay if it changes meanwhile.
    async fn wait_delay(&mut self, step: &Step) {
        let extra = self
            .fill(step, "extra_ms")
            .trim()
            .parse::<u64>()
            .unwrap_or(0);
        let follow = step.param("follow") != "no";
        let start_delay = self.host.live().delay_ms as u64;
        if self.ctx.test {
            self.record(
                "Wait for the delay",
                StepStatus::Skipped,
                format!(
                    "would wait {} (skipped in tests)",
                    fmt_duration(start_delay.saturating_add(extra))
                ),
            );
            return;
        }
        let since = self.started;
        loop {
            let delay = if follow {
                self.host.live().delay_ms as u64
            } else {
                start_delay
            };
            // `extra` can come from chat, so it can be anything.
            let due = Duration::from_millis(delay)
                .min(MAX_WAIT)
                .saturating_add(Duration::from_millis(extra).min(wait_cap(step, "extra_ms")));
            // One reading: a second one could pass `due` and underflow.
            let elapsed = since.elapsed();
            if elapsed >= due {
                break;
            }
            // The place is given back once, for the whole wait.
            if let Some(gate) = &mut self.ctx.gate {
                gate.give_back();
            }
            tokio::time::sleep((due - elapsed).min(DELAY_POLL)).await;
        }
        if let Some(gate) = &mut self.ctx.gate {
            gate.take_again().await;
        }
        self.record(
            "Wait for the delay",
            StepStatus::Ok,
            format!(
                "aired after {}",
                fmt_duration(since.elapsed().as_millis() as u64)
            ),
        );
    }

    fn marked(&self, text: String) -> String {
        if self.ctx.test {
            format!("[TEST] {text}")
        } else {
            text
        }
    }

    async fn discord(&mut self, step: &Step) {
        let id = step.param("connection").to_string();
        let Some(channel) = self.env.discord.iter().find(|c| c.id == id).cloned() else {
            self.record(
                "Discord",
                StepStatus::Failed,
                "that Discord channel was removed; pick another",
            );
            return;
        };
        let label = format!("Discord · {}", channel.name);
        if self.ctx.dry {
            let shown = discord_shown(&self.discord_message(step));
            self.dry(label, shown);
            return;
        }
        // Test messages are tracked apart, so a test never edits a real one.
        // "About" keeps one message per subject (per destination, say)
        // instead of one per channel.
        let about = self.fill(step, "key");
        let key = format!(
            "{}{}:{}{}{}",
            if self.ctx.test { "test:" } else { "" },
            self.ctx.integration_id,
            channel.id,
            if about.trim().is_empty() { "" } else { ":" },
            about.trim().chars().take(100).collect::<String>()
        );
        // `last`: edit the last one, or post when there is none. `close`:
        // edit the last one into its final state and let it go, so the next
        // alert starts a new message; with none to finish, say nothing (a
        // "back" with no "down" before it is noise).
        let mode = step.param("edit");
        let turns = self.env.discord_turns.clone();
        let turn = turns.turn(&key);
        let number = if mode == "last" {
            turn.edits.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            0
        };
        // Waiting for the turn isn't doing anything: the place goes back.
        if let Some(gate) = &mut self.ctx.gate {
            gate.give_back();
        }
        let queued = turn.queue.lock().await;
        if let Some(gate) = &mut self.ctx.gate {
            gate.take_again().await;
        }
        if mode == "last" && turn.edits.load(Ordering::SeqCst) != number {
            drop(queued);
            turns.done(&key, turn);
            self.record(
                label,
                StepStatus::Skipped,
                "a newer update replaced this one",
            );
            return;
        }
        // Filled in on its turn, so it says what is true now: a timeline
        // card edited twice in a row shows every line either way.
        let mut message = self.discord_message(step);
        if !super::effects::discord_has_body(&message) {
            drop(queued);
            turns.done(&key, turn);
            self.record(label, StepStatus::Skipped, "the message came out empty");
            return;
        }
        let previous = matches!(mode, "last" | "close")
            .then(|| self.env.discord_messages.lock().get(&key).cloned())
            .flatten();
        if mode == "close" && previous.is_none() {
            drop(queued);
            turns.done(&key, turn);
            self.record(label, StepStatus::Skipped, "no earlier message to finish");
            return;
        }
        // A test must never ping a whole server. An edit doesn't ping; the
        // ping still goes out if the message was deleted and is posted anew.
        message.ping = if self.ctx.test {
            String::new()
        } else {
            step.param("ping").to_string()
        };
        let shown = discord_shown(&message);
        let sent = self
            .blocking(move |h| h.discord(&channel.url, &message, previous.as_deref()))
            .await;
        if let Ok(posted) = &sent {
            let mut messages = self.env.discord_messages.lock();
            if mode == "close" {
                messages.remove(&key);
            } else if !posted.message_id.is_empty()
                && (messages.len() < MAX_REMEMBERED_MESSAGES || messages.contains_key(&key))
            {
                messages.insert(key.clone(), posted.message_id.clone());
            }
        }
        drop(queued);
        turns.done(&key, turn);
        match sent {
            Ok(posted) => {
                let label = if posted.edited {
                    format!("{label} (updated)")
                } else {
                    label
                };
                self.record(label, StepStatus::Ok, shown);
            }
            Err(e) => self.record(label, StepStatus::Failed, e),
        }
    }

    /// The message a Discord step sends, filled in. As a card, `text` is the
    /// card's body and `above` the plain line over it (a clip link there
    /// unfurls into a player); otherwise `text` is the whole message.
    fn discord_message(&self, step: &Step) -> DiscordMessage {
        if step.param("style") != "card" {
            return DiscordMessage {
                content: self.marked(self.fill(step, "text")),
                ..DiscordMessage::default()
            };
        }
        let mut card = DiscordCard {
            title: self.fill(step, "title"),
            url: self.fill(step, "url"),
            description: self.fill(step, "text"),
            color: parse_color(&self.fill(step, "color")).unwrap_or(DEFAULT_CARD_COLOR),
            fields: parse_fields(step.param("fields"))
                .into_iter()
                .map(|(name, value, inline)| {
                    (self.fill_text(&name), self.fill_text(&value), inline)
                })
                .collect(),
            footer: self.fill(step, "footer"),
            timestamp: step.param("timestamp") == "yes",
            image: self.fill(step, "image"),
            thumbnail: self.fill(step, "thumbnail"),
        };
        if self.ctx.test {
            if card.title.trim().is_empty() {
                card.description = self.marked(card.description);
            } else {
                card.title = self.marked(card.title);
            }
        }
        DiscordMessage {
            content: self.fill(step, "above"),
            card: Some(card),
            ping: String::new(),
        }
    }

    /// Add a line to this stream's timeline, at the moment the stream (the
    /// VOD) shows it: now on air (`aired`), or once the delay has passed
    /// (`live`, for things happening in front of the camera right now).
    /// Between streams there is no timeline: a line that only got here
    /// after the stream ended (it waited for the delay) is left out.
    fn timeline(&mut self, step: &Step) {
        let text = self.fill(step, "text");
        let kind = tl::Kind::from_id(step.param("kind"));
        let label = match kind {
            tl::Kind::Chapter => "Timeline chapter",
            tl::Kind::Highlight => "Timeline highlight",
            tl::Kind::Note => "Timeline",
        };
        if text.trim().is_empty() {
            self.record(label, StepStatus::Skipped, "the line came out empty");
            return;
        }
        let live = self.host.live();
        if !live.streaming && !self.ctx.test {
            self.record(label, StepStatus::Skipped, "no stream is on");
            return;
        }
        let delay = if step.param("at") == "aired" {
            0
        } else {
            u64::from(live.delay_ms)
        };
        let at_ms = live.uptime_ms + delay;
        if self.ctx.test {
            self.record(
                label,
                StepStatus::Skipped,
                format!("would add `{}` {text}", fmt_clock(at_ms)),
            );
            return;
        }
        let line = self.env.timeline.lock().add(at_ms, kind, &text);
        (self.env.emit)(
            Event::new(EventKind::TimelineUpdated)
                .with("line", line.clone())
                .with("kind", kind.id()),
        );
        self.record(label, StepStatus::Ok, line);
    }

    async fn chat(&mut self, step: &Step) {
        let text = safe_chat_text(&self.marked(self.fill(step, "text")));
        if text.is_empty() {
            self.record("Chat", StepStatus::Skipped, "the message came out empty");
            return;
        }
        if self.dry("Twitch chat", text.clone()) {
            return;
        }
        let as_bot = step.param("as") == "bot";
        let reply_to = (step.param("reply") == "yes")
            .then(|| self.ctx.reply_to.clone())
            .flatten();
        let shown = text.clone();
        match self
            .blocking(move |h| h.chat(&text, as_bot, reply_to.as_deref()))
            .await
        {
            Ok(()) => self.record("Twitch chat", StepStatus::Ok, shown),
            Err(e) => self.record("Twitch chat", StepStatus::Failed, e),
        }
    }

    async fn phone(&mut self, step: &Step) {
        let phone = self.env.phone.clone();
        if !phone.is_set() {
            self.record("Phone", StepStatus::Failed, "connect a phone first");
            return;
        }
        let title = self.marked(self.fill(step, "title"));
        let text = self.fill(step, "text");
        if self.dry("Phone", text.clone()) {
            return;
        }
        let priority = step.param("priority").to_string();
        let shown = text.clone();
        let result = self
            .blocking(move |h| {
                h.phone(
                    phone.server_or_default(),
                    phone.topic.trim(),
                    &title,
                    &text,
                    &priority,
                )
            })
            .await;
        match result {
            Ok(()) => self.record("Phone", StepStatus::Ok, shown),
            Err(e) => self.record("Phone", StepStatus::Failed, e),
        }
    }

    async fn http(&mut self, step: &Step) {
        let save_as = match step.param("save_as").trim() {
            "" => "response".to_string(),
            name => name.to_string(),
        };
        // Values can come from chat, so each lands escaped for where it goes:
        // one URL component, one JSON string, one form field, one header line.
        let body = step.param("body").trim_start();
        // `{"a": …}` or `[…]` is JSON; `{user} said hi` is a variable.
        let json_body = body.starts_with('[')
            || body
                .strip_prefix('{')
                .is_some_and(|rest| rest.trim_start().starts_with(['"', '}']));
        let form_body = step
            .param("headers")
            .to_ascii_lowercase()
            .contains("application/x-www-form-urlencoded");
        let request = HttpRequest {
            method: match step.param("method").trim() {
                "" => "POST".to_string(),
                m => m.to_ascii_uppercase(),
            },
            url: self
                .fill_escaped(step, "url", &template::url_component)
                .trim()
                .to_string(),
            headers: parse_headers(&self.fill_escaped(step, "headers", &template::one_line)),
            body: if json_body {
                self.fill_escaped(step, "body", &template::json_string_content)
            } else if form_body {
                self.fill_escaped(step, "body", &template::url_component)
            } else {
                self.fill(step, "body")
            },
        };
        let label = format!("{} {}", request.method, short_url(&request.url));
        if self.dry(label.clone(), request.body.clone()) {
            return;
        }
        match self.blocking(move |h| h.http(request)).await {
            Ok(resp) => {
                let ok = (200..300).contains(&resp.status);
                self.set(format!("{save_as}.status"), resp.status.to_string());
                self.set(format!("{save_as}.ok"), if ok { "yes" } else { "no" });
                self.set(format!("{save_as}.body"), resp.body);
                let status = if ok {
                    StepStatus::Ok
                } else {
                    StepStatus::Failed
                };
                self.record(label, status, format!("answered {}", resp.status));
            }
            Err(e) => {
                self.set(format!("{save_as}.ok"), "no");
                self.record(label, StepStatus::Failed, e);
            }
        }
    }

    async fn delay_action(&mut self, step: &Step) {
        let mut action = step.param("action").to_string();
        let filled = self.fill(step, "seconds");
        let filled = filled.trim();
        // Whole seconds only: `!setdelay 2.5` or `!setdelay -5` fails the
        // step instead of quietly becoming some other delay. Blank means
        // the default delay, which the host reads from 0 ms.
        let seconds = match filled {
            "" => None,
            text if action == "arm" && !is_whole_number(text) => {
                self.record(
                    "Set the delay",
                    StepStatus::Failed,
                    format!("\"{text}\" isn't whole seconds, like 30"),
                );
                return;
            }
            // Too big for a number is past the 600 s top too.
            text if is_whole_number(text) => Some(text.parse::<u64>().unwrap_or(u64::MAX)),
            _ => None,
        };
        // A delay of 0 is no delay: what Disarm does (`!setdelay 0`).
        if action == "arm" && seconds == Some(0) {
            action = "disarm".to_string();
        }
        let ms = (seconds.unwrap_or(0).min(600) * 1000) as u32;
        let label = delay_action_label(&action, ms);
        if self.ctx.test {
            self.record(label, StepStatus::Skipped, "tests never touch the stream");
            return;
        }
        match self.blocking(move |h| h.delay_action(&action, ms)).await {
            Ok(()) => self.record(label, StepStatus::Ok, ""),
            Err(e) => self.record(label, StepStatus::Failed, e),
        }
    }

    async fn marker(&mut self, step: &Step) {
        let description = self.fill(step, "description");
        if self.ctx.test {
            self.record(
                "VOD marker",
                StepStatus::Skipped,
                format!("would mark \"{description}\""),
            );
            return;
        }
        let shown = description.clone();
        match self.blocking(move |h| h.marker(&description)).await {
            Ok(()) => self.record("VOD marker", StepStatus::Ok, shown),
            Err(e) => self.record("VOD marker", StepStatus::Failed, e),
        }
    }

    async fn clip(&mut self) {
        if self.ctx.test {
            self.set("clip.ok", "yes");
            self.set("clip.id", "TestClip");
            self.set("clip.url", "https://clips.twitch.tv/TestClip");
            self.record(
                "Clip",
                StepStatus::Skipped,
                "would clip; the test uses a sample link",
            );
            return;
        }
        match self.blocking(|h| h.clip()).await {
            Ok(clip) => {
                self.set("clip.ok", "yes");
                self.set("clip.id", clip.id);
                self.set("clip.url", clip.url.clone());
                self.record("Clip", StepStatus::Ok, clip.url);
            }
            Err(e) => {
                self.set("clip.ok", "no");
                self.set("clip.error", redact_urls(&e));
                self.record("Clip", StepStatus::Failed, e);
            }
        }
    }

    async fn program(&mut self, step: &Step) {
        // Never templated: see `model::validate_steps`.
        let path = step.param("path").trim().to_string();
        // Split first, then fill each piece: a value from chat stays one
        // argument whatever spaces it holds, so a viewer can't add arguments
        // of their own, and `program_arg` strips what a shell would run.
        let args: Vec<String> = split_args(step.param("args"))
            .iter()
            .map(|arg| self.fill_text_escaped(arg, &template::program_arg))
            .collect();
        let label = format!("Run {}", file_name(&path));
        if self.ctx.test {
            self.record(label, StepStatus::Skipped, "tests never run programs");
            return;
        }
        match self.blocking(move |h| h.program(&path, &args)).await {
            Ok(()) => self.record(label, StepStatus::Ok, ""),
            Err(e) => self.record(label, StepStatus::Failed, e),
        }
    }

    async fn file(&mut self, step: &Step) {
        // Never templated: see `model::validate_steps`.
        let path = step.param("path").trim().to_string();
        let text = self.fill(step, "text");
        let append = step.param("mode") == "append";
        let label = format!("Write {}", file_name(&path));
        if self.ctx.test {
            self.record(
                label,
                StepStatus::Skipped,
                format!("would write \"{text}\""),
            );
            return;
        }
        let shown = text.clone();
        match self.blocking(move |h| h.file(&path, &text, append)).await {
            Ok(()) => self.record(label, StepStatus::Ok, shown),
            Err(e) => self.record(label, StepStatus::Failed, e),
        }
    }

    fn overlay(&mut self, step: &Step) {
        let title = self.fill(step, "title");
        let text = self.fill(step, "text");
        if text.trim().is_empty() {
            self.record("On stream", StepStatus::Skipped, "the text came out empty");
            return;
        }
        let seconds = self
            .fill(step, "seconds")
            .trim()
            .parse::<u64>()
            .unwrap_or(OVERLAY_SECONDS);
        if self.ctx.test {
            self.record(
                "On stream",
                StepStatus::Skipped,
                format!("would show \"{text}\" (tests stay off stream)"),
            );
            return;
        }
        self.host.overlay(&title, &text, seconds);
        self.record("On stream", StepStatus::Ok, text);
    }

    /// Ask OBS to switch a scene, show or hide a source, or set a text.
    async fn obs(&mut self, step: &Step) {
        // Never templated: see `model::validate_steps`.
        let scene = step.param("scene").trim().to_string();
        let source = step.param("source").trim().to_string();
        let action = match step.param("action") {
            "scene" => obsws::Action::Scene(scene),
            "show" | "hide" | "toggle" => obsws::Action::Source {
                scene,
                source,
                visible: match step.param("action") {
                    "show" => Some(true),
                    "hide" => Some(false),
                    _ => None,
                },
            },
            "text" => obsws::Action::Text {
                source,
                text: self.fill(step, "text"),
            },
            other => {
                self.record(
                    "OBS",
                    StepStatus::Failed,
                    format!("unknown OBS action \"{other}\""),
                );
                return;
            }
        };
        let label = obs_label(&action);
        if self.ctx.test {
            self.record(label, StepStatus::Skipped, "tests never touch OBS");
            return;
        }
        match self.blocking(move |h| h.obs(action)).await {
            Ok(()) => self.record(label, StepStatus::Ok, ""),
            Err(e) => self.record(label, StepStatus::Failed, e),
        }
    }

    fn edit_text(&mut self, step: &Step) {
        let input = self.fill(step, "input");
        let (a, b) = (self.fill(step, "a"), self.fill(step, "b"));
        let out = template::edit_text(step.param("op"), &input, &a, &b);
        let name = match step.param("save_as").trim() {
            "" => "text".to_string(),
            name => name.to_string(),
        };
        self.record(format!("Edit text → {name}"), StepStatus::Ok, out.clone());
        self.set(name, out);
    }

    fn counter(&mut self, step: &Step) {
        // Names can hold `{user}`, so viewers decide how many there are.
        let name: String = self
            .fill(step, "name")
            .trim()
            .chars()
            .take(MAX_COUNTER_NAME)
            .collect();
        let by = self.fill(step, "by").trim().parse::<i64>().unwrap_or(1);
        let op = step.param("op");
        let value = {
            let mut counters = self.env.counters.lock();
            if counters.len() >= MAX_COUNTERS && !counters.contains_key(&name) {
                drop(counters);
                self.record(
                    format!("Counter {name}"),
                    StepStatus::Failed,
                    format!("there are already {MAX_COUNTERS} counters; reset some first"),
                );
                return;
            }
            let current = counters.get(&name).copied().unwrap_or(0);
            let next = match op {
                "subtract" => current.saturating_sub(by),
                "set" => by,
                "reset" => 0,
                _ => current.saturating_add(by),
            };
            if !self.ctx.test {
                counters.insert(name.clone(), next);
            }
            next
        };
        if !self.ctx.test {
            (self.env.counters_changed)();
        }
        self.set(format!("counter.{name}"), value.to_string());
        self.record(format!("Counter {name}"), StepStatus::Ok, value.to_string());
    }
}

/// Where template values come from during a run, in priority order: the
/// run's own variables, saved counters, then the live stream state.
struct Lookup<'a> {
    vars: &'a BTreeMap<String, String>,
    counters: &'a crate::sync::Mutex<BTreeMap<String, i64>>,
    timeline: &'a crate::sync::Mutex<Timeline>,
    live: &'a LiveState,
}

impl Vars for Lookup<'_> {
    fn lookup(&self, name: &str) -> Option<String> {
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
        }
        match name {
            "timeline" => return Some(self.timeline.lock().render(TIMELINE_CHARS)),
            "chapters" => return Some(self.timeline.lock().chapters()),
            "previous_chapter" => return Some(self.timeline.lock().previous_chapter()),
            _ => {}
        }
        if let Some(counter) = name.strip_prefix("counter.") {
            return Some(
                self.counters
                    .lock()
                    .get(counter)
                    .copied()
                    .unwrap_or(0)
                    .to_string(),
            );
        }
        // `{response.json.user.name}` reads a field of a saved JSON body;
        // `{body.x}` does the same for a web call's body.
        if let Some((base, path)) = name.split_once(".json.") {
            return json_field(self.vars.get(&format!("{base}.body"))?, path);
        }
        if let Some(path) = name.strip_prefix("body.") {
            return json_field(self.vars.get("body")?, path);
        }
        // `{query.team}` reads one part of a web call's `?team=red`.
        if let Some(field) = name.strip_prefix("query.") {
            return Some(query_field(self.vars.get("query")?, field));
        }
        live_var(name, self.live)
    }
}

/// One field of a query string (`a=1&team=red`), decoded; empty when it
/// isn't there.
fn query_field(query: &str, field: &str) -> String {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('=').or(Some((pair, ""))))
        .find(|(name, _)| percent_decode(name) == field)
        .map(|(_, value)| percent_decode(value))
        .unwrap_or_default()
}

/// `%20` and `+` back to spaces, and every other `%xx` to its byte.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn json_field(body: &str, path: &str) -> Option<String> {
    let doc = json::parse(body).ok()?;
    doc.path(path).map(|v| v.to_display())
}

/// The variables a chat command or chat message brings, with the samples
/// previews use. `arg4` to `arg9` exist too; they are left out here so the
/// editor's list stays short.
pub const CHAT_VARS: &[(&str, &str)] = &[
    ("user", "Ana"),
    ("user_login", "ana"),
    ("user_role", "mod"),
    ("target", "Ana"),
    ("message", "!delay"),
    ("args", "30"),
    ("arg1", "30"),
];

/// The variables every run can use, whatever triggered it. `uses` is the
/// engine's to fill (it counts runs); the rest come from the live state.
pub const GLOBAL_VARS: &[(&str, &str)] = &[
    ("delay", "30s"),
    ("delay_ms", "30000"),
    ("delay_state", "on"),
    ("phase", "active"),
    ("hold_active", "no"),
    ("hold_left", "1m 42s"),
    ("destinations_live", "3"),
    ("destinations_total", "3"),
    ("obs_live", "yes"),
    ("bitrate", "6200"),
    ("channel", "yourchannel"),
    ("time", "21:04"),
    ("date", "2026-09-30"),
    ("uptime", "1:02:14"),
    ("vod_time", "1:02:44"),
    ("onair", "live"),
    ("timeline", "`0:12` 🟢 Live\n`1:02:14` ⭐ Highlight"),
    ("chapters", "0:00 Start\n12:31 Ranked\n1:04:10 Boss fight"),
    ("previous_chapter", "Ranked"),
    ("uses", "42"),
];

fn live_var(name: &str, live: &LiveState) -> Option<String> {
    let yes_no = |b: bool| if b { "yes" } else { "no" }.to_string();
    Some(match name {
        "delay" => fmt_delay(live.delay_ms),
        "delay_ms" => live.delay_ms.to_string(),
        "delay_state" => if live.delay_ms > 0 { "on" } else { "off" }.to_string(),
        "phase" => live.phase.clone(),
        "hold_active" => yes_no(live.hold_active),
        "hold_left" => {
            if live.hold_active {
                fmt_duration(live.hold_left_ms)
            } else {
                String::new()
            }
        }
        "destinations_live" => live.destinations_live.to_string(),
        "destinations_total" => live.destinations_total.to_string(),
        "obs_live" => yes_no(live.obs_live),
        "bitrate" => live.bitrate_kbps.to_string(),
        "channel" => live.channel.clone(),
        "time" => clock::now().hm(),
        "date" => clock::now().date(),
        "uptime" if live.uptime_ms > 0 => fmt_clock(live.uptime_ms),
        // Where a moment happening now lands in the VOD: it airs, and is
        // recorded, once the delay has passed.
        "vod_time" if live.uptime_ms > 0 => fmt_clock(live.uptime_ms + u64::from(live.delay_ms)),
        "uptime" | "vod_time" => String::new(),
        "onair" => live.onair.clone(),
        _ => return None,
    })
}

/// `#5ac8fa`, `5ac8fa` or `#5cf` as a color, None when it isn't one.
pub fn parse_color(text: &str) -> Option<u32> {
    let hex = text.trim().trim_start_matches('#');
    let full = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => hex.to_string(),
        _ => return None,
    };
    u32::from_str_radix(&full, 16).ok()
}

/// A card's fields, one per line: `Name | value` sits side by side with
/// its neighbours, `Name || value` takes the whole width. Split before
/// filling, so a `|` in a value from chat can't move it into the name, and
/// at the first bar outside braces, so `{reason|unknown}` in a name stays
/// whole.
pub fn parse_fields(text: &str) -> Vec<(String, String, bool)> {
    text.lines()
        .filter_map(|line| {
            let at = field_bar(line)?;
            let inline = !line[at..].starts_with("||");
            let name = &line[..at];
            let value = &line[at + if inline { 1 } else { 2 }..];
            let (name, value) = (name.trim(), value.trim());
            (!name.is_empty() && !value.is_empty())
                .then(|| (name.to_string(), value.to_string(), inline))
        })
        .collect()
}

/// Where the bar between a field's name and value is: the first `|`
/// outside `{...}`.
fn field_bar(line: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, c) in line.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => return Some(at),
            _ => {}
        }
    }
    None
}

/// Evaluate an `if` step. Comparisons ignore case; `greater` / `less`
/// compare as numbers when both sides are numbers.
pub fn compare(left: &str, op: &str, right: &str) -> bool {
    let l = left.trim().to_lowercase();
    let r = right.trim().to_lowercase();
    let numbers = || Some((l.parse::<f64>().ok()?, r.parse::<f64>().ok()?));
    match op {
        "is" => l == r,
        "is_not" => l != r,
        "contains" => l.contains(&r),
        "not_contains" => !l.contains(&r),
        "starts_with" => l.starts_with(&r),
        "empty" => l.is_empty() || l == "0" || l == "no",
        "not_empty" => !(l.is_empty() || l == "0" || l == "no"),
        "greater" => numbers().is_some_and(|(a, b)| a > b),
        "less" => numbers().is_some_and(|(a, b)| a < b),
        "whole_number" => is_whole_number(&l),
        _ => false,
    }
}

/// `0`, `30`, `600`: digits only, so no sign, decimals or exponent.
fn is_whole_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Keep a chat message one line, within Twitch's limit, and unable to run
/// a chat command even when it starts with text a viewer typed.
pub fn safe_chat_text(text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = one_line.trim();
    let command_like = trimmed.starts_with('/') || trimmed.starts_with('.');
    // The guard character counts toward the limit too.
    let room = MAX_CHAT_LEN - usize::from(command_like);
    let mut out: String = trimmed.chars().take(room).collect();
    if command_like {
        out.insert(0, '\u{200B}');
    }
    out
}

/// `Name: value` lines to header pairs.
fn parse_headers(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            let name = name.trim();
            (!name.is_empty()).then(|| (name.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Cut every URL in `text` down to its scheme and host, so error messages
/// that quote an address never show the token in its path or query.
pub fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = ["https://", "http://"]
        .iter()
        .filter_map(|p| rest.find(p))
        .min()
    {
        out.push_str(&rest[..at]);
        let url = &rest[at..];
        let end = url
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '>' | ','))
            .unwrap_or(url.len());
        // Sentence punctuation after an address is not part of it.
        let end = url[..end]
            .trim_end_matches(['.', ',', ':', ';', '!', '?'])
            .len();
        let (whole, tail) = url.split_at(end);
        let scheme_end = whole.find("://").map_or(0, |i| i + 3);
        let host_end = whole[scheme_end..]
            .find(['/', '?', '#'])
            .map_or(whole.len(), |i| scheme_end + i);
        // `user:password@` before the host is a secret too.
        let host_start = whole[scheme_end..host_end]
            .rfind('@')
            .map_or(scheme_end, |i| scheme_end + i + 1);
        out.push_str(&whole[..scheme_end]);
        out.push_str(&whole[host_start..host_end]);
        if host_end < whole.len() {
            out.push_str("/…");
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// Split program arguments on spaces, keeping "quoted parts" together.
pub fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has_token = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has_token = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(current);
    }
    args
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn short_url(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Without any `user:password@` in front of it.
    host.rsplit('@').next().unwrap_or(host).to_string()
}

/// The longest a wait step's `name` may be: its own number as written, or
/// `MAX_FILLED_WAIT` when it is filled in from a variable.
fn wait_cap(step: &Step, name: &str) -> Duration {
    if step.param(name).contains('{') {
        MAX_FILLED_WAIT
    } else {
        MAX_WAIT
    }
}

/// What the activity log calls a Discord message: its title, its body, or
/// its plain text.
fn discord_shown(message: &DiscordMessage) -> String {
    match &message.card {
        Some(card) if !card.title.is_empty() => card.title.clone(),
        Some(card) if !card.description.is_empty() => card.description.clone(),
        _ => message.content.clone(),
    }
}

fn obs_label(action: &obsws::Action) -> String {
    match action {
        obsws::Action::Scene(scene) => format!("OBS scene {scene}"),
        obsws::Action::Source {
            source,
            visible: Some(true),
            ..
        } => format!("Show {source}"),
        obsws::Action::Source {
            source,
            visible: Some(false),
            ..
        } => format!("Hide {source}"),
        obsws::Action::Source { source, .. } => format!("Show or hide {source}"),
        obsws::Action::Text { source, .. } => format!("Text of {source}"),
    }
}

fn delay_action_label(action: &str, ms: u32) -> String {
    match action {
        "arm" if ms == 0 => "Set the delay to your default".to_string(),
        "arm" => format!("Set the delay to {}", fmt_delay(ms)),
        "activate" => "Turn the delay on".to_string(),
        "cut" => "Back to live now".to_string(),
        "cut_after" => "Back to live after this airs".to_string(),
        "toggle" => "Turn the delay on or off".to_string(),
        "disarm" => "Turn the delay off".to_string(),
        "end_hold" => "End the reconnect screen".to_string(),
        other => format!("Delay action {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::host::{ClipInfo, DiscordPosted, HttpResponse};
    use super::*;
    use crate::sync::Mutex;

    #[derive(Default)]
    struct FakeHost {
        calls: Mutex<Vec<String>>,
        delay_ms: u32,
        fail_discord: bool,
        /// No stream on.
        offline: bool,
    }

    impl Host for FakeHost {
        fn live(&self) -> LiveState {
            LiveState {
                streaming: !self.offline,
                delay_ms: self.delay_ms,
                hold_active: true,
                hold_left_ms: 102_000,
                channel: "texaz".into(),
                ..LiveState::default()
            }
        }
        fn discord(
            &self,
            url: &str,
            message: &DiscordMessage,
            edit: Option<&str>,
        ) -> Result<DiscordPosted, String> {
            let mut calls = self.calls.lock();
            let card = message.card.as_ref().map_or_else(String::new, |c| {
                format!(
                    "card[{}|{}|{:06x}|{:?}]",
                    c.title, c.description, c.color, c.fields
                )
            });
            calls.push(format!(
                "discord {url} [{}] edit={edit:?} {}{card}",
                message.ping, message.content
            ));
            if self.fail_discord {
                return Err("Discord answered 500".into());
            }
            Ok(DiscordPosted {
                message_id: edit.map_or_else(|| format!("m{}", calls.len()), str::to_string),
                edited: edit.is_some(),
            })
        }
        fn http(&self, r: HttpRequest) -> Result<HttpResponse, String> {
            self.calls
                .lock()
                .push(format!("http {} {} {}", r.method, r.url, r.body));
            Ok(HttpResponse {
                status: 200,
                body: r#"{"user":{"name":"ana"}}"#.into(),
            })
        }
        fn phone(
            &self,
            _: &str,
            topic: &str,
            title: &str,
            text: &str,
            _: &str,
        ) -> Result<(), String> {
            self.calls
                .lock()
                .push(format!("phone {topic} {title} {text}"));
            Ok(())
        }
        fn chat(&self, text: &str, as_bot: bool, reply_to: Option<&str>) -> Result<(), String> {
            self.calls
                .lock()
                .push(format!("chat bot={as_bot} reply={reply_to:?} {text}"));
            Ok(())
        }
        fn marker(&self, d: &str) -> Result<(), String> {
            self.calls.lock().push(format!("marker {d}"));
            Ok(())
        }
        fn clip(&self) -> Result<ClipInfo, String> {
            self.calls.lock().push("clip".into());
            Ok(ClipInfo {
                id: "Abc".into(),
                url: "https://clips.twitch.tv/Abc".into(),
            })
        }
        fn delay_action(&self, action: &str, ms: u32) -> Result<(), String> {
            self.calls.lock().push(format!("delay {action} {ms}"));
            Ok(())
        }
        fn program(&self, path: &str, args: &[String]) -> Result<(), String> {
            self.calls.lock().push(format!("program {path} {args:?}"));
            Ok(())
        }
        fn file(&self, path: &str, text: &str, append: bool) -> Result<(), String> {
            self.calls
                .lock()
                .push(format!("file {path} {append} {text}"));
            Ok(())
        }
        fn overlay(&self, title: &str, text: &str, seconds: u64) {
            self.calls
                .lock()
                .push(format!("overlay {title}|{text}|{seconds}"));
        }
        fn obs(&self, action: obsws::Action) -> Result<(), String> {
            self.calls.lock().push(format!("obs {action:?}"));
            Ok(())
        }
    }

    fn env() -> Arc<RunEnv> {
        Arc::new(RunEnv {
            discord: vec![DiscordChannel {
                id: "mods".into(),
                name: "Mods".into(),
                url: "https://discord.com/api/webhooks/1/x".into(),
            }],
            phone: PhoneConnection {
                server: String::new(),
                topic: "my-topic".into(),
            },
            counters: Arc::new(Mutex::new(BTreeMap::new())),
            counters_changed: Box::new(|| {}),
            discord_messages: Arc::new(Mutex::new(HashMap::new())),
            discord_turns: Arc::new(DiscordTurns::default()),
            timeline: Arc::new(Mutex::new(Timeline::default())),
            emit: Box::new(|_| {}),
        })
    }

    #[tokio::test]
    async fn a_card_fills_every_part_and_keeps_fields_in_place() {
        let host = Arc::new(FakeHost::default());
        let step = Step::new(
            StepKind::Discord,
            &[
                ("connection", "mods"),
                ("style", "card"),
                ("title", "🔴 {destination} is down"),
                ("text", "Since {down_for}"),
                ("color", "#f2665a"),
                ("fields", "Reason | {reason}\nNotes || a|b"),
                ("above", ""),
            ],
        );
        let c = ctx(
            &[
                ("destination", "YouTube"),
                ("down_for", "1m"),
                ("reason", "x | y"),
            ],
            false,
        );
        go(&host, &[step], c).await;
        let calls = host.calls.lock();
        assert!(
            calls[0].contains("card[🔴 YouTube is down|Since 1m|f2665a|"),
            "{}",
            calls[0]
        );
        assert!(
            calls[0].contains(r#"("Reason", "x | y", true)"#),
            "{}",
            calls[0]
        );
        assert!(
            calls[0].contains(r#"("Notes", "a|b", false)"#),
            "{}",
            calls[0]
        );
    }

    #[tokio::test]
    async fn an_empty_message_is_skipped_not_sent() {
        let host = Arc::new(FakeHost::default());
        let step = Step::new(
            StepKind::Discord,
            &[("connection", "mods"), ("text", "{nothing}")],
        );
        let (_, log) = go(&host, &[step], ctx(&[], false)).await;
        assert_eq!(log[0].status, StepStatus::Skipped);
        assert!(host.calls.lock().is_empty());
    }

    #[tokio::test]
    async fn messages_about_different_subjects_edit_their_own_message() {
        let host = Arc::new(FakeHost::default());
        let env = env();
        let post = |dest: &str, edit: &str| {
            let step = Step::new(
                StepKind::Discord,
                &[
                    ("connection", "mods"),
                    ("text", "{destination}"),
                    ("edit", edit),
                    ("key", "{destination}"),
                ],
            );
            (step, ctx(&[("destination", dest)], false))
        };
        for (dest, edit) in [("YouTube", ""), ("Kick", ""), ("YouTube", "last")] {
            let (step, c) = post(dest, edit);
            run(&[step], c, host.clone(), env.clone()).await;
        }
        let calls = host.calls.lock();
        assert!(calls[2].contains("edit=Some(\"m1\")"), "{}", calls[2]);
    }

    #[tokio::test]
    async fn finishing_a_message_edits_it_once_and_never_posts() {
        let host = Arc::new(FakeHost::default());
        let env = env();
        let send = |edit: &str| {
            Step::new(
                StepKind::Discord,
                &[
                    ("connection", "mods"),
                    ("text", "x"),
                    ("edit", edit),
                    ("key", "yt"),
                ],
            )
        };
        // Nothing to finish: no "back" without a "down".
        let (_, log) = run(&[send("close")], ctx(&[], false), host.clone(), env.clone()).await;
        assert_eq!(log[0].status, StepStatus::Skipped);
        for edit in ["", "close", "close"] {
            run(&[send(edit)], ctx(&[], false), host.clone(), env.clone()).await;
        }
        let calls = host.calls.lock();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[1].contains("edit=Some(\"m1\")"));
    }

    #[tokio::test]
    async fn timeline_lines_land_at_their_stream_position() {
        let host = Arc::new(FakeHost {
            delay_ms: 20_000,
            ..FakeHost::default()
        });
        let env = env();
        let line = |at: &str| {
            Step::new(
                StepKind::Timeline,
                &[("text", "Boss {arg1}"), ("kind", "chapter"), ("at", at)],
            )
        };
        let steps = [line("live"), line("aired")];
        run(&steps, ctx(&[("arg1", "fight")], false), host, env.clone()).await;
        let shown = env.timeline.lock().render(1000);
        assert_eq!(shown, "`0:00` Boss fight\n`0:20` Boss fight");
    }

    #[tokio::test]
    async fn no_timeline_line_lands_after_the_stream_ended() {
        let host = Arc::new(FakeHost {
            offline: true,
            ..FakeHost::default()
        });
        let env = env();
        let step = Step::new(StepKind::Timeline, &[("text", "late highlight")]);
        let (_, log) = run(&[step], ctx(&[], false), host, env.clone()).await;
        assert_eq!(log[0].status, StepStatus::Skipped);
        assert_eq!(env.timeline.lock().render(1000), "");
    }

    #[tokio::test]
    async fn a_dry_run_sends_nothing_and_says_what_it_would_have() {
        let host = Arc::new(FakeHost::default());
        let steps = [
            Step::new(
                StepKind::Discord,
                &[("connection", "mods"), ("text", "Hi {user}")],
            ),
            Step::new(StepKind::Chat, &[("text", "hello chat")]),
            Step::new(StepKind::Http, &[("url", "https://example.com/x")]),
            Step::new(StepKind::Obs, &[("action", "scene"), ("scene", "Game")]),
        ];
        let mut c = ctx(&[("user", "Ana")], true);
        c.dry = true;
        let (_, log) = go(&host, &steps, c).await;
        assert!(host.calls.lock().is_empty(), "{:?}", host.calls.lock());
        assert!(log.iter().all(|l| l.status == StepStatus::Skipped));
        assert!(log[0].detail.contains("[TEST] Hi Ana"), "{}", log[0].detail);
    }

    #[tokio::test]
    async fn switched_off_steps_are_skipped_with_what_is_inside() {
        let host = Arc::new(FakeHost::default());
        let mut check = Step::new(StepKind::If, &[("left", "a"), ("op", "is"), ("right", "a")]);
        check
            .then
            .push(Step::new(StepKind::Chat, &[("text", "inside")]));
        check.enabled = false;
        let steps = [check, Step::new(StepKind::Chat, &[("text", "after")])];
        go(&host, &steps, ctx(&[], false)).await;
        assert_eq!(*host.calls.lock(), ["chat bot=false reply=None after"]);
    }

    #[tokio::test]
    async fn an_edit_with_a_newer_one_waiting_gives_way() {
        let host = Arc::new(FakeHost::default());
        let env = env();
        let edit = Step::new(
            StepKind::Discord,
            &[("connection", "mods"), ("text", "{n}"), ("edit", "last")],
        );
        // The first posts; then three edits queue behind a held turn.
        run(
            std::slice::from_ref(&edit),
            ctx(&[("n", "1")], false),
            host.clone(),
            env.clone(),
        )
        .await;
        let turn = env.discord_turns.turn("crash:mods");
        let held = turn.queue.lock().await;
        let mut runs = Vec::new();
        for n in ["2", "3", "4"] {
            let (step, env, host) = (edit.clone(), env.clone(), host.clone());
            runs.push(tokio::spawn(async move {
                run(&[step], ctx(&[("n", n)], false), host, env).await.1
            }));
            tokio::task::yield_now().await;
        }
        drop(held);
        drop(turn);
        let mut skipped = 0;
        for r in runs {
            skipped += r
                .await
                .unwrap()
                .iter()
                .filter(|l| l.status == StepStatus::Skipped)
                .count();
        }
        assert_eq!(skipped, 2, "only the newest edit goes out");
        let calls = host.calls.lock();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[1].ends_with(" 4"), "{}", calls[1]);
    }

    #[tokio::test]
    async fn obs_steps_say_what_they_do_and_tests_leave_obs_alone() {
        let host = Arc::new(FakeHost::default());
        let hide = Step::new(StepKind::Obs, &[("action", "hide"), ("source", "Cam")]);
        go(&host, std::slice::from_ref(&hide), ctx(&[], false)).await;
        assert!(host.calls.lock()[0].contains("visible: Some(false)"));
        let (_, log) = go(&host, &[hide], ctx(&[], true)).await;
        assert_eq!(log[0].status, StepStatus::Skipped);
        assert_eq!(host.calls.lock().len(), 1);
    }

    #[tokio::test]
    async fn a_wait_chat_fills_in_is_kept_short() {
        let step = Step::new(StepKind::Wait, &[("ms", "{arg1}")]);
        assert_eq!(wait_cap(&step, "ms"), MAX_FILLED_WAIT);
        let fixed = Step::new(StepKind::Wait, &[("ms", "120000")]);
        assert_eq!(wait_cap(&fixed, "ms"), MAX_WAIT);
    }

    #[test]
    fn query_fields_are_read_and_decoded() {
        assert_eq!(query_field("team=red%20one&x=1", "team"), "red one");
        assert_eq!(query_field("a=1+2", "a"), "1 2");
        assert_eq!(query_field("flag&b=2", "flag"), "");
        assert_eq!(query_field("a=1", "missing"), "");
        assert_eq!(percent_decode("100%"), "100%", "a stray % stays");
    }

    #[test]
    fn colors_and_fields_parse() {
        assert_eq!(parse_color("#5ac8fa"), Some(0x5ac8fa));
        assert_eq!(parse_color("5cf"), Some(0x55ccff));
        assert_eq!(parse_color("blue"), None);
        assert_eq!(
            parse_fields("A | 1\n\nB || 2\nno separator\n | empty name"),
            vec![
                ("A".into(), "1".into(), true),
                ("B".into(), "2".into(), false)
            ]
        );
        assert_eq!(
            parse_fields("{why|Reason} | {reason|unknown}"),
            vec![("{why|Reason}".into(), "{reason|unknown}".into(), true)]
        );
    }

    fn ctx(vars: &[(&str, &str)], test: bool) -> RunContext {
        RunContext {
            integration_id: "crash".into(),
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            reply_to: Some("msg-1".into()),
            test,
            dry: false,
            gate: None,
        }
    }

    async fn go(host: &Arc<FakeHost>, steps: &[Step], c: RunContext) -> (RunStatus, Vec<StepLog>) {
        run(steps, c, host.clone(), env()).await
    }

    #[tokio::test]
    async fn fills_templates_from_event_and_live_state() {
        let host = Arc::new(FakeHost {
            delay_ms: 30_000,
            ..FakeHost::default()
        });
        let steps = [Step::new(
            StepKind::Discord,
            &[
                ("connection", "mods"),
                (
                    "text",
                    "OBS dropped ({reason}), {hold_left} left, delay {delay|off}",
                ),
            ],
        )];
        let (status, log) = go(&host, &steps, ctx(&[("reason", "crash")], false)).await;
        assert_eq!(status, RunStatus::Ok);
        assert_eq!(log[0].status, StepStatus::Ok);
        let calls = host.calls.lock();
        assert!(
            calls[0].ends_with("OBS dropped (crash), 1m 42s left, delay 30s"),
            "{calls:?}"
        );
    }

    #[tokio::test]
    async fn an_update_edits_the_last_message() {
        let host = Arc::new(FakeHost::default());
        let env = env();
        let down = [Step::new(
            StepKind::Discord,
            &[("connection", "mods"), ("text", "down"), ("ping", "here")],
        )];
        let back = [Step::new(
            StepKind::Discord,
            &[
                ("connection", "mods"),
                ("text", "back"),
                ("edit", "last"),
                ("ping", "here"),
            ],
        )];
        run(&back, ctx(&[], false), host.clone(), env.clone()).await;
        run(&down, ctx(&[], false), host.clone(), env.clone()).await;
        let (_, log) = run(&back, ctx(&[], false), host.clone(), env.clone()).await;
        // A test's message is its own: it never edits the real one.
        run(&back, ctx(&[], true), host.clone(), env.clone()).await;
        let calls = host.calls.lock();
        assert!(
            calls[0].contains("[here] edit=None back"),
            "nothing to edit yet: {calls:?}"
        );
        assert!(calls[1].contains("[here] edit=None down"));
        // The ping rides along for when the message is gone and is reposted.
        assert!(
            calls[2].contains("[here] edit=Some(\"m2\") back"),
            "{calls:?}"
        );
        assert!(calls[3].contains("edit=None [TEST] back"), "{calls:?}");
        assert!(log[0].label.ends_with("(updated)"));
    }

    #[tokio::test]
    async fn chat_values_stay_one_program_argument() {
        let host = Arc::new(FakeHost::default());
        let steps = [Step::new(
            StepKind::Program,
            &[("path", "say.exe"), ("args", "--from \"{user}\" {message}")],
        )];
        let said = [("user", "ana b"), ("message", "hi\" --admin \"x & calc")];
        go(&host, &steps, ctx(&said, false)).await;
        // The quotes and `&` are gone: a shell can't read a command into it.
        assert_eq!(
            host.calls.lock()[0],
            r#"program say.exe ["--from", "ana b", "hi admin x  calc"]"#
        );
    }

    #[tokio::test]
    async fn overlay_shows_on_stream_except_in_tests() {
        let host = Arc::new(FakeHost::default());
        let steps = [
            Step::new(
                StepKind::Overlay,
                &[("title", "Clip"), ("text", "by {user}"), ("seconds", "8")],
            ),
            Step::new(StepKind::Overlay, &[("text", "{nothing}")]),
        ];
        let (_, log) = go(&host, &steps, ctx(&[("user", "Ana")], false)).await;
        assert_eq!(log[1].status, StepStatus::Skipped);
        let (_, log) = go(&host, &steps, ctx(&[("user", "Ana")], true)).await;
        assert_eq!(log[0].status, StepStatus::Skipped);
        assert_eq!(host.calls.lock().as_slice(), ["overlay Clip|by Ana|8"]);
    }

    #[tokio::test]
    async fn edited_text_feeds_later_steps() {
        let host = Arc::new(FakeHost::default());
        let steps = [
            Step::new(
                StepKind::EditText,
                &[
                    ("input", "{answer}"),
                    ("op", "between"),
                    ("a", "rank:"),
                    ("b", "("),
                    ("save_as", "rank"),
                ],
            ),
            Step::new(StepKind::EditText, &[("input", "{rank}"), ("op", "upper")]),
            Step::new(StepKind::Chat, &[("text", "{rank} / {text}")]),
        ];
        go(
            &host,
            &steps,
            ctx(&[("answer", "rank: Gold 2 (34 RR)")], false),
        )
        .await;
        assert!(host.calls.lock()[0].ends_with("Gold 2 / GOLD 2"));
    }

    #[tokio::test]
    async fn checks_branch_and_stop_ends_the_run() {
        let host = Arc::new(FakeHost::default());
        let mut check = Step::new(
            StepKind::If,
            &[("left", "{user_role}"), ("op", "is"), ("right", "MOD")],
        );
        check.then.push(Step::new(
            StepKind::Chat,
            &[("text", "ok {user}"), ("reply", "yes")],
        ));
        check.otherwise.push(Step::new(StepKind::Stop, &[]));
        let steps = [
            check.clone(),
            Step::new(StepKind::Marker, &[("description", "after")]),
        ];

        let (status, _) = go(
            &host,
            &steps,
            ctx(&[("user_role", "mod"), ("user", "Ana")], false),
        )
        .await;
        assert_eq!(status, RunStatus::Ok);
        let (status, _) = go(
            &host,
            &steps,
            ctx(&[("user_role", "viewer"), ("user", "Bo")], false),
        )
        .await;
        assert_eq!(status, RunStatus::Stopped);

        let calls = host.calls.lock();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[0].contains("reply=Some(\"msg-1\") ok Ana"));
        assert_eq!(calls[1], "marker after");
    }

    #[tokio::test]
    async fn a_failing_step_does_not_stop_the_rest() {
        let host = Arc::new(FakeHost {
            fail_discord: true,
            ..FakeHost::default()
        });
        let steps = [
            Step::new(StepKind::Discord, &[("connection", "mods"), ("text", "a")]),
            Step::new(StepKind::Chat, &[("text", "b")]),
        ];
        let (status, log) = go(&host, &steps, ctx(&[], false)).await;
        assert_eq!(status, RunStatus::Failed);
        assert_eq!(log[0].status, StepStatus::Failed);
        assert_eq!(log[1].status, StepStatus::Ok);
    }

    #[tokio::test]
    async fn step_outputs_feed_later_steps() {
        let host = Arc::new(FakeHost::default());
        let steps = [
            Step::new(
                StepKind::Http,
                &[
                    ("method", "get"),
                    ("url", "https://api.example/u"),
                    ("save_as", "who"),
                ],
            ),
            Step::new(StepKind::Clip, &[]),
            Step::new(
                StepKind::Chat,
                &[("text", "{who.json.user.name} {who.status} {clip.url}")],
            ),
        ];
        go(&host, &steps, ctx(&[], false)).await;
        let calls = host.calls.lock();
        assert!(calls[0].starts_with("http GET https://api.example/u"));
        assert!(
            calls[2].ends_with("ana 200 https://clips.twitch.tv/Abc"),
            "{calls:?}"
        );
    }

    /// What a viewer types fills its own part of a request and nothing
    /// more: one URL component, one JSON string.
    #[tokio::test]
    async fn chat_values_cannot_escape_their_place_in_a_request() {
        let host = Arc::new(FakeHost::default());
        let steps = [
            Step::new(
                StepKind::Http,
                &[
                    ("method", "GET"),
                    ("url", "https://api.example/rank/{arg1}?q={args}"),
                ],
            ),
            Step::new(
                StepKind::Http,
                &[
                    ("url", "https://api.example/log"),
                    ("body", r#"{"who":"{args}"}"#),
                ],
            ),
        ];
        let typed = r#"ana/../admin?key=1&x="#;
        go(
            &host,
            &steps,
            ctx(
                &[("arg1", typed), ("args", r#"a","admin":true,"b":""#)],
                false,
            ),
        )
        .await;
        let calls = host.calls.lock();
        assert_eq!(
            calls[0].trim_end(),
            "http GET https://api.example/rank/ana%2F..%2Fadmin%3Fkey%3D1%26x%3D?q=a%22%2C%22admin%22%3Atrue%2C%22b%22%3A%22"
        );
        assert!(
            calls[1].ends_with(r#"{"who":"a\",\"admin\":true,\"b\":\""}"#),
            "{calls:?}"
        );
    }

    #[tokio::test]
    async fn setting_the_delay_to_zero_turns_it_off() {
        let host = Arc::new(FakeHost::default());
        let set = |seconds: &str| {
            Step::new(
                StepKind::DelayAction,
                &[("action", "arm"), ("seconds", seconds)],
            )
        };
        let steps = [set("{arg1}"), set(""), set("45"), set("9999")];
        let (status, log) = go(&host, &steps, ctx(&[("arg1", "0")], false)).await;
        assert_eq!(status, RunStatus::Ok);
        let calls = host.calls.lock();
        assert_eq!(
            calls.as_slice(),
            [
                "delay disarm 0",
                "delay arm 0",
                "delay arm 45000",
                "delay arm 600000"
            ],
            "0 turns it off, blank means the default, the top is 600 s"
        );
        assert_eq!(log[0].label, "Turn the delay off");
        assert_eq!(log[1].label, "Set the delay to your default");
    }

    #[tokio::test]
    async fn the_delay_takes_whole_seconds_only() {
        for typed in ["2.5", "-5", "1e3", "0.0004", "thirty"] {
            let host = Arc::new(FakeHost::default());
            let steps = [Step::new(
                StepKind::DelayAction,
                &[("action", "arm"), ("seconds", "{arg1}")],
            )];
            let (_, log) = go(&host, &steps, ctx(&[("arg1", typed)], false)).await;
            assert!(host.calls.lock().is_empty(), "{typed} changed the delay");
            assert_eq!(log[0].status, StepStatus::Failed, "{typed}");
        }
        assert!(compare("30", "whole_number", ""));
        assert!(!compare("2.5", "whole_number", ""));
        assert!(!compare("", "whole_number", ""));
    }

    #[tokio::test]
    async fn test_runs_mark_messages_and_leave_the_stream_alone() {
        let host = Arc::new(FakeHost::default());
        let steps = [
            Step::new(StepKind::Wait, &[("ms", "600000")]),
            Step::new(StepKind::DelayAction, &[("action", "cut")]),
            Step::new(StepKind::Program, &[("path", "C:\\x.exe")]),
            Step::new(StepKind::Chat, &[("text", "hi")]),
        ];
        let (status, log) = go(&host, &steps, ctx(&[], true)).await;
        assert_eq!(status, RunStatus::Ok);
        assert_eq!(log[0].status, StepStatus::Skipped);
        assert_eq!(log[1].status, StepStatus::Skipped);
        let calls = host.calls.lock();
        assert_eq!(calls.as_slice(), ["chat bot=false reply=None [TEST] hi"]);
    }

    #[tokio::test]
    async fn counters_persist_between_runs() {
        let host = Arc::new(FakeHost::default());
        let env = env();
        let steps = [
            Step::new(StepKind::Counter, &[("name", "crashes")]),
            Step::new(
                StepKind::File,
                &[("path", "c.txt"), ("text", "Crashes: {counter.crashes}")],
            ),
        ];
        run(&steps, ctx(&[], false), host.clone(), env.clone()).await;
        run(&steps, ctx(&[], false), host.clone(), env.clone()).await;
        assert_eq!(env.counters.lock().get("crashes"), Some(&2));
        assert!(host.calls.lock()[1].ends_with("Crashes: 2"));
    }

    #[tokio::test]
    async fn waiting_for_the_delay_follows_it() {
        let host = Arc::new(FakeHost {
            delay_ms: 50,
            ..FakeHost::default()
        });
        let started = Instant::now();
        let (_, log) = go(
            &host,
            &[Step::new(StepKind::WaitDelay, &[])],
            ctx(&[], false),
        )
        .await;
        assert!(started.elapsed() >= Duration::from_millis(50));
        assert_eq!(log[0].status, StepStatus::Ok);
    }

    #[test]
    fn compare_handles_text_and_numbers() {
        assert!(compare("Crash", "is", "crash"));
        assert!(compare("12", "greater", "9"));
        assert!(!compare("abc", "greater", "9"));
        assert!(compare("", "empty", ""));
        assert!(compare("no", "empty", ""));
        assert!(compare("hello world", "contains", "WORLD"));
        assert!(!compare("x", "bogus_op", "x"));
    }

    #[test]
    fn urls_in_errors_lose_their_secrets() {
        assert_eq!(
            redact_urls("request failed: https://discord.com/api/webhooks/1/tok3n: 404"),
            "request failed: https://discord.com/…: 404"
        );
        assert_eq!(
            redact_urls("see http://host?key=1, ok"),
            "see http://host/…, ok"
        );
        assert_eq!(redact_urls("no urls here"), "no urls here");
        assert_eq!(redact_urls("https://bare.host"), "https://bare.host");
        assert_eq!(
            redact_urls("failed: https://ana:pw@api.host/x"),
            "failed: https://api.host/…"
        );
        assert_eq!(short_url("https://ana:pw@api.host/x?k=1"), "api.host");
    }

    #[tokio::test]
    async fn failed_steps_never_log_a_webhook_token() {
        struct Fails;
        impl Host for Fails {
            fn live(&self) -> LiveState {
                LiveState::default()
            }
            fn discord(
                &self,
                url: &str,
                _: &DiscordMessage,
                _: Option<&str>,
            ) -> Result<DiscordPosted, String> {
                Err(format!("couldn't reach {url}"))
            }
            fn http(&self, _: HttpRequest) -> Result<super::super::host::HttpResponse, String> {
                Err("x".into())
            }
            fn phone(&self, _: &str, _: &str, _: &str, _: &str, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn chat(&self, _: &str, _: bool, _: Option<&str>) -> Result<(), String> {
                Ok(())
            }
            fn marker(&self, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn clip(&self) -> Result<super::super::host::ClipInfo, String> {
                Err("x".into())
            }
            fn delay_action(&self, _: &str, _: u32) -> Result<(), String> {
                Ok(())
            }
            fn program(&self, _: &str, _: &[String]) -> Result<(), String> {
                Ok(())
            }
            fn file(&self, _: &str, _: &str, _: bool) -> Result<(), String> {
                Ok(())
            }
            fn overlay(&self, _: &str, _: &str, _: u64) {}
            fn obs(&self, _: obsws::Action) -> Result<(), String> {
                Ok(())
            }
        }
        let steps = [Step::new(
            StepKind::Discord,
            &[("connection", "mods"), ("text", "x")],
        )];
        let (_, log) = run(&steps, ctx(&[], false), Arc::new(Fails), env()).await;
        assert!(!log[0].detail.contains("/1/x"), "{}", log[0].detail);
    }

    #[test]
    fn chat_text_cannot_run_commands_or_span_lines() {
        assert_eq!(safe_chat_text("/ban someone"), "\u{200B}/ban someone");
        assert_eq!(safe_chat_text("a\r\nb"), "a  b");
        assert_eq!(safe_chat_text(&"x".repeat(900)).chars().count(), 500);
        assert_eq!(safe_chat_text(&"/".repeat(900)).chars().count(), 500);
    }

    #[test]
    fn args_keep_quoted_parts_together() {
        assert_eq!(
            split_args(r#"--scene "Clip replay" --for 10 """#),
            vec!["--scene", "Clip replay", "--for", "10", ""]
        );
    }
}
