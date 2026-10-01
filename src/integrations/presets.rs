//! The catalog: ready-made integrations, and the packs that add several at
//! once. Each entry builds an ordinary `Integration`, so anything added
//! from here can be edited freely afterwards.
//!
//! Also home to the one-time migration of the old single Discord webhook
//! setting, which becomes a Discord connection plus a "Stream alerts"
//! integration sending exactly the messages it used to.

use super::event::EventKind;
use super::model::{Handler, Integration, Roles, Step, StepKind, Trigger};
use crate::json::{self, Value};
use std::collections::BTreeMap;

pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    /// `alerts`, `chat` or `auto`: the catalog's categories.
    pub category: &'static str,
    pub description: &'static str,
    /// Connection it posts through: `discord`, `twitch`, `phone`, `web` or `file`.
    pub needs: &'static str,
    pub recommended: bool,
    /// What the catalog card previews: (kind, sample text, sample chat line).
    pub preview: (&'static str, &'static str, &'static str),
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "crash_alert",
        name: "Crash alert",
        category: "alerts",
        description: "One Discord message per drop that updates itself: down, then back.",
        needs: "discord",
        recommended: true,
        preview: (
            "discord",
            "OBS dropped (crash). Holding the stream for 2m 00s.",
            "",
        ),
    },
    Preset {
        id: "destination_down",
        name: "Destination down",
        category: "alerts",
        description: "YouTube, Kick or any destination drops? You know why, right away.",
        needs: "discord",
        recommended: false,
        preview: ("discord", "YouTube dropped: connection timed out.", ""),
    },
    Preset {
        id: "going_live",
        name: "Going live ping",
        category: "alerts",
        description: "Tells your community the moment OBS starts streaming.",
        needs: "discord",
        recommended: false,
        preview: ("discord", "We're live! Come hang out.", ""),
    },
    Preset {
        id: "phone_crash",
        name: "Phone push on crash",
        category: "alerts",
        description: "A push on your lock screen, so you know to run back to the PC.",
        needs: "phone",
        recommended: false,
        preview: (
            "phone",
            "Crash protection is on for 2m 00s. Get back to the PC.",
            "",
        ),
    },
    Preset {
        id: "tell_chat",
        name: "Tell chat",
        category: "chat",
        description: "After 20 s, so short blips never reach chat. Says \"we're back\" too.",
        needs: "twitch",
        recommended: true,
        preview: (
            "chat",
            "OBS dropped, we're coming back. Don't go anywhere!",
            "frozen??",
        ),
    },
    Preset {
        id: "delay_command",
        name: "!delay",
        category: "chat",
        description: "Anyone types it, the bot answers with your delay. Always up to date.",
        needs: "twitch",
        recommended: true,
        preview: ("chat", "Delay: 30s", "!delay"),
    },
    Preset {
        id: "delay_notice",
        name: "Delay on / off notice",
        category: "chat",
        description: "Tells chat when the delay switches. Never says how long.",
        needs: "twitch",
        recommended: false,
        preview: ("chat", "Stream delay is now on.", ""),
    },
    Preset {
        id: "mod_controls",
        name: "Mod controls",
        category: "chat",
        description: "!cut and !setdelay 30 for your mods. Controls the stream.",
        needs: "twitch",
        recommended: false,
        preview: ("chat", "Cut by mod_anna.", "!cut"),
    },
    Preset {
        id: "socials",
        name: "!discord",
        category: "chat",
        description: "A command with your own reply. Rename it to anything.",
        needs: "twitch",
        recommended: false,
        preview: ("chat", "Join the Discord: [YOUR INVITE LINK]", "!discord"),
    },
    Preset {
        id: "api_command",
        name: "Command from a website",
        category: "chat",
        description: "Answers with anything a website returns: ranks, stats, quotes. Paste a Nightbot or StreamElements command, or start from !uptime and friends.",
        needs: "twitch",
        recommended: false,
        preview: ("chat", "Radiant 450RR", "!rank"),
    },
    Preset {
        id: "clip_button",
        name: "Clip button",
        category: "auto",
        description: "A key or a MIDI pad clips the last moments, drops the link in chat and says so on stream.",
        needs: "twitch",
        recommended: false,
        preview: ("chat", "🎬 Clipped: https://clips.twitch.tv/BraveSnipe", ""),
    },
    Preset {
        id: "vod_markers",
        name: "VOD markers",
        category: "auto",
        description: "A marker at every crash and cut, so your editor finds them in seconds.",
        needs: "twitch",
        recommended: false,
        preview: ("marker", "Crash", ""),
    },
    Preset {
        id: "webhook",
        name: "Webhook",
        category: "auto",
        description: "Every event as JSON to n8n, Home Assistant, Zapier or your own tool.",
        needs: "web",
        recommended: false,
        preview: ("json", "", ""),
    },
    Preset {
        id: "crash_counter",
        name: "Crash counter",
        category: "auto",
        description: "Counts crashes into a text file an OBS text source can show.",
        needs: "file",
        recommended: false,
        preview: ("file", "Crashes today: 2", ""),
    },
];

