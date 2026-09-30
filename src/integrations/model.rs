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
pub const MAX_HANDLERS: usize = 16;
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
    pub handlers: Vec<Handler>,
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
}

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
}

impl StepKind {
    pub const ALL: [StepKind; 15] = [
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
        }
    }

    pub fn from_id(id: &str) -> Option<StepKind> {
        StepKind::ALL.into_iter().find(|k| k.id() == id)
    }

    /// Parameters the step cannot run without, each with what to tell
    /// the user when it is blank.
    fn required(self) -> &'static [(&'static str, &'static str)] {
        match self {
            StepKind::Discord => &[
                ("connection", "pick a Discord channel"),
                ("text", "write the Discord message"),
            ],
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
    pub params: BTreeMap<String, String>,
    /// For `If`: the steps run when the check passes / fails.
    pub then: Vec<Step>,
    pub otherwise: Vec<Step>,
}

impl Step {
    pub fn new(kind: StepKind, params: &[(&str, &str)]) -> Self {
        Self {
            kind,
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
            validate_trigger(&handler.trigger, &mut incomplete);
            validate_steps(&handler.steps, 1, &mut errors, &mut incomplete);
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
            handlers,
        })
    }
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
        _ => {}
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
        for (name, ask) in step.kind.required() {
            if step.param(name).trim().is_empty() {
                incomplete.push(ask.to_string());
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
        if step.params.values().any(|v| v.len() > MAX_PARAM_LEN) {
            errors.push(format!(
                "a {} step has a value that is too long",
                step.kind.id()
            ));
        }
        if step.kind != StepKind::If && !(step.then.is_empty() && step.otherwise.is_empty()) {
            errors.push(format!("a {} step cannot hold other steps", step.kind.id()));
        }
        validate_steps(&step.then, depth + 1, errors, incomplete);
        validate_steps(&step.otherwise, depth + 1, errors, incomplete);
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
    }
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
        other => Err(format!("unknown trigger \"{other}\"")),
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
        let errors = bad.validate();
        assert!(errors.iter().any(|e| e.contains("name")));
        assert!(errors.iter().any(|e| e == "pick a Discord channel"));
        assert!(errors.iter().any(|e| e.contains("\"clip\"")));
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
