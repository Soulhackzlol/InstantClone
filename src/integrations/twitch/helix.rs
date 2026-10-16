//! The Helix API calls integrations use: VOD markers, clips, subscribing
//! to Twitch events, the channel points rewards made here, and the chat
//! commands a chat step runs. Blocking; run off the async thread.

use super::auth::{read_body, url_encode};
use crate::integrations::host::ChatCommand;
use crate::json::{self, Value};

const API: &str = "https://api.twitch.tv/helix";

pub struct Auth<'a> {
    pub client_id: &'a str,
    pub token: &'a str,
    pub user_id: &'a str,
}

/// Mark this moment in the live VOD. Twitch only accepts markers while the
/// channel is live, and says so.
pub fn create_marker(auth: &Auth, description: &str) -> Result<(), String> {
    let description: String = description.chars().take(140).collect();
    let body = json::obj([
        ("user_id", json::str(auth.user_id)),
        ("description", json::str(description)),
    ])
    .to_json();
    let (status, text) = post(auth, &format!("{API}/streams/markers"), &body)?;
    match status {
        200 => Ok(()),
        _ => Err(explain(status, &text, "add the marker")),
    }
}

/// Clip the last moments of the live stream. Returns the clip's id; the
/// clip page works right away, the video appears after Twitch processes it.
pub fn create_clip(auth: &Auth) -> Result<String, String> {
    let url = format!(
        "{API}/clips?broadcaster_id={}",
        super::auth::url_encode(auth.user_id)
    );
    let (status, text) = post(auth, &url, "")?;
    if status != 202 && status != 200 {
        return Err(explain(status, &text, "clip"));
    }
    json::parse(&text)
        .ok()
        .as_ref()
        .and_then(|v| v.path("data.0.id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "Twitch accepted the clip but sent no id".to_string())
}

/// Whether Twitch took a subscription.
pub enum Subscribed {
    Yes,
    /// The login lacks the permission this event needs.
    NeedsConsent,
}

/// Subscribe a WebSocket session to one EventSub event.
pub fn subscribe_event(
    auth: &Auth,
    kind: (&str, &str),
    condition: Value,
    session_id: &str,
) -> Result<Subscribed, String> {
    let (kind_type, version) = kind;
    let body = json::obj([
        ("type", json::str(kind_type)),
        ("version", json::str(version)),
        ("condition", condition),
        (
            "transport",
            json::obj([
                ("method", json::str("websocket")),
                ("session_id", json::str(session_id)),
            ]),
        ),
    ])
    .to_json();
    let (status, text) = post(auth, &format!("{API}/eventsub/subscriptions"), &body)?;
    match status {
        // 409: this session already has it.
        202 | 409 => Ok(Subscribed::Yes),
        403 => Ok(Subscribed::NeedsConsent),
        _ => Err(explain(status, &text, "listen for it")),
    }
}

/// A channel points reward this app made. Twitch lets an app list and
/// change only the rewards it created itself, never the streamer's others.
#[derive(Clone, Debug, PartialEq)]
pub struct Reward {
    pub id: String,
    pub title: String,
    pub cost: u64,
    pub enabled: bool,
    /// What viewers read under the title.
    pub prompt: String,
    /// The viewer types a message with it (`{input}` in the trigger).
    pub wants_input: bool,
}

/// What a reward is made with, or changed to: each part that is given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RewardEdit {
    pub title: Option<String>,
    pub cost: Option<u64>,
    pub prompt: Option<String>,
    pub wants_input: Option<bool>,
    pub enabled: Option<bool>,
}

impl RewardEdit {
    fn to_json(&self) -> String {
        let mut fields = Vec::new();
        if let Some(title) = &self.title {
            fields.push(("title".to_string(), json::str(title)));
        }
        if let Some(cost) = self.cost {
            fields.push(("cost".to_string(), Value::Num(cost as f64)));
        }
        if let Some(prompt) = &self.prompt {
            fields.push(("prompt".to_string(), json::str(prompt)));
        }
        if let Some(wants_input) = self.wants_input {
            let key = "is_user_input_required".to_string();
            fields.push((key, Value::Bool(wants_input)));
        }
        if let Some(enabled) = self.enabled {
            fields.push(("is_enabled".to_string(), Value::Bool(enabled)));
        }
        Value::Obj(fields).to_json()
    }
}

/// The longest title and description Twitch takes for a reward.
pub const MAX_REWARD_TITLE: usize = 45;
pub const MAX_REWARD_PROMPT: usize = 200;

fn rewards_url(auth: &Auth) -> String {
    format!(
        "{API}/channel_points/custom_rewards?broadcaster_id={}",
        super::auth::url_encode(auth.user_id)
    )
}

/// The rewards this app made on the channel.
pub fn rewards(auth: &Auth) -> Result<Vec<Reward>, String> {
    let url = format!("{}&only_manageable_rewards=true", rewards_url(auth));
    let (status, text) = send(auth, "GET", &url, "")?;
    match status {
        200 => Ok(parse_rewards(&text)),
        _ => Err(explain_rewards(status, &text, "list your rewards")),
    }
}

/// Make a reward viewers can redeem. `spec` needs a title and a cost.
pub fn create_reward(auth: &Auth, spec: &RewardEdit) -> Result<Reward, String> {
    let (status, text) = send(auth, "POST", &rewards_url(auth), &spec.to_json())?;
    let made = parse_rewards(&text).into_iter().next();
    match (status, made) {
        (200, Some(reward)) => Ok(reward),
        _ => Err(explain_rewards(status, &text, "make the reward")),
    }
}

/// Change the parts of a reward that `edit` gives: its name, cost,
/// description, whether it asks for a message, whether it is on. Off,
/// viewers no longer see it.
pub fn update_reward(auth: &Auth, id: &str, edit: &RewardEdit) -> Result<(), String> {
    let url = format!("{}&id={}", rewards_url(auth), super::auth::url_encode(id));
    let (status, text) = send(auth, "PATCH", &url, &edit.to_json())?;
    match status {
        200 => Ok(()),
        _ => Err(explain_rewards(status, &text, "change the reward")),
    }
}

pub fn delete_reward(auth: &Auth, id: &str) -> Result<(), String> {
    let url = format!("{}&id={}", rewards_url(auth), super::auth::url_encode(id));
    let (status, text) = send(auth, "DELETE", &url, "")?;
    match status {
        204 | 200 => Ok(()),
        _ => Err(explain_rewards(status, &text, "delete the reward")),
    }
}

/// The rewards in a Twitch answer; one that is missing a field is skipped.
fn parse_rewards(body: &str) -> Vec<Reward> {
    let doc = json::parse(body).unwrap_or(Value::Null);
    let list = doc.get("data").and_then(Value::as_array).unwrap_or(&[]);
    list.iter()
        .filter_map(|r| {
            Some(Reward {
                id: r.get("id")?.as_str()?.to_string(),
                title: r.get("title")?.as_str()?.to_string(),
                cost: r.get("cost").and_then(Value::as_f64).unwrap_or(0.0) as u64,
                enabled: r.bool_or("is_enabled", true),
                prompt: r.str_or("prompt", "").to_string(),
                wants_input: r.bool_or("is_user_input_required", false),
            })
        })
        .collect()
}

/// The longest title Twitch takes for a stream.
pub const MAX_STREAM_TITLE: usize = 140;

/// A category on Twitch: a game, or something like "Just Chatting".
#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    pub id: String,
    pub name: String,
}

