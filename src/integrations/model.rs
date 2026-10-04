//! The shape of an integration, and how it is saved.
//!
//! An integration is one or more handlers; a handler is a trigger plus the
//! steps it runs. Catalog modules and hand-built ones are the same thing:
//! a module is an integration whose `preset` names the catalog entry it came
//! from, which is what lets the dashboard show it with the simple editor.
//!
//! Step settings are string parameters rather than typed fields. Every one
//! is a template the runner fills in at run time, and new step options can
//! be added without changing the saved format.

use super::event::EventKind;
use crate::json::{self, Value};
use std::collections::BTreeMap;

pub const MAX_INTEGRATIONS: usize = 100;
/// Room for the "every event" webhook, one trigger per event.
pub const MAX_HANDLERS: usize = 32;
pub const MAX_STEPS: usize = 64;
pub const MAX_DEPTH: usize = 4;
const MAX_NAME_LEN: usize = 80;
const MAX_PARAM_LEN: usize = 4000;
const MAX_ID_LEN: usize = 40;

#[derive(Clone, Debug, PartialEq)]
pub struct Integration {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Catalog entry this came from, or empty when built by hand.
    pub preset: String,
    /// Least time between two runs of the same handler. 0 = no limit.
    pub cooldown_ms: u64,
    /// Hours it stays silent, on this PC's clock.
    pub quiet: Option<QuietHours>,
    pub handlers: Vec<Handler>,
}

/// A daily window, in minutes since midnight, when an integration doesn't
/// run. `from` after `to` spans midnight (23:00 to 08:00).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuietHours {
    pub from: u16,
    pub to: u16,
}

impl QuietHours {
    pub fn contains(self, minute_of_day: u16) -> bool {
        if self.from <= self.to {
            (self.from..self.to).contains(&minute_of_day)
        } else {
            minute_of_day >= self.from || minute_of_day < self.to
        }
    }

    fn to_json(self) -> Value {
        json::obj([
            ("from", json::str(hhmm(self.from))),
            ("to", json::str(hhmm(self.to))),
        ])
    }

    /// `{"from":"23:00","to":"08:00"}`; both blank is no quiet hours.
    fn from_json(v: Option<&Value>) -> Result<Option<QuietHours>, String> {
        let Some(v) = v.filter(|v| !matches!(v, Value::Null)) else {
            return Ok(None);
        };
        let (from, to) = (v.str_or("from", "").trim(), v.str_or("to", "").trim());
        if from.is_empty() && to.is_empty() {
            return Ok(None);
        }
        match (parse_hhmm(from), parse_hhmm(to)) {
            (Some(from), Some(to)) if from != to => Ok(Some(QuietHours { from, to })),
            (Some(_), Some(_)) => Err("quiet hours can't start and end at the same time".into()),
            _ => Err("quiet hours need times like 23:00".into()),
        }
    }
}

