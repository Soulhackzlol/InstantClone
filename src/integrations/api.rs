//! The dashboard's integrations API. Every route here is admin-only (the
//! web layer's access gate defaults to that) except `/hooks/<token>`, which
//! is how outside apps trigger a "web call" integration and is guarded by
//! its secret token instead.
//!
//! Requests and answers are JSON. Settings writes follow the web layer's
//! pattern: take the write lock, clone, change, validate, save, publish.

use super::event;
use super::model::{
    valid_discord_webhook, valid_id, DiscordChannel, Integration, PhoneConnection, MAX_INTEGRATIONS,
};
use super::{presets, recipe, runner, twitch::Which};
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

pub fn new_id() -> String {
    format!("i{}", &crate::crypto::random_token()[..12])
}

/// Handle `/integrations*`, `/connections/*`, `/twitch/*`. `None` when the
/// path is not ours.
pub async fn route(
    method: &str,
    path: &str,
    body: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> Option<Reply> {
    let handle = ctrl.integrations()?.clone();
    let req = || json::parse(body).unwrap_or(Value::Null);
    let reply = match (method, path) {
        ("GET", "/integrations") => ok(overview(&handle, &settings.borrow())),
        ("GET", "/integrations/activity") => ok(json::obj([
            ("activity", handle.activity_json()),
            ("stats", handle.stats_json()),
            ("twitch", handle.twitch.status_json()),
        ])),
        ("POST", "/integrations/save") => save(&req(), settings, cfg_path),
        ("POST", "/integrations/add") => add(&req(), settings, cfg_path),
        ("POST", "/integrations/toggle") => toggle(&req(), settings, cfg_path),
        ("POST", "/integrations/delete") => {
            let id = req().str_or("id", "").to_string();
            update(settings, cfg_path, |s| {
                s.integrations.retain(|i| i.id != id);
                Ok(())
            })
        }
        ("POST", "/integrations/duplicate") => duplicate(&req(), settings, cfg_path),
        ("POST", "/integrations/test") => test(&req(), &handle).await,
        ("POST", "/integrations/fetch") => fetch(&req()).await,
        ("POST", "/integrations/export") => export(&req(), &settings.borrow()),
        ("POST", "/integrations/import") => import(&req(), settings, cfg_path),
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
        ("POST", "/twitch/logout") => {
            handle.twitch_logout(Which::from_id(req().str_or("which", "main")));
            done()
        }
        _ => return None,
    };
    Some(reply)
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

fn overview(handle: &super::Handle, s: &Settings) -> Value {
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
        ("global", event::vars_json(runner::GLOBAL_VARS)),
        (
            "chat",
            event::vars_json(&[
                ("user", "Ana"),
                ("user_login", "ana"),
                ("user_role", "mod"),
                ("message", "!delay"),
                ("args", "30"),
                ("arg1", "30"),
            ]),
        ),
        (
            "web",
            event::vars_json(&[("body", "{\"x\":1}"), ("query", "a=1")]),
        ),
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
        ("catalog", presets::catalog_json()),
        ("events", event::catalog_json()),
        ("vars", vars),
        ("activity", handle.activity_json()),
        ("stats", handle.stats_json()),
        ("dropped_events", Value::Num(handle.dropped_events() as f64)),
    ])
}

/// `https://discord.com/api/webhooks/1234/abcd...` shown as `…/1234/••••`.
fn mask_webhook(url: &str) -> String {
    let parts: Vec<&str> = url.trim_end_matches('/').rsplitn(3, '/').collect();
    match parts.as_slice() {
        [_token, id, _] => format!("…/webhooks/{id}/••••"),
        _ => "set".to_string(),
    }
}

fn save(req: &Value, settings: &Arc<watch::Sender<Settings>>, cfg_path: &Path) -> Reply {
    let mut integration = match Integration::from_json(req) {
        Ok(i) => i,
        Err(e) => return fail(e),
    };
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
    use super::model::{visit_steps, StepKind, Trigger};
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
            Trigger::ChatCommand { .. } | Trigger::ChatMessage { .. } => known.extend(
                [
                    "user",
                    "user_login",
                    "user_role",
                    "message",
                    "args",
                    "arg1",
                    "arg2",
                    "arg3",
                ]
                .map(String::from),
            ),
            Trigger::Webhook { .. } => {
                known.extend(["body", "query"].map(String::from));
                prefixes.push("body.".to_string());
            }
            Trigger::Timer { .. } => {}
        }
        visit_steps(&h.steps, &mut |s| match s.kind {
            StepKind::Http => prefixes.push(match s.param("save_as").trim() {
                "" => "response.".to_string(),
                name => format!("{name}."),
            }),
            StepKind::SetVar => known.push(s.param("name").trim().to_string()),
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

/// Every Discord step must point at a connection that exists.
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

fn toggle(req: &Value, settings: &Arc<watch::Sender<Settings>>, cfg_path: &Path) -> Reply {
    let id = req.str_or("id", "").to_string();
    let enabled = req.bool_or("enabled", true);
    update(settings, cfg_path, |s| {
        let channels = s.discord_channels.clone();
        let integration = s
            .integrations
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or("that integration no longer exists")?;
        if enabled {
            let errors = integration.validate();
            if !errors.is_empty() {
                return Err(format!("finish setting it up first: {}", errors.join("; ")));
            }
            let probe = Settings {
                discord_channels: channels,
                ..Settings::defaults()
            };
            check_connections(integration, &probe)?;
        }
        integration.enabled = enabled;
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
        let mut copy = original;
        copy.id = copy_id;
        copy.name = format!("{} (copy)", copy.name).chars().take(80).collect();
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
    match handle.test(integration, handler).await {
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
fn import(req: &Value, settings: &Arc<watch::Sender<Settings>>, cfg_path: &Path) -> Reply {
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

/// One line describing an integration, for the import preview.
fn summary(i: &Integration) -> String {
    let triggers: Vec<String> = i
        .handlers
        .iter()
        .map(|h| match &h.trigger {
            super::model::Trigger::Event { kind, .. } => kind.label().to_string(),
            super::model::Trigger::ChatCommand { command, .. } => format!("{command} in chat"),
            super::model::Trigger::ChatMessage { .. } => "a chat message".to_string(),
            super::model::Trigger::Timer { every_ms, .. } => {
                format!("every {} min", every_ms / 60_000)
            }
            super::model::Trigger::Webhook { .. } => "a web call".to_string(),
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
        let body = super::effects::discord_body("🧪 Test message: InstantClone can post here.", "");
        crate::https::https_agent()
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
        crate::https::https_agent()
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
    fn webhooks_are_masked() {
        assert_eq!(
            mask_webhook("https://discord.com/api/webhooks/123/abcDEF"),
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
    fn summaries_name_the_triggers() {
        let i = presets::build("mod_controls", "m".into(), "").unwrap();
        assert_eq!(
            summary(&i),
            "When !cut in chat, !setdelay in chat · 6 steps"
        );
    }
}
