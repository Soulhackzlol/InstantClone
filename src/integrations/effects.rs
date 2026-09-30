//! The real `Host`: reads the controller, and does the I/O each step asks
//! for. Every method may block and is only called from `spawn_blocking`.

use super::host::{ClipInfo, Host, HttpRequest, HttpResponse, LiveState};
use super::twitch::Twitch;
use crate::controller::Controller;
use crate::json::{self, Value};
use std::io::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest response body a step keeps.
const MAX_RESPONSE: u64 = 1024 * 1024;
/// Largest text a file step writes.
const MAX_FILE_TEXT: usize = 1024 * 1024;

pub struct RealHost {
    pub ctrl: Arc<Controller>,
    pub twitch: Arc<Twitch>,
    /// The streamer's usual delay, used when `arm` names none.
    pub default_delay_ms: AtomicU32,
}

impl Host for RealHost {
    fn live(&self) -> LiveState {
        let (alive, total) = self.ctrl.destination_alive_summary();
        let hold = self.ctrl.hold_status();
        LiveState {
            delay_ms: self.ctrl.current_delay_ms(),
            phase: self.ctrl.phase().to_string(),
            hold_active: self.ctrl.hold_active(),
            hold_left_ms: hold.map_or(0, |h| h.remaining.as_millis() as u64),
            destinations_live: alive as usize,
            destinations_total: total as usize,
            obs_live: self.ctrl.ingest_alive(),
            bitrate_kbps: self.ctrl.bitrate_kbps() as u64,
            channel: self.twitch.channel(),
        }
    }

    fn discord(&self, webhook_url: &str, content: &str, ping: &str) -> Result<(), String> {
        let body = discord_body(content, ping);
        let resp = crate::https::https_agent()
            .post(webhook_url)
            .config()
            .timeout_global(Some(HTTP_TIMEOUT))
            .build()
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| format!("couldn't reach Discord: {e}"))?;
        match resp.status().as_u16() {
            200..=299 => Ok(()),
            401 | 403 => {
                Err("Discord refused this webhook link; copy it again from Discord".to_string())
            }
            404 => Err("Discord says this webhook no longer exists".to_string()),
            429 => Err("Discord is rate limiting this webhook; slow down".to_string()),
            s => Err(format!("Discord answered {s}")),
        }
    }

    fn http(&self, r: HttpRequest) -> Result<HttpResponse, String> {
        if !(r.url.starts_with("http://") || r.url.starts_with("https://")) {
            return Err("the address must start with http:// or https://".to_string());
        }
        let agent = crate::https::https_agent();
        let result = match r.method.as_str() {
            "GET" | "DELETE" | "HEAD" => {
                let mut req = match r.method.as_str() {
                    "GET" => agent.get(&r.url),
                    "HEAD" => agent.head(&r.url),
                    _ => agent.delete(&r.url),
                }
                .config()
                .timeout_global(Some(HTTP_TIMEOUT))
                .build();
                for (name, value) in &r.headers {
                    req = req.header(name.as_str(), value.as_str());
                }
                req.call()
            }
            "POST" | "PUT" | "PATCH" => {
                let mut req = match r.method.as_str() {
                    "PUT" => agent.put(&r.url),
                    "PATCH" => agent.patch(&r.url),
                    _ => agent.post(&r.url),
                }
                .config()
                .timeout_global(Some(HTTP_TIMEOUT))
                .build();
                let has_type = r
                    .headers
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case("content-type"));
                if !has_type && !r.body.is_empty() {
                    let kind = if json::parse(&r.body).is_ok() {
                        "application/json"
                    } else {
                        "text/plain; charset=utf-8"
                    };
                    req = req.header("Content-Type", kind);
                }
                for (name, value) in &r.headers {
                    req = req.header(name.as_str(), value.as_str());
                }
                req.send(r.body.as_str())
            }
            other => return Err(format!("{other} is not a supported method")),
        };
        let mut resp = result.map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE)
            .read_to_string()
            .unwrap_or_default();
        Ok(HttpResponse { status, body })
    }

    fn phone(
        &self,
        server: &str,
        topic: &str,
        title: &str,
        text: &str,
        priority: &str,
    ) -> Result<(), String> {
        let priority = match priority {
            "high" => 4.0,
            "urgent" => 5.0,
            _ => 3.0,
        };
        let body = json::obj([
            ("topic", json::str(topic)),
            ("title", json::str(title)),
            ("message", json::str(text)),
            ("priority", Value::Num(priority)),
        ])
        .to_json();
        let resp = crate::https::https_agent()
            .post(server)
            .config()
            .timeout_global(Some(HTTP_TIMEOUT))
            .build()
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| format!("couldn't reach {server}: {e}"))?;
        match resp.status().as_u16() {
            200..=299 => Ok(()),
            429 => Err("ntfy is rate limiting; slow down".to_string()),
            s => Err(format!("ntfy answered {s}")),
        }
    }

    fn chat(&self, text: &str, as_bot: bool, reply_to: Option<&str>) -> Result<(), String> {
        self.twitch.send(text, as_bot, reply_to)
    }

    fn marker(&self, description: &str) -> Result<(), String> {
        self.twitch.marker(description)
    }

    fn clip(&self) -> Result<ClipInfo, String> {
        let id = self.twitch.clip()?;
        Ok(ClipInfo {
            url: format!("https://clips.twitch.tv/{id}"),
            id,
        })
    }

    fn delay_action(&self, action: &str, ms: u32) -> Result<(), String> {
        let default_ms = if ms > 0 {
            ms
        } else {
            self.default_delay_ms.load(Ordering::Relaxed)
        };
        match action {
            // "Set the delay", not the hotkey's arm/disarm toggle.
            "arm" => {
                self.ctrl.set_delay_to(default_ms, "integration");
                Ok(())
            }
            "toggle" | "activate" | "cut" | "cut_after" | "end_hold" => {
                match self
                    .ctrl
                    .run_named_action(action, default_ms, "integration")
                {
                    None => Ok(()),
                    Some(problem) => Err(problem),
                }
            }
            other => Err(format!("unknown delay action \"{other}\"")),
        }
    }

    fn program(&self, path: &str, args: &[String]) -> Result<(), String> {
        if path.is_empty() {
            return Err("pick a program to run".to_string());
        }
        let mut child = std::process::Command::new(path)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start it: {e}"))?;
        // Reap it whenever it ends, so nothing is left behind.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn file(&self, path: &str, text: &str, append: bool) -> Result<(), String> {
        if path.is_empty() {
            return Err("pick a file".to_string());
        }
        if text.len() > MAX_FILE_TEXT {
            return Err("that text is too big to write".to_string());
        }
        let path = std::path::Path::new(path);
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("couldn't make the folder: {e}"))?;
        }
        let result = if append {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| writeln!(f, "{text}"))
        } else {
            std::fs::write(path, text)
        };
        result.map_err(|e| format!("couldn't write it: {e}"))
    }
}