fn hhmm(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// `23:00` or `8:05` to minutes since midnight.
fn parse_hhmm(text: &str) -> Option<u16> {
    let (h, m) = text.split_once(':')?;
    let (h, m) = (h.trim().parse::<u16>().ok()?, m.trim().parse::<u16>().ok()?);
    (h < 24 && m < 60 && text.len() <= 5).then_some(h * 60 + m)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Handler {
    pub enabled: bool,
    pub trigger: Trigger,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    /// An InstantClone event, optionally narrowed to events whose
    /// variables match (`reason` = `crash`).
    Event {
        kind: EventKind,
        filters: BTreeMap<String, String>,
    },
    /// `!command` in Twitch chat.
    ChatCommand {
        command: String,
        aliases: Vec<String>,
        roles: Roles,
        user_cooldown_ms: u64,
    },
    /// Any chat message that matches.
    ChatMessage {
        pattern: String,
        mode: MatchMode,
        roles: Roles,
    },
    /// Every `every_ms`, optionally only while OBS is streaming.
    Timer { every_ms: u64, only_live: bool },
    /// A call to `/hooks/<token>`, from a Stream Deck, a script or any app.
    Webhook { token: String },
    /// A button: a global hotkey (`Ctrl+Alt+K`), a MIDI pad
    /// (`note:1:36@Device`) on this PC, or a secret `/hooks/<token>`
    /// address a Stream Deck or any program presses. Any may be blank, not
    /// all three. `only_live`: presses while OBS isn't streaming do nothing.
    Shortcut {
        hotkey: String,
        midi: String,
        token: String,
        only_live: bool,
    },
    /// Chat gets busy in a way the rules describe (see `ChatActivity`).
    ChatActivity(ChatActivity),
    /// OBS switches its program scene to one matching `scene` (`*` and `?`
    /// wildcards) but not `except` (blank: none), coming from one matching
    /// `from` (blank: any), and stays there `settle_ms`. `only_live`: only
    /// while OBS is streaming; a stream that starts on a matching scene
    /// counts as switching to it. Needs obs-websocket.
    Scene {
        scene: String,
        from: String,
        except: String,
        settle_ms: u64,
        only_live: bool,
    },
}

/// A chat activity trigger: rules over the last `window_ms` of chat, all of
/// them or any one of them. Only messages from `roles` count.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatActivity {
    pub window_ms: u64,
    pub match_all: bool,
    pub rules: Vec<ActivityRule>,
    pub roles: Roles,
    pub only_live: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityRule {
    pub kind: RuleKind,
    pub value: f64,
    /// For `Words`: what counts, comma separated.
    pub words: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleKind {
    /// At least `value` messages in the window.
    Messages,
    /// At least `value` times the channel's normal speed.
    Busier,
    /// At least `value` different people talking.
    Chatters,
    /// At least `value` percent of messages contain one of `words`.
    Words,
}

impl RuleKind {
    pub fn id(self) -> &'static str {
        match self {
            RuleKind::Messages => "messages",
            RuleKind::Busier => "busier",
            RuleKind::Chatters => "chatters",
            RuleKind::Words => "words",
        }
    }

    fn from_id(id: &str) -> Option<RuleKind> {
        [
            RuleKind::Messages,
            RuleKind::Busier,
            RuleKind::Chatters,
            RuleKind::Words,
        ]
        .into_iter()
        .find(|k| k.id() == id)
    }
}

const MIN_ACTIVITY_WINDOW_MS: u64 = 5_000;
const MAX_ACTIVITY_WINDOW_MS: u64 = 120_000;
const MAX_ACTIVITY_RULES: usize = 8;
/// Longest a scene trigger may wait for the scene to settle.
pub const MAX_SETTLE_MS: u64 = 60_000;

/// Who may fire a chat trigger. The broadcaster always may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Roles {
    pub everyone: bool,
    pub subs: bool,
    pub vips: bool,
    pub mods: bool,
}

impl Roles {
    pub const EVERYONE: Roles = Roles {
        everyone: true,
        subs: true,
        vips: true,
        mods: true,
    };
    pub const MODS: Roles = Roles {
        everyone: false,
        subs: false,
        vips: false,
        mods: true,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchMode {
    Contains,
    StartsWith,
    Exact,
    /// `*` matches any run of characters, `?` one character.
    Wildcard,
}

impl MatchMode {
    fn id(self) -> &'static str {
        match self {
            MatchMode::Contains => "contains",
            MatchMode::StartsWith => "starts_with",
            MatchMode::Exact => "exact",
            MatchMode::Wildcard => "wildcard",
        }
    }

    fn from_id(id: &str) -> MatchMode {
        match id {
            "starts_with" => MatchMode::StartsWith,
            "exact" => MatchMode::Exact,
            "wildcard" => MatchMode::Wildcard,
            _ => MatchMode::Contains,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Discord,
    Chat,
    Phone,
    Http,
    Wait,
    WaitDelay,
    If,
    Stop,
    DelayAction,
    Marker,
    Clip,
    Program,
    File,
    SetVar,
    Counter,
    /// A card on the alerts browser source, on stream.
    Overlay,
    /// Reshape a value (first line, between two texts, round...) into a
    /// new variable.
    EditText,
    /// Add a line, a chapter or a highlight to this stream's timeline.
    Timeline,
    /// Switch a scene, show or hide a source, or set a text source in OBS.
    Obs,
}

impl StepKind {
    pub const ALL: [StepKind; 19] = [
        StepKind::Discord,
        StepKind::Chat,
        StepKind::Phone,
        StepKind::Http,
        StepKind::Wait,
        StepKind::WaitDelay,
        StepKind::If,
        StepKind::Stop,
        StepKind::DelayAction,
        StepKind::Marker,
        StepKind::Clip,
        StepKind::Program,
        StepKind::File,
        StepKind::SetVar,
        StepKind::Counter,
        StepKind::Overlay,
        StepKind::EditText,
        StepKind::Timeline,
        StepKind::Obs,
    ];

    pub fn id(self) -> &'static str {
        match self {
            StepKind::Discord => "discord",
            StepKind::Chat => "chat",
            StepKind::Phone => "phone",
            StepKind::Http => "http",
            StepKind::Wait => "wait",
            StepKind::WaitDelay => "wait_delay",
            StepKind::If => "if",
            StepKind::Stop => "stop",
            StepKind::DelayAction => "delay_action",
            StepKind::Marker => "marker",
            StepKind::Clip => "clip",
            StepKind::Program => "program",
            StepKind::File => "file",
            StepKind::SetVar => "set_var",
            StepKind::Counter => "counter",
            StepKind::Overlay => "overlay",
            StepKind::EditText => "edit_text",
            StepKind::Timeline => "timeline",
            StepKind::Obs => "obs",
        }
    }

    pub fn from_id(id: &str) -> Option<StepKind> {
        StepKind::ALL.into_iter().find(|k| k.id() == id)
    }

    /// Parameters the step cannot run without, each with what to tell
    /// the user when it is blank.
    fn required(self) -> &'static [(&'static str, &'static str)] {
        match self {
            // A card needs any one of its parts: see `validate_steps`.
            StepKind::Discord => &[("connection", "pick a Discord channel")],
            StepKind::Chat => &[("text", "write the chat message")],
            StepKind::Phone => &[("text", "write the phone message")],
            StepKind::Http => &[("url", "add the web address")],
            StepKind::Wait => &[("ms", "set how long to wait")],
            StepKind::If => &[
                ("left", "say what the check looks at"),
                ("op", "pick how the check compares"),
            ],
            StepKind::DelayAction => &[("action", "pick a delay action")],
            StepKind::Program => &[("path", "pick the program to run")],
            StepKind::File => &[("path", "pick the file to write")],
            StepKind::SetVar | StepKind::Counter => &[("name", "name the value")],
            StepKind::Overlay => &[("text", "write what shows on stream")],
            StepKind::EditText => &[("op", "pick how to change the text")],
            StepKind::Timeline => &[("text", "write the timeline line")],
            StepKind::Obs => &[("action", "pick what to do in OBS")],
            StepKind::WaitDelay | StepKind::Stop | StepKind::Marker | StepKind::Clip => &[],
        }
    }

    /// Steps that can touch this PC outside InstantClone. An imported
    /// recipe carrying one needs the user's say-so.
    pub fn is_local_effect(self) -> bool {
        matches!(self, StepKind::Program | StepKind::File)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub kind: StepKind,
    /// A switched-off step is skipped, with everything inside it.
    pub enabled: bool,
    pub params: BTreeMap<String, String>,
    /// For `If`: the steps run when the check passes / fails.
    pub then: Vec<Step>,
    pub otherwise: Vec<Step>,
}

impl Step {
    pub fn new(kind: StepKind, params: &[(&str, &str)]) -> Self {
        Self {
            kind,
            enabled: true,
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            then: Vec::new(),
            otherwise: Vec::new(),
        }
    }

    pub fn param(&self, name: &str) -> &str {
        self.params.get(name).map(String::as_str).unwrap_or("")
    }
}

/// Count steps including nested ones.
pub fn count_steps(steps: &[Step]) -> usize {
    steps
        .iter()
        .map(|s| 1 + count_steps(&s.then) + count_steps(&s.otherwise))
        .sum()
}

/// Walk every step, nested ones included.
pub fn visit_steps<'a>(steps: &'a [Step], f: &mut dyn FnMut(&'a Step)) {
    for step in steps {
        f(step);
        visit_steps(&step.then, f);
        visit_steps(&step.otherwise, f);
    }
}

impl Integration {
    /// Problems that stop it being saved, worded for the dashboard.
    /// Every problem, for switching it on: it must be complete and valid.
    pub fn validate(&self) -> Vec<String> {
        let problems = self.problems();
        let mut all = problems.invalid;
        all.extend(problems.incomplete);
        all
    }

    /// Problems split by what they block. `invalid` ones stop any save;
    /// `incomplete` ones only stop it running, so a switched-off
    /// integration can be saved half-finished as a draft.
    pub fn problems(&self) -> Problems {
        let mut errors = Vec::new();
        let mut incomplete = Vec::new();
        if !valid_id(&self.id) {
            errors.push("invalid id".to_string());
        }
        if self.name.trim().is_empty() {
            errors.push("give it a name".to_string());
        }
        if self.name.chars().count() > MAX_NAME_LEN {
            errors.push(format!("the name is longer than {MAX_NAME_LEN} characters"));
        }
        if self.handlers.is_empty() {
            incomplete.push("add at least one trigger".to_string());
        }
        if self.handlers.len() > MAX_HANDLERS {
            errors.push(format!("more than {MAX_HANDLERS} triggers"));
        }
        let total: usize = self.handlers.iter().map(|h| count_steps(&h.steps)).sum();
        if total > MAX_STEPS {
            errors.push(format!("more than {MAX_STEPS} steps"));
        }
        for handler in &self.handlers {
            // A switched-off trigger can stay half set up: it never runs, so
            // only what makes it unsafe to save counts.
            let mut unused = Vec::new();
            let missing = if handler.enabled {
                &mut incomplete
            } else {
                &mut unused
            };
            validate_trigger(&handler.trigger, missing);
            validate_steps(&handler.steps, 1, &mut errors, missing);
        }
        errors.dedup();
        incomplete.dedup();
        Problems {
            invalid: errors,
            incomplete,
        }
    }

    /// Whether any step runs a program or writes a file.
    pub fn has_local_effects(&self) -> bool {
        let mut found = false;
        for handler in &self.handlers {
            visit_steps(&handler.steps, &mut |s| found |= s.kind.is_local_effect());
        }
        found
    }

    pub fn to_json(&self) -> Value {
        json::obj([
            ("id", json::str(&self.id)),
            ("name", json::str(&self.name)),
            ("enabled", Value::Bool(self.enabled)),
            ("preset", json::str(&self.preset)),
            ("cooldown_ms", Value::Num(self.cooldown_ms as f64)),
            ("quiet", self.quiet.map_or(Value::Null, QuietHours::to_json)),
            (
                "handlers",
                Value::Arr(self.handlers.iter().map(handler_json).collect()),
            ),
        ])
    }

    /// Read an integration from its JSON form. Unknown step or trigger
    /// types are errors, so nothing is silently dropped on save.
    pub fn from_json(v: &Value) -> Result<Integration, String> {
        let handlers = v
            .get("handlers")
            .and_then(Value::as_array)
            .unwrap_or(&[])
            .iter()
            .map(handler_from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Integration {
            id: v.str_or("id", "").to_string(),
            name: v.str_or("name", "").trim().to_string(),
            enabled: v.bool_or("enabled", true),
            preset: v.str_or("preset", "").to_string(),
            cooldown_ms: v.u64_or("cooldown_ms", 0),
            quiet: QuietHours::from_json(v.get("quiet"))?,
            handlers,
        })
    }
}

/// The hotkeys and MIDI pads that switched-on integrations listen for,
/// each once.
pub fn shortcuts(integrations: &[Integration]) -> (Vec<String>, Vec<String>) {
    let (mut keys, mut pads) = (Vec::new(), Vec::new());
    let live = integrations
        .iter()
        .filter(|i| i.enabled)
        .flat_map(|i| i.handlers.iter().filter(|h| h.enabled));
    for handler in live {
        if let Trigger::Shortcut { hotkey, midi, .. } = &handler.trigger {
            if !hotkey.is_empty() && !keys.contains(hotkey) {
                keys.push(hotkey.clone());
            }
            if !midi.is_empty() && !pads.contains(midi) {
                pads.push(midi.clone());
            }
        }
    }
    (keys, pads)
}

/// Whether a pad a trigger is bound to is the one pressed. The press always
/// names its device; a binding that names none matches that control on any
/// device (the same rule as the delay actions' MIDI bindings).
pub fn pad_matches(bound: &str, pressed: &str) -> bool {
    !bound.is_empty() && (bound == pressed || pressed.split('@').next() == Some(bound))
}

/// Whether one press could set off both pad bindings: the same control,
/// on the same device or with either one bound to any device.
pub fn pads_overlap(a: &str, b: &str) -> bool {
    let (control_a, device_a) = a.split_once('@').map_or((a, None), |(c, d)| (c, Some(d)));
    let (control_b, device_b) = b.split_once('@').map_or((b, None), |(c, d)| (c, Some(d)));
    !a.is_empty()
        && control_a == control_b
        && (device_a.is_none() || device_b.is_none() || device_a == device_b)
}

/// The switched-on integration that already starts from `hotkey` or `pad`
/// (either may be empty), by name.
pub fn shortcut_owner<'a>(
    integrations: &'a [Integration],
    hotkey: &str,
    pad: &str,
) -> Option<&'a str> {
    integrations.iter().filter(|i| i.enabled).find_map(|i| {
        let owns = i
            .handlers
            .iter()
            .filter(|h| h.enabled)
            .any(|h| match &h.trigger {
                Trigger::Shortcut {
                    hotkey: key, midi, ..
                } => (!hotkey.is_empty() && key == hotkey) || pads_overlap(midi, pad),
                _ => false,
            });
        owns.then_some(i.name.as_str())
    })
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn validate_trigger(trigger: &Trigger, errors: &mut Vec<String>) {
    match trigger {
        Trigger::ChatCommand {
            command, aliases, ..
        } => {
            for c in std::iter::once(command).chain(aliases) {
                if !valid_command(c) {
                    errors.push(format!(
                        "\"{c}\" is not a chat command: start it with ! and use no spaces"
                    ));
                }
            }
        }
        Trigger::ChatMessage { pattern, .. } if pattern.trim().is_empty() => {
            errors.push("the chat message trigger needs something to match".to_string());
        }
        Trigger::Timer { every_ms, .. } if *every_ms < 60_000 => {
            errors.push("timers run at most once a minute".to_string());
        }
        Trigger::Webhook { token } if token.len() < 16 || !valid_id(token) => {
            errors.push("the web call link is invalid; make a new one".to_string());
        }
        Trigger::Shortcut {
            hotkey,
            midi,
            token,
            ..
        } => {
            if hotkey.is_empty() && midi.is_empty() && token.is_empty() {
                errors.push(
                    "set a hotkey, a MIDI pad or a Stream Deck address for the button".to_string(),
                );
            }
            if !token.is_empty() && (token.len() < 16 || !valid_id(token)) {
                errors.push("the button's web address is invalid; make a new one".to_string());
            }
            if !hotkey.is_empty()
                && crate::config::canonicalize_hotkey(hotkey).as_ref() != Some(hotkey)
            {
                errors.push(format!(
                    "\"{hotkey}\" isn't a hotkey: hold Ctrl, Alt, Shift or Win and press one key"
                ));
            }
            if !midi.is_empty() && crate::config::canonicalize_midi(midi).as_ref() != Some(midi) {
                errors.push("that MIDI pad isn't valid; learn it again".to_string());
            }
        }
        Trigger::ChatActivity(activity) => validate_activity(activity, errors),
        Trigger::Scene {
            scene, settle_ms, ..
        } => {
            if scene.trim().is_empty() {
                errors.push("pick the OBS scene it reacts to".to_string());
            }
            if *settle_ms > MAX_SETTLE_MS {
                errors.push("a scene can settle for at most a minute".to_string());
            }
        }
        _ => {}
    }
}

fn validate_activity(activity: &ChatActivity, errors: &mut Vec<String>) {
    if !(MIN_ACTIVITY_WINDOW_MS..=MAX_ACTIVITY_WINDOW_MS).contains(&activity.window_ms) {
        errors.push("watch chat for between 5 seconds and 2 minutes".to_string());
    }
    if activity.rules.is_empty() {
        errors.push("add at least one chat rule".to_string());
    }
    if activity.rules.len() > MAX_ACTIVITY_RULES {
        errors.push(format!("at most {MAX_ACTIVITY_RULES} chat rules"));
    }
    for rule in &activity.rules {
        if !(rule.value.is_finite() && rule.value > 0.0) {
            errors.push("every chat rule needs a number above 0".to_string());
        }
        if rule.kind == RuleKind::Words && rule.words.trim().is_empty() {
            errors.push("the words rule needs words to look for".to_string());
        }
        if rule.kind == RuleKind::Words && rule.value > 100.0 {
            errors.push("the words rule is a percentage, 100 at most".to_string());
        }
    }
}

pub fn valid_command(c: &str) -> bool {
    c.len() >= 2
        && c.len() <= 32
        && c.starts_with('!')
        && !c[1..].contains(|ch: char| ch.is_whitespace() || ch == '!')
}

fn validate_steps(
    steps: &[Step],
    depth: usize,
    errors: &mut Vec<String>,
    incomplete: &mut Vec<String>,
) {
    if depth > MAX_DEPTH {
        errors.push(format!("checks nest more than {MAX_DEPTH} deep"));
        return;
    }
    for step in steps {
        // A switched-off step never runs: half-finished is fine. What would
        // make it unsafe (below) still counts, as it can be switched on.
        let mut unused = Vec::new();
        let missing = if step.enabled {
            &mut *incomplete
        } else {
            &mut unused
        };
        for (name, ask) in step.kind.required() {
            if step.param(name).trim().is_empty() {
                missing.push(ask.to_string());
            }
        }
        if step.kind == StepKind::Discord {
            if let Some(ask) = discord_message_missing(step) {
                missing.push(ask.to_string());
            }
        }
        let ms = step.param("ms").trim();
        if step.kind == StepKind::Wait
            && !ms.is_empty()
            && !ms.contains('{')
            && ms.parse::<u64>().is_err()
        {
            missing.push("set how long to wait, in milliseconds".to_string());
        }
        if step.kind == StepKind::Obs {
            if let Some(ask) = obs_step_missing(step) {
                missing.push(ask.to_string());
            }
            // Like a program's path: chat must never pick what OBS shows.
            if step.param("scene").contains('{') || step.param("source").contains('{') {
                errors.push("an OBS step's scene and source can't use variables".to_string());
            }
        }
        // Where a program runs, a file is written or a request goes must be
        // fixed by the streamer, never filled in from chat or a web call:
        // otherwise a viewer's message could pick the program or the host.
        if matches!(step.kind, StepKind::Program | StepKind::File)
            && step.param("path").contains('{')
        {
            errors.push(format!(
                "a {} step's path can't use variables",
                step.kind.id()
            ));
        }
        if step.kind == StepKind::Http && !host_is_fixed(step.param("url")) {
            errors.push("a web request's address can't put variables in its host".to_string());
        }
        // A card's link and pictures are fetched and shown by Discord:
        // chat must not pick the site they come from.
        if step.kind == StepKind::Discord
            && ["url", "image", "thumbnail"]
                .iter()
                .any(|name| !host_is_fixed(step.param(name)))
        {
            errors.push(
                "a Discord card's link and images can't put variables in their site".to_string(),
            );
        }
        // The value's name is fixed: a viewer must not overwrite `{user}`.
        let named = match step.kind {
            StepKind::SetVar => step.param("name"),
            StepKind::EditText => step.param("save_as"),
            _ => "",
        };
        if named.contains('{') {
            errors.push("a value's name can't use variables".to_string());
        }
        if step.params.values().any(|v| v.len() > MAX_PARAM_LEN) {
            errors.push(format!(
                "a {} step has a value that is too long",
                step.kind.id()
            ));
        }
        if step.kind != StepKind::If && !(step.then.is_empty() && step.otherwise.is_empty()) {
            errors.push(format!("a {} step cannot hold other steps", step.kind.id()));
        }
        validate_steps(&step.then, depth + 1, errors, missing);
        validate_steps(&step.otherwise, depth + 1, errors, missing);
    }
}

/// What a Discord step still needs to say anything. A plain message needs
/// its text; a card needs any one of its parts, since a title or fields
/// alone make a whole card.
fn discord_message_missing(step: &Step) -> Option<&'static str> {
    let has = |name: &str| !step.param(name).trim().is_empty();
    if step.param("style") != "card" {
        return (!has("text")).then_some("write the Discord message");
    }
    let any = ["title", "text", "fields", "image", "above"]
        .iter()
        .any(|name| has(name));
    (!any).then_some("give the Discord card a title or some text")
}

/// The OBS actions a step can take, with what each needs.
pub const OBS_ACTIONS: [&str; 5] = ["scene", "show", "hide", "toggle", "text"];

fn obs_step_missing(step: &Step) -> Option<&'static str> {
    let blank = |name: &str| step.param(name).trim().is_empty();
    match step.param("action") {
        "" => None, // `required` asks for it
        "scene" if blank("scene") => Some("pick the OBS scene to switch to"),
        "show" | "hide" | "toggle" | "text" if blank("source") => Some("pick the OBS source"),
        action if !OBS_ACTIONS.contains(&action) => Some("pick what to do in OBS"),
        _ => None,
    }
}

