//! Twitch login without a server: the Device Code flow.
//!
//! InstantClone is a public client (open source, so it cannot keep a client
//! secret). Twitch allows public clients to log in with a device code the
//! user types on twitch.tv/activate and to refresh tokens without a secret.
//! A public client's refresh token expires after 30 days *unused*; the
//! manager refreshes well inside that, so a streamer who opens InstantClone
//! at least once a month never has to log in again.
//!
//! Every call here blocks (ureq); callers run them off the async thread.

use crate::json::{self, Value};

/// Chat, VOD markers and clips for the streamer's account.
pub const MAIN_SCOPES: &str = "chat:read chat:edit channel:manage:broadcast clips:edit";
/// A bot account only reads and posts chat.
pub const BOT_SCOPES: &str = "chat:read chat:edit";

const DEVICE_URL: &str = "https://id.twitch.tv/oauth2/device";
const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const VALIDATE_URL: &str = "https://id.twitch.tv/oauth2/validate";

#[derive(Clone, Debug)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Clone, Debug)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    pub expires_in: u64,
}

pub enum Poll {
    Pending,
    SlowDown,
    Done(Tokens),
    /// The code ran out or the user refused; start over.
    Ended(String),
}

pub enum TokenError {
    /// Twitch no longer accepts the token: the user must log in again.
    Revoked,
    /// Network or server trouble: try again later.
    Temporary(String),
}

pub struct Validated {
    pub login: String,
    pub user_id: String,
}

pub fn start_device(client_id: &str, scopes: &str) -> Result<DeviceCode, String> {
    let (status, body) = post_form(DEVICE_URL, &[("client_id", client_id), ("scopes", scopes)])?;
    let v = json::parse(&body).map_err(|_| format!("Twitch answered {status}"))?;
    if status != 200 {
        return Err(message(&v, status));
    }
    Ok(DeviceCode {
        device_code: v.str_or("device_code", "").to_string(),
        user_code: v.str_or("user_code", "").to_string(),
        verification_uri: v
            .str_or("verification_uri", "https://www.twitch.tv/activate")
            .to_string(),
        expires_in: v.u64_or("expires_in", 1800),
        interval: v.u64_or("interval", 5).max(1),
    })
}

pub fn poll_device(client_id: &str, scopes: &str, device_code: &str) -> Poll {
    let form = [
        ("client_id", client_id),
        ("scopes", scopes),
        ("device_code", device_code),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ];
    let (status, body) = match post_form(TOKEN_URL, &form) {
        Ok(r) => r,
        Err(_) => return Poll::Pending,
    };
    let Ok(v) = json::parse(&body) else {
        return Poll::Pending;
    };
    if status == 200 {
        return Poll::Done(tokens(&v));
    }
    match v.str_or("message", "") {
        "authorization_pending" => Poll::Pending,
        "slow_down" => Poll::SlowDown,
        "access_denied" => Poll::Ended("You declined the login on Twitch.".to_string()),
        "expired_token" | "invalid device code" => {
            Poll::Ended("The code expired before it was entered. Try again.".to_string())
        }
        _ => Poll::Ended(message(&v, status)),
    }
}

pub fn refresh(client_id: &str, refresh_token: &str) -> Result<Tokens, TokenError> {
    let form = [
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    let (status, body) = post_form(TOKEN_URL, &form).map_err(TokenError::Temporary)?;
    let v = json::parse(&body)
        .map_err(|_| TokenError::Temporary(format!("Twitch answered {status}")))?;
    match status {
        200 => Ok(tokens(&v)),
        400 | 401 => Err(TokenError::Revoked),
        _ => Err(TokenError::Temporary(message(&v, status))),
    }
}

pub fn validate(access: &str) -> Result<Validated, TokenError> {
    let resp = crate::https::https_agent()
        .get(VALIDATE_URL)
        .header("Authorization", format!("OAuth {access}"))
        .call()
        .map_err(|e| TokenError::Temporary(e.to_string()))?;
    let status = resp.status().as_u16();
    let body = read_body(resp);
    if status == 401 {
        return Err(TokenError::Revoked);
    }
    let v = json::parse(&body)
        .map_err(|_| TokenError::Temporary(format!("Twitch answered {status}")))?;
    if status != 200 {
        return Err(TokenError::Temporary(message(&v, status)));
    }
    Ok(Validated {
        login: v.str_or("login", "").to_string(),
        user_id: v.str_or("user_id", "").to_string(),
    })
}

fn tokens(v: &Value) -> Tokens {
    Tokens {
        access: v.str_or("access_token", "").to_string(),
        refresh: v.str_or("refresh_token", "").to_string(),
        expires_in: v.u64_or("expires_in", 3600),
    }
}

fn message(v: &Value, status: u16) -> String {
    match v.str_or("message", "") {
        "" => format!("Twitch answered {status}"),
        m => format!("Twitch: {m}"),
    }
}

fn post_form(url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String> {
    let resp = crate::https::https_agent()
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(form_encode(form))
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    Ok((status, read_body(resp)))
}

pub fn read_body(mut resp: ureq::http::Response<ureq::Body>) -> String {
    resp.body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()
        .unwrap_or_default()
}

pub fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_are_percent_encoded() {
        assert_eq!(
            form_encode(&[("scopes", "chat:read chat:edit"), ("a", "x&y=z")]),
            "scopes=chat%3Aread%20chat%3Aedit&a=x%26y%3Dz"
        );
    }
}