/// What to change on the channel: each part that is given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelEdit {
    pub title: Option<String>,
    pub category_id: Option<String>,
}

impl ChannelEdit {
    fn to_json(&self) -> String {
        let mut fields = Vec::new();
        if let Some(title) = &self.title {
            let title: String = title.chars().take(MAX_STREAM_TITLE).collect();
            fields.push(("title".to_string(), json::str(title)));
        }
        if let Some(id) = &self.category_id {
            fields.push(("game_id".to_string(), json::str(id)));
        }
        Value::Obj(fields).to_json()
    }
}

/// Set the stream's title, its category, or both.
pub fn update_channel(auth: &Auth, edit: &ChannelEdit) -> Result<(), String> {
    let url = format!(
        "{API}/channels?broadcaster_id={}",
        super::auth::url_encode(auth.user_id)
    );
    let (status, text) = send(auth, "PATCH", &url, &edit.to_json())?;
    match status {
        204 | 200 => Ok(()),
        _ => Err(explain_scope(
            status,
            &text,
            "change the stream's title or category",
            "change them",
        )),
    }
}

/// The category a streamer means by `name`. A mod types "valo", not
/// "VALORANT": the one called exactly `name` if there is one, otherwise
/// the first Twitch's own search finds.
pub fn find_category(auth: &Auth, name: &str) -> Result<Category, String> {
    let name = super::auth::url_encode(name.trim());
    let lookups = [
        format!("{API}/games?name={name}"),
        format!("{API}/search/categories?query={name}&first=1"),
    ];
    for url in lookups {
        let (status, text) = send(auth, "GET", &url, "")?;
        if status != 200 {
            return Err(explain(status, &text, "look up the category"));
        }
        if let Some(found) = parse_categories(&text).into_iter().next() {
            return Ok(found);
        }
    }
    Err("Twitch has no category by that name".to_string())
}

