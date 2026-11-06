//! "Get my key from Twitch": the streamer approves once on twitch.tv and the
//! Twitch destination gets its stream key, with nothing to find, copy or
//! paste. Twitch's dashboard link needs the channel name, which a new user
//! hasn't given us, so sending them there was the confusing step this
//! replaces.
//!
//! The login is the device-code login Integrations uses (see
//! `integrations::twitch::auth`), asking only to read the stream key, and it
//! lasts as long as one fetch: the key is read, the token is revoked, and
//! nothing of the login is kept. A streamer who never uses Integrations is
//! never asked for chat or channel-point permissions.
//!
//! Every call here blocks (ureq); callers run them off the async thread.

use crate::integrations::twitch::auth::{self, Poll, TokenError};
use crate::json::{self, Value};
use crate::sync::Mutex;
use std::time::{Duration, Instant};

const SCOPE: &str = "channel:read:stream_key";
const KEY_URL: &str = "https://api.twitch.tv/helix/streams/key";
const REVOKE_URL: &str = "https://id.twitch.tv/oauth2/revoke";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// Twitch's own rule when it answers `slow_down`.
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);

/// The login waiting for the streamer's approval on twitch.tv.
struct Pending {
    client_id: String,
    device_code: String,
    interval: Duration,
    next_poll: Instant,
    expires: Instant,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// What the page shows while it waits: the code and where to approve it.
pub struct Code {
    pub user_code: String,
    pub uri: String,
    pub expires_in: u64,
}

pub enum Check {
    /// Not approved yet; ask again in a moment.
    Waiting,
    /// The stream key, and the channel it belongs to.
    Done { key: String, login: String },
    /// Over: declined, expired, or Twitch said no. Start again.
    Ended(String),
}

/// What one `check` should do, decided from the waiting login.
#[derive(Debug, PartialEq)]
enum Step {
    Wait,
    End(&'static str),
    Poll {
        client_id: String,
        device_code: String,
    },
}

/// Ask Twitch for a code. A login already waiting is replaced.
pub fn start(client_id: &str) -> Result<Code, String> {
    if client_id.is_empty() {
        return Err("This copy of InstantClone has no Twitch app. Paste the key instead.".into());
    }
    let code = auth::start_device(client_id, SCOPE)?;
    let now = Instant::now();
    *PENDING.lock() = Some(Pending {
        client_id: client_id.to_string(),
        device_code: code.device_code,
        interval: Duration::from_secs(code.interval),
        next_poll: now,
        expires: now + Duration::from_secs(code.expires_in),
    });
    Ok(Code {
        user_code: code.user_code,
        uri: code.verification_uri,
        expires_in: code.expires_in,
    })
}

pub fn cancel() {
    *PENDING.lock() = None;
}

/// Whether the streamer approved yet; on approval, the key. The page asks
/// every couple of seconds, and Twitch is asked no faster than it allows.
pub fn check() -> Check {
    let step = next_step(&mut PENDING.lock(), Instant::now());
    let (client_id, device_code) = match step {
        Step::Wait => return Check::Waiting,
        Step::End(why) => return Check::Ended(why.to_string()),
        Step::Poll {
            client_id,
            device_code,
        } => (client_id, device_code),
    };
    match auth::poll_device(&client_id, SCOPE, &device_code) {
        Poll::Pending => Check::Waiting,
        Poll::SlowDown => {
            if let Some(p) = PENDING.lock().as_mut() {
                p.interval += SLOW_DOWN_STEP;
                p.next_poll = Instant::now() + p.interval;
            }
            Check::Waiting
        }
        Poll::Ended(why) => {
            forget(&device_code);
            Check::Ended(why)
        }
        Poll::Done(tokens) => {
            forget(&device_code);
            let fetched = fetch_key(&client_id, &tokens.access);
            revoke(&client_id, &tokens.access);
            match fetched {
                Ok((key, login)) => Check::Done { key, login },
                Err(why) => Check::Ended(why),
            }
        }
    }
}

fn next_step(slot: &mut Option<Pending>, now: Instant) -> Step {
    let Some(p) = slot.as_mut() else {
        return Step::End("No Twitch login is waiting. Click the button again.");
    };
    if now >= p.expires {
        *slot = None;
        return Step::End("The code ran out before it was approved. Click the button again.");
    }
    if now < p.next_poll {
        return Step::Wait;
    }
    p.next_poll = now + p.interval;
    Step::Poll {
        client_id: p.client_id.clone(),
        device_code: p.device_code.clone(),
    }
}

/// Drop the waiting login, unless a newer one replaced it meanwhile.
fn forget(device_code: &str) {
    let mut slot = PENDING.lock();
    if slot.as_ref().is_some_and(|p| p.device_code == device_code) {
        *slot = None;
    }
}

/// The channel's stream key and name, read with a fresh login.
fn fetch_key(client_id: &str, access: &str) -> Result<(String, String), String> {
    let me = auth::validate(access).map_err(|e| match e {
        TokenError::Revoked => "Twitch didn't accept the login. Click the button again.".into(),
        TokenError::Temporary(why) => why,
    })?;
    let url = format!("{KEY_URL}?broadcaster_id={}", auth::url_encode(&me.user_id));
    let resp = crate::https::https_agent_with_timeout(HTTP_TIMEOUT)
        .get(&url)
        .header("Client-Id", client_id)
        .header("Authorization", format!("Bearer {access}"))
        .call()
        .map_err(|e| format!("Couldn't reach Twitch: {e}"))?;
    let status = resp.status().as_u16();
    let body = auth::read_body(resp);
    if status != 200 {
        return Err(format!(
            "Twitch didn't hand over the key (HTTP {status}). Paste it instead."
        ));
    }
    let key = key_from_answer(&body).ok_or("Twitch answered without a stream key.")?;
    Ok((key, me.login))
}

fn key_from_answer(body: &str) -> Option<String> {
    json::parse(body)
        .ok()?
        .path("data.0.stream_key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

/// Best effort: a token Twitch still holds would expire on its own anyway.
fn revoke(client_id: &str, access: &str) {
    let _ = crate::https::https_agent_with_timeout(HTTP_TIMEOUT)
        .post(REVOKE_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send(auth::form_encode(&[
            ("client_id", client_id),
            ("token", access),
        ]));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(now: Instant) -> Pending {
        Pending {
            client_id: "app".into(),
            device_code: "dev".into(),
            interval: Duration::from_secs(5),
            next_poll: now,
            expires: now + Duration::from_secs(600),
        }
    }

    #[test]
    fn nothing_waiting_ends_at_once() {
        let mut slot = None;
        assert!(matches!(next_step(&mut slot, Instant::now()), Step::End(_)));
    }

    #[test]
    fn twitch_is_asked_no_faster_than_its_interval() {
        let now = Instant::now();
        let mut slot = Some(pending(now));
        assert!(matches!(next_step(&mut slot, now), Step::Poll { .. }));
        assert_eq!(
            next_step(&mut slot, now + Duration::from_secs(2)),
            Step::Wait
        );
        assert!(matches!(
            next_step(&mut slot, now + Duration::from_secs(5)),
            Step::Poll { .. }
        ));
    }

    #[test]
    fn an_expired_code_ends_and_is_dropped() {
        let now = Instant::now();
        let mut slot = Some(pending(now));
        let later = now + Duration::from_secs(601);
        assert!(matches!(next_step(&mut slot, later), Step::End(_)));
        assert!(slot.is_none());
    }

    #[test]
    fn the_key_is_read_from_what_twitch_answers() {
        let body = r#"{"data":[{"stream_key":"live_44322889_a34ub37c8ajv98a0"}]}"#;
        assert_eq!(
            key_from_answer(body).as_deref(),
            Some("live_44322889_a34ub37c8ajv98a0")
        );
        assert_eq!(key_from_answer(r#"{"data":[{"stream_key":""}]}"#), None);
        assert_eq!(key_from_answer(r#"{"data":[]}"#), None);
        assert_eq!(key_from_answer("not json"), None);
    }
}