/// True when the scheme and host of a URL template contain no variable
/// (the path and query may).
fn host_is_fixed(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let scheme = url.split_once("://").map_or("", |(s, _)| s);
    !host.contains('{') && !scheme.contains('{')
}

/// See `Integration::problems`.
pub struct Problems {
    pub invalid: Vec<String>,
    pub incomplete: Vec<String>,
}

fn roles_json(r: Roles) -> Value {
    json::obj([
        ("everyone", Value::Bool(r.everyone)),
        ("subs", Value::Bool(r.subs)),
        ("vips", Value::Bool(r.vips)),
        ("mods", Value::Bool(r.mods)),
    ])
}

fn roles_from_json(v: Option<&Value>) -> Roles {
    match v {
        Some(v) => Roles {
            everyone: v.bool_or("everyone", false),
            subs: v.bool_or("subs", false),
            vips: v.bool_or("vips", false),
            mods: v.bool_or("mods", false),
        },
        None => Roles::EVERYONE,
    }
}

fn string_map_json(map: &BTreeMap<String, String>) -> Value {
    Value::Obj(map.iter().map(|(k, v)| (k.clone(), json::str(v))).collect())
}

fn string_map_from_json(v: Option<&Value>) -> BTreeMap<String, String> {
    match v {
        Some(Value::Obj(fields)) => fields
            .iter()
            .map(|(k, v)| (k.clone(), v.to_display()))
            .collect(),
        _ => BTreeMap::new(),
    }
}

