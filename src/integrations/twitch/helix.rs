//! The Helix API calls integrations use: VOD markers, clips, subscribing
//! to Twitch events, and the channel points rewards made here.
//! Blocking; run off the async thread.

use super::auth::read_body;
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

/// Why Twitch said no to a rewards call, in the streamer's terms. A login
/// from before this app managed rewards lacks the permission (401), and a
/// channel that isn't Affiliate or Partner has no channel points (403).
fn explain_rewards(status: u16, body: &str, what: &str) -> String {
    match status {
        401 => format!(
            "Twitch refused to {what}: log out of Twitch here and in again, so InstantClone may manage rewards"
        ),
        403 => format!(
            "Twitch refused to {what}: channel points need an Affiliate or Partner channel, and an app can only change rewards it made"
        ),
        _ => explain(status, body, what),
    }
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
