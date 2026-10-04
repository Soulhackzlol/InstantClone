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
    MAX_INTEGRATIONS,
};
use super::{engine, presets, recipe, runner, twitch::Which};
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
const LOCAL_STEPS_LOCKED: &str = "running programs and writing files can only be set up on the \
     streaming PC itself, or from here once the dashboard has a password (System > Security)";

pub fn new_id() -> String {
    format!("i{}", &crate::crypto::random_token()[..12])
}

/// Handle `/integrations*`, `/connections/*`, `/twitch/*`. `None` when the
/// path is not ours.
///
/// `local_steps_allowed` is false for a request from another device while
/// the dashboard has no password. Anyone on the network could make such a
/// request, so it may not add, change or switch on a step that runs a
/// program or writes a file: from chat, that would run anything on the PC.
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
        ("GET", "/integrations") => ok(overview(&handle, &settings.borrow(), local_steps_allowed)),
        ("GET", "/integrations/activity") => ok(json::obj([
            ("activity", handle.activity_json()),
            ("stats", handle.stats_json()),
            ("twitch", handle.twitch.status_json()),
        ])),
        ("POST", "/integrations/save") => save(&req(), settings, cfg_path, local_steps_allowed),
        ("POST", "/integrations/add") => add(&req(), settings, cfg_path),
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
        // The live meter of a chat activity trigger being tuned.
        ("POST", "/integrations/meter") => match req().get("trigger").map(meter_rules) {
            Some(Ok(rules)) => ok(handle.chat_meter(&rules)),
            Some(Err(e)) => fail(e),
            None => fail("nothing to measure"),
        },
        ("GET", "/integrations/obs") => ok(handle.obs_status()),
        ("POST", "/integrations/fetch") => fetch(&req()).await,
        ("POST", "/integrations/convert") => convert(&req()),
        ("POST", "/integrations/alerts/test") => {
            handle.alerts.show(
                "InstantClone",
                "This is where your alerts show up on stream.",
                6,
            );
            done()
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
        ("POST", "/integrations/export") => export(&req(), &settings.borrow()),
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

fn overview(handle: &super::Handle, s: &Settings, local_steps_allowed: bool) -> Value {
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
                        (
                            i.id.clone(),
                            Value::Arr(p.invalid.into_iter().map(json::str).collect()),
                        )
                    })
                    .collect(),
            ),
        ),
        ("twitch", handle.twitch.status_json()),
        ("twitch_shared_chat", Value::Bool(s.twitch_shared_chat)),
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
    // Switched on, it must be ready to run; switched off, it saves as a
    // draft and the dashboard lists what is still missing.
    if integration.enabled && !problems.incomplete.is_empty() {
        return fail(format!(
            "{}. Finish it, or switch it off to save it as a draft.",
            problems.incomplete.join("; ")
        ));
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
        let mut prefixes = vec!["counter.".to_string(), "clip.".to_string()];
        match &h.trigger {
            Trigger::Event { kind, .. } => {
                known.extend(kind.vars().iter().map(|(n, _)| n.to_string()))
            }
            Trigger::ChatCommand { .. } | Trigger::ChatMessage { .. } => {
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
            StepKind::EditText => known.push(match s.param("save_as").trim() {
                "" => "text".to_string(),
                name => name.to_string(),
            }),
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

fn add(req: &Value, settings: &Arc<watch::Sender<Settings>>, cfg_path: &Path) -> Reply {
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
            let built =
                presets::build(preset, new_id(), &channel).ok_or("unknown catalog entry")?;
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
        let original = s
            .integrations
            .iter()
            .find(|i| i.id == id)
            .cloned()
            .ok_or("that integration no longer exists")?;
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
        s.integrations.push(copy);
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

fn export(req: &Value, s: &Settings) -> Reply {
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
    ok(json::obj([
        ("ok", Value::Bool(true)),
        ("recipe", json::str(recipe::export(&name, &chosen))),
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
                ])
            })
            .collect();
        return ok(json::obj([
            ("ok", Value::Bool(true)),
            ("name", json::str(&parsed.name)),
            ("items", Value::Arr(items)),
            ("uses_discord", Value::Bool(parsed.uses_discord())),
            ("local_effects", Value::Bool(parsed.has_local_effects())),
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
    let reply = update(settings, cfg_path, move |s| {
        if s.integrations.len() + count > MAX_INTEGRATIONS {
            return Err(format!(
                "that would go over {MAX_INTEGRATIONS} integrations"
            ));
        }
        s.integrations.extend(parsed.integrations);
        Ok(())
    });
    if reply.0 != "200 OK" {
        return reply;
    }
    ok(json::obj([
        ("ok", Value::Bool(true)),
        ("added", Value::Num(count as f64)),
    ]))
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
            StepKind::Obs => add("caution", "Controls OBS (scenes, sources, text)"),
            StepKind::Http => add("caution", "Calls a web address you fill in"),
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
    format!("When {} · {steps} steps", triggers.join(", "))
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
        for p in presets::PRESETS {
            let built = presets::build(p.id, "x".into(), "c").unwrap();
            assert!(
                unknown_vars(&built).is_empty(),
                "{}: {:?}",
                p.id,
                unknown_vars(&built)
            );
        }
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
    fn programs_and_files_are_set_up_only_from_this_pc_without_a_password() {
        use super::super::model::{Step, StepKind};
        let dir = std::env::temp_dir().join(format!("ic-local-steps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("instantclone.config.json");
        let settings = Arc::new(watch::channel(Settings::defaults()).0);
        let mut i = presets::build("delay_command", "d".into(), "").unwrap();
        i.enabled = false;
        i.handlers[0].steps.push(Step::new(
            StepKind::Program,
            &[("path", r"C:\Tools\thing.exe"), ("args", "")],
        ));
        let locked = |reply: Reply| reply.0 != "200 OK" && reply.2.contains("streaming PC");

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
                json::str(recipe::export("kit", std::slice::from_ref(&i))),
            ),
            ("apply", Value::Bool(true)),
            ("allow_local", Value::Bool(true)),
        ]);
        assert!(locked(import(&shared, &settings, &cfg, false)));

        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn summaries_name_the_triggers() {
        let i = presets::build("mod_controls", "m".into(), "").unwrap();
        assert_eq!(
            summary(&i),
            "When !cut in chat, !setdelay in chat · 8 steps"
        );
    }
}