fn trigger_json(t: &Trigger) -> Value {
    match t {
        Trigger::Event { kind, filters } => json::obj([
            ("type", json::str("event")),
            ("event", json::str(kind.id())),
            ("filters", string_map_json(filters)),
        ]),
        Trigger::ChatCommand {
            command,
            aliases,
            roles,
            user_cooldown_ms,
        } => json::obj([
            ("type", json::str("chat_command")),
            ("command", json::str(command)),
            (
                "aliases",
                Value::Arr(aliases.iter().map(json::str).collect()),
            ),
            ("roles", roles_json(*roles)),
            ("user_cooldown_ms", Value::Num(*user_cooldown_ms as f64)),
        ]),
        Trigger::ChatMessage {
            pattern,
            mode,
            roles,
        } => json::obj([
            ("type", json::str("chat_message")),
            ("pattern", json::str(pattern)),
            ("mode", json::str(mode.id())),
            ("roles", roles_json(*roles)),
        ]),
        Trigger::Timer {
            every_ms,
            only_live,
        } => json::obj([
            ("type", json::str("timer")),
            ("every_ms", Value::Num(*every_ms as f64)),
            ("only_live", Value::Bool(*only_live)),
        ]),
        Trigger::Webhook { token } => {
            json::obj([("type", json::str("webhook")), ("token", json::str(token))])
        }
        Trigger::Shortcut {
            hotkey,
            midi,
            token,
            only_live,
        } => json::obj([
            ("type", json::str("shortcut")),
            ("hotkey", json::str(hotkey)),
            ("midi", json::str(midi)),
            ("token", json::str(token)),
            ("only_live", Value::Bool(*only_live)),
        ]),
        Trigger::ChatActivity(a) => json::obj([
            ("type", json::str("chat_activity")),
            ("window_ms", Value::Num(a.window_ms as f64)),
            ("match", json::str(if a.match_all { "all" } else { "any" })),
            (
                "rules",
                Value::Arr(
                    a.rules
                        .iter()
                        .map(|r| {
                            json::obj([
                                ("kind", json::str(r.kind.id())),
                                ("value", Value::Num(r.value)),
                                ("words", json::str(&r.words)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("roles", roles_json(a.roles)),
            ("only_live", Value::Bool(a.only_live)),
        ]),
        Trigger::Scene {
            scene,
            from,
            except,
            settle_ms,
            only_live,
        } => json::obj([
            ("type", json::str("scene")),
            ("scene", json::str(scene)),
            ("from", json::str(from)),
            ("except", json::str(except)),
            ("settle_ms", Value::Num(*settle_ms as f64)),
            ("only_live", Value::Bool(*only_live)),
        ]),
    }
}

fn activity_from_json(v: &Value) -> Result<ChatActivity, String> {
    let rules = v
        .get("rules")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .map(|r| {
            let id = r.str_or("kind", "");
            Ok(ActivityRule {
                kind: RuleKind::from_id(id).ok_or_else(|| format!("unknown chat rule \"{id}\""))?,
                value: r.get("value").and_then(Value::as_f64).unwrap_or(0.0),
                words: r.str_or("words", "").to_string(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ChatActivity {
        window_ms: v.u64_or("window_ms", 10_000),
        match_all: v.str_or("match", "all") != "any",
        rules,
        roles: roles_from_json(v.get("roles")),
        only_live: v.bool_or("only_live", true),
    })
}

fn trigger_from_json(v: &Value) -> Result<Trigger, String> {
    match v.str_or("type", "") {
        "event" => {
            let id = v.str_or("event", "");
            let kind = EventKind::from_id(id).ok_or_else(|| format!("unknown event \"{id}\""))?;
            Ok(Trigger::Event {
                kind,
                filters: string_map_from_json(v.get("filters")),
            })
        }
        "chat_command" => Ok(Trigger::ChatCommand {
            command: v.str_or("command", "").trim().to_lowercase(),
            aliases: v
                .get("aliases")
                .and_then(Value::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(Value::as_str)
                .map(|a| a.trim().to_lowercase())
                .filter(|a| !a.is_empty())
                .collect(),
            roles: roles_from_json(v.get("roles")),
            user_cooldown_ms: v.u64_or("user_cooldown_ms", 0),
        }),
        "chat_message" => Ok(Trigger::ChatMessage {
            pattern: v.str_or("pattern", "").to_string(),
            mode: MatchMode::from_id(v.str_or("mode", "")),
            roles: roles_from_json(v.get("roles")),
        }),
        "timer" => Ok(Trigger::Timer {
            every_ms: v.u64_or("every_ms", 0),
            only_live: v.bool_or("only_live", true),
        }),
        "webhook" => Ok(Trigger::Webhook {
            token: v.str_or("token", "").to_string(),
        }),
        "shortcut" => Ok(Trigger::Shortcut {
            hotkey: v.str_or("hotkey", "").trim().to_string(),
            midi: v.str_or("midi", "").trim().to_string(),
            token: v.str_or("token", "").trim().to_string(),
            only_live: v.bool_or("only_live", false),
        }),
        "chat_activity" => activity_from_json(v).map(Trigger::ChatActivity),
        "scene" => Ok(Trigger::Scene {
            scene: v.str_or("scene", "").trim().to_string(),
            from: v.str_or("from", "").trim().to_string(),
            except: v.str_or("except", "").trim().to_string(),
            settle_ms: v.u64_or("settle_ms", 3_000),
            only_live: v.bool_or("only_live", false),
        }),
        other => Err(format!("unknown trigger \"{other}\"")),
    }
}

impl Handler {
    pub fn to_json(&self) -> Value {
        handler_json(self)
    }
}

fn handler_json(h: &Handler) -> Value {
    json::obj([
        ("enabled", Value::Bool(h.enabled)),
        ("trigger", trigger_json(&h.trigger)),
        ("steps", steps_json(&h.steps)),
    ])
}

fn handler_from_json(v: &Value) -> Result<Handler, String> {
    let trigger = v
        .get("trigger")
        .ok_or("a trigger is missing")
        .map_err(str::to_string)
        .and_then(trigger_from_json)?;
    Ok(Handler {
        enabled: v.bool_or("enabled", true),
        trigger,
        steps: steps_from_json(v.get("steps"), 0)?,
    })
}

fn steps_json(steps: &[Step]) -> Value {
    Value::Arr(
        steps
            .iter()
            .map(|s| {
                let mut fields = vec![
                    ("type".to_string(), json::str(s.kind.id())),
                    ("params".to_string(), string_map_json(&s.params)),
                ];
                // Only written when off: most steps are on.
                if !s.enabled {
                    fields.push(("enabled".to_string(), Value::Bool(false)));
                }
                if s.kind == StepKind::If {
                    fields.push(("then".to_string(), steps_json(&s.then)));
                    fields.push(("else".to_string(), steps_json(&s.otherwise)));
                }
                Value::Obj(fields)
            })
            .collect(),
    )
}

fn steps_from_json(v: Option<&Value>, depth: usize) -> Result<Vec<Step>, String> {
    if depth > MAX_DEPTH {
        return Err(format!("checks nest more than {MAX_DEPTH} deep"));
    }
    v.and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .map(|s| {
            let id = s.str_or("type", "");
            let kind = StepKind::from_id(id).ok_or_else(|| format!("unknown step \"{id}\""))?;
            Ok(Step {
                kind,
                enabled: s.bool_or("enabled", true),
                params: string_map_from_json(s.get("params")),
                then: steps_from_json(s.get("then"), depth + 1)?,
                otherwise: steps_from_json(s.get("else"), depth + 1)?,
            })
        })
        .collect()
}

/// A saved Discord webhook the integrations can post to.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscordChannel {
    pub id: String,
    pub name: String,
    pub url: String,
}

/// Phone pushes through ntfy (a topic on ntfy.sh or a self-hosted server).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhoneConnection {
    pub server: String,
    pub topic: String,
}

pub const DEFAULT_NTFY_SERVER: &str = "https://ntfy.sh";

impl PhoneConnection {
    pub fn is_set(&self) -> bool {
        !self.topic.trim().is_empty()
    }

    pub fn server_or_default(&self) -> &str {
        match self.server.trim() {
            "" => DEFAULT_NTFY_SERVER,
            s => s.trim_end_matches('/'),
        }
    }
}

pub fn valid_discord_webhook(url: &str) -> bool {
    let url = url.trim();
    [
        "https://discord.com/api/webhooks/",
        "https://discordapp.com/api/webhooks/",
        "https://ptb.discord.com/api/webhooks/",
        "https://canary.discord.com/api/webhooks/",
    ]
    .iter()
    .any(|p| url.starts_with(p) && url.len() > p.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Integration {
        let mut check = Step::new(
            StepKind::If,
            &[("left", "{hold_active}"), ("op", "is"), ("right", "yes")],
        );
        check.then.push(Step::new(
            StepKind::Chat,
            &[("text", "OBS dropped, back soon")],
        ));
        check.otherwise.push(Step::new(StepKind::Stop, &[]));
        Integration {
            id: "tell-chat".into(),
            name: "Tell chat".into(),
            enabled: true,
            preset: "tell_chat".into(),
            cooldown_ms: 60_000,
            quiet: Some(QuietHours {
                from: 23 * 60,
                to: 8 * 60,
            }),
            handlers: vec![
                Handler {
                    enabled: true,
                    trigger: Trigger::Event {
                        kind: EventKind::HoldOpened,
                        filters: BTreeMap::from([("reason".to_string(), "crash".to_string())]),
                    },
                    steps: vec![Step::new(StepKind::Wait, &[("ms", "20000")]), check],
                },
                Handler {
                    enabled: false,
                    trigger: Trigger::ChatCommand {
                        command: "!clip".into(),
                        aliases: vec!["!c".into()],
                        roles: Roles::MODS,
                        user_cooldown_ms: 30_000,
                    },
                    steps: vec![Step::new(StepKind::Clip, &[])],
                },
                Handler {
                    enabled: true,
                    trigger: Trigger::Shortcut {
                        hotkey: "Ctrl+Alt+K".into(),
                        midi: "note:1:36@Launchpad".into(),
                        token: String::new(),
                        only_live: false,
                    },
                    steps: vec![Step::new(StepKind::Overlay, &[("text", "Clipped!")])],
                },
            ],
        }
    }

    #[test]
    fn json_round_trip_is_lossless() {
        let original = sample();
        let text = original.to_json().to_json();
        assert!(crate::config::is_valid_json(&text));
        let back = Integration::from_json(&json::parse(&text).unwrap()).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn valid_sample_passes_validation() {
        assert!(sample().validate().is_empty(), "{:?}", sample().validate());
    }

    #[test]
    fn validation_names_the_problem() {
        let mut bad = sample();
        bad.name = " ".into();
        bad.handlers[0]
            .steps
            .push(Step::new(StepKind::Discord, &[("text", "hi")]));
        bad.handlers[1].trigger = Trigger::ChatCommand {
            command: "clip".into(),
            aliases: vec![],
            roles: Roles::EVERYONE,
            user_cooldown_ms: 0,
        };
        bad.handlers[1].enabled = true;
        let errors = bad.validate();
        assert!(errors.iter().any(|e| e.contains("name")));
        assert!(errors.iter().any(|e| e == "pick a Discord channel"));
        assert!(errors.iter().any(|e| e.contains("\"clip\"")));
        // Switched off, a half-set trigger doesn't stop it running.
        bad.handlers[1].enabled = false;
        assert!(!bad.validate().iter().any(|e| e.contains("\"clip\"")));
    }

    #[test]
    fn a_card_needs_any_part_and_a_plain_message_its_text() {
        let mut i = sample();
        let card = Step::new(
            StepKind::Discord,
            &[("connection", "c"), ("style", "card"), ("title", "Live")],
        );
        i.handlers[0].steps = vec![card];
        assert!(i.validate().is_empty(), "{:?}", i.validate());
        i.handlers[0].steps[0].params.remove("title");
        assert!(i
            .validate()
            .iter()
            .any(|e| e.contains("title or some text")));
        let mut off = Step::new(StepKind::Discord, &[("connection", "c")]);
        off.enabled = false;
        i.handlers[0].steps = vec![off];
        assert!(i.validate().is_empty(), "a switched-off step can wait");
    }

    #[test]
    fn nesting_is_capped() {
        let mut deepest = Step::new(StepKind::Stop, &[]);
        for _ in 0..MAX_DEPTH + 1 {
            let mut wrap = Step::new(StepKind::If, &[("left", "a"), ("op", "is")]);
            wrap.then.push(deepest);
            deepest = wrap;
        }
        let mut i = sample();
        i.handlers[0].steps = vec![deepest];
        assert!(i.validate().iter().any(|e| e.contains("nest")));
    }

    #[test]
    fn unknown_types_are_errors_not_silently_dropped() {
        let v = json::parse(r#"{"id":"x","name":"x","handlers":[{"trigger":{"type":"event","event":"nope"},"steps":[]}]}"#).unwrap();
        assert!(Integration::from_json(&v).is_err());
        let v = json::parse(r#"{"id":"x","name":"x","handlers":[{"trigger":{"type":"timer","every_ms":60000},"steps":[{"type":"teleport"}]}]}"#).unwrap();
        assert!(Integration::from_json(&v).is_err());
    }

    #[test]
    fn local_effects_are_detected_when_nested() {
        let mut i = sample();
        assert!(!i.has_local_effects());
        i.handlers[0].steps[1]
            .otherwise
            .push(Step::new(StepKind::Program, &[("path", "calc.exe")]));
        assert!(i.has_local_effects());
    }

    #[test]
    fn viewers_can_never_choose_a_program_file_or_host() {
        let mut i = sample();
        i.handlers[0].steps = vec![Step::new(StepKind::Program, &[("path", "{message}")])];
        assert!(!i.problems().invalid.is_empty());
        i.handlers[0].steps = vec![Step::new(
            StepKind::File,
            &[("path", "C:/{arg1}.txt"), ("text", "x")],
        )];
        assert!(!i.problems().invalid.is_empty());
        i.handlers[0].steps = vec![Step::new(StepKind::Http, &[("url", "https://{arg1}/x")])];
        assert!(!i.problems().invalid.is_empty());
        i.handlers[0].steps = vec![Step::new(
            StepKind::Http,
            &[("url", "https://api.example/{arg1}?q={user}")],
        )];
        assert!(
            i.problems().invalid.is_empty(),
            "{:?}",
            i.problems().invalid
        );
    }

    #[test]
    fn drafts_are_incomplete_not_invalid() {
        let mut i = sample();
        i.handlers[0].steps = vec![Step::new(StepKind::Http, &[("url", "")])];
        let p = i.problems();
        assert!(p.invalid.is_empty());
        assert!(!p.incomplete.is_empty());
    }

    #[test]
    fn quiet_hours_can_span_midnight() {
        let night = QuietHours {
            from: 23 * 60,
            to: 8 * 60,
        };
        assert!(night.contains(23 * 60));
        assert!(night.contains(2 * 60));
        assert!(!night.contains(8 * 60));
        assert!(!night.contains(12 * 60));
        let lunch = QuietHours {
            from: 12 * 60,
            to: 13 * 60,
        };
        assert!(lunch.contains(12 * 60 + 30));
        assert!(!lunch.contains(13 * 60));
    }

    #[test]
    fn quiet_hours_read_clock_times_and_refuse_nonsense() {
        let read = |text: &str| QuietHours::from_json(Some(&json::parse(text).unwrap()));
        assert_eq!(
            read(r#"{"from":"23:00","to":"8:05"}"#),
            Ok(Some(QuietHours {
                from: 23 * 60,
                to: 8 * 60 + 5
            }))
        );
        assert_eq!(read(r#"{"from":"","to":""}"#), Ok(None));
        assert_eq!(QuietHours::from_json(None), Ok(None));
        assert!(read(r#"{"from":"25:00","to":"08:00"}"#).is_err());
        assert!(read(r#"{"from":"10:00","to":"10:00"}"#).is_err());
        assert!(read(r#"{"from":"10:00","to":""}"#).is_err());
    }

    #[test]
    fn pads_match_on_their_device_or_any() {
        assert!(pad_matches("note:1:36@Deck A", "note:1:36@Deck A"));
        assert!(!pad_matches("note:1:36@Deck A", "note:1:36@Deck B"));
        assert!(pad_matches("note:1:36", "note:1:36@Deck B"));
        assert!(!pad_matches("note:1:37", "note:1:36@Deck B"));
        assert!(!pad_matches("", "note:1:36@Deck B"));
    }

    #[test]
    fn pads_overlap_when_one_press_sets_off_both() {
        assert!(pads_overlap("note:1:36@Deck A", "note:1:36@Deck A"));
        assert!(pads_overlap("note:1:36", "note:1:36@Deck B"), "any device");
        assert!(
            pads_overlap("note:1:36@Deck B", "note:1:36"),
            "either way round"
        );
        assert!(!pads_overlap("note:1:36@Deck A", "note:1:36@Deck B"));
        assert!(!pads_overlap("note:1:36", "cc:1:36"));
        assert!(!pads_overlap("", ""));
    }

    #[test]
    fn shortcuts_need_a_real_key_or_pad() {
        let mut i = sample();
        let at = i.handlers.len() - 1;
        let button = |hotkey: &str, midi: &str, token: &str| Trigger::Shortcut {
            hotkey: hotkey.into(),
            midi: midi.into(),
            token: token.into(),
            only_live: false,
        };
        i.handlers[at].trigger = button("", "", "");
        assert!(i.validate().iter().any(|e| e.contains("set a hotkey")));
        i.handlers[at].trigger = button("K", "", "");
        assert!(i.validate().iter().any(|e| e.contains("isn't a hotkey")));
        i.handlers[at].trigger = button("", "note:1:36", "");
        assert!(i.validate().is_empty(), "{:?}", i.validate());
        i.handlers[at].trigger = button("", "", "0123456789abcdef0123");
        assert!(i.validate().is_empty(), "a web address alone is a button");
        i.handlers[at].trigger = button("", "", "short");
        assert!(i.validate().iter().any(|e| e.contains("web address")));
    }

    #[test]
    fn chat_activity_and_scene_triggers_round_trip_and_validate() {
        let mut i = sample();
        i.handlers[0].trigger = Trigger::ChatActivity(ChatActivity {
            window_ms: 10_000,
            match_all: false,
            rules: vec![
                ActivityRule {
                    kind: RuleKind::Busier,
                    value: 3.0,
                    words: String::new(),
                },
                ActivityRule {
                    kind: RuleKind::Words,
                    value: 40.0,
                    words: "clip, pog".into(),
                },
            ],
            roles: Roles::EVERYONE,
            only_live: true,
        });
        i.handlers[1].trigger = Trigger::Scene {
            scene: "Game*".into(),
            from: String::new(),
            except: "Game over".into(),
            settle_ms: 3_000,
            only_live: true,
        };
        assert!(i.validate().is_empty(), "{:?}", i.validate());
        let back = Integration::from_json(&json::parse(&i.to_json().to_json()).unwrap()).unwrap();
        assert_eq!(back, i);
    }

    #[test]
    fn chat_activity_rules_are_checked() {
        let mut i = sample();
        let mut activity = ChatActivity {
            window_ms: 1_000,
            match_all: true,
            rules: vec![ActivityRule {
                kind: RuleKind::Words,
                value: 150.0,
                words: " ".into(),
            }],
            roles: Roles::EVERYONE,
            only_live: true,
        };
        i.handlers[0].trigger = Trigger::ChatActivity(activity.clone());
        let errors = i.validate();
        assert!(errors.iter().any(|e| e.contains("5 seconds")));
        assert!(errors.iter().any(|e| e.contains("words to look for")));
        assert!(errors.iter().any(|e| e.contains("percentage")));
        activity.rules.clear();
        i.handlers[0].trigger = Trigger::ChatActivity(activity);
        assert!(i
            .validate()
            .iter()
            .any(|e| e.contains("at least one chat rule")));
    }

    #[test]
    fn only_live_shortcuts_are_listened_for() {
        let mut i = sample();
        let (keys, pads) = shortcuts(std::slice::from_ref(&i));
        assert_eq!(keys, ["Ctrl+Alt+K"]);
        assert_eq!(pads, ["note:1:36@Launchpad"]);
        i.enabled = false;
        assert_eq!(shortcuts(&[i]), (vec![], vec![]));
    }

    #[test]
    fn commands_need_a_bang_and_no_spaces() {
        assert!(valid_command("!delay"));
        assert!(!valid_command("delay"));
        assert!(!valid_command("!"));
        assert!(!valid_command("!two words"));
    }

    #[test]
    fn discord_webhook_urls_are_recognised() {
        assert!(valid_discord_webhook(
            "https://discord.com/api/webhooks/1/abc"
        ));
        assert!(!valid_discord_webhook(
            "https://evil.example/api/webhooks/1"
        ));
        assert!(!valid_discord_webhook("https://discord.com/api/webhooks/"));
    }
}
