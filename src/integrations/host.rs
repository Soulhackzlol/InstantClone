//! Everything a running integration needs from the outside world, behind
//! one trait: the live stream state it reads and the effects it causes.
//!
//! The runner only talks to `Host`, so its logic (variables, checks,
//! waits, stop) is tested against a fake that records calls, and the real
//! implementation in `effects.rs` is the only place that does I/O.
//!
//! Effect methods block (HTTPS through ureq, file and process calls); the
//! runner calls them from `spawn_blocking` so a slow endpoint never stalls
//! other integrations.

/// A snapshot of the stream, read when a step needs it.
#[derive(Clone, Debug, Default)]
pub struct LiveState {
    pub delay_ms: u32,
    pub phase: String,
    pub hold_active: bool,
    pub hold_left_ms: u64,
    pub destinations_live: usize,
    pub destinations_total: usize,
    /// Whether OBS is streaming to InstantClone right now.
    pub obs_live: bool,
    pub bitrate_kbps: u64,
    /// The connected Twitch channel's login, or empty.
    pub channel: String,
}

pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub struct ClipInfo {
    pub id: String,
    pub url: String,
}

pub trait Host: Send + Sync {
    fn live(&self) -> LiveState;
    fn discord(&self, webhook_url: &str, content: &str, ping: &str) -> Result<(), String>;
    fn http(&self, request: HttpRequest) -> Result<HttpResponse, String>;
    fn phone(
        &self,
        server: &str,
        topic: &str,
        title: &str,
        text: &str,
        priority: &str,
    ) -> Result<(), String>;
    /// Post in the connected channel's chat. `reply_to` is the id of the
    /// message being answered, when there is one.
    fn chat(&self, text: &str, as_bot: bool, reply_to: Option<&str>) -> Result<(), String>;
    fn marker(&self, description: &str) -> Result<(), String>;
    fn clip(&self) -> Result<ClipInfo, String>;
    /// Run one of the named delay actions (`arm`, `cut`, ...). `ms` is the
    /// delay for `arm`.
    fn delay_action(&self, action: &str, ms: u32) -> Result<(), String>;
    fn program(&self, path: &str, args: &[String]) -> Result<(), String>;
    fn file(&self, path: &str, text: &str, append: bool) -> Result<(), String>;
}

/// `30s`, `1m 30s`, or empty for no delay, so `{delay|off}` reads naturally.
pub fn fmt_delay(ms: u32) -> String {
    if ms == 0 {
        return String::new();
    }
    let total = (ms + 500) / 1000;
    match (total / 60, total % 60) {
        (0, s) => format!("{s}s"),
        (m, 0) => format!("{m}m"),
        (m, s) => format!("{m}m {s}s"),
    }
}

/// `2m 00s` style durations, matching the crash protection messages.
pub fn fmt_duration(ms: u64) -> String {
    let total = ms / 1000;
    if total < 60 {
        format!("{total} s")
    } else {
        format!("{}m {:02}s", total / 60, total % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_read_like_people_say_them() {
        assert_eq!(fmt_delay(0), "");
        assert_eq!(fmt_delay(30_000), "30s");
        assert_eq!(fmt_delay(90_000), "1m 30s");
        assert_eq!(fmt_delay(120_000), "2m");
        assert_eq!(fmt_delay(29_600), "30s");
    }

    #[test]
    fn durations_pad_seconds() {
        assert_eq!(fmt_duration(38_000), "38 s");
        assert_eq!(fmt_duration(120_000), "2m 00s");
        assert_eq!(fmt_duration(72_500), "1m 12s");
    }
}