/// A Discord message that can only ping what the streamer chose. Text from
/// chat or a web service can contain `@everyone`; `allowed_mentions` keeps
/// that from pinging anyone unless the ping option asked for it.
pub fn discord_body(content: &str, ping: &str) -> String {
    let (prefix, allowed) = match ping {
        "here" => (
            "@here ".to_string(),
            json::obj([("parse", Value::Arr(vec![json::str("everyone")]))]),
        ),
        "everyone" => (
            "@everyone ".to_string(),
            json::obj([("parse", Value::Arr(vec![json::str("everyone")]))]),
        ),
        role if role.starts_with("role:")
            && role[5..].bytes().all(|b| b.is_ascii_digit())
            && role.len() > 5 =>
        {
            (
                format!("<@&{}> ", &role[5..]),
                json::obj([("roles", Value::Arr(vec![json::str(&role[5..])]))]),
            )
        }
        _ => (String::new(), json::obj([("parse", Value::Arr(vec![]))])),
    };
    let content: String = format!("{prefix}{content}").chars().take(2000).collect();
    json::obj([
        ("content", json::str(content)),
        ("username", json::str("InstantClone")),
        ("allowed_mentions", allowed),
    ])
    .to_json()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discord_never_pings_unless_asked() {
        let quiet = json::parse(&discord_body("@everyone look", "")).unwrap();
        assert_eq!(
            quiet
                .path("allowed_mentions.parse")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            0
        );
        let here = json::parse(&discord_body("OBS dropped", "here")).unwrap();
        assert_eq!(here.str_or("content", ""), "@here OBS dropped");
        let role = json::parse(&discord_body("x", "role:123")).unwrap();
        assert_eq!(role.str_or("content", ""), "<@&123> x");
        assert_eq!(
            role.path("allowed_mentions.roles.0").unwrap().as_str(),
            Some("123")
        );
        let bad = json::parse(&discord_body("x", "role:1 OR 1")).unwrap();
        assert_eq!(bad.str_or("content", ""), "x");
    }

    #[test]
    fn discord_messages_are_capped() {
        let long = json::parse(&discord_body(&"a".repeat(3000), "")).unwrap();
        assert_eq!(long.str_or("content", "").len(), 2000);
    }
}
