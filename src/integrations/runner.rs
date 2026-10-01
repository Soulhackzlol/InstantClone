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
//! VOD, programs or files: those are reported as what would have happened.

use super::clock;
use super::host::{fmt_delay, fmt_duration, Host, HttpRequest, LiveState};
use super::model::{DiscordChannel, PhoneConnection, Step, StepKind};
use super::template::{self, Vars};
use crate::json;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Longest single wait a step may ask for.
const MAX_WAIT: Duration = Duration::from_secs(3600);
/// How often `wait_delay` re-reads the delay while following it.
const DELAY_POLL: Duration = Duration::from_millis(200);
/// Chat messages are cut to Twitch's limit.
const MAX_CHAT_LEN: usize = 500;

/// Connections and persistent values a run can use.
pub struct RunEnv {
    pub discord: Vec<DiscordChannel>,
    pub phone: PhoneConnection,
    pub counters: Arc<crate::sync::Mutex<BTreeMap<String, i64>>>,
    /// Called after a counter changes, to save it.
    pub counters_changed: Box<dyn Fn() + Send + Sync>,
}

/// Per-run state: the trigger's variables plus what steps produce.
pub struct RunContext {
    pub vars: BTreeMap<String, String>,
    /// Chat message to reply to, when a chat trigger started the run.
    pub reply_to: Option<String>,
    pub test: bool,
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
            for step in steps {
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
        let live = self.host.live();
        let lookup = Lookup {
            vars: &self.ctx.vars,
            counters: &self.env.counters,
            live: &live,
        };
        template::render(step.param(name), &lookup)
    }

    /// `fill`, escaping inserted values with `escape`.
    fn fill_escaped(&self, step: &Step, name: &str, escape: &dyn Fn(&str) -> String) -> String {
        let live = self.host.live();
        let lookup = Lookup {
            vars: &self.ctx.vars,
            counters: &self.env.counters,
            live: &live,
        };
        template::render_escaped(step.param(name), &lookup, escape)
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
        }
        Flow::Continue
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
        let dur = Duration::from_millis(ms).min(MAX_WAIT);
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
        tokio::time::sleep(dur).await;
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
                    fmt_duration(start_delay + extra)
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
            let due = Duration::from_millis(delay + extra).min(MAX_WAIT);
            if since.elapsed() >= due {
                break;
            }
            tokio::time::sleep((due - since.elapsed()).min(DELAY_POLL)).await;
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
        let text = self.marked(self.fill(step, "text"));
        // A test must never ping a whole server.
        let ping = if self.ctx.test {
            String::new()
        } else {
            step.param("ping").to_string()
        };
        let label = format!("Discord · {}", channel.name);
        let shown = text.clone();
        match self
            .blocking(move |h| h.discord(&channel.url, &text, &ping))
            .await
        {
            Ok(()) => self.record(label, StepStatus::Ok, shown),
            Err(e) => self.record(label, StepStatus::Failed, e),
        }
    }

    async fn chat(&mut self, step: &Step) {
        let text = safe_chat_text(&self.marked(self.fill(step, "text")));
        if text.is_empty() {
            self.record("Chat", StepStatus::Skipped, "the message came out empty");
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
        // one URL component, one JSON string, one header line.
        let json_body = matches!(
            step.param("body").trim_start().chars().next(),
            Some('{' | '[')
        );
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
            } else {
                self.fill(step, "body")
            },
        };
        let label = format!("{} {}", request.method, short_url(&request.url));
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
        let action = step.param("action").to_string();
        let seconds = self
            .fill(step, "seconds")
            .trim()
            .parse::<f64>()
            .unwrap_or(0.0);
        let ms = (seconds.clamp(0.0, 600.0) * 1000.0) as u32;
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
                self.record("Clip", StepStatus::Failed, e);
            }
        }
    }

    async fn program(&mut self, step: &Step) {
        // Never templated: see `model::validate_steps`.
        let path = step.param("path").trim().to_string();
        let args = split_args(&self.fill(step, "args"));
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

    fn counter(&mut self, step: &Step) {
        let name = self.fill(step, "name").trim().to_string();
        let by = self.fill(step, "by").trim().parse::<i64>().unwrap_or(1);
        let op = step.param("op");
        let value = {
            let mut counters = self.env.counters.lock();
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
    live: &'a LiveState,
}

impl Vars for Lookup<'_> {
    fn lookup(&self, name: &str) -> Option<String> {
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
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
        live_var(name, self.live)
    }
}

fn json_field(body: &str, path: &str) -> Option<String> {
    let doc = json::parse(body).ok()?;
    doc.path(path).map(|v| v.to_display())
}

/// The variables every run can use, whatever triggered it.
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
        _ => return None,
    })
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
        _ => false,
    }
}

/// Keep a chat message one line, within Twitch's limit, and unable to run
/// a chat command even when it starts with text a viewer typed.
pub fn safe_chat_text(text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = one_line.trim();
    let mut out: String = trimmed.chars().take(MAX_CHAT_LEN).collect();
    if out.starts_with('/') || out.starts_with('.') {
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
        out.push_str(&whole[..host_end]);
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
    rest.split(['/', '?']).next().unwrap_or(rest).to_string()
}

fn delay_action_label(action: &str, ms: u32) -> String {
    match action {
        "arm" if ms == 0 => "Set the delay to 0s".to_string(),
        "arm" => format!("Set the delay to {}", fmt_delay(ms)),
        "activate" => "Turn the delay on".to_string(),
        "cut" => "Cut the delay".to_string(),
        "cut_after" => "Cut after this airs".to_string(),
        "toggle" => "Toggle the delay".to_string(),
        "disarm" => "Disarm the delay".to_string(),
        "end_hold" => "End the reconnect screen".to_string(),
        other => format!("Delay action {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::host::{ClipInfo, HttpResponse};
    use super::*;
    use crate::sync::Mutex;

    #[derive(Default)]
    struct FakeHost {
        calls: Mutex<Vec<String>>,
        delay_ms: u32,
        fail_discord: bool,
    }

    impl Host for FakeHost {
        fn live(&self) -> LiveState {
            LiveState {
                delay_ms: self.delay_ms,
                hold_active: true,
                hold_left_ms: 102_000,
                channel: "texaz".into(),
                ..LiveState::default()
            }
        }
        fn discord(&self, url: &str, content: &str, ping: &str) -> Result<(), String> {
            self.calls
                .lock()
                .push(format!("discord {url} [{ping}] {content}"));
            if self.fail_discord {
                Err("Discord answered 500".into())
            } else {
                Ok(())
            }
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
        })
    }

    fn ctx(vars: &[(&str, &str)], test: bool) -> RunContext {
        RunContext {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            reply_to: Some("msg-1".into()),
            test,
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
    }

    #[tokio::test]
    async fn failed_steps_never_log_a_webhook_token() {
        struct Fails;
        impl Host for Fails {
            fn live(&self) -> LiveState {
                LiveState::default()
            }
            fn discord(&self, url: &str, _: &str, _: &str) -> Result<(), String> {
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
    }

    #[test]
    fn args_keep_quoted_parts_together() {
        assert_eq!(
            split_args(r#"--scene "Clip replay" --for 10 """#),
            vec!["--scene", "Clip replay", "--for", "10", ""]
        );
    }
}