pub struct Pack {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub presets: &'static [&'static str],
}

pub const PACKS: &[Pack] = &[
    Pack {
        id: "crash_safety",
        name: "Crash safety",
        description: "OBS drops: Discord and chat hear about it, and your editor gets a marker.",
        presets: &["crash_alert", "tell_chat", "vod_markers"],
    },
    Pack {
        id: "chat_helper",
        name: "Chat helper",
        description: "Viewers ask, the bot answers. No more editing commands by hand.",
        presets: &["delay_command", "delay_notice", "socials"],
    },
    Pack {
        id: "watchtower",
        name: "Watchtower",
        description: "For when you step away: every problem reaches your phone and Discord.",
        presets: &["destination_down", "phone_crash", "crash_alert"],
    },
];

pub fn find(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

fn on(kind: EventKind, steps: Vec<Step>) -> Handler {
    on_filtered(kind, &[], steps)
}

fn on_filtered(kind: EventKind, filters: &[(&str, &str)], steps: Vec<Step>) -> Handler {
    Handler {
        enabled: true,
        trigger: Trigger::Event {
            kind,
            filters: filters
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
        },
        steps,
    }
}

fn command(command: &str, roles: Roles, user_cooldown_ms: u64, steps: Vec<Step>) -> Handler {
    Handler {
        enabled: true,
        trigger: Trigger::ChatCommand {
            command: command.to_string(),
            aliases: Vec::new(),
            roles,
            user_cooldown_ms,
        },
        steps,
    }
}

fn discord(channel: &str, text: &str) -> Step {
    Step::new(
        StepKind::Discord,
        &[("connection", channel), ("text", text)],
    )
}

/// Edits the message this integration last posted in the channel, so a
/// drop stays one message that changes ("down", then "back").
fn discord_update(channel: &str, text: &str) -> Step {
    Step::new(
        StepKind::Discord,
        &[("connection", channel), ("text", text), ("edit", "last")],
    )
}

fn chat(text: &str) -> Step {
    Step::new(StepKind::Chat, &[("text", text)])
}

fn reply(text: &str) -> Step {
    Step::new(StepKind::Chat, &[("text", text), ("reply", "yes")])
}

fn check(left: &str, op: &str, right: &str, then: Vec<Step>, otherwise: Vec<Step>) -> Step {
    let mut step = Step::new(
        StepKind::If,
        &[("left", left), ("op", op), ("right", right)],
    );
    step.then = then;
    step.otherwise = otherwise;
    step
}

/// Build a catalog entry. `discord_channel` is the id of the Discord
/// connection to post to (empty when none exists yet).
pub fn build(preset_id: &str, id: String, discord_channel: &str) -> Option<Integration> {
    let p = find(preset_id)?;
    let d = discord_channel;
    let mut cooldown_ms = 0;
    let handlers = match preset_id {
        "crash_alert" => {
            let mut ended = on(
                EventKind::HoldEnded,
                vec![discord_update(d, "⏹️ Crash protection ended by the streamer. The stream has ended.")],
            );
            ended.enabled = false;
            vec![
                // `<t:…:R>` is Discord's own countdown: it ticks in the
                // message without anyone editing it.
                on(EventKind::HoldOpened, vec![discord(d, "🔴 OBS dropped ({reason}). Holding the stream: it ends <t:{hold_ends_at}:R> unless OBS is back.")]),
                on(EventKind::ObsBack, vec![discord_update(d, "🟢 OBS dropped, and was back after {down_for}. Live again.")]),
                on(EventKind::HoldExpired, vec![discord_update(d, "⚫ OBS didn't come back within {hold}. The stream has ended.")]),
                ended,
            ]
        }
        "destination_down" => vec![on(
            EventKind::DestinationDropped,
            vec![discord(d, "{destination} dropped: {reason}")],
        )],
        "going_live" => {
            cooldown_ms = 10 * 60_000;
            vec![on(
                EventKind::ObsConnected,
                vec![discord(d, "We're live! Come hang out.")],
            )]
        }
        "phone_crash" => vec![on(
            EventKind::HoldOpened,
            vec![Step::new(
                StepKind::Phone,
                &[
                    ("title", "OBS crashed"),
                    ("text", "Crash protection is on for {hold}. Get back to the PC."),
                    ("priority", "high"),
                ],
            )],
        )],
        "tell_chat" => vec![
            on(
                EventKind::HoldOpened,
                vec![
                    Step::new(StepKind::Wait, &[("ms", "20000")]),
                    check(
                        "{hold_active}",
                        "is",
                        "yes",
                        vec![
                            chat("OBS dropped, we're coming back. Don't go anywhere!"),
                            Step::new(StepKind::Counter, &[("name", "chat_told"), ("op", "set"), ("by", "1")]),
                        ],
                        vec![],
                    ),
                ],
            ),
            on(
                EventKind::ObsBack,
                vec![check(
                    "{counter.chat_told}",
                    "greater",
                    "0",
                    vec![
                        chat("We're back!"),
                        Step::new(StepKind::Counter, &[("name", "chat_told"), ("op", "reset")]),
                    ],
                    vec![],
                )],
            ),
        ],
        "delay_command" => {
            cooldown_ms = 5_000;
            vec![command("!delay", Roles::EVERYONE, 30_000, vec![reply("Delay: {delay|off}")])]
        }
        "delay_notice" => {
            // A burst of cuts must not become a burst of chat lines.
            cooldown_ms = 30_000;
            vec![
                on(EventKind::DelayOn, vec![chat("Stream delay is now on.")]),
                on(EventKind::DelayOff, vec![chat("Stream delay is now off.")]),
            ]
        }
        "mod_controls" => vec![
            command(
                "!cut",
                Roles::MODS,
                0,
                vec![
                    Step::new(StepKind::DelayAction, &[("action", "cut")]),
                    reply("Cut by {user}."),
                ],
            ),
            command(
                "!setdelay",
                Roles::MODS,
                0,
                vec![check(
                    "{arg1}",
                    "greater",
                    "-1",
                    vec![
                        Step::new(StepKind::DelayAction, &[("action", "arm"), ("seconds", "{arg1}")]),
                        reply("Delay set to {arg1}s by {user}."),
                    ],
                    vec![reply("Usage: !setdelay 30")],
                )],
            ),
        ],
        "socials" => {
            cooldown_ms = 10_000;
            vec![command("!discord", Roles::EVERYONE, 30_000, vec![reply("Join the Discord: [YOUR INVITE LINK]")])]
        }
        "api_command" => {
            cooldown_ms = 3_000;
            vec![api_handler(&ApiStart {
                id: "",
                command: "!rank",
                url: "",
                reply: super::botcmd::ANSWER,
                unless: None,
            })]
        }
        "clip_button" => {
            // Twitch refuses clips made seconds apart anyway.
            cooldown_ms = 10_000;
            vec![Handler {
                enabled: true,
                trigger: Trigger::Shortcut {
                    hotkey: String::new(),
                    midi: String::new(),
                },
                steps: vec![
                    Step::new(StepKind::Clip, &[]),
                    check(
                        "{clip.ok}",
                        "is",
                        "yes",
                        vec![
                            chat("🎬 Clipped: {clip.url}"),
                            Step::new(
                                StepKind::Overlay,
                                &[("title", "Clip"), ("text", "That moment is a clip now"), ("seconds", "5")],
                            ),
                        ],
                        vec![],
                    ),
                ],
            }]
        }
        "vod_markers" => vec![
            on(EventKind::HoldOpened, vec![Step::new(StepKind::Marker, &[("description", "Crash ({reason})")])]),
            on(EventKind::DelayOff, vec![Step::new(StepKind::Marker, &[("description", "Delay cut")])]),
        ],
        "webhook" => EventKind::ALL
            .iter()
            .map(|kind| {
                let body = format!(
                    r#"{{"event":"{}","delay_ms":{{delay_ms}},"phase":"{{phase}}","destinations_live":{{destinations_live}}}}"#,
                    kind.id()
                );
                on(
                    *kind,
                    vec![Step::new(StepKind::Http, &[("method", "POST"), ("url", ""), ("body", &body)])],
                )
            })
            .collect(),
        "crash_counter" => vec![on(
            EventKind::HoldOpened,
            vec![
                Step::new(StepKind::Counter, &[("name", "crashes")]),
                Step::new(StepKind::File, &[("path", ""), ("text", "Crashes today: {counter.crashes}")]),
            ],
        )],
        _ => return None,
    };
    let mut integration = Integration {
        id,
        name: p.name.to_string(),
        enabled: true,
        preset: p.id.to_string(),
        cooldown_ms,
        quiet: None,
        handlers,
    };
    // Something still to fill in (a webhook address, a Discord channel):
    // add it switched off; the editor opens right away to finish it.
    if !integration.validate().is_empty() {
        integration.enabled = false;
    }
    Some(integration)
}

/// A ready-made "command from a website": one click in the editor fills
/// the address, the reply and the command. DecAPI answers in plain text
/// and needs no key, which is what makes these work out of the box.
pub struct ApiStart {
    pub id: &'static str,
    pub command: &'static str,
    pub url: &'static str,
    pub reply: &'static str,
    /// When the answer contains this, reply with that instead: the
    /// website's way of saying "offline" or "no such user".
    pub unless: Option<(&'static str, &'static str)>,
}

pub const API_STARTS: &[ApiStart] = &[
    ApiStart {
        id: "uptime",
        command: "!uptime",
        url: "https://decapi.me/twitch/uptime/{channel}",
        reply: "{channel} has been live for {api.body}",
        unless: Some(("offline", "{channel} isn't live right now.")),
    },
    ApiStart {
        id: "accountage",
        command: "!accountage",
        url: "https://decapi.me/twitch/accountage/{target}",
        reply: "{target} made their account {api.body} ago",
        unless: Some(("not found", "Couldn't find {target} on Twitch.")),
    },
    ApiStart {
        id: "followers",
        command: "!followers",
        url: "https://decapi.me/twitch/followcount/{channel}",
        reply: "{channel} has {api.body} followers",
        unless: None,
    },
    ApiStart {
        id: "game",
        command: "!game",
        url: "https://decapi.me/twitch/game/{channel}",
        reply: "Playing {api.body} right now",
        unless: None,
    },
];

/// The handler of a "command from a website": fetch, then reply with the
/// answer, a nicer line for a known "no", or a sorry when the site is down.
pub fn api_handler(start: &ApiStart) -> Handler {
    let answer = match start.unless {
        // The answer's reply comes first in walk order, so it is the one
        // the simple editor edits, not the fallback.
        Some((says, instead)) => vec![check(
            "{api.body}",
            "not_contains",
            says,
            vec![reply(start.reply)],
            vec![reply(instead)],
        )],
        None => vec![reply(start.reply)],
    };
    command(
        start.command,
        Roles::EVERYONE,
        10_000,
        vec![
            Step::new(
                StepKind::Http,
                &[("method", "GET"), ("url", start.url), ("save_as", "api")],
            ),
            check(
                "{api.ok}",
                "is",
                "yes",
                answer,
                vec![reply("Couldn't get that right now, try again in a bit.")],
            ),
        ],
    )
}

/// The integration that replaces the old "Discord webhook" setting: the
/// same events, the same words.
pub fn legacy_alerts(id: String, channel: &str) -> Integration {
    let c = channel;
    Integration {
        id,
        name: "Stream alerts".to_string(),
        enabled: true,
        preset: "stream_alerts".to_string(),
        cooldown_ms: 0,
        quiet: None,
        // Ordered by the life of a stream; the card previews the first.
        handlers: vec![
            on(
                EventKind::ObsConnected,
                vec![discord(c, "✅ OBS publisher connected - going live.")],
            ),
            on_filtered(
                EventKind::ObsDisconnected,
                &[("protected", "no")],
                vec![discord(c, "⚠️ OBS publisher disconnected.")],
            ),
            on(
                EventKind::HoldOpened,
                vec![discord(
                    c,
                    "🛡️ OBS dropped ({reason}). The reconnect screen is on air for up to {hold}.",
                )],
            ),
            on(
                EventKind::ObsBack,
                vec![discord(c, "✅ OBS is back after {down_for}. Live again.")],
            ),
            on(
                EventKind::HoldExpired,
                vec![discord(
                    c,
                    "🔴 OBS didn't come back within {hold}. The stream has ended.",
                )],
            ),
            on(
                EventKind::HoldEnded,
                vec![discord(
                    c,
                    "⏹️ Crash protection ended by the streamer. The stream has ended.",
                )],
            ),
            on(
                EventKind::DestinationLive,
                vec![discord(c, "🟢 **{destination}** is now live.")],
            ),
            on(
                EventKind::DestinationDropped,
                vec![discord(c, "🔴 **{destination}** disconnected: {reason}")],
            ),
            on(
                EventKind::EbDetected,
                vec![discord(
                    c,
                    "🎚️ Enhanced Broadcasting detected - multi-track forwarding active.",
                )],
            ),
        ],
    }
}

pub fn catalog_json() -> Value {
    let presets = PRESETS
        .iter()
        .map(|p| {
            json::obj([
                ("id", json::str(p.id)),
                ("name", json::str(p.name)),
                ("category", json::str(p.category)),
                ("description", json::str(p.description)),
                ("needs", json::str(p.needs)),
                ("recommended", Value::Bool(p.recommended)),
                (
                    "preview",
                    json::obj([
                        ("kind", json::str(p.preview.0)),
                        ("text", json::str(p.preview.1)),
                        ("ask", json::str(p.preview.2)),
                    ]),
                ),
            ])
        })
        .collect();
    let packs = PACKS
        .iter()
        .map(|p| {
            json::obj([
                ("id", json::str(p.id)),
                ("name", json::str(p.name)),
                ("description", json::str(p.description)),
                (
                    "presets",
                    Value::Arr(p.presets.iter().map(|s| json::str(*s)).collect()),
                ),
            ])
        })
        .collect();
    let starts = API_STARTS
        .iter()
        .map(|s| {
            json::obj([
                ("id", json::str(s.id)),
                ("command", json::str(s.command)),
                ("handler", api_handler(s).to_json()),
            ])
        })
        .collect();
    json::obj([
        ("presets", Value::Arr(presets)),
        ("packs", Value::Arr(packs)),
        ("api_starts", Value::Arr(starts)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_builds_and_validates_once_connected() {
        for p in PRESETS {
            let mut i = build(p.id, format!("t_{}", p.id), "chan1")
                .unwrap_or_else(|| panic!("{} did not build", p.id));
            // Fill what a user fills in before switching these on.
            for h in &mut i.handlers {
                fill_blank_paths(&mut h.steps);
                if let Trigger::Shortcut { hotkey, .. } = &mut h.trigger {
                    *hotkey = "Ctrl+Alt+C".into();
                }
            }
            assert!(i.validate().is_empty(), "{}: {:?}", p.id, i.validate());
            assert_eq!(i.preset, p.id);
        }
    }

    fn fill_blank_paths(steps: &mut [Step]) {
        for s in steps {
            for key in ["url", "path"] {
                if s.params.get(key).is_some_and(|v| v.is_empty()) {
                    s.params.insert(key.into(), "https://example.com/x".into());
                }
            }
            fill_blank_paths(&mut s.then);
            fill_blank_paths(&mut s.otherwise);
        }
    }

    #[test]
    fn unfinished_presets_start_switched_off() {
        let webhook = build("webhook", "w".into(), "").unwrap();
        assert!(!webhook.enabled);
        let crash = build("crash_alert", "c".into(), "").unwrap();
        assert!(!crash.enabled, "no Discord channel yet");
        assert!(build("delay_command", "d".into(), "").unwrap().enabled);
    }

    #[test]
    fn packs_only_name_real_presets() {
        for pack in PACKS {
            for id in pack.presets {
                assert!(find(id).is_some(), "{} names unknown {id}", pack.id);
            }
        }
    }

    #[test]
    fn webhook_bodies_are_json_once_filled() {
        let i = build("webhook", "w".into(), "").unwrap();
        let body = i.handlers[0].steps[0].param("body").to_string();
        let filled = super::super::template::render(&body, &|name: &str| {
            Some(match name {
                "phase" => "active".to_string(),
                _ => "3".to_string(),
            })
        });
        assert!(crate::config::is_valid_json(&filled), "{filled}");
    }

    #[test]
    fn api_starts_validate_and_their_reply_is_the_one_edited() {
        for start in API_STARTS {
            let mut i = build("api_command", "a".into(), "").unwrap();
            i.handlers = vec![api_handler(start)];
            assert!(i.validate().is_empty(), "{}: {:?}", start.id, i.validate());
            let unknown = super::super::api::unknown_vars(&i);
            assert!(unknown.is_empty(), "{}: {unknown:?}", start.id);
            let mut first = None;
            super::super::model::visit_steps(&i.handlers[0].steps, &mut |s| {
                if s.kind == StepKind::Chat && first.is_none() {
                    first = Some(s.param("text").to_string());
                }
            });
            assert_eq!(first.as_deref(), Some(start.reply));
        }
    }

    #[test]
    fn crash_alert_is_one_message_that_updates() {
        let i = build("crash_alert", "c".into(), "chan").unwrap();
        let edits: Vec<&str> = i
            .handlers
            .iter()
            .map(|h| h.steps[0].param("edit"))
            .collect();
        assert_eq!(edits, ["", "last", "last", "last"]);
    }

    #[test]
    fn legacy_alerts_validate() {
        assert!(legacy_alerts("legacy".into(), "chan").validate().is_empty());
    }

    #[test]
    fn catalog_is_valid_json() {
        assert!(crate::config::is_valid_json(&catalog_json().to_json()));
    }
}