/// The categories in a Twitch answer; one missing a field is skipped.
fn parse_categories(body: &str) -> Vec<Category> {
    let doc = json::parse(body).unwrap_or(Value::Null);
    let list = doc.get("data").and_then(Value::as_array).unwrap_or(&[]);
    list.iter()
        .filter_map(|c| {
            Some(Category {
                id: c.get("id")?.as_str()?.to_string(),
                name: c.get("name")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// The id of the Twitch user called `login`.
fn user_id(auth: &Auth, login: &str) -> Result<String, String> {
    let url = format!("{API}/users?login={}", url_encode(login));
    let (status, text) = send(auth, "GET", &url, "")?;
    if status != 200 {
        return Err(explain(status, &text, "look up the viewer"));
    }
    json::parse(&text)
        .ok()
        .as_ref()
        .and_then(|v| v.path("data.0.id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("Twitch has no user called {login}"))
}

/// A ban, or a timeout of `seconds`, as Twitch takes it.
fn ban_body(user_id: &str, seconds: Option<u32>, reason: &str) -> String {
    let mut data = vec![("user_id".to_string(), json::str(user_id))];
    if let Some(seconds) = seconds {
        data.push(("duration".to_string(), Value::Num(f64::from(seconds))));
    }
    if !reason.is_empty() {
        data.push(("reason".to_string(), json::str(reason)));
    }
    Value::Obj(vec![("data".to_string(), Value::Obj(data))]).to_json()
}

/// Run a chat step's command in the channel, as its broadcaster (who
/// moderates their own chat).
pub fn run_command(auth: &Auth, command: &ChatCommand) -> Result<(), String> {
    let me = url_encode(auth.user_id);
    let bans = format!("{API}/moderation/bans?broadcaster_id={me}&moderator_id={me}");
    let (what, method, url, body) = match command {
        ChatCommand::Timeout {
            login,
            seconds,
            reason,
        } => {
            let id = user_id(auth, login)?;
            (
                "time them out",
                "POST",
                bans,
                ban_body(&id, Some(*seconds), reason),
            )
        }
        ChatCommand::Ban { login, reason } => {
            let id = user_id(auth, login)?;
            ("ban them", "POST", bans, ban_body(&id, None, reason))
        }
        ChatCommand::Unban { login } => {
            let id = user_id(auth, login)?;
            let url = format!("{bans}&user_id={}", url_encode(&id));
            ("lift their ban or timeout", "DELETE", url, String::new())
        }
        ChatCommand::Shoutout { login } => {
            let id = user_id(auth, login)?;
            let url = format!(
                "{API}/chat/shoutouts?from_broadcaster_id={me}&to_broadcaster_id={}&moderator_id={me}",
                url_encode(&id)
            );
            ("shout them out", "POST", url, String::new())
        }
        ChatCommand::Announce { text } => {
            let url = format!("{API}/chat/announcements?broadcaster_id={me}&moderator_id={me}");
            let body = json::obj([("message", json::str(text.as_str()))]).to_json();
            ("post the announcement", "POST", url, body)
        }
    };
    let (status, text) = send(auth, method, &url, &body)?;
    match status {
        200 | 204 => Ok(()),
        _ => Err(explain_scope(status, &text, what, "run chat commands")),
    }
}

/// Why Twitch said no to a rewards call, in the streamer's terms. A login
/// from before this app managed rewards lacks the permission (401), and a
/// channel that isn't Affiliate or Partner has no channel points (403).
fn explain_rewards(status: u16, body: &str, what: &str) -> String {
    match status {
        403 => format!(
            "Twitch refused to {what}: channel points need an Affiliate or Partner channel, and an app can only change rewards it made"
        ),
        _ => explain_scope(status, body, what, "manage rewards"),
    }
}

/// A 401 from a login made before InstantClone asked for what `needs` (Twitch
/// says "Missing scope") is fixed by logging in again. Any other 401 is a
/// token that ran out, which `explain` words so the call is renewed and
/// tried again.
fn explain_scope(status: u16, body: &str, what: &str, needs: &str) -> String {
    if status == 401 && body.to_ascii_lowercase().contains("missing scope") {
        return format!(
            "Twitch refused to {what}: log out of Twitch here and in again, so InstantClone may {needs}"
        );
    }
    explain(status, body, what)
}

fn post(auth: &Auth, url: &str, body: &str) -> Result<(u16, String), String> {
    send(auth, "POST", url, body)
}

/// One Helix call. `method` is GET, POST, PATCH or DELETE; the first and
/// last send no body.
fn send(auth: &Auth, method: &str, url: &str, body: &str) -> Result<(u16, String), String> {
    let agent = crate::https::https_agent_with_timeout(super::HTTP_TIMEOUT);
    let token = format!("Bearer {}", auth.token);
    let resp = match method {
        "GET" | "DELETE" => {
            let request = if method == "GET" {
                agent.get(url)
            } else {
                agent.delete(url)
            };
            request
                .header("Client-Id", auth.client_id)
                .header("Authorization", &token)
                .call()
        }
        _ => {
            let request = if method == "PATCH" {
                agent.patch(url)
            } else {
                agent.post(url)
            };
            request
                .header("Client-Id", auth.client_id)
                .header("Authorization", &token)
                .header("Content-Type", "application/json")
                .send(body)
        }
    }
    .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    Ok((status, read_body(resp)))
}

fn explain(status: u16, body: &str, what: &str) -> String {
    let message = json::parse(body)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    match (status, message.is_empty()) {
        (401, _) => format!("Twitch refused to {what}: log in to Twitch again"),
        (_, false) => format!("Twitch couldn't {what}: {message}"),
        (_, true) => format!("Twitch couldn't {what} (HTTP {status})"),
    }
}

#[cfg(test)]
mod reward_tests {
    use super::*;

    #[test]
    fn a_timeout_says_how_long_and_a_ban_does_not() {
        assert_eq!(
            ban_body("42", Some(600), "spam"),
            r#"{"data":{"user_id":"42","duration":600,"reason":"spam"}}"#
        );
        assert_eq!(ban_body("42", None, ""), r#"{"data":{"user_id":"42"}}"#);
    }

    #[test]
    fn rewards_are_read_from_what_twitch_answers() {
        let body = r#"{"data":[
            {"id":"r1","title":"Hydrate","cost":500,"is_enabled":true,"is_paused":false},
            {"id":"r2","title":"Buy coins","cost":1000,"is_enabled":false,"prompt":"10 points a coin","is_user_input_required":true},
            {"title":"no id"}]}"#;
        let found = parse_rewards(body);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].title, "Hydrate");
        assert_eq!((found[1].cost, found[1].enabled), (1000, false));
        assert_eq!(
            (found[1].prompt.as_str(), found[1].wants_input),
            ("10 points a coin", true)
        );
        assert_eq!(
            (found[0].prompt.as_str(), found[0].wants_input),
            ("", false)
        );
        assert!(parse_rewards("not json").is_empty());
    }

    #[test]
    fn a_channel_edit_sends_only_what_is_given() {
        let title_only = ChannelEdit {
            title: Some("Rankeds hasta Ascendente".into()),
            category_id: None,
        };
        assert_eq!(
            title_only.to_json(),
            r#"{"title":"Rankeds hasta Ascendente"}"#
        );
        let both = ChannelEdit {
            title: Some("x".repeat(MAX_STREAM_TITLE + 20)),
            category_id: Some("516575".into()),
        };
        let sent = json::parse(&both.to_json()).unwrap();
        assert_eq!(sent.str_or("title", "").chars().count(), MAX_STREAM_TITLE);
        assert_eq!(sent.str_or("game_id", ""), "516575");
        assert_eq!(ChannelEdit::default().to_json(), "{}");
    }

    #[test]
    fn categories_are_read_from_both_of_twitchs_answers() {
        // `games` and `search/categories` name the same two fields.
        let games =
            r#"{"data":[{"id":"516575","name":"VALORANT","box_art_url":"x","igdb_id":"126459"}]}"#;
        let search = r#"{"data":[{"broadcaster_language":"","id":"509658","name":"Just Chatting"},{"name":"no id"}]}"#;
        let valorant = Category {
            id: "516575".into(),
            name: "VALORANT".into(),
        };
        assert_eq!(parse_categories(games), vec![valorant]);
        let found = parse_categories(search);
        assert_eq!((found.len(), found[0].name.as_str()), (1, "Just Chatting"));
        assert!(parse_categories(r#"{"data":[]}"#).is_empty());
        assert!(parse_categories("not json").is_empty());
    }

    #[test]
    fn an_edit_sends_only_what_changed() {
        let switch = RewardEdit {
            enabled: Some(false),
            ..RewardEdit::default()
        };
        assert_eq!(switch.to_json(), r#"{"is_enabled":false}"#);
        let renamed = RewardEdit {
            title: Some("Hydrate".into()),
            cost: Some(300),
            prompt: Some(String::new()),
            wants_input: Some(true),
            enabled: None,
        };
        assert_eq!(
            renamed.to_json(),
            r#"{"title":"Hydrate","cost":300,"prompt":"","is_user_input_required":true}"#
        );
    }

    #[test]
    fn a_refusal_says_what_the_streamer_can_do_about_it() {
        let old_login = explain_rewards(401, r#"{"message":"Missing scope"}"#, "make the reward");
        assert!(
            old_login.contains("log out of Twitch here and in again"),
            "{old_login}"
        );
        // A token that ran out reads as the words a renewal answers to.
        let expired = r#"{"status":401,"message":"Invalid OAuth token"}"#;
        for refused in [
            explain_rewards(401, expired, "make the reward"),
            explain_scope(401, expired, "post the announcement", "run chat commands"),
        ] {
            assert!(refused.ends_with("log in to Twitch again"), "{refused}");
        }
        let no_points = explain_rewards(403, "{}", "make the reward");
        assert!(no_points.contains("Affiliate or Partner"), "{no_points}");
        let twice = explain_rewards(
            400,
            r#"{"message":"CREATE_CUSTOM_REWARD_DUPLICATE_REWARD"}"#,
            "make the reward",
        );
        assert!(twice.contains("DUPLICATE"), "{twice}");
    }
}
