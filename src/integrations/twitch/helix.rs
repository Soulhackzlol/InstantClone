//! The two Helix API calls integrations use: VOD markers and clips.
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

fn post(auth: &Auth, url: &str, body: &str) -> Result<(u16, String), String> {
    let resp = crate::https::https_agent_with_timeout(super::HTTP_TIMEOUT)
        .post(url)
        .header("Client-Id", auth.client_id)
        .header("Authorization", format!("Bearer {}", auth.token))
        .header("Content-Type", "application/json")
        .send(body)
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
