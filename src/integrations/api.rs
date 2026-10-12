//! The dashboard's integrations API. Every route here is admin-only (the
//! web layer's access gate defaults to that) except `/hooks/<token>`, which
//! is how outside apps trigger a "web call" integration and is guarded by
//! its secret token instead.
//!
//! Requests and answers are JSON. Settings writes follow the web layer's
//! pattern: take the write lock, clone, change, validate, save, publish.

use super::event;
use super::model::{
    valid_discord_webhook, valid_id, DiscordChannel, Integration, PhoneConnection, Roles, Trigger,
    UserValue, MAX_INTEGRATIONS, MAX_USER_VALUES,
};
use super::twitch::helix::{RewardEdit, MAX_REWARD_PROMPT, MAX_REWARD_TITLE};
use super::{alert_media, alerts, engine, lights, obsws, presets, recipe, runner, twitch::Which};
use crate::config::Settings;
use crate::controller::Controller;
use crate::json::{self, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::watch;

pub type Reply = (&'static str, &'static str, String);

fn ok(v: Value) -> Reply {
    ("200 OK", "application/json", v.to_json())
}

fn fail(message: impl Into<String>) -> Reply {
    let message = capitalize(&message.into());
    (
        "400 Bad Request",
        "application/json",
        json::obj([("ok", Value::Bool(false)), ("error", json::str(message))]).to_json(),
    )
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn done() -> Reply {
    ok(json::obj([("ok", Value::Bool(true))]))
}

/// Why a Program or File step can't be set up from this request.
const LOCAL_STEPS_LOCKED: &str = "running programs and writing files needs a dashboard password \
     while the dashboard is open to your network (System > Security)";

pub fn new_id() -> String {
    format!("i{}", &crate::crypto::random_token()[..12])
}

/// Handle `/integrations*`, `/connections/*`, `/twitch/*`. `None` when the
/// path is not ours.
///
/// `local_steps_allowed` is false while the dashboard is open to the
/// network with no password (see `web::may_set_up_local_steps`). Anyone on
/// the network could then make the request, so it may not add, change or
/// switch on a step that runs a program or writes a file: from chat, that
/// would run anything on the PC.
pub async fn route(
    method: &str,
    path: &str,
    body: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    local_steps_allowed: bool,
) -> Option<Reply> {
    let handle = ctrl.integrations()?.clone();
    let req = || json::parse(body).unwrap_or(Value::Null);
    let reply = match (method, path) {
        ("GET", "/integrations") => ok(overview(
            &handle,
            &settings.borrow(),
            cfg_path,
            local_steps_allowed,
        )),
        ("GET", "/integrations/activity") => ok(json::obj([
            ("activity", handle.activity_json()),
            ("stats", handle.stats_json()),
            ("twitch", handle.twitch.status_json()),
            ("obs", handle.obs_status()),
            ("alerts", alerts_json(&handle, &settings.borrow(), cfg_path)),
        ])),
        ("POST", "/integrations/save") => save(&req(), settings, cfg_path, local_steps_allowed),
        ("POST", "/integrations/add") => add(&req(), settings, cfg_path, local_steps_allowed),
        ("POST", "/integrations/toggle") => toggle(&req(), settings, cfg_path, local_steps_allowed),
        ("POST", "/integrations/delete") => {
            let id = req().str_or("id", "").to_string();
            update(settings, cfg_path, |s| {
                s.integrations.retain(|i| i.id != id);
                Ok(())
            })
        }
        ("POST", "/integrations/duplicate") => duplicate(&req(), settings, cfg_path),
        ("POST", "/integrations/test") => test(&req(), &handle).await,
        // The streamer's own values (`{my.name}`), all at once.
        ("POST", "/integrations/values") => save_user_values(&req(), settings, cfg_path),
        // What viewers hold (coins, points): the dashboard lists and edits it.
        ("GET", "/integrations/viewers") => ok(handle.viewers_json()),
        ("POST", "/integrations/viewers") => match handle.change_viewers(&req()) {
            Ok(()) => ok(handle.viewers_json()),
            Err(why) => fail(why),
        },
        // The live meter of a chat activity trigger being tuned.
        ("POST", "/integrations/meter") => match req().get("trigger").map(meter_rules) {
            Some(Ok(rules)) => ok(handle.chat_meter(&rules)),
            Some(Err(e)) => fail(e),
            None => fail("nothing to measure"),
        },
        ("GET", "/integrations/obs") => ok(handle.obs_status()),
        ("POST", "/integrations/lights/scan") => scan_lights().await,
        // Channel points rewards made here: list, make, switch, delete.
        ("GET", "/integrations/rewards") => rewards(&handle, RewardCall::List).await,
        ("POST", "/integrations/rewards") => rewards(&handle, RewardCall::from(&req())).await,
        ("POST", "/integrations/lights/try") => try_light(&req()).await,
        ("POST", "/integrations/fetch") => fetch(&req()).await,
        ("POST", "/integrations/convert") => convert(&req()),
        ("POST", "/integrations/alerts/test") => {
            handle.alerts.show(test_card(&req()));
            done()
        }
        ("POST", "/integrations/alerts/media/upload") => upload_media(&req(), cfg_path),
        ("POST", "/integrations/alerts/media/delete") => {
            let folder = alert_media::folder(cfg_path);
            match alert_media::delete(&folder, req().str_or("name", "")) {
                Ok(()) => done(),
                Err(e) => fail(e),
            }
        }
        ("POST", "/integrations/alerts/look") => {
            let look = alerts::Look::from_json(&req());
            update(settings, cfg_path, |s| {
                s.alerts_look = look;
                Ok(())
            })
        }
        ("POST", "/integrations/alerts/add-to-obs") => {
            let action = obsws::Action::BrowserSource {
                name: ALERTS_SOURCE.to_string(),
                url: alerts_url(settings.borrow().web_port),
            };
            match handle.obs_now(action).await {
                Ok(()) => done(),
                Err(e) => fail(e),
            }
        }
        ("POST", "/integrations/midi/learn") => midi_learn(ctrl),
        ("POST", "/integrations/midi/poll") => {
            let captured = ctrl.midi().take_captured_for(&[MIDI_LEARN]);
            ok(json::obj([
                (
                    "learning",
                    Value::Bool(ctrl.midi().learning().as_deref() == Some(MIDI_LEARN)),
                ),
                (
                    "captured",
                    captured.map_or(Value::Null, |(_, sig)| json::str(sig)),
                ),
            ]))
        }
        ("POST", "/integrations/midi/cancel") => {
            if ctrl.midi().learning().as_deref() == Some(MIDI_LEARN) {
                ctrl.midi().cancel_learn();
            }
            done()
        }
        ("POST", "/integrations/export") => export(&req(), &settings.borrow(), cfg_path),
        ("POST", "/integrations/import") => import(&req(), settings, cfg_path, local_steps_allowed),
        ("POST", "/connections/discord") => save_discord(&req(), settings, cfg_path),
        ("POST", "/connections/discord/delete") => delete_discord(&req(), settings, cfg_path),
        ("POST", "/connections/discord/test") => {
            // Copy out first: a settings borrow must not live across an await.
            let channels = settings.borrow().discord_channels.clone();
            test_discord(&req(), channels).await
        }
        ("POST", "/connections/phone") => save_phone(&req(), settings, cfg_path),
        ("POST", "/connections/phone/test") => {
            let phone = settings.borrow().phone.clone();
            test_phone(phone).await
        }
        ("POST", "/twitch/login") => {
            handle.twitch_login(Which::from_id(req().str_or("which", "main")));
            done()
        }
        ("POST", "/twitch/cancel") => {
            handle.twitch_cancel_login();
            done()
        }
        ("POST", "/twitch/shared-chat") => {
            let on = req().bool_or("on", false);
            update(settings, cfg_path, |s| {
                s.twitch_shared_chat = on;
                Ok(())
            })
        }
        ("POST", "/twitch/logout") => {
            handle.twitch_logout(Which::from_id(req().str_or("which", "main")));
            done()
        }
        _ => return None,
    };
    Some(reply)
}

/// The rules of a chat activity trigger sent for the live meter.
fn meter_rules(trigger: &Value) -> Result<super::model::ChatActivity, String> {
    let handler = json::obj([
        ("trigger", trigger.clone()),
        ("steps", Value::Arr(Vec::new())),
    ]);
    let probe = json::obj([("handlers", Value::Arr(vec![handler]))]);
    let integration = Integration::from_json(&probe)?;
    match integration.handlers.into_iter().next().map(|h| h.trigger) {
        Some(Trigger::ChatActivity(rules)) => Ok(rules),
        _ => Err("that isn't a chat activity trigger".to_string()),
    }
}

/// What a MIDI learn for an integration's shortcut is filed under, apart
/// from the delay actions' own learns.
const MIDI_LEARN: &str = crate::midi::INTEGRATION_LEARN;

/// Arm a MIDI learn: the next pad pressed becomes the shortcut. The
/// dashboard polls `/integrations/midi/poll` for it.
fn midi_learn(ctrl: &Arc<Controller>) -> Reply {
    if !cfg!(windows) {
        return fail("MIDI shortcuts work on Windows only");
    }
    // Any device will do: the learn opens them all, not only the one
    // picked in Controls.
    if ctrl.midi().devices().is_empty() {
        return fail("no MIDI controller found; plug one in and try again");
    }
    ctrl.midi().start_learn(MIDI_LEARN);
    done()
}

/// `/hooks/<token>`: fire the web call integrations holding this token.
pub fn hook(ctrl: &Arc<Controller>, token: &str, body: &str, query: &str) -> Reply {
    if let Some(handle) = ctrl.integrations() {
        handle.hook(
            token.to_string(),
            body.chars().take(64 * 1024).collect(),
            query.to_string(),
        );
    }
    (
        "202 Accepted",
        "application/json",
        r#"{"ok":true}"#.to_string(),
    )
}

/// Clone the settings, change them, validate, save and publish.
fn update(
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    change: impl FnOnce(&mut Settings) -> Result<(), String>,
) -> Reply {
    let _guard = crate::web::settings_write_guard();
    let mut next = settings.borrow().clone();
    if let Err(e) = change(&mut next) {
        return fail(e);
    }
    if let Err(e) = next.save(cfg_path) {
        return fail(format!("couldn't save: {e}"));
    }
    settings.send_replace(next);
    done()
}

/// What the alerts browser source is called when the dashboard adds it.
const ALERTS_SOURCE: &str = "InstantClone alerts";

/// The alerts page's address for OBS on this PC.
fn alerts_url(web_port: u16) -> String {
    format!("http://127.0.0.1:{web_port}/alerts")
}

/// The alerts page: its look, where it's open right now, and the sounds
/// and pictures uploaded for it.
fn alerts_json(handle: &super::Handle, s: &Settings, cfg_path: &Path) -> Value {
    let (in_obs, elsewhere) = handle.alerts.pages_open();
    let media = alert_media::list(&alert_media::folder(cfg_path))
        .into_iter()
        .map(|m| {
            json::obj([
                ("name", json::str(&m.name)),
                ("kind", json::str(m.kind.id())),
                ("bytes", Value::Num(m.bytes as f64)),
            ])
        })
        .collect();
    json::obj([
        ("look", s.alerts_look.to_json()),
        ("in_obs", Value::Num(in_obs as f64)),
        ("elsewhere", Value::Num(elsewhere as f64)),
        ("url", json::str(alerts_url(s.web_port))),
        ("media", Value::Arr(media)),
    ])
}

/// Keep a sound or picture sent from the dashboard (base64url in JSON:
/// the API's bodies are text).
fn upload_media(req: &Value, cfg_path: &Path) -> Reply {
    let Some(data) = recipe::base64url_decode(req.str_or("data", "")) else {
        return fail("that upload arrived damaged; try again");
    };
    match alert_media::save(
        &alert_media::folder(cfg_path),
        req.str_or("name", ""),
        &data,
    ) {
        Ok(name) => ok(json::obj([
            ("ok", Value::Bool(true)),
            ("name", json::str(name)),
        ])),
        Err(e) => fail(e),
    }
}

/// The alert "Show it on stream" sends from the editor, already filled in
/// with sample values; with nothing sent, a generic one. An alert with only
/// a small title is still that alert, not the generic one.
fn test_card(req: &Value) -> alerts::Card {
    let text = req.str_or("text", "").trim();
    if text.is_empty() && req.str_or("title", "").trim().is_empty() {
        return alerts::Card::plain(
            "InstantClone",
            "This is where your alerts show up on stream.",
            6,
        );
    }
    alerts::Card {
        title: req.str_or("title", "").to_string(),
        text: text.to_string(),
        seconds: req.str_or("seconds", "6").trim().parse().unwrap_or(6),
        color: req.str_or("color", "").trim().to_string(),
        sound: req.str_or("sound", "").trim().to_string(),
        volume: req
            .str_or("volume", "")
            .trim()
            .parse()
            .unwrap_or(alerts::DEFAULT_VOLUME),
        image: req.str_or("image", "").trim().to_string(),
    }
}

fn overview(
    handle: &super::Handle,
    s: &Settings,
    cfg_path: &Path,
    local_steps_allowed: bool,
) -> Value {
    // Previews and "Try it" use the real channel once Twitch is connected.
    let channel = handle.twitch.channel();
    let global = Value::Arr(
        runner::GLOBAL_VARS
            .iter()
            .map(|&(name, sample)| {
                let sample = match name {
                    "channel" if !channel.is_empty() => channel.as_str(),
                    _ => sample,
                };
                json::obj([("name", json::str(name)), ("sample", json::str(sample))])
            })
            .collect(),
    );
    let discord = s
        .discord_channels
        .iter()
        .map(|c| {
            json::obj([
                ("id", json::str(&c.id)),
                ("name", json::str(&c.name)),
                ("hint", json::str(mask_webhook(&c.url))),
            ])
        })
        .collect();
    let vars = json::obj([
        ("global", global),
        ("chat", event::vars_json(runner::CHAT_VARS)),
        (
            "web",
            event::vars_json(&[
                ("body", "{\"x\":1}"),
                ("query", "team=red"),
                ("source", "stream_deck"),
            ]),
        ),
        ("activity", event::vars_json(engine::ACTIVITY_SAMPLES)),
        ("scene", event::vars_json(engine::SCENE_SAMPLES)),
    ]);
    json::obj([
        (
            "integrations",
            Value::Arr(s.integrations.iter().map(Integration::to_json).collect()),
        ),
        (
            "connections",
            json::obj([
                ("discord", Value::Arr(discord)),
                (
                    "phone",
                    json::obj([
                        ("server", json::str(&s.phone.server)),
                        ("topic", json::str(&s.phone.topic)),
                    ]),
                ),
            ]),
        ),
        // What each integration still needs before it can run, so a card
        // can say "Pick a Discord channel" without opening the editor.
        (
            "issues",
            Value::Obj(
                s.integrations
                    .iter()
                    .map(|i| {
                        let mut p = i.problems();
                        p.invalid.extend(p.incomplete);
                        if let Err(e) = check_connections(i, s) {
                            p.invalid.push(e);
                        }
                        if let Some(e) = command_taken(i, s) {
                            p.invalid.push(e);
                        }
                        (
                            i.id.clone(),
                            Value::Arr(p.invalid.into_iter().map(json::str).collect()),
                        )
                    })
                    .collect(),
            ),
        ),
        ("twitch", handle.twitch.status_json()),
        ("obs", handle.obs_status()),
        ("alerts", alerts_json(handle, s, cfg_path)),
        // For the "Report it" links, which fill in the issue form's version.
        ("version", json::str(env!("CARGO_PKG_VERSION"))),
        ("twitch_shared_chat", Value::Bool(s.twitch_shared_chat)),
        (
            "values",
            Value::Arr(s.user_values.iter().map(UserValue::to_json).collect()),
        ),
        // Markers, clips and chat act on the logged-in channel; say so when
        // the Twitch destinations stream somewhere else.
        (
            "twitch_other_channel",
            Value::Bool(streams_to_other_channel(
                &handle.twitch.main_user_id(),
                &s.destinations,
            )),
        ),
        // False on another device while the dashboard has no password:
        // Run a program and Write a file are locked there (see `route`).
        ("local_steps", Value::Bool(local_steps_allowed)),
        ("catalog", presets::catalog_json()),
        // Global hotkeys and MIDI pads only exist on Windows, and a combo
        // or pad that runs a delay action can't start an integration too.
        (
            "shortcuts",
            json::obj([
                ("available", Value::Bool(cfg!(windows))),
                (
                    "refused",
                    Value::Arr(handle.refused_keys().into_iter().map(json::str).collect()),
                ),
                (
                    "taken",
                    Value::Arr(
                        s.hotkeys
                            .entries()
                            .iter()
                            .chain(s.midi.entries().iter())
                            .filter(|(_, v)| !v.is_empty())
                            .map(|(_, v)| json::str(*v))
                            .collect(),
                    ),
                ),
            ]),
        ),
        ("events", event::catalog_json()),
        ("vars", vars),
        ("activity", handle.activity_json()),
        ("stats", handle.stats_json()),
        ("dropped_events", Value::Num(handle.dropped_events() as f64)),
    ])
}

/// `https://discord.com/api/webhooks/1234/abcd...` shown as `…/1234/••••`.
/// The id is read right after `/webhooks/`, never by counting from the
/// end: a link ending in `/slack` would show the token in its place.
fn mask_webhook(url: &str) -> String {
    let id = url
        .split_once("/webhooks/")
        .and_then(|(_, rest)| rest.split('/').next())
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
    match id {
        Some(id) => format!("…/webhooks/{id}/••••"),
        None => "set".to_string(),
    }
}

fn save(
    req: &Value,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    local_steps_allowed: bool,
) -> Reply {
    let mut integration = match Integration::from_json(req) {
        Ok(i) => i,
        Err(e) => return fail(e),
    };
    if !local_steps_allowed && integration.has_local_effects() {
        return fail(LOCAL_STEPS_LOCKED);
    }
    if integration.id.is_empty() {
        integration.id = new_id();
    }
    let mut problems = integration.problems();
    if !problems.invalid.is_empty() {
        return fail(problems.invalid.join("; "));
    }
    if let Err(e) = check_connections(&integration, &settings.borrow()) {
        problems.incomplete.push(e);
    }
    if let Some(e) = shortcut_taken(&integration, &settings.borrow()) {
        return fail(e);
    }
    let was_on = settings
        .borrow()
        .integrations
        .iter()
        .any(|i| i.id == integration.id && i.enabled);
    if integration.enabled {
        if let Some(e) = command_taken(&integration, &settings.borrow()) {
            if was_on {
                return fail(e);
            }
            problems.incomplete.push(e);
        }
    }
    // Switched on, it must be ready to run. One already running keeps
    // running as it was rather than stop over a half-made edit; a new one
    // or a draft saves as a draft, switched off, so no work is lost, and
    // the dashboard lists what is still missing.
    let drafted = integration.enabled && !problems.incomplete.is_empty();
    if drafted {
        if was_on {
            return fail(format!(
                "{}. Finish it, or switch it off to save it as a draft.",
                problems.incomplete.join("; ")
            ));
        }
        integration.enabled = false;
    }
    let id = integration.id.clone();
    let warnings = unknown_vars(&integration);
    let missing = problems.incomplete;
    // Undoing a delete puts it back where it was.
    let restore_at = req.get("at").and_then(Value::as_f64).map(|n| n as usize);
    let reply = update(settings, cfg_path, move |s| {
        match s.integrations.iter().position(|i| i.id == integration.id) {
            Some(at) => s.integrations[at] = integration,
            None if s.integrations.len() >= MAX_INTEGRATIONS => {
                return Err(format!("you already have {MAX_INTEGRATIONS} integrations"))
            }
            None => {
                let at = restore_at.unwrap_or(usize::MAX).min(s.integrations.len());
                s.integrations.insert(at, integration);
            }
        }
        Ok(())
    });
    if reply.0 != "200 OK" {
        return reply;
    }
    ok(json::obj([
        ("ok", Value::Bool(true)),
        ("id", json::str(&id)),
        ("drafted", Value::Bool(drafted)),
        (
            "warnings",
            Value::Arr(warnings.into_iter().map(json::str).collect()),
        ),
        (
            "missing",
            Value::Arr(missing.into_iter().map(json::str).collect()),
        ),
    ]))
}

/// Variables the integration's messages use that nothing provides: most
/// likely a typo, which would otherwise render as nothing.
pub fn unknown_vars(integration: &Integration) -> Vec<String> {
    use super::model::{visit_steps, StepKind};
    let mut unknown: Vec<String> = Vec::new();
    for h in &integration.handlers {
        let mut known: Vec<String> = runner::GLOBAL_VARS
            .iter()
            .map(|(n, _)| n.to_string())
            .collect();
        // What is kept between runs is read by name; nothing lists the names.
        let mut prefixes: Vec<String> = [
            "counter.", "clip.", "viewer.", "target.", "top.", "saved.", "my.",
        ]
        .map(String::from)
        .to_vec();
        known.push("active.count".to_string());
        match &h.trigger {
            Trigger::Event { kind, .. } => {
                known.extend(kind.vars().iter().map(|(n, _)| n.to_string()))
            }
            // A block runs inside whatever started it: most often a command.
            Trigger::ChatCommand { .. } | Trigger::ChatMessage { .. } | Trigger::Block { .. } => {
                for n in 1..=9 {
                    known.push(format!("arg{n}"));
                }
                known.extend(runner::CHAT_VARS.iter().map(|(n, _)| n.to_string()));
            }
            // A button pressed through its Stream Deck address can carry a
            // body too.
            Trigger::Webhook { .. } | Trigger::Shortcut { .. } => {
                known.extend(["body", "query", "source"].map(String::from));
                prefixes.push("body.".to_string());
                prefixes.push("query.".to_string());
            }
            Trigger::ChatActivity(_) => {
                known.extend(engine::ACTIVITY_SAMPLES.iter().map(|(n, _)| n.to_string()))
            }
            Trigger::Scene { .. } => {
                known.extend(engine::SCENE_SAMPLES.iter().map(|(n, _)| n.to_string()))
            }
            Trigger::Timer { .. } => {}
        }
        visit_steps(&h.steps, &mut |s| match s.kind {
            StepKind::Http => prefixes.push(match s.param("save_as").trim() {
                "" => "response.".to_string(),
                name => format!("{name}."),
            }),
            StepKind::SetVar => known.push(s.param("name").trim().to_string()),
            StepKind::Counter if !s.param("save_as").trim().is_empty() => {
                known.push(s.param("save_as").trim().to_string())
            }
            StepKind::Viewer if s.param("op") == "spend" => known.push("paid".to_string()),
            StepKind::Math => known.push(match s.param("save_as").trim() {
                "" => "result".to_string(),
                name => name.to_string(),
            }),
            StepKind::WaitChat => prefixes.push(match s.param("save_as").trim() {
                "" => "reply.".to_string(),
                name => format!("{name}."),
            }),
            StepKind::EditText => known.push(match s.param("save_as").trim() {
                "" => "text".to_string(),
                name => name.to_string(),
            }),
            StepKind::Obs if s.param("action") == "check" => {
                let name = match s.param("save_as").trim() {
                    "" => "obs",
                    name => name,
                };
                let mut fields: Vec<String> = ["scene", "streaming", "recording"]
                    .map(String::from)
                    .to_vec();
                let sources = super::obsws::listed_names(s.param("source"));
                if !sources.is_empty() {
                    fields.extend(["muted", "visible"].map(String::from));
                }
                if sources.len() > 1 {
                    for key in super::obsws::source_keys(&sources) {
                        fields.extend([format!("{key}.muted"), format!("{key}.visible")]);
                    }
                }
                known.extend(fields.iter().map(|field| format!("{name}.{field}")));
            }
            StepKind::Program if s.param("wait") == "yes" => {
                let name = match s.param("save_as").trim() {
                    "" => "program",
                    name => name,
                };
                known.extend(["output", "code", "ok"].map(|field| format!("{name}.{field}")));
            }
            _ => {}
        });
        visit_steps(&h.steps, &mut |s| {
            for value in s.params.values() {
                for name in super::template::names(value) {
                    let provided = known.contains(&name)
                        || prefixes.iter().any(|p| name.starts_with(p.as_str()));
                    if !provided && !unknown.contains(&name) {
                        unknown.push(name);
                    }
                }
            }
        });
    }
    unknown
}

fn with_id(reply: Reply, id: &str) -> Reply {
    if reply.0 != "200 OK" {
        return reply;
    }
    ok(json::obj([
        ("ok", Value::Bool(true)),
        ("id", json::str(id)),
    ]))
}

/// A hotkey or pad an integration wants that already runs a delay action:
/// Windows gives a hotkey to one owner, and a pad would do both.
fn shortcut_taken(integration: &Integration, s: &Settings) -> Option<String> {
    integration.handlers.iter().find_map(|h| {
        let Trigger::Shortcut { hotkey, midi, .. } = &h.trigger else {
            return None;
        };
        let key_owner = s
            .hotkeys
            .entries()
            .into_iter()
            .find(|(_, bound)| !hotkey.is_empty() && *bound == hotkey.as_str());
        if let Some((action, _)) = key_owner {
            return Some(format!(
                "{hotkey} already runs the delay's {} action (Controls tab); pick another key",
                action.replace('_', " ")
            ));
        }
        let pad_owner = s
            .midi
            .entries()
            .into_iter()
            .find(|(_, bound)| super::model::pads_overlap(bound, midi));
        pad_owner.map(|(action, _)| {
            format!(
                "that MIDI pad already runs the delay's {} action (Controls tab); pick another",
                action.replace('_', " ")
            )
        })
    })
}

/// A chat command or alias `integration` answers that another switched-on
/// integration answers too: the engine runs every match, so one `!delay`
/// would get two replies. Whether `integration` itself is on is the
/// caller's question.
fn command_taken(integration: &Integration, s: &Settings) -> Option<String> {
    fn commands(i: &Integration) -> impl Iterator<Item = &String> {
        i.handlers
            .iter()
            .filter(|h| h.enabled)
            .filter_map(|h| match &h.trigger {
                Trigger::ChatCommand {
                    command, aliases, ..
                } => Some(std::iter::once(command).chain(aliases)),
                _ => None,
            })
            .flatten()
    }
    s.integrations
        .iter()
        .filter(|other| other.enabled && other.id != integration.id)
        .find_map(|other| {
            let word = commands(integration).find(|w| commands(other).any(|o| o == *w))?;
            Some(format!(
                "\"{}\" already answers {word}; change the command, or switch that one off",
                other.name
            ))
        })
}

/// Every Discord step must point at a connection that exists.
/// Whether every enabled Twitch destination streams to a channel other
/// than `user_id`'s. The channel comes from the id Twitch puts in a stream
/// key (`live_<id>_…`); keys in another shape say nothing either way.
fn streams_to_other_channel(user_id: &str, destinations: &[crate::config::Destination]) -> bool {
    let channels: Vec<&str> = destinations
        .iter()
        .filter(|d| d.enabled && d.platform == "twitch")
        .filter_map(|d| key_channel(&d.stream_key))
        .collect();
    !user_id.is_empty() && !channels.is_empty() && !channels.contains(&user_id)
}

/// The channel id in a Twitch stream key, `live_<digits>_<secret>`.
fn key_channel(key: &str) -> Option<&str> {
    let (id, _) = key.trim().strip_prefix("live_")?.split_once('_')?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

fn check_connections(integration: &Integration, s: &Settings) -> Result<(), String> {
    let mut gone = false;
    for h in &integration.handlers {
        super::model::visit_steps(&h.steps, &mut |step| {
            // A blank channel is reported by validation; this catches one
            // that was set and has since been removed.
            let id = step.param("connection");
            if step.kind == super::model::StepKind::Discord
                && !id.is_empty()
                && !s.discord_channels.iter().any(|c| c.id == id)
            {
                gone = true;
            }
        });
    }
    if gone {
        Err("a Discord channel it posts to was removed; pick another".to_string())
    } else {
        Ok(())
    }
}

fn add(
    req: &Value,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    local_steps_allowed: bool,
) -> Reply {
    let presets_to_add: Vec<String> = match (req.get("preset"), req.get("pack")) {
        (Some(p), _) => vec![p.as_str().unwrap_or("").to_string()],
        (_, Some(pack)) => match presets::PACKS.iter().find(|p| Some(p.id) == pack.as_str()) {
            Some(pack) => pack.presets.iter().map(|p| p.to_string()).collect(),
            None => return fail("unknown pack"),
        },
        _ => return fail("pick something to add"),
    };
    let mut ids = Vec::new();
    let reply = update(settings, cfg_path, |s| {
        let channel = s
            .discord_channels
            .first()
            .map(|c| c.id.clone())
            .unwrap_or_default();
        for preset in &presets_to_add {
            // A pack skips what the user already has from the catalog.
            if presets_to_add.len() > 1 && s.integrations.iter().any(|i| &i.preset == preset) {
                continue;
            }
            if s.integrations.len() >= MAX_INTEGRATIONS {
                return Err(format!("you already have {MAX_INTEGRATIONS} integrations"));
            }
            let mut built =
                presets::build(preset, new_id(), &channel).ok_or("unknown catalog entry")?;
            // "Add another" of a chat command: the copy waits, switched off,
            // until it gets a command of its own.
            if built.enabled && command_taken(&built, s).is_some() {
                built.enabled = false;
            }
            // A catalog entry that writes a file could never be finished.
            if !local_steps_allowed && built.has_local_effects() {
                if presets_to_add.len() > 1 {
                    continue;
                }
                return Err(LOCAL_STEPS_LOCKED.to_string());
            }
            ids.push(built.id.clone());
            s.integrations.push(built);
        }
        Ok(())
    });
    if reply.0 != "200 OK" {
        return reply;
    }
    ok(json::obj([
        ("ok", Value::Bool(true)),
        ("ids", Value::Arr(ids.into_iter().map(json::str).collect())),
    ]))
}

fn toggle(
    req: &Value,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    local_steps_allowed: bool,
) -> Reply {
    let id = req.str_or("id", "").to_string();
    let enabled = req.bool_or("enabled", true);
    update(settings, cfg_path, |s| {
        let at = s
            .integrations
            .iter()
            .position(|i| i.id == id)
            .ok_or("that integration no longer exists")?;
        if enabled {
            let integration = &s.integrations[at];
            if !local_steps_allowed && integration.has_local_effects() {
                return Err(LOCAL_STEPS_LOCKED.to_string());
            }
            let errors = integration.validate();
            if !errors.is_empty() {
                return Err(format!("finish setting it up first: {}", errors.join("; ")));
            }
            check_connections(integration, s)?;
            // A delay hotkey bound since it was switched off wins otherwise,
            // and the integration would never start from it.
            if let Some(taken) = shortcut_taken(integration, s) {
                return Err(taken);
            }
            if let Some(taken) = command_taken(integration, s) {
                return Err(taken);
            }
        }
        s.integrations[at].enabled = enabled;
        Ok(())
    })
}

fn duplicate(req: &Value, settings: &Arc<watch::Sender<Settings>>, cfg_path: &Path) -> Reply {
    let id = req.str_or("id", "").to_string();
    let copy_id = new_id();
    let returned = copy_id.clone();
    let reply = update(settings, cfg_path, move |s| {
        if s.integrations.len() >= MAX_INTEGRATIONS {
            return Err(format!("you already have {MAX_INTEGRATIONS} integrations"));
        }
        let at = s
            .integrations
            .iter()
            .position(|i| i.id == id)
            .ok_or("that integration no longer exists")?;
        let original = s.integrations[at].clone();
        // The copy starts off, with no hotkey or pad and its own web call
        // link: otherwise one press, command or link would run both.
        let mut copy = original;
        copy.id = copy_id;
        copy.name = format!("{} (copy)", copy.name).chars().take(80).collect();
        copy.enabled = false;
        for h in &mut copy.handlers {
            match &mut h.trigger {
                Trigger::Webhook { token } => *token = new_id() + &new_id(),
                Trigger::Shortcut {
                    hotkey,
                    midi,
                    token,
                    ..
                } => {
                    hotkey.clear();
                    midi.clear();
                    if !token.is_empty() {
                        *token = new_id() + &new_id();
                    }
                }
                _ => {}
            }
        }
        // Right after the original, where the user is looking.
        s.integrations.insert(at + 1, copy);
        Ok(())
    });
    with_id(reply, &returned)
}

async fn test(req: &Value, handle: &super::Handle) -> Reply {
    let integration = match req.get("integration").map(Integration::from_json) {
        Some(Ok(i)) => i,
        Some(Err(e)) => return fail(e),
        None => return fail("nothing to test"),
    };
    let handler = req.u64_or("handler", 0) as usize;
    if handler >= integration.handlers.len() {
        return fail("that trigger doesn't exist");
    }
    let values = match req.get("values") {
        Some(Value::Obj(fields)) => fields
            .iter()
            .map(|(k, v)| (k.clone(), v.to_display()))
            .collect(),
        _ => Default::default(),
    };
    let run = engine::TestRun {
        values,
        dry: req.bool_or("dry", false),
    };
    match handle.test(integration, handler, run).await {
        Some(record) => ok(json::obj([
            ("ok", Value::Bool(true)),
            ("status", json::str(record.status)),
            (
                "steps",
                Value::Arr(
                    record
                        .steps
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
        ])),
        None => fail("the test didn't finish within a minute"),
    }
}

/// The editor's paste box: a bot command (or an address) to the pieces of
/// a "command from a website". See `botcmd`.
fn convert(req: &Value) -> Reply {
    match super::botcmd::convert(req.str_or("text", "")) {
        Ok(c) => ok(json::obj([
            ("ok", Value::Bool(true)),
            ("command", c.command.map_or(Value::Null, json::str)),
            ("url", json::str(c.url)),
            ("reply", c.reply.map_or(Value::Null, json::str)),
            ("from", json::str(c.from)),
            (
                "unknown",
                Value::Arr(c.unknown.into_iter().map(json::str).collect()),
            ),
        ])),
        Err(e) => fail(e),
    }
}

/// Longest answer the editor's "Send request" shows.
const FETCH_SHOWN: usize = 200_000;

/// Look for lights and plugs on the network (a few seconds).
async fn scan_lights() -> Reply {
    let found = tokio::task::spawn_blocking(|| lights::scan(lights::SCAN_WAIT))
        .await
        .unwrap_or_default();
    ok(json::obj([(
        "devices",
        Value::Arr(found.iter().map(lights::Device::to_json).collect()),
    )]))
}

/// What the dashboard asks of the channel's rewards.
enum RewardCall {
    List,
    Create(RewardEdit),
    Update { id: String, edit: RewardEdit },
    Delete { id: String },
    Invalid(String),
}

impl RewardCall {
    /// With an `id`: `delete` removes it, anything else changes the parts
    /// that are given (`enabled` alone switches it). Without: makes one,
    /// which needs a `title` and a `cost`.
    fn from(req: &Value) -> RewardCall {
        let id = req.str_or("id", "").to_string();
        if !id.is_empty() && req.bool_or("delete", false) {
            return RewardCall::Delete { id };
        }
        let text = |name: &str| {
            req.get(name)
                .and_then(Value::as_str)
                .map(|t| t.trim().to_string())
        };
        let edit = RewardEdit {
            title: text("title"),
            cost: req.get("cost").and_then(Value::as_f64).map(|c| c as u64),
            prompt: text("prompt"),
            wants_input: req.get("wants_input").and_then(Value::as_bool),
            enabled: req.get("enabled").and_then(Value::as_bool),
        };
        let making = id.is_empty();
        let title_fits = |t: &String| !t.is_empty() && t.chars().count() <= MAX_REWARD_TITLE;
        let problem = if making && (edit.title.is_none() || edit.cost.is_none()) {
            Some("a reward needs a name and a cost".to_string())
        } else if !edit.title.as_ref().is_none_or(title_fits) {
            Some(format!(
                "a reward's name is 1 to {MAX_REWARD_TITLE} characters"
            ))
        } else if !edit
            .cost
            .is_none_or(|cost| (1..=1_000_000_000).contains(&cost))
        {
            Some("a reward costs at least 1 point".to_string())
        } else if edit
            .prompt
            .as_ref()
            .is_some_and(|p| p.chars().count() > MAX_REWARD_PROMPT)
        {
            Some(format!(
                "a description is {MAX_REWARD_PROMPT} characters at most"
            ))
        } else {
            None
        };
        match (problem, making) {
            (Some(why), _) => RewardCall::Invalid(why),
            (None, true) => RewardCall::Create(RewardEdit {
                enabled: Some(true),
                ..edit
            }),
            (None, false) => RewardCall::Update { id, edit },
        }
    }
}

/// Do `call` on Twitch, then answer with the rewards as they are now.
async fn rewards(handle: &Arc<super::Handle>, call: RewardCall) -> Reply {
    let twitch = handle.twitch.clone();
    let listed = tokio::task::spawn_blocking(move || {
        match call {
            RewardCall::List => {}
            RewardCall::Create(spec) => {
                twitch.create_reward(&spec)?;
            }
            RewardCall::Update { id, edit } => twitch.update_reward(&id, &edit)?,
            RewardCall::Delete { id } => twitch.delete_reward(&id)?,
            RewardCall::Invalid(why) => return Err(why),
        }
        twitch.rewards()
    })
    .await
    .unwrap_or_else(|_| Err("the call to Twitch crashed".to_string()));
    match listed {
        Ok(list) => {
            let rewards = list.iter().map(|r| {
                json::obj([
                    ("id", json::str(&r.id)),
                    ("title", json::str(&r.title)),
                    ("cost", Value::Num(r.cost as f64)),
                    ("enabled", Value::Bool(r.enabled)),
                    ("prompt", json::str(&r.prompt)),
                    ("wants_input", Value::Bool(r.wants_input)),
                ])
            });
            ok(json::obj([("rewards", Value::Arr(rewards.collect()))]))
        }
        Err(why) => fail(why),
    }
}

/// Flash a device so the user sees which one it is.
async fn try_light(req: &Value) -> Reply {
    let device = req.str_or("device", "").to_string();
    let rgb = lights::parse_color(req.str_or("color", "")).unwrap_or([255, 255, 255]);
    let flash = lights::Command::Flash {
        rgb,
        brightness: None,
    };
    match tokio::task::spawn_blocking(move || lights::send(&device, flash)).await {
        Ok(Ok(note)) => ok(json::obj([("note", json::str(note.unwrap_or_default()))])),
        Ok(Err(why)) => fail(why),
        Err(_) => fail("the light test crashed"),
    }
}

/// The editor's "Send request": make the request a web request step would,
/// with the values the user typed, and show what came back.
async fn fetch(req: &Value) -> Reply {
    let headers = req
        .str_or("headers", "")
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            let name = name.trim();
            (!name.is_empty()).then(|| (name.to_string(), value.trim().to_string()))
        })
        .collect();
    let request = super::host::HttpRequest {
        method: match req.str_or("method", "GET").trim() {
            "" => "GET".to_string(),
            m => m.to_ascii_uppercase(),
        },
        url: req.str_or("url", "").trim().to_string(),
        headers,
        body: req.str_or("body", "").to_string(),
    };
    let started = std::time::Instant::now();
    let result = tokio::task::spawn_blocking(move || super::effects::send_http(request))
        .await
        .unwrap_or_else(|_| Err("the request crashed".to_string()));
    let ms = started.elapsed().as_millis() as f64;
    match result {
        Ok(resp) => {
            let truncated = resp.body.chars().count() > FETCH_SHOWN;
            let body: String = resp.body.chars().take(FETCH_SHOWN).collect();
            ok(json::obj([
                ("ok", Value::Bool(true)),
                ("status", Value::Num(f64::from(resp.status))),
                ("ms", Value::Num(ms)),
                ("body", json::str(body)),
                ("truncated", Value::Bool(truncated)),
            ]))
        }
        Err(e) => fail(e),
    }
}

fn export(req: &Value, s: &Settings, cfg_path: &Path) -> Reply {
    let ids: Vec<&str> = req
        .get("ids")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let chosen: Vec<Integration> = s
        .integrations
        .iter()
        .filter(|i| ids.contains(&i.id.as_str()))
        .cloned()
        .collect();
    if chosen.is_empty() {
        return fail("pick what to share");
    }
    let name = match req.str_or("name", "").trim() {
        "" => chosen[0].name.clone(),
        n => n.to_string(),
    };
    // Alerts bring the theme and their uploads unless that's unticked.
    let look = req.bool_or("with_theme", true).then_some(&s.alerts_look);
    let folder = alert_media::folder(cfg_path);
    ok(json::obj([
        ("ok", Value::Bool(true)),
        (
            "recipe",
            json::str(recipe::export(
                &name,
                &chosen,
                (look, &folder),
                &s.user_values,
            )),
        ),
    ]))
}

/// Preview a recipe (`apply` false) or add it (`apply` true).
fn import(
    req: &Value,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    local_steps_allowed: bool,
) -> Reply {
    let mut parsed = match recipe::parse(req.str_or("recipe", ""), new_id) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    if !req.bool_or("apply", false) {
        let items = parsed
            .integrations
            .iter()
            .map(|i| {
                json::obj([
                    ("name", json::str(&i.name)),
                    ("summary", json::str(summary(i))),
                    ("triggers", Value::Arr(trigger_review(i))),
                    ("effects", Value::Arr(effect_review(i))),
                    // Every step, for "See every step". Parsing already
                    // dropped what a recipe never carries (addresses, paths).
                    ("integration", i.to_json()),
                ])
            })
            .collect();
        return ok(json::obj([
            ("ok", Value::Bool(true)),
            ("name", json::str(&parsed.name)),
            ("items", Value::Arr(items)),
            ("uses_discord", Value::Bool(parsed.uses_discord())),
            ("local_effects", Value::Bool(parsed.has_local_effects())),
            ("values", shared_values_review(&parsed, &settings.borrow())),
            (
                "alerts",
                shared_alerts_review(&parsed, &settings.borrow().alerts_look),
            ),
        ]));
    }
    if parsed.has_local_effects() && !local_steps_allowed {
        return fail(LOCAL_STEPS_LOCKED);
    }
    if parsed.has_local_effects() && !req.bool_or("allow_local", false) {
        return fail("this recipe runs programs or writes files; confirm that you trust it");
    }
    if parsed.uses_discord() {
        let channel = req.str_or("discord", "").to_string();
        if !settings
            .borrow()
            .discord_channels
            .iter()
            .any(|c| c.id == channel)
        {
            return fail("pick which Discord channel it should post to");
        }
        recipe::map_discord(&mut parsed, &channel);
    }
    let count = parsed.integrations.len();
    if would_overflow(settings, count) {
        return fail(format!(
            "that would go over {MAX_INTEGRATIONS} integrations"
        ));
    }
    let (theme, added_media) =
        match keep_shared_alerts(&mut parsed, req.bool_or("use_theme", false), cfg_path) {
            Ok(kept) => kept,
            Err(e) => return fail(e),
        };
    let ids: Vec<Value> = parsed
        .integrations
        .iter()
        .map(|i| json::str(&i.id))
        .collect();
    let reply = update(settings, cfg_path, move |s| {
        if s.integrations.len() + count > MAX_INTEGRATIONS {
            return Err(format!(
                "that would go over {MAX_INTEGRATIONS} integrations"
            ));
        }
        s.integrations.extend(parsed.integrations);
        // A value the importer already set stays theirs.
        for value in parsed.values {
            let known = s.user_values.iter().any(|v| v.name == value.name);
            if !known && s.user_values.len() < MAX_USER_VALUES {
                s.user_values.push(value);
            }
        }
        if let Some(theme) = theme {
            s.alerts_look = theme;
        }
        Ok(())
    });
    if reply.0 != "200 OK" {
        // Nothing was added, so the uploads saved for it go too.
        forget_media(&alert_media::folder(cfg_path), &added_media);
        return reply;
    }
    ok(json::obj([
        ("ok", Value::Bool(true)),
        ("added", Value::Num(count as f64)),
        ("ids", Value::Arr(ids)),
    ]))
}

/// Whether adding this many integrations goes over the limit; checked
/// again under the write lock, this one keeps uploads from being saved
/// for an import that can't happen.
fn would_overflow(settings: &Arc<watch::Sender<Settings>>, adding: usize) -> bool {
    settings.borrow().integrations.len() + adding > MAX_INTEGRATIONS
}

/// What a recipe's alerts bring, for the import preview: its theme, the
/// uploads that came along and those that didn't, and whether this PC's
/// alerts page still has the plain default theme (then taking theirs is
/// ticked to start).
fn shared_alerts_review(parsed: &recipe::Recipe, mine: &alerts::Look) -> Value {
    let Some(shared) = &parsed.alerts else {
        return Value::Null;
    };
    let named = |name: &str| {
        json::obj([
            ("name", json::str(name)),
            ("kind", json::str(recipe::media_kind(name))),
        ])
    };
    json::obj([
        ("look", shared.look.to_json()),
        (
            "media",
            Value::Arr(shared.media.iter().map(|(n, _)| named(n)).collect()),
        ),
        (
            "left_out",
            Value::Arr(shared.left_out.iter().map(|n| named(n)).collect()),
        ),
        (
            "mine_is_default",
            Value::Bool(*mine == alerts::Look::default()),
        ),
    ])
}

/// The values a recipe brings, for the import preview: each with whether
/// the importer already has one under that name (theirs is then kept).
fn shared_values_review(parsed: &recipe::Recipe, s: &Settings) -> Value {
    let review = |value: &UserValue| {
        let mine = s.user_values.iter().find(|v| v.name == value.name);
        json::obj([
            ("name", json::str(&value.name)),
            ("label", json::str(&value.label)),
            ("value", json::str(&value.value)),
            ("mine", mine.map_or(Value::Null, |v| json::str(&v.value))),
        ])
    };
    Value::Arr(parsed.values.iter().map(review).collect())
}

/// Replace the streamer's own values with the list the dashboard sent.
fn save_user_values(
    req: &Value,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> Reply {
    let sent = req.get("values").and_then(Value::as_array).unwrap_or(&[]);
    let mut values: Vec<UserValue> = Vec::new();
    for value in sent {
        let Some(value) = UserValue::from_json(value) else {
            return fail(format!(
                "\"{}\" can't be saved: a name is letters, digits and _, and a value has a length limit",
                value.str_or("name", "")
            ));
        };
        if values.iter().any(|v| v.name == value.name) {
            return fail(format!("two values are named \"{}\"", value.name));
        }
        values.push(value);
    }
    if values.len() > MAX_USER_VALUES {
        return fail(format!("at most {MAX_USER_VALUES} values"));
    }
    update(settings, cfg_path, |s| {
        s.user_values = values;
        Ok(())
    })
}

/// Save the uploads a recipe brought and point its alerts at them.
/// Returns the sharer's theme (pointed at those uploads too) when the
/// importer chose to use it.
fn keep_shared_alerts(
    parsed: &mut recipe::Recipe,
    use_theme: bool,
    cfg_path: &Path,
) -> Result<(Option<alerts::Look>, Vec<String>), String> {
    let Some(shared) = parsed.alerts.take() else {
        // Shared without its uploads: an alert naming one falls back (the
        // theme sound, no picture) instead of pointing at a file that isn't
        // here, or at one of the importer's that happens to share the name.
        recipe::point_media(parsed, None, &std::collections::BTreeMap::new());
        return Ok((None, Vec::new()));
    };
    let folder = alert_media::folder(cfg_path);
    // What was here before: an identical upload is reused, and a failed
    // import may only take back the files it saved itself.
    let here: std::collections::HashSet<String> = alert_media::list(&folder)
        .into_iter()
        .map(|m| m.name)
        .collect();
    let mut saved = std::collections::BTreeMap::new();
    let mut added = Vec::new();
    for (name, data) in &shared.media {
        let kept = match recipe::keep_shared_media(&folder, name, data) {
            Ok(kept) => kept,
            Err(e) => {
                forget_media(&folder, &added);
                return Err(format!("couldn't keep {name}: {e}"));
            }
        };
        if !here.contains(&kept) && !added.contains(&kept) {
            added.push(kept.clone());
        }
        saved.insert(name.clone(), kept);
    }
    let mut theme = use_theme.then_some(shared.look);
    recipe::point_media(parsed, theme.as_mut(), &saved);
    Ok((theme, added))
}

/// Remove the uploads a failed import saved, so they don't stay behind
/// unused and count toward the uploads limit.
fn forget_media(folder: &Path, names: &[String]) {
    for name in names {
        if let Err(e) = alert_media::delete(folder, name) {
            log_line!("[integrations] couldn't remove {name} after a failed import: {e}");
        }
    }
}

/// Who can start a chat trigger, in words. The broadcaster always can.
fn who_can(roles: Roles) -> &'static str {
    match (roles.everyone, roles.subs, roles.vips, roles.mods) {
        (true, ..) => "anyone in chat",
        (false, false, false, false) => "only you",
        (false, false, false, true) => "only mods",
        (false, false, true, true) => "mods and VIPs",
        (false, true, _, _) => "subs and up",
        _ => "chosen roles",
    }
}

/// Every trigger of an imported integration and who can set it off, for
/// the import preview.
fn trigger_review(i: &Integration) -> Vec<Value> {
    i.handlers
        .iter()
        .map(|h| {
            let (what, who) = match &h.trigger {
                Trigger::ChatCommand { command, roles, .. } => {
                    (format!("{command} in chat"), who_can(*roles))
                }
                Trigger::ChatMessage { roles, .. } => ("a chat message".into(), who_can(*roles)),
                Trigger::Block { name } => (format!("the block \"{name}\""), "its other triggers"),
                Trigger::ChatActivity(a) => ("chat gets busy".into(), who_can(a.roles)),
                Trigger::Webhook { .. } => ("a web call".into(), "apps with its secret link"),
                Trigger::Shortcut { .. } => ("a button".into(), "you"),
                Trigger::Event { kind, .. } => (kind.label().to_string(), ""),
                Trigger::Timer { every_ms, .. } => (format!("every {} min", every_ms / 60_000), ""),
                Trigger::Scene { scene, .. } => (format!("OBS switches to {scene}"), ""),
            };
            json::obj([("what", json::str(what)), ("who", json::str(who))])
        })
        .collect()
}

/// What an imported integration reaches, most serious first: `danger` for
/// anything that runs on this PC or lets anyone in chat change the delay,
/// `caution` for the stream and OBS, `info` for messages it sends.
fn effect_review(i: &Integration) -> Vec<Value> {
    use super::model::{visit_steps, StepKind};
    let mut found: Vec<(&'static str, &'static str)> = Vec::new();
    let mut add = |level, text| {
        if !found.contains(&(level, text)) {
            found.push((level, text));
        }
    };
    for h in &i.handlers {
        let from_anyone = match &h.trigger {
            Trigger::ChatCommand { roles, .. } | Trigger::ChatMessage { roles, .. } => {
                roles.everyone
            }
            Trigger::ChatActivity(a) => a.roles.everyone,
            _ => false,
        };
        visit_steps(&h.steps, &mut |s| match s.kind {
            StepKind::Program => add("danger", "Runs a program on this PC"),
            StepKind::File => add("danger", "Writes a file on this PC"),
            StepKind::DelayAction if from_anyone => {
                add("danger", "Anyone in chat can change your delay")
            }
            StepKind::DelayAction => add("caution", "Changes your delay"),
            StepKind::Obs => add(
                "caution",
                "Controls OBS (scenes, sources, recording, audio)",
            ),
            StepKind::Http => add("caution", "Calls a web address you fill in"),
            StepKind::Light => add("caution", "Controls a light or plug on your network"),
            StepKind::Clip => add("info", "Makes clips"),
            StepKind::Marker => add("info", "Adds VOD markers"),
            StepKind::Discord => add("info", "Posts on Discord"),
            StepKind::Chat => add("info", "Writes in your Twitch chat"),
            StepKind::Phone => add("info", "Sends a push to your phone"),
            StepKind::Overlay => add("info", "Shows an alert on stream"),
            _ => {}
        });
    }
    let rank = |level: &str| match level {
        "danger" => 0,
        "caution" => 1,
        _ => 2,
    };
    found.sort_by_key(|(level, _)| rank(level));
    found
        .into_iter()
        .map(|(level, text)| json::obj([("level", json::str(level)), ("text", json::str(text))]))
        .collect()
}

/// One line describing an integration, for the import preview.
fn summary(i: &Integration) -> String {
    let triggers: Vec<String> = i
        .handlers
        .iter()
        .map(|h| match &h.trigger {
            Trigger::Event { kind, .. } => kind.label().to_string(),
            Trigger::ChatCommand { command, .. } => format!("{command} in chat"),
            Trigger::ChatMessage { .. } => "a chat message".to_string(),
            Trigger::Block { name } => format!("the block \"{name}\" is run"),
            Trigger::Timer { every_ms, .. } => {
                format!("every {} min", every_ms / 60_000)
            }
            Trigger::Webhook { .. } => "a web call".to_string(),
            Trigger::Shortcut { .. } => "a button (hotkey, MIDI pad or Stream Deck)".to_string(),
            Trigger::ChatActivity(_) => "chat gets busy".to_string(),
            Trigger::Scene { scene, .. } => format!("OBS switches to {scene}"),
        })
        .collect();
    let steps: usize = i
        .handlers
        .iter()
        .map(|h| super::model::count_steps(&h.steps))
        .sum();
    let unit = if steps == 1 { "step" } else { "steps" };
    format!("When {} · {steps} {unit}", triggers.join(", "))
}

fn save_discord(req: &Value, settings: &Arc<watch::Sender<Settings>>, cfg_path: &Path) -> Reply {
    let name = req
        .str_or("name", "")
        .trim()
        .chars()
        .take(40)
        .collect::<String>();
    let url = req.str_or("url", "").trim().to_string();
    let id = match req.str_or("id", "") {
        "" => new_id(),
        id => id.to_string(),
    };
    if name.is_empty() {
        return fail("give the channel a name, like Mods");
    }
    if !valid_id(&id) {
        return fail("invalid id");
    }
    let existing_url = settings
        .borrow()
        .discord_channels
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.url.clone());
    // Editing keeps the saved URL unless a new one is pasted.
    let url = match (url.is_empty(), existing_url) {
        (true, Some(saved)) => saved,
        _ => url,
    };
    if !valid_discord_webhook(&url) {
        return fail("paste a Discord webhook link (Server settings > Integrations > Webhooks)");
    }
    let returned = id.clone();
    let reply = update(settings, cfg_path, move |s| {
        let channel = DiscordChannel { id, name, url };
        match s.discord_channels.iter().position(|c| c.id == channel.id) {
            Some(at) => s.discord_channels[at] = channel,
            None if s.discord_channels.len() >= 32 => {
                return Err("that's a lot of Discord channels; remove one first".into())
            }
            None => s.discord_channels.push(channel),
        }
        Ok(())
    });
    with_id(reply, &returned)
}

fn delete_discord(req: &Value, settings: &Arc<watch::Sender<Settings>>, cfg_path: &Path) -> Reply {
    let id = req.str_or("id", "").to_string();
    update(settings, cfg_path, |s| {
        s.discord_channels.retain(|c| c.id != id);
        // Integrations that posted there stop until a new channel is picked.
        for integration in &mut s.integrations {
            let mut uses = false;
            for h in &integration.handlers {
                super::model::visit_steps(&h.steps, &mut |step| {
                    uses |= step.kind == super::model::StepKind::Discord
                        && step.param("connection") == id;
                });
            }
            if uses {
                integration.enabled = false;
            }
        }
        Ok(())
    })
}

async fn test_discord(req: &Value, channels: Vec<DiscordChannel>) -> Reply {
    let id = req.str_or("id", "");
    let Some(channel) = channels.into_iter().find(|c| c.id == id) else {
        return fail("that channel no longer exists");
    };
    let sent = tokio::task::spawn_blocking(move || {
        let body = super::effects::discord_body(&super::host::DiscordMessage {
            content: String::new(),
            card: Some(super::host::DiscordCard {
                title: "🧪 Connected".into(),
                description: "InstantClone can post here. Alerts, highlights and your stream timeline will show up like this.".into(),
                color: runner::DEFAULT_CARD_COLOR,
                footer: "InstantClone".into(),
                timestamp: true,
                ..Default::default()
            }),
            ping: String::new(),
        });
        crate::https::https_agent_with_timeout(super::effects::HTTP_TIMEOUT)
            .post(&channel.url)
            .header("Content-Type", "application/json")
            .send(body)
            .map(|r| r.status().as_u16())
            .map_err(|e| e.to_string())
    })
    .await;
    match sent {
        Ok(Ok(status)) if (200..300).contains(&status) => done(),
        Ok(Ok(401 | 403)) => fail("Discord refused this webhook link; copy it again from Discord"),
        Ok(Ok(404)) => fail("Discord says this webhook no longer exists"),
        Ok(Ok(status)) => fail(format!("Discord answered {status}")),
        Ok(Err(e)) => fail(format!("couldn't reach Discord: {e}")),
        Err(_) => fail("the test crashed"),
    }
}

fn save_phone(req: &Value, settings: &Arc<watch::Sender<Settings>>, cfg_path: &Path) -> Reply {
    let topic = req.str_or("topic", "").trim().to_string();
    let server = req
        .str_or("server", "")
        .trim()
        .trim_end_matches('/')
        .to_string();
    if !topic.is_empty()
        && (topic.len() > 64
            || !topic
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    {
        return fail("use letters, numbers, - and _ for the topic");
    }
    if !server.is_empty() && !server.starts_with("https://") && !server.starts_with("http://") {
        return fail("the server must start with https://");
    }
    update(settings, cfg_path, |s| {
        s.phone = PhoneConnection { server, topic };
        Ok(())
    })
}

async fn test_phone(phone: PhoneConnection) -> Reply {
    if !phone.is_set() {
        return fail("set a topic first");
    }
    let body = json::obj([
        ("topic", json::str(phone.topic.trim())),
        ("title", json::str("InstantClone")),
        (
            "message",
            json::str("🧪 Test push: your phone is connected."),
        ),
    ])
    .to_json();
    let sent = tokio::task::spawn_blocking(move || {
        crate::https::https_agent_with_timeout(super::effects::HTTP_TIMEOUT)
            .post(phone.server_or_default())
            .header("Content-Type", "application/json")
            .send(body)
            .map(|r| r.status().as_u16())
            .map_err(|e| e.to_string())
    })
    .await;
    match sent {
        Ok(Ok(status)) if (200..300).contains(&status) => done(),
        Ok(Ok(status)) => fail(format!("the server answered {status}")),
        Ok(Err(e)) => fail(format!("couldn't reach it: {e}")),
        Err(_) => fail("the test crashed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_key_names_its_channel() {
        assert_eq!(key_channel("live_123456789_AbCdEf"), Some("123456789"));
        assert_eq!(key_channel("live_abc_AbCdEf"), None);
        assert_eq!(key_channel("sk_us-west_123"), None);
        assert_eq!(key_channel("live_123"), None);
    }

    #[test]
    fn streaming_to_another_channel_is_noticed() {
        let twitch = |key: &str, enabled: bool| crate::config::Destination {
            id: key.into(),
            name: "Twitch".into(),
            enabled,
            platform: "twitch".into(),
            stream_key: key.into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        };
        let mut list = vec![twitch("live_222_x", true)];
        assert!(streams_to_other_channel("111", &list));
        assert!(
            !streams_to_other_channel("", &list),
            "logged out says nothing"
        );
        list.push(twitch("live_111_y", true));
        assert!(
            !streams_to_other_channel("111", &list),
            "one is the login's"
        );
        let off = vec![twitch("live_222_x", false)];
        assert!(
            !streams_to_other_channel("111", &off),
            "only enabled ones count"
        );
    }

    #[test]
    fn webhooks_are_masked() {
        assert_eq!(
            mask_webhook("https://discord.com/api/webhooks/123/abcDEF"),
            "…/webhooks/123/••••"
        );
        assert_eq!(
            mask_webhook("https://discord.com/api/webhooks/123/abcDEF/slack"),
            "…/webhooks/123/••••"
        );
    }

    #[test]
    fn typos_in_variables_are_reported() {
        let mut i = presets::build("delay_command", "d".into(), "").unwrap();
        assert!(unknown_vars(&i).is_empty());
        i.handlers[0].steps[0]
            .params
            .insert("text".into(), "{delya} for {user} at {time}".into());
        assert_eq!(unknown_vars(&i), vec!["delya".to_string()]);
    }

    #[test]
    fn checks_and_waited_programs_fill_their_values() {
        use super::super::model::{Step, StepKind};
        let mut i = presets::build("delay_command", "d".into(), "").unwrap();
        let text =
            "{obs.muted?muted:unmuted} {cam.scene} {cam.desk.muted} {rank.output} {program.code}";
        i.handlers[0].steps = vec![
            Step::new(StepKind::Obs, &[("action", "check"), ("source", "Mic")]),
            Step::new(
                StepKind::Obs,
                &[
                    ("action", "check"),
                    ("source", "Desk\nMic"),
                    ("save_as", "cam"),
                ],
            ),
            Step::new(StepKind::Program, &[("wait", "yes"), ("save_as", "rank")]),
            Step::new(StepKind::Chat, &[("text", text)]),
        ];
        assert_eq!(unknown_vars(&i), vec!["program.code".to_string()]);
        i.handlers[0].steps[0]
            .params
            .insert("source".into(), String::new());
        assert_eq!(
            unknown_vars(&i),
            vec!["obs.muted".to_string(), "program.code".to_string()]
        );
    }

    #[test]
    fn a_delay_hotkey_or_pad_cannot_start_an_integration_too() {
        use super::super::model::{Handler, Step, StepKind, Trigger};
        let mut s = Settings::defaults();
        s.hotkeys.set("cut", "Ctrl+Alt+C");
        s.midi.set("toggle", "note:1:36");
        let mut i = presets::build("delay_command", "d".into(), "").unwrap();
        let mut shortcut = |hotkey: &str, midi: &str| {
            i.handlers = vec![Handler {
                enabled: true,
                trigger: Trigger::Shortcut {
                    hotkey: hotkey.into(),
                    midi: midi.into(),
                    token: String::new(),
                    only_live: false,
                },
                steps: vec![Step::new(StepKind::Clip, &[])],
            }];
            shortcut_taken(&i, &s)
        };
        assert!(shortcut("Ctrl+Alt+C", "").unwrap().contains("cut"));
        assert!(shortcut("", "note:1:36").unwrap().contains("toggle"));
        assert_eq!(shortcut("Ctrl+Alt+K", "note:1:37"), None);
    }

    #[test]
    fn programs_and_files_are_locked_when_the_request_may_not_set_them_up() {
        use super::super::model::{Step, StepKind};
        let scratch = crate::test_support::TestDir::new("ic-local-steps");
        let dir = scratch.path().to_path_buf();
        let cfg = dir.join("instantclone.config.json");
        let settings = Arc::new(watch::channel(Settings::defaults()).0);
        let mut i = presets::build("delay_command", "d".into(), "").unwrap();
        i.enabled = false;
        i.handlers[0].steps.push(Step::new(
            StepKind::Program,
            &[("path", r"C:\Tools\thing.exe"), ("args", "")],
        ));
        let locked =
            |reply: Reply| reply.0 != "200 OK" && reply.2.contains("needs a dashboard password");

        assert!(locked(save(&i.to_json(), &settings, &cfg, false)));
        assert_eq!(save(&i.to_json(), &settings, &cfg, true).0, "200 OK");
        let switch = |on: bool| json::obj([("id", json::str(&i.id)), ("enabled", Value::Bool(on))]);
        assert!(locked(toggle(&switch(true), &settings, &cfg, false)));
        assert_eq!(
            toggle(&switch(false), &settings, &cfg, false).0,
            "200 OK",
            "switching it off is always allowed"
        );
        let shared = json::obj([
            (
                "recipe",
                json::str(recipe::export(
                    "kit",
                    std::slice::from_ref(&i),
                    (None, Path::new("")),
                    &[],
                )),
            ),
            ("apply", Value::Bool(true)),
            ("allow_local", Value::Bool(true)),
        ]);
        assert!(locked(import(&shared, &settings, &cfg, false)));
    }

    #[test]
    fn the_import_preview_says_who_can_start_it_and_what_it_reaches() {
        use super::super::model::{Step, StepKind};
        let mut i = presets::build("mod_controls", "m".into(), "").unwrap();
        let levels = |i: &Integration| -> Vec<(String, String)> {
            effect_review(i)
                .iter()
                .map(|e| (e.str_or("level", "").into(), e.str_or("text", "").into()))
                .collect()
        };
        assert_eq!(
            trigger_review(&i)[0].str_or("who", ""),
            "only mods",
            "mod controls stay with mods"
        );
        assert!(levels(&i).contains(&("caution".into(), "Changes your delay".into())));

        if let Trigger::ChatCommand { roles, .. } = &mut i.handlers[0].trigger {
            *roles = Roles::EVERYONE;
        }
        i.handlers[0]
            .steps
            .push(Step::new(StepKind::Program, &[("path", ""), ("args", "")]));
        let effects = levels(&i);
        assert_eq!(effects[0].0, "danger", "the worst comes first");
        assert!(effects.contains(&(
            "danger".into(),
            "Anyone in chat can change your delay".into()
        )));
        assert!(effects.contains(&("danger".into(), "Runs a program on this PC".into())));
    }

    /// A recipe's uploads are saved before its integrations are added. If
    /// that fails, only the files it saved are taken back: an identical file
    /// that was already here was reused, and stays.
    #[test]
    fn a_failed_import_only_takes_back_the_uploads_it_saved() {
        let scratch = crate::test_support::TestDir::new("ic-import-media");
        let cfg = scratch.path().join("instantclone.config.json");
        let folder = alert_media::folder(&cfg);
        let mine = alert_media::save(&folder, "ding.mp3", b"mine").unwrap();
        let mut parsed = recipe::Recipe {
            name: "shared".into(),
            integrations: Vec::new(),
            values: Vec::new(),
            alerts: Some(recipe::SharedAlerts {
                look: alerts::Look::default(),
                media: vec![
                    ("ding.mp3".into(), b"mine".to_vec()),
                    ("pop.mp3".into(), b"theirs".to_vec()),
                ],
                left_out: Vec::new(),
            }),
        };
        let (_, added) = keep_shared_alerts(&mut parsed, false, &cfg).unwrap();
        assert_eq!(added, ["pop.mp3"]);
        forget_media(&folder, &added);
        let left: Vec<String> = alert_media::list(&folder)
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(left, [mine]);
    }

    /// A recipe shared without its uploads: its alerts fall back instead of
    /// naming a file the importer doesn't have (or a different one of
    /// theirs with the same name).
    #[test]
    fn alerts_shared_without_their_uploads_fall_back() {
        use crate::integrations::model::{Step, StepKind};
        let scratch = crate::test_support::TestDir::new("ic-import-no-media");
        let cfg = scratch.path().join("instantclone.config.json");
        let mut integration = presets::build("delay_command", "d".into(), "").unwrap();
        integration.handlers[0].steps = vec![Step::new(
            StepKind::Overlay,
            &[("sound", "media:ding.mp3"), ("image", "media:party.gif")],
        )];
        let mut parsed = recipe::Recipe {
            name: "shared".into(),
            integrations: vec![integration],
            alerts: None,
            values: Vec::new(),
        };
        keep_shared_alerts(&mut parsed, false, &cfg).unwrap();
        let step = &parsed.integrations[0].handlers[0].steps[0];
        assert_eq!(step.param("sound"), alerts::THEME_SOUND);
        assert_eq!(step.param("image"), "");
    }

    /// "Show it on stream" for an alert with a title and no text used to
    /// show the generic card and still say the alert was on the page.
    #[test]
    fn an_alert_with_only_a_title_is_shown_as_itself() {
        let titled = test_card(&json::parse(r#"{"title":"New follower","text":" "}"#).unwrap());
        assert_eq!(titled.title, "New follower");
        assert_eq!(titled.text, "");
        assert_eq!(test_card(&json::parse("{}").unwrap()).title, "InstantClone");
    }

    #[test]
    fn summaries_name_the_triggers() {
        // The step count is the preset's own business: only its shape here.
        let i = presets::build("mod_controls", "m".into(), "").unwrap();
        let text = summary(&i);
        assert!(
            text.starts_with("When !cut in chat, !setdelay in chat · "),
            "{text}"
        );
        assert!(text.ends_with(" steps"), "{text}");
        let one = presets::build("delay_command", "d".into(), "").unwrap();
        assert_eq!(summary(&one), "When !delay in chat · 1 step");
    }

    /// Settings and a config file of their own, for tests that save.
    fn scratch(name: &str) -> (Arc<watch::Sender<Settings>>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("ic-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        (
            Arc::new(watch::channel(Settings::defaults()).0),
            dir.join("instantclone.config.json"),
        )
    }

    fn stored(settings: &watch::Sender<Settings>, id: &str) -> Option<Integration> {
        settings
            .borrow()
            .integrations
            .iter()
            .find(|i| i.id == id)
            .cloned()
    }

    #[test]
    fn two_integrations_never_answer_the_same_chat_command() {
        let (settings, cfg) = scratch("commands");
        let first = presets::build("delay_command", "first".into(), "").unwrap();
        assert_eq!(save(&first.to_json(), &settings, &cfg, true).0, "200 OK");

        // A second "!delay", switched on: saved, but as a draft.
        let second = presets::build("delay_command", "second".into(), "").unwrap();
        let reply = save(&second.to_json(), &settings, &cfg, true);
        assert_eq!(reply.0, "200 OK");
        assert!(reply.2.contains(r#""drafted":true"#), "{}", reply.2);
        assert!(reply.2.contains("already answers !delay"), "{}", reply.2);
        assert!(!stored(&settings, "second").unwrap().enabled);

        // Switching it on is refused, and so is an alias that clashes.
        let on = json::obj([("id", json::str("second")), ("enabled", Value::Bool(true))]);
        let refused = toggle(&on, &settings, &cfg, true);
        assert!(
            refused.0 != "200 OK" && refused.2.contains("already answers"),
            "{}",
            refused.2
        );
        let mut alias = second;
        if let Trigger::ChatCommand {
            command, aliases, ..
        } = &mut alias.handlers[0].trigger
        {
            *command = "!wait".into();
            *aliases = vec!["!delay".into()];
        }
        assert!(command_taken(&alias, &settings.borrow()).is_some());

        // Its own command: no clash, and it switches on.
        if let Trigger::ChatCommand { aliases, .. } = &mut alias.handlers[0].trigger {
            aliases.clear();
        }
        let reply = save(&alias.to_json(), &settings, &cfg, true);
        assert!(reply.2.contains(r#""drafted":false"#), "{}", reply.2);
        assert!(stored(&settings, "second").unwrap().enabled);

        // A switched-off one is no clash, and one integration never clashes
        // with itself.
        assert_eq!(command_taken(&first, &settings.borrow()), None);
        let off = json::obj([("id", json::str("first")), ("enabled", Value::Bool(false))]);
        assert_eq!(toggle(&off, &settings, &cfg, true).0, "200 OK");
        let mut again = alias.clone();
        again.id = "third".into();
        if let Trigger::ChatCommand { command, .. } = &mut again.handlers[0].trigger {
            *command = "!delay".into();
        }
        assert_eq!(command_taken(&again, &settings.borrow()), None);

        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn adding_another_chat_command_adds_it_switched_off() {
        let (settings, cfg) = scratch("add-another");
        let add_one = || {
            let reply = add(
                &json::obj([("preset", json::str("delay_command"))]),
                &settings,
                &cfg,
                true,
            );
            assert_eq!(reply.0, "200 OK", "{}", reply.2);
        };
        add_one();
        add_one();
        let on: Vec<bool> = settings
            .borrow()
            .integrations
            .iter()
            .map(|i| i.enabled)
            .collect();
        assert_eq!(on, vec![true, false]);
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn an_unfinished_save_keeps_the_work_without_stopping_a_running_one() {
        let (settings, cfg) = scratch("drafts");
        let mut webhook = presets::build("webhook", "hook".into(), "").unwrap();
        webhook.enabled = true;
        let reply = save(&webhook.to_json(), &settings, &cfg, true);
        assert_eq!(reply.0, "200 OK", "a new one is kept as a draft");
        assert!(reply.2.contains(r#""drafted":true"#), "{}", reply.2);
        assert!(!stored(&settings, "hook").unwrap().enabled);

        // One that runs keeps running as it was: the edit is refused.
        let running = presets::build("delay_command", "cmd".into(), "").unwrap();
        assert_eq!(save(&running.to_json(), &settings, &cfg, true).0, "200 OK");
        let mut broken = running.clone();
        broken.handlers[0].steps[0]
            .params
            .insert("text".into(), String::new());
        let reply = save(&broken.to_json(), &settings, &cfg, true);
        assert_ne!(reply.0, "200 OK");
        assert_eq!(stored(&settings, "cmd").unwrap(), running);
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }

    #[test]
    fn a_copy_lands_right_after_its_original() {
        let (settings, cfg) = scratch("duplicate");
        for id in ["a", "b"] {
            let i = presets::build("delay_notice", id.into(), "").unwrap();
            assert_eq!(save(&i.to_json(), &settings, &cfg, true).0, "200 OK");
        }
        let reply = duplicate(&json::obj([("id", json::str("a"))]), &settings, &cfg);
        assert_eq!(reply.0, "200 OK");
        let order: Vec<String> = settings
            .borrow()
            .integrations
            .iter()
            .map(|i| i.id.clone())
            .collect();
        assert_eq!(order[0], "a");
        assert!(reply.2.contains(&order[1]), "the copy is second: {order:?}");
        assert_eq!(order[2], "b");
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }
}
