//! The real `Host`: reads the controller, and does the I/O each step asks
//! for. Every method may block and is only called from `spawn_blocking`.

use super::host::{ClipInfo, DiscordPosted, Host, HttpRequest, HttpResponse, LiveState};
use super::twitch::Twitch;
use crate::controller::Controller;
use crate::json::{self, Value};
use std::io::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub(super) const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest response body a step keeps.
const MAX_RESPONSE: u64 = 1024 * 1024;
/// Largest text a file step writes.
const MAX_FILE_TEXT: usize = 1024 * 1024;

/// Programs started by integrations that may still be running. Kept so
/// they are reaped (no zombies on Unix) without a waiting thread each, and
/// capped so a runaway trigger can't fork the machine to its knees.
const MAX_RUNNING_PROGRAMS: usize = 16;

/// Make a web request for a step, or for the editor's "Send request".
pub fn send_http(r: HttpRequest) -> Result<HttpResponse, String> {
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
            .max_redirects(0)
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
            .max_redirects(0)
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
    // Redirects aren't followed: one could point a request made from this
    // PC at the dashboard itself, and post what it answers to chat.
    if (300..400).contains(&status) {
        let to = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("another address");
        return Err(format!("the server moved this to {to}; use that address"));
    }
    let body = resp
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE)
        .read_to_string()
        .unwrap_or_default();
    Ok(HttpResponse { status, body })
}

/// Post a Discord message, or edit message `edit` (see `Host::discord`).
/// An edit never pings; `ping` is for when it has to be posted anew.
pub fn send_discord(
    webhook_url: &str,
    content: &str,
    ping: &str,
    edit: Option<&str>,
) -> Result<DiscordPosted, String> {
    if let Some(id) = edit {
        let resp = crate::https::https_agent()
            .patch(&webhook_message_url(webhook_url, id))
            .config()
            .timeout_global(Some(HTTP_TIMEOUT))
            .build()
            .header("Content-Type", "application/json")
            .send(discord_body(content, ""))
            .map_err(|e| format!("couldn't reach Discord: {e}"))?;
        // 404 here is the message, not the webhook: someone deleted it.
        // The update still has to reach the channel, as a new message.
        if resp.status().as_u16() != 404 {
            discord_status(resp.status().as_u16())?;
            return Ok(DiscordPosted {
                message_id: id.to_string(),
                edited: true,
            });
        }
    }
    // `wait=true` makes Discord answer with the message, and its id.
    let mut resp = crate::https::https_agent()
        .post(&with_query(webhook_url, "wait=true"))
        .config()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .header("Content-Type", "application/json")
        .send(discord_body(content, ping))
        .map_err(|e| format!("couldn't reach Discord: {e}"))?;
    discord_status(resp.status().as_u16())?;
    let answer = resp
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_string()
        .unwrap_or_default();
    let message_id = json::parse(&answer)
        .ok()
        .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_string))
        .filter(|id| id.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or_default();
    Ok(DiscordPosted {
        message_id,
        edited: false,
    })
}

pub struct RealHost {
    pub ctrl: Arc<Controller>,
    pub twitch: Arc<Twitch>,
    /// The streamer's usual delay, used when `arm` names none.
    pub default_delay_ms: AtomicU32,
    pub programs: crate::sync::Mutex<Vec<std::process::Child>>,
    pub alerts: Arc<super::alerts::Board>,
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

    fn discord(
        &self,
        webhook_url: &str,
        content: &str,
        ping: &str,
        edit: Option<&str>,
    ) -> Result<DiscordPosted, String> {
        send_discord(webhook_url, content, ping, edit)
    }

    fn http(&self, r: HttpRequest) -> Result<HttpResponse, String> {
        send_http(r)
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
            "disarm" => {
                self.ctrl.disarm("integration");
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
        let mut running = self.programs.lock();
        // Reap whatever has finished since last time.
        running.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
        if running.len() >= MAX_RUNNING_PROGRAMS {
            return Err(format!(
                "{MAX_RUNNING_PROGRAMS} programs started by integrations are still running; not starting another"
            ));
        }
        let child = std::process::Command::new(path)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start it: {e}"))?;
        running.push(child);
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

    fn overlay(&self, title: &str, text: &str, seconds: u64) {
        self.alerts.show(title, text, seconds);
    }
}

fn discord_status(status: u16) -> Result<(), String> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => {
            Err("Discord refused this webhook link; copy it again from Discord".to_string())
        }
        404 => Err("Discord says this webhook no longer exists".to_string()),
        429 => Err("Discord is rate limiting this webhook; slow down".to_string()),
        s => Err(format!("Discord answered {s}")),
    }
}

