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
    /// What it can't work without: `discord`, `twitch`, `phone`, `web`
    /// (an address to send to), `file` or `obs`. The first is the main one.
    pub needs: &'static [&'static str],
    pub recommended: bool,
    /// What the catalog card previews: (kind, sample text, sample chat line).
    pub preview: (&'static str, &'static str, &'static str),
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "crash_alert",
        name: "OBS crash alert",
        category: "alerts",
        description: "One Discord message per OBS crash that updates itself: down, then back on air.",
        needs: &["discord"],
        recommended: true,
        preview: (
            "discord",
            "OBS dropped (crash). Holding the stream for 2m 00s.",
            "",
        ),
    },
    Preset {
        id: "smart_alerts",
        name: "Smart platform alerts",
        category: "alerts",
        description: "Quiet on blips. One live-updating card when YouTube, Kick or any platform is really down or keeps dropping, and a health report when you end.",
        needs: &["discord"],
        recommended: true,
        preview: ("discord", "🔴 YouTube is down", ""),
    },
    Preset {
        id: "stream_timeline",
        name: "Stream timeline",
        category: "alerts",
        description: "One Discord card per stream that writes itself: crashes, delay changes and platform drops, each with its time in the VOD. Add Highlight button or Chapters and they land here too.",
        needs: &["discord"],
        recommended: true,
        preview: ("discord", "📼 Stream timeline", ""),
    },
    Preset {
        id: "destination_down",
        name: "Platform dropped",
        category: "alerts",
        description: "Every time YouTube, Kick or any platform drops, you know why, right away. Even short blips.",
        needs: &["discord"],
        recommended: false,
        preview: ("discord", "YouTube dropped: connection timed out.", ""),
    },
    Preset {
        id: "going_live",
        name: "Going live ping",
        category: "alerts",
        description: "Tells your community the moment OBS starts streaming.",
        needs: &["discord"],
        recommended: false,
        preview: ("discord", "We're live! Come hang out.", ""),
    },
    Preset {
        id: "delay_status",
        name: "Delay status",
        category: "alerts",
        description: "One Discord message that follows your delay: armed, ready, on, off.",
        needs: &["discord"],
        recommended: false,
        preview: ("discord", "✅ Delay ready: 30s. It can go on air.", ""),
    },
    Preset {
        id: "phone_crash",
        name: "Phone push on crash",
        category: "alerts",
        description: "A push on your lock screen, so you know to run back to the PC.",
        needs: &["phone"],
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
        needs: &["twitch"],
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
        needs: &["twitch"],
        recommended: true,
        preview: ("chat", "Delay: 30s", "!delay"),
    },
    Preset {
        id: "delay_notice",
        name: "Delay on / off notice",
        category: "chat",
        description: "Tells chat when the delay switches. Never says how long.",
        needs: &["twitch"],
        recommended: false,
        preview: ("chat", "Stream delay is now on.", ""),
    },
    Preset {
        id: "mod_controls",
        name: "Mod controls",
        category: "chat",
        description: "!cut and !setdelay 30 for your mods. Controls the stream.",
        needs: &["twitch"],
        recommended: false,
        preview: ("chat", "Cut by mod_anna.", "!cut"),
    },
    Preset {
        id: "socials",
        name: "!discord",
        category: "chat",
        description: "A command with your own reply. Rename it to anything.",
        needs: &["twitch"],
        recommended: false,
        preview: ("chat", "Join the Discord: [YOUR INVITE LINK]", "!discord"),
    },
    Preset {
        id: "chapters",
        name: "Chapters",
        category: "chat",
        description: "Mods type \"!chapter Boss fight\" in chat to mark the VOD. When you end, you get a YouTube chapter list ready to paste.",
        needs: &["twitch", "discord"],
        recommended: false,
        preview: ("chat", "📌 Chapter \"Boss fight\" at 1:04:10", "!chapter Boss fight"),
    },
    Preset {
        id: "api_command",
        name: "Command from a website",
        category: "chat",
        description: "Replies with anything a website returns: a rank, a stat, a quote.",
        needs: &["twitch"],
        recommended: false,
        preview: ("chat", "Radiant 450RR", "!rank"),
    },
    Preset {
        id: "highlight",
        name: "Highlight button",
        category: "auto",
        description: "A hotkey, MIDI pad or Stream Deck button clips the moment once it aired, marks the VOD and posts the clip to Discord.",
        needs: &["twitch", "discord"],
        recommended: true,
        preview: ("discord", "⭐ New highlight", ""),
    },
    Preset {
        id: "hype_clip",
        name: "Hype clip",
        category: "auto",
        description: "Chat goes wild, it clips. Rules you can stack, measured against your own chat, so it fits any channel size.",
        needs: &["twitch", "discord"],
        recommended: true,
        preview: ("discord", "🔥 Chat went wild", ""),
    },
    Preset {
        id: "on_air_light",
        name: "On-air light",
        category: "auto",
        description: "A real light by your door: red when live, amber with the delay on, flashing if OBS crashed. Home Assistant, WLED, Hue.",
        needs: &["web"],
        recommended: false,
        preview: ("json", "", ""),
    },
    Preset {
        id: "scene_delay",
        name: "Scene delay",
        category: "auto",
        description: "Your OBS scene runs the delay: on in game, cut back to live in the lobby. Waits a moment, so flicking through scenes does nothing.",
        needs: &["obs"],
        recommended: false,
        preview: ("action", "", ""),
    },
    Preset {
        id: "clip_button",
        name: "Clip button",
        category: "auto",
        description: "One key or MIDI pad clips the moment and posts the link in chat.",
        needs: &["twitch"],
        recommended: false,
        preview: ("chat", "🎬 Clipped: https://clips.twitch.tv/BraveSnipe", ""),
    },
    Preset {
        id: "vod_markers",
        name: "VOD markers",
        category: "auto",
        description: "A marker at every crash and cut, so your editor finds them in seconds.",
        needs: &["twitch"],
        recommended: false,
        preview: ("marker", "Crash", ""),
    },
    Preset {
        id: "webhook",
        name: "Webhook",
        category: "auto",
        description: "Every event as JSON to n8n, Home Assistant, Zapier or your own tool.",
        needs: &["web"],
        recommended: false,
        preview: ("json", "", ""),
    },
    Preset {
        id: "crash_counter",
        name: "Crash counter",
        category: "auto",
        description: "Counts crashes into a text file an OBS text source can show.",
        needs: &["file"],
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
        id: "stream_pro",
        name: "Stream pro",
        description: "Your whole stream in Discord: a live timeline, highlights, chapters and alerts that only speak up when it matters.",
        presets: &["stream_timeline", "smart_alerts", "highlight", "chapters"],
    },
    Pack {
        id: "highlights",
        name: "Highlights",
        description: "Never miss a clip: a button for you, and chat's reaction for when you're too busy to press it.",
        presets: &["highlight", "hype_clip", "chapters"],
    },
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
        description: "For when you step away: OBS crashes reach your phone, platform problems reach Discord.",
        presets: &["smart_alerts", "phone_crash", "crash_alert"],
    },
    Pack {
        id: "studio",
        name: "Studio",
        description: "Your room and OBS join in: an on-air light by the door, and the delay following your scenes.",
        presets: &["on_air_light", "scene_delay"],
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

// Card colors: the dashboard's own palette, which reads well on Discord's
// dark theme too.
const RED: &str = "#f2665a";
const GREEN: &str = "#3fcf8e";
const AMBER: &str = "#f0a53a";
const BLUE: &str = "#5ac8fa";
const GOLD: &str = "#fcd34d";
const GREY: &str = "#8b949e";
const PURPLE: &str = "#a78bfa";

/// A Discord card: a colored embed with a title, a body and a timestamp.
fn card(channel: &str, color: &str, title: &str, text: &str) -> Step {
    Step::new(
        StepKind::Discord,
        &[
            ("connection", channel),
            ("style", "card"),
            ("color", color),
            ("title", title),
            ("text", text),
            ("footer", "InstantClone"),
            ("timestamp", "yes"),
        ],
    )
}

/// `step` with more parameters set.
fn with(mut step: Step, params: &[(&str, &str)]) -> Step {
    for (k, v) in params {
        step.params.insert(k.to_string(), v.to_string());
    }
    step
}

fn chat(text: &str) -> Step {
    Step::new(StepKind::Chat, &[("text", text)])
}

/// A line for this stream's timeline.
fn line(kind: &str, at: &str, text: &str) -> Step {
    Step::new(
        StepKind::Timeline,
        &[("kind", kind), ("at", at), ("text", text)],
    )
}

/// A button: a hotkey, a MIDI pad or a Stream Deck address. It comes with
/// an address so a Stream Deck works straight away; keys are picked after.
fn button(steps: Vec<Step>) -> Handler {
    Handler {
        enabled: true,
        trigger: Trigger::Shortcut {
            hotkey: String::new(),
            midi: String::new(),
            token: crate::crypto::random_token()[..32].to_string(),
            only_live: false,
        },
        steps,
    }
}

fn wait_aired(extra_ms: &str) -> Step {
    Step::new(StepKind::WaitDelay, &[("extra_ms", extra_ms)])
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
        "crash_alert" => crash_alert(d),
        "delay_status" => delay_status(d),
        "smart_alerts" => smart_alerts(d),
        "stream_timeline" => stream_timeline(d),
        "chapters" => chapters(d),
        "highlight" => {
            // Twitch refuses clips made seconds apart anyway.
            cooldown_ms = 10_000;
            highlight(d)
        }
        "hype_clip" => {
            cooldown_ms = 2 * 60_000;
            hype_clip(d)
        }
        "on_air_light" => on_air_light(),
        "scene_delay" => scene_delay(),
        "destination_down" => vec![on(
            EventKind::DestinationDropped,
            vec![with(
                card(d, RED, "🔴 {destination} dropped", "{reason}"),
                &[("fields", "Platform | {platform}")],
            )],
        )],
        "going_live" => {
            cooldown_ms = 10 * 60_000;
            vec![on(
                EventKind::StreamStarted,
                vec![with(
                    card(d, PURPLE, "🔴 Live now", "We're live! Come hang out."),
                    &[("url", "https://twitch.tv/{channel}")],
                )],
            )]
        }
        "phone_crash" => vec![on(
            EventKind::HoldOpened,
            vec![Step::new(
                StepKind::Phone,
                &[
                    ("title", "OBS crashed"),
                    (
                        "text",
                        "Crash protection is on for {hold}. Get back to the PC.",
                    ),
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
                            Step::new(
                                StepKind::Counter,
                                &[("name", "chat_told"), ("op", "set"), ("by", "1")],
                            ),
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
            vec![command(
                "!delay",
                Roles::EVERYONE,
                30_000,
                vec![reply("Delay: {delay|off}")],
            )]
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
                    "whole_number",
                    "",
                    vec![check(
                        "{arg1}",
                        "less",
                        "601",
                        vec![
                            Step::new(
                                StepKind::DelayAction,
                                &[("action", "arm"), ("seconds", "{arg1}")],
                            ),
                            reply("Delay set to {arg1}s by {user}."),
                        ],
                        vec![reply("The delay goes up to 600s. Usage: !setdelay 30")],
                    )],
                    vec![reply(
                        "Usage: !setdelay 30, in whole seconds (0 turns it off)",
                    )],
                )],
            ),
        ],
        "socials" => {
            cooldown_ms = 10_000;
            vec![command(
                "!discord",
                Roles::EVERYONE,
                30_000,
                vec![reply("Join the Discord: [YOUR INVITE LINK]")],
            )]
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
                    token: String::new(),
                    only_live: false,
                },
                steps: vec![
                    // Twitch clips what it has received; with a delay, the
                    // moment the key was pressed for arrives that much later.
                    wait_aired("3000"),
                    Step::new(StepKind::Clip, &[]),
                    check(
                        "{clip.ok}",
                        "is",
                        "yes",
                        vec![
                            chat("🎬 Clipped: {clip.url}"),
                            Step::new(
                                StepKind::Overlay,
                                &[
                                    ("title", "Clip"),
                                    ("text", "That moment is a clip now"),
                                    ("seconds", "5"),
                                ],
                            ),
                        ],
                        vec![],
                    ),
                ],
            }]
        }
        "vod_markers" => vec![
            on(
                EventKind::HoldOpened,
                vec![Step::new(
                    StepKind::Marker,
                    &[("description", "Crash ({reason})")],
                )],
            ),
            on(
                EventKind::DelayOff,
                vec![Step::new(StepKind::Marker, &[("description", "Delay cut")])],
            ),
        ],
        "webhook" => EventKind::ALL
            .iter()
            .map(|kind| {
                let body = webhook_body(*kind);
                on(
                    *kind,
                    vec![Step::new(
                        StepKind::Http,
                        &[("method", "POST"), ("url", ""), ("body", &body)],
                    )],
                )
            })
            .collect(),
        "crash_counter" => vec![on(
            EventKind::HoldOpened,
            vec![
                Step::new(StepKind::Counter, &[("name", "crashes")]),
                Step::new(
                    StepKind::File,
                    &[("path", ""), ("text", "Crashes today: {counter.crashes}")],
                ),
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

/// What the catalog's webhook sends for one event.
pub fn webhook_body(kind: EventKind) -> String {
    format!(
        r#"{{"event":"{}","delay_ms":{{delay_ms}},"phase":"{{phase}}","destinations_live":{{destinations_live}}}}"#,
        kind.id()
    )
}

/// One card per drop that changes as the drop does: down, then back.
fn crash_alert(d: &str) -> Vec<Handler> {
    let finish = |color, title, text| with(card(d, color, title, text), &[("edit", "last")]);
    let mut ended = on(
        EventKind::HoldEnded,
        vec![finish(
            GREY,
            "⏹️ Stream ended",
            "You ended crash protection after {down_for}.",
        )],
    );
    ended.enabled = false;
    vec![
        // `<t:…:R>` is Discord's own countdown: it ticks in the message
        // without anyone editing it.
        on(
            EventKind::HoldOpened,
            vec![with(
                card(
                    d,
                    RED,
                    "🔴 OBS dropped",
                    "Viewers see the reconnect screen. The stream ends <t:{hold_ends_at}:R> unless OBS is back.",
                ),
                &[("fields", "Why | {reason}\nHolding for | {hold}")],
            )],
        ),
        on(
            EventKind::ObsBack,
            vec![finish(GREEN, "🟢 Back on air", "OBS was gone for {down_for}. Viewers never left.")],
        ),
        on(
            EventKind::HoldExpired,
            vec![finish(GREY, "⚫ Stream ended", "OBS didn't come back within {hold}.")],
        ),
        ended,
    ]
}

/// One card per arming, edited as the delay moves along.
fn delay_status(d: &str) -> Vec<Handler> {
    let state = |kind, color, title, text, edit| {
        let step = card(d, color, title, text);
        on(
            kind,
            vec![if edit {
                with(step, &[("edit", "last")])
            } else {
                step
            }],
        )
    };
    vec![
        state(
            EventKind::DelayArmed,
            AMBER,
            "⏳ Delay armed: {delay}",
            "Filling the buffer…",
            false,
        ),
        state(
            EventKind::DelayReady,
            BLUE,
            "✅ Delay ready: {delay}",
            "It can go on air.",
            true,
        ),
        state(
            EventKind::DelayOn,
            GREEN,
            "🟢 Delay on air: {delay}",
            "Viewers are {delay} behind.",
            true,
        ),
        state(
            EventKind::DelayChanged,
            BLUE,
            "🔁 Delay now {delay}",
            "It was {previous}.",
            true,
        ),
        state(
            EventKind::DelayOff,
            GREY,
            "⚪ Delay off",
            "Viewers see you live.",
            true,
        ),
        state(
            EventKind::DelayDisarmed,
            GREY,
            "⚪ Delay cancelled",
            "It never went on air.",
            true,
        ),
    ]
}

/// Destinations, judged on patterns instead of every blip: one card per
/// outage that turns green when it ends, one per run of drops, and a
/// health report at the end.
fn smart_alerts(d: &str) -> Vec<Handler> {
    let about = |key: &str, step: Step| with(step, &[("key", key)]);
    let down = about(
        "{destination}",
        with(
            card(
                d,
                RED,
                "🔴 {destination} is down",
                "It dropped {down_for} ago and hasn't come back. InstantClone keeps retrying.",
            ),
            &[("fields", "Why | {reason}\nDrops this stream | {drops}")],
        ),
    );
    let back = about(
        "{destination}",
        with(
            card(
                d,
                GREEN,
                "🟢 {destination} is back",
                "It was down for {down_for}.",
            ),
            &[("edit", "close")],
        ),
    );
    let flapping = about(
        "flap {destination}",
        with(
            card(
                d,
                AMBER,
                "🟠 {destination} keeps dropping",
                "{drops} drops in the last {within}. Viewers there may see buffering.",
            ),
            &[("edit", "last"), ("fields", "Last reason | {reason}")],
        ),
    );
    let steady = about(
        "flap {destination}",
        with(
            card(
                d,
                GREEN,
                "✅ {destination} is steady again",
                "{drops} drops this stream, down {down_total} in total.",
            ),
            &[("edit", "close")],
        ),
    );
    let report = with(
        card(d, BLUE, "📊 Stream health", "{report}"),
        &[(
            "fields",
            "Duration | {duration}\nCrashes | {crashes}\nDrops | {drops}",
        )],
    );
    let mut phone = on(
        EventKind::DestinationStillDown,
        vec![Step::new(
            StepKind::Phone,
            &[
                ("title", "{destination} is still down"),
                ("text", "Down for {down_for}: {reason}"),
                ("priority", "high"),
            ],
        )],
    );
    set_filter(&mut phone, "after_s", "180");
    phone.enabled = false;
    vec![
        on(EventKind::DestinationStillDown, vec![down]),
        on(EventKind::DestinationRecovered, vec![back]),
        on(EventKind::DestinationUnstable, vec![flapping]),
        on(EventKind::DestinationSteady, vec![steady]),
        on(EventKind::StreamEnded, vec![report]),
        phone,
    ]
}

fn set_filter(handler: &mut Handler, name: &str, value: &str) {
    if let Trigger::Event { filters, .. } = &mut handler.trigger {
        filters.insert(name.to_string(), value.to_string());
    }
}

/// A card that writes itself as the stream goes: every line any
/// integration adds lands in it, with its place in the VOD.
fn stream_timeline(d: &str) -> Vec<Handler> {
    let timeline_card = |title: &str| {
        with(
            card(d, BLUE, title, "{timeline|Waiting for the first moment…}"),
            &[
                ("key", "timeline"),
                ("footer", "Updates as it happens · InstantClone"),
            ],
        )
    };
    let note = |kind, at: &str, text: &str| on(kind, vec![line("note", at, text)]);
    // A platform gets a "down" line once it stayed down a minute, and a
    // "back" line only then: a flapping one would fill the timeline (and
    // edit the card) every few seconds. The two numbers go together.
    let mut down = note(
        EventKind::DestinationStillDown,
        "aired",
        "📡 {destination} down ({reason})",
    );
    set_filter(&mut down, "after_s", "60");
    let back = on(
        EventKind::DestinationRecovered,
        vec![check(
            "{down_s}",
            "greater",
            "59",
            vec![line(
                "note",
                "aired",
                "📡 {destination} back after {down_for}",
            )],
            vec![],
        )],
    );
    vec![
        // The card goes out before the first line, so that line's update
        // finds it and edits it.
        on(
            EventKind::StreamStarted,
            vec![
                timeline_card("📼 Stream timeline · {date}"),
                line("note", "aired", "🟢 Stream started"),
            ],
        ),
        on(
            EventKind::TimelineUpdated,
            vec![with(
                timeline_card("📼 Stream timeline · {date}"),
                &[("edit", "last")],
            )],
        ),
        note(EventKind::HoldOpened, "live", "🔴 OBS off air ({reason})"),
        note(EventKind::ObsBack, "live", "🟢 OBS back after {down_for}"),
        note(EventKind::DelayOn, "aired", "⏱️ Delay on ({delay})"),
        note(
            EventKind::DelayChanged,
            "aired",
            "⏱️ Delay {previous} → {delay}",
        ),
        note(EventKind::DelayOff, "aired", "⏱️ Delay off"),
        down,
        back,
        on(
            EventKind::StreamEnded,
            vec![with(
                timeline_card("📼 Stream timeline · {date}"),
                &[
                    ("edit", "last"),
                    ("color", GREY),
                    ("footer", "Stream ended · InstantClone"),
                    (
                        "fields",
                        "Duration | {duration}\nHighlights | {highlights}\nCrashes | {crashes}",
                    ),
                ],
            )],
        ),
    ]
}

/// VOD markers plus a YouTube chapter list, from chat or a button.
fn chapters(d: &str) -> Vec<Handler> {
    let mark = |name: &str| {
        vec![
            line("chapter", "aired", name),
            Step::new(StepKind::Marker, &[("description", name)]),
        ]
    };
    let mut from_chat = mark("{args}");
    from_chat.push(reply("📌 Chapter \"{args}\" at {uptime}"));
    // Mods watch the stream, so what they name has already aired: no wait.
    let typed = command(
        "!chapter",
        Roles::MODS,
        0,
        vec![check(
            "{args}",
            "not_empty",
            "",
            from_chat,
            vec![reply("Name it: !chapter Boss fight")],
        )],
    );
    // A button is pressed live: the chapter starts once that moment airs.
    let mut pressed = vec![wait_aired("0")];
    pressed.extend(mark("New chapter"));
    let mut pressed = button(pressed);
    pressed.enabled = false;
    // A crash gets its own chapter, and the stream goes back to the one
    // before once OBS is back. Both off at first: not everyone wants it.
    let mut crash = on(
        EventKind::HoldOpened,
        vec![line("chapter", "live", "Technical difficulties")],
    );
    crash.enabled = false;
    let mut back = on(
        EventKind::ObsBack,
        vec![line("chapter", "live", "{previous_chapter}")],
    );
    back.enabled = false;
    let list = with(
        card(
            d,
            PURPLE,
            "📑 YouTube chapters",
            "```\n{chapters}\n```\nPaste these into the video's description and YouTube makes them chapters.",
        ),
        &[("fields", "Stream | {duration}")],
    );
    // YouTube needs three chapters or more; fewer is no list, and no card.
    let posted = check("{chapters}", "not_empty", "", vec![list], vec![]);
    vec![
        typed,
        pressed,
        crash,
        back,
        on(EventKind::StreamEnded, vec![posted]),
    ]
}

/// Clip the moment once viewers have seen it, mark the VOD, put it on the
/// timeline and in Discord, where the link unfurls into a player.
fn highlight(d: &str) -> Vec<Handler> {
    let post = |by: &str| {
        let mut steps = vec![
            Step::new(StepKind::Clip, &[]),
            check(
                "{clip.ok}",
                "is",
                "yes",
                vec![
                    Step::new(StepKind::Marker, &[("description", "Highlight")]),
                    line("highlight", "aired", "⭐ Highlight · [clip]({clip.url})"),
                    with(
                        card(d, GOLD, "⭐ New highlight", "[Watch the clip]({clip.url})"),
                        &[
                            ("above", "{clip.url}"),
                            ("fields", &format!("Stream time | {{uptime}}\nBy | {by}")),
                        ],
                    ),
                    Step::new(
                        StepKind::Overlay,
                        &[
                            ("title", "Highlight"),
                            ("text", "That moment is saved"),
                            ("seconds", "4"),
                        ],
                    ),
                ],
                // No clip (Twitch said no, or isn't connected): the moment
                // is still marked, and the streamer sees why.
                vec![
                    Step::new(StepKind::Marker, &[("description", "Highlight")]),
                    line("highlight", "aired", "⭐ Highlight (no clip: {clip.error})"),
                    Step::new(
                        StepKind::Overlay,
                        &[
                            ("title", "Highlight"),
                            ("text", "Marked, but Twitch couldn't clip it"),
                            ("seconds", "4"),
                        ],
                    ),
                ],
            ),
        ];
        steps.insert(0, wait_aired("3000"));
        steps
    };
    // Mods hear back in chat either way.
    let mut from_mods = post("{user}")[1..].to_vec();
    if let Some(found) = from_mods.iter_mut().find(|s| s.kind == StepKind::If) {
        found.then.push(reply("⭐ Clipped: {clip.url}"));
        found
            .otherwise
            .push(reply("Couldn't clip that: {clip.error}"));
    }
    let mut from_mods = command("!highlight", Roles::MODS, 0, from_mods);
    from_mods.enabled = false;
    vec![button(post("{channel}")), from_mods]
}

/// Chat decides: when it gets busy in the ways the rules say, clip it.
fn hype_clip(d: &str) -> Vec<Handler> {
    use super::model::{ActivityRule, ChatActivity, RuleKind};
    let rule = |kind, value, words: &str| ActivityRule {
        kind,
        value,
        words: words.to_string(),
    };
    let trigger = Trigger::ChatActivity(ChatActivity {
        window_ms: 15_000,
        match_all: true,
        rules: vec![
            rule(RuleKind::Busier, 3.0, ""),
            rule(RuleKind::Chatters, 5.0, ""),
            rule(
                RuleKind::Words,
                25.0,
                "clip, pog, lul, kekw, omg, wtf, lol, w",
            ),
        ],
        roles: Roles::EVERYONE,
        only_live: true,
    });
    let steps =
        vec![
            Step::new(StepKind::Clip, &[]),
            check(
                "{clip.ok}",
                "is",
                "yes",
                vec![
                line("highlight", "aired", "🔥 Chat went wild · [clip]({clip.url})"),
                with(
                    card(d, AMBER, "🔥 Chat went wild", "[Watch the clip]({clip.url})"),
                    &[
                        ("above", "{clip.url}"),
                        (
                            "fields",
                            "Chat | {busier}× busier\nPeople | {chatters}\nTop word | {top_word|-}",
                        ),
                    ],
                ),
            ],
                vec![line(
                    "highlight",
                    "aired",
                    "🔥 Chat went wild (no clip: {clip.error})",
                )],
            ),
        ];
    vec![Handler {
        enabled: true,
        trigger,
        steps,
    }]
}

/// The on-air state as a light. Bodies follow Home Assistant's webhook
/// style; the editor's device presets rewrite them for WLED or Hue.
fn on_air_light() -> Vec<Handler> {
    ["live", "delay", "crash", "off"]
        .iter()
        .map(|state| {
            let body = format!(r#"{{"state":"{state}"}}"#);
            let mut h = on(
                EventKind::OnAirChanged,
                vec![Step::new(
                    StepKind::Http,
                    &[("method", "POST"), ("url", ""), ("body", &body)],
                )],
            );
            set_filter(&mut h, "state", state);
            h
        })
        .collect()
}

/// Two scene rules to fill in: a delay for the game scene, live again for
/// the rest. Scenes are picked from OBS's own list in the editor.
fn scene_delay() -> Vec<Handler> {
    // Only while streaming: flicking through scenes before going live does
    // nothing, and the stream starting on the game scene counts.
    let scene = |pattern: &str, steps| Handler {
        enabled: true,
        trigger: Trigger::Scene {
            scene: pattern.to_string(),
            from: String::new(),
            except: String::new(),
            settle_ms: 3_000,
            only_live: true,
        },
        steps,
    };
    vec![
        scene(
            "",
            vec![Step::new(
                StepKind::DelayAction,
                &[("action", "arm"), ("seconds", "30")],
            )],
        ),
        // Any other scene: the editor keeps the game scene out of it.
        scene(
            "*",
            vec![Step::new(StepKind::DelayAction, &[("action", "cut_after")])],
        ),
    ]
}

/// A device the on-air light can drive: where it listens and what each
/// state sends it. `{ip}` style parts are for the user to replace.
pub struct LightStart {
    pub id: &'static str,
    pub label: &'static str,
    pub method: &'static str,
    pub url: &'static str,
    /// live, delay, crash, off
    pub bodies: [&'static str; 4],
}

pub const LIGHT_STARTS: &[LightStart] = &[
    LightStart {
        id: "home_assistant",
        label: "Home Assistant",
        method: "POST",
        url: "http://homeassistant.local:8123/api/webhook/instantclone-onair",
        bodies: [
            r#"{"state":"live"}"#,
            r#"{"state":"delay"}"#,
            r#"{"state":"crash"}"#,
            r#"{"state":"off"}"#,
        ],
    },
    LightStart {
        id: "wled",
        label: "WLED",
        method: "POST",
        url: "http://wled.local/json/state",
        bodies: [
            r#"{"on":true,"bri":200,"seg":[{"fx":0,"col":[[255,40,40]]}]}"#,
            r#"{"on":true,"bri":200,"seg":[{"fx":0,"col":[[255,150,0]]}]}"#,
            r#"{"on":true,"bri":255,"seg":[{"fx":1,"col":[[255,0,0]]}]}"#,
            r#"{"on":false}"#,
        ],
    },
    LightStart {
        id: "hue",
        label: "Philips Hue",
        method: "PUT",
        url: "http://BRIDGE-IP/api/USERNAME/lights/1/state",
        bodies: [
            r#"{"on":true,"bri":254,"hue":0,"sat":254,"alert":"none"}"#,
            r#"{"on":true,"bri":254,"hue":7000,"sat":254,"alert":"none"}"#,
            r#"{"on":true,"bri":254,"hue":0,"sat":254,"alert":"lselect"}"#,
            r#"{"on":false}"#,
        ],
    },
];

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

/// Everything a pack's integrations need, each once, in catalog order.
fn pack_needs(pack: &Pack) -> Vec<&'static str> {
    let mut needs = Vec::new();
    for need in pack
        .presets
        .iter()
        .filter_map(|id| find(id))
        .flat_map(|p| p.needs.iter())
    {
        if !needs.contains(need) {
            needs.push(*need);
        }
    }
    needs
}

pub fn catalog_json() -> Value {
    let presets = PRESETS
        .iter()
        .map(|p| {
            // The catalog card previews what it really sends: its first
            // switched-on trigger, built for real.
            let sample = build(p.id, "preview".into(), "")
                .and_then(|i| i.handlers.into_iter().find(|h| h.enabled))
                .map_or(Value::Null, |h| h.to_json());
            json::obj([
                ("id", json::str(p.id)),
                ("name", json::str(p.name)),
                ("category", json::str(p.category)),
                ("description", json::str(p.description)),
                (
                    "needs",
                    Value::Arr(p.needs.iter().map(|n| json::str(*n)).collect()),
                ),
                ("recommended", Value::Bool(p.recommended)),
                (
                    "preview",
                    json::obj([
                        ("kind", json::str(p.preview.0)),
                        ("text", json::str(p.preview.1)),
                        ("ask", json::str(p.preview.2)),
                    ]),
                ),
                ("sample", sample),
            ])
        })
        .collect();
    let lights = LIGHT_STARTS
        .iter()
        .map(|l| {
            json::obj([
                ("id", json::str(l.id)),
                ("label", json::str(l.label)),
                ("method", json::str(l.method)),
                ("url", json::str(l.url)),
                (
                    "bodies",
                    json::obj([
                        ("live", json::str(l.bodies[0])),
                        ("delay", json::str(l.bodies[1])),
                        ("crash", json::str(l.bodies[2])),
                        ("off", json::str(l.bodies[3])),
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
                    "needs",
                    Value::Arr(pack_needs(p).into_iter().map(json::str).collect()),
                ),
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
        ("light_starts", Value::Arr(lights)),
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
                match &mut h.trigger {
                    Trigger::Shortcut { hotkey, .. } => *hotkey = "Ctrl+Alt+C".into(),
                    Trigger::Scene { scene, .. } => *scene = "Game".into(),
                    _ => {}
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
    fn presets_only_use_variables_that_exist() {
        for p in PRESETS {
            let i = build(p.id, "v".into(), "chan1").unwrap();
            let unknown = super::super::api::unknown_vars(&i);
            assert!(unknown.is_empty(), "{}: {unknown:?}", p.id);
        }
    }

    #[test]
    fn packs_say_everything_they_need() {
        let pro = PACKS.iter().find(|p| p.id == "stream_pro").unwrap();
        assert_eq!(pack_needs(pro), ["discord", "twitch"]);
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
    fn buttons_come_with_a_stream_deck_address() {
        let i = build("highlight", "h".into(), "chan").unwrap();
        match &i.handlers[0].trigger {
            Trigger::Shortcut { token, .. } => assert_eq!(token.len(), 32),
            other => panic!("{other:?}"),
        }
        assert!(i.enabled, "an address alone is enough to start");
    }

    #[test]
    fn smart_alerts_only_finish_messages_they_started() {
        let i = build("smart_alerts", "s".into(), "chan").unwrap();
        let edit_of = |kind: EventKind| {
            i.handlers
                .iter()
                .find(|h| matches!(&h.trigger, Trigger::Event { kind: k, .. } if *k == kind))
                .map(|h| h.steps[0].param("edit").to_string())
        };
        assert_eq!(
            edit_of(EventKind::DestinationStillDown).as_deref(),
            Some("")
        );
        assert_eq!(
            edit_of(EventKind::DestinationRecovered).as_deref(),
            Some("close")
        );
        assert_eq!(
            edit_of(EventKind::DestinationSteady).as_deref(),
            Some("close")
        );
    }

    #[test]
    fn the_scene_rules_wait_to_be_picked() {
        let i = build("scene_delay", "s".into(), "").unwrap();
        assert!(!i.enabled, "no scene picked yet");
        assert!(i.validate().iter().any(|e| e.contains("scene")));
    }

    #[test]
    fn light_presets_have_valid_json_bodies() {
        for light in LIGHT_STARTS {
            for body in light.bodies {
                assert!(crate::config::is_valid_json(body), "{}: {body}", light.id);
            }
        }
    }

    #[test]
    fn catalog_cards_preview_a_real_handler() {
        let catalog = catalog_json();
        for p in catalog.get("presets").and_then(Value::as_array).unwrap() {
            assert!(
                p.get("sample").is_some_and(|s| !matches!(s, Value::Null)),
                "{} has no sample",
                p.str_or("id", "")
            );
        }
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