/// `url` with `param` added to its query (a webhook link can already carry
/// one, like `?thread_id=` for a forum thread).
fn with_query(url: &str, param: &str) -> String {
    let joint = if url.contains('?') { '&' } else { '?' };
    format!("{url}{joint}{param}")
}

/// Where one of a webhook's messages is edited: `<webhook>/messages/<id>`,
/// keeping the webhook's query so a thread message is found in its thread.
fn webhook_message_url(url: &str, id: &str) -> String {
    let (base, query) = url
        .split_once('?')
        .map_or((url, None), |(b, q)| (b, Some(q)));
    let base = base.trim_end_matches('/');
    match query {
        Some(q) => format!("{base}/messages/{id}?{q}"),
        None => format!("{base}/messages/{id}"),
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

    /// A webhook that answers like Discord does: a new message gets an id,
    /// an edit of a known id works, an edit of a deleted one is a 404.
    fn fake_discord(answers: usize) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/api/webhooks/1/tok",
            listener.local_addr().unwrap()
        );
        let seen = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for stream in listener.incoming().take(answers) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Vec<&str> = line.split_whitespace().collect();
                let (method, path) = (request[0].to_string(), request[1].to_string());
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let (status, answer) = match (method.as_str(), path.as_str()) {
                    ("POST", _) => ("200 OK", r#"{"id":"111"}"#),
                    ("PATCH", p) if p.ends_with("/messages/111") => ("200 OK", r#"{"id":"111"}"#),
                    _ => ("404 Not Found", r#"{"message":"Unknown Message"}"#),
                };
                let pinged = String::from_utf8_lossy(&body).contains("@here");
                seen.push(format!(
                    "{method} {path}{}",
                    if pinged { " @here" } else { "" }
                ));
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                stream.write_all(reply.as_bytes()).unwrap();
            }
            seen
        });
        (url, seen)
    }

    #[test]
    fn a_message_is_posted_edited_and_reposted_once_deleted() {
        let (url, seen) = fake_discord(4);
        let first = send_discord(&url, "down", "", None).unwrap();
        assert_eq!((first.message_id.as_str(), first.edited), ("111", false));
        let edit = send_discord(&url, "back", "here", Some("111")).unwrap();
        assert_eq!((edit.message_id.as_str(), edit.edited), ("111", true));
        // Someone deleted message 999: the update still goes out, as new,
        // and pings like a new message does.
        let gone = send_discord(&url, "back", "here", Some("999")).unwrap();
        assert_eq!((gone.message_id.as_str(), gone.edited), ("111", false));
        assert_eq!(
            seen.join().unwrap(),
            [
                "POST /api/webhooks/1/tok?wait=true",
                "PATCH /api/webhooks/1/tok/messages/111",
                "PATCH /api/webhooks/1/tok/messages/999",
                "POST /api/webhooks/1/tok?wait=true @here",
            ]
        );
    }

    #[test]
    fn edits_go_to_the_message_and_keep_the_thread() {
        let hook = "https://discord.com/api/webhooks/1/tok";
        assert_eq!(with_query(hook, "wait=true"), format!("{hook}?wait=true"));
        assert_eq!(
            webhook_message_url(hook, "99"),
            format!("{hook}/messages/99")
        );
        let thread = "https://discord.com/api/webhooks/1/tok?thread_id=5";
        assert_eq!(
            with_query(thread, "wait=true"),
            format!("{thread}&wait=true")
        );
        assert_eq!(
            webhook_message_url(thread, "99"),
            "https://discord.com/api/webhooks/1/tok/messages/99?thread_id=5"
        );
    }

    #[test]
    fn discord_messages_are_capped() {
        let long = json::parse(&discord_body(&"a".repeat(3000), "")).unwrap();
        assert_eq!(long.str_or("content", "").len(), 2000);
    }
}
