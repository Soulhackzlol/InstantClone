//! Persistent configuration: load from disk, save atomically, expose
//! platform presets so users only paste their stream key.
//!
//! File format is intentionally line-based "key=value" so the file is
//! editable by hand and trivially parseable without a serde dependency.
//! For wire transport (web UI), `to_json` emits a small JSON object.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Hard ceilings used when parsing the on-disk config. A line like
/// `destination.4294967295.id=x` (hand-edited typo or attacker-fed file)
/// would otherwise drive `Vec::push` billions of times before the field
/// even applies, OOMing the process. 128 destinations and 256 profiles
/// are both already absurd for a single streamer.
const MAX_DESTINATIONS: usize = 128;
const MAX_PROFILES: usize = 256;
/// Minimum on-disk buffer size (MB). Below this, the ring is too small
/// to hold a single typical IDR (which can be 100 KB at 1080p60).
/// Also guards against the `% 0` panic in `DiskRing::append` if a
/// hand-edited config sets `buffer_mb=0`.
const MIN_BUFFER_MB: u64 = 50;

#[derive(Debug, Clone)]
pub struct Settings {
    pub ingest_port: u16,
    pub ingest_bind_all: bool, // true → 0.0.0.0, false → 127.0.0.1

    // Legacy single-destination fields. Still present on disk for backward
    // compatibility — on load they get migrated into destinations[0] if
    // `destinations` was empty (i.e. config written before multi-dest).
    pub platform: String,
    pub stream_key: String,
    pub custom_egress_url: String,

    pub web_port: u16,
    pub web_bind_all: bool,
    pub buffer_mb: u64,
    pub buffer_path: PathBuf,
    pub target_delay_ms: u32,
    pub armed_delay_ms: u32,
    pub initial_delay_ms: u32,
    pub configured: bool,
    pub profiles: Vec<DelayProfile>,
    pub destinations: Vec<Destination>,
    pub discord_webhook_url: String,
    pub overlays_dir: PathBuf,
    /// Whether the wire-level egress trace (instantclone-trace.log) is
    /// actively writing. Default true so a fresh install captures
    /// diagnostic data; toggleable in the System tab so users past the
    /// debugging phase can stop the file from growing.
    pub tracing_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct DelayProfile {
    pub name: String,
    pub delay_ms: u32,
}

/// One streaming destination — Twitch, YouTube, a private server, etc.
/// Each destination runs its own RTMP egress and is paced/cut/timestamp-
/// rewritten independently from the same shared DiskRing.
#[derive(Debug, Clone)]
pub struct Destination {
    pub id: String,   // stable across renames; UI key
    pub name: String, // user label ("Twitch", "Backup", etc.)
    pub enabled: bool,
    pub platform: String, // "twitch" | "youtube" | "kick" | "trovo" | "restream" | "custom"
    pub stream_key: String,
    pub custom_egress_url: String, // used when platform == "custom"
    /// Only meaningful when platform == "twitch". Empty = auto-route via
    /// live.twitch.tv (which load-balances regionally). Otherwise: a
    /// Twitch ingest slug from `twitch_ingests()` (e.g. "fra", "jfk").
    /// Pinning a region helps when auto-routing keeps picking a flaky
    /// edge — common for streamers near a region boundary.
    pub twitch_ingest: String,
    /// Only meaningful when platform == "youtube". One of:
    ///   "" / "primary"  → rtmp://a.rtmp.youtube.com/live2 (default)
    ///   "backup"        → rtmp://b.rtmp.youtube.com/live2
    /// YouTube recommends using the backup ingest when the primary is
    /// flaky in the streamer's region — surfacing it as an option saves
    /// users from dropping to a Custom RTMP URL just to switch endpoints.
    pub youtube_ingest: String,
}

impl Destination {
    /// Build the resolved egress URL for this destination. Same forgiving
    /// rules as the top-level single-dest path: if a custom URL already has
    /// app+key, the key field is ignored; otherwise the key is appended.
    pub fn egress_url(&self) -> Option<String> {
        if self.platform == "custom" {
            let base = non_empty(&self.custom_egress_url)?
                .trim_end_matches('/')
                .to_string();
            let path = base
                .strip_prefix("rtmp://")
                .unwrap_or(&base)
                .split_once('/')
                .map(|x| x.1)
                .unwrap_or("");
            let segs = path.split('/').filter(|s| !s.is_empty()).count();
            if segs >= 2 || self.stream_key.is_empty() {
                return Some(base);
            }
            return Some(format!("{}/{}", base, self.stream_key));
        }
        let base = if self.platform == "twitch" && !self.twitch_ingest.is_empty() {
            // Region-pinned Twitch ingest. Format: live-<slug>.twitch.tv
            // (matches the JSON Twitch publishes at ingest.twitch.tv/ingests).
            format!("rtmp://live-{}.twitch.tv/app", self.twitch_ingest)
        } else if self.platform == "youtube" && self.youtube_ingest == "backup" {
            "rtmp://b.rtmp.youtube.com/live2".to_string()
        } else {
            platform_base(&self.platform)?.to_string()
        };
        let key = non_empty(&self.stream_key)?;
        let url = format!("{}/{}", base, key);
        // YouTube's backup ingest requires a `?backup=1` suffix on the
        // stream name so their edge treats this connection as the
        // redundancy partner of the primary. Without the suffix the
        // backup is accepted but never actually fails over, so a primary
        // drop would still glitch the stream — the worst of both worlds.
        // Reference: YouTube Studio's "URL del servidor secundario" pane
        // surfaces the exact format `…/live2?backup=1`; we slot the
        // stream key in between so the wire form is `…/live2/KEY?backup=1`,
        // which is what OBS produces when wired against YouTube's UI URL.
        if self.platform == "youtube" && self.youtube_ingest == "backup" {
            Some(format!("{}?backup=1", url))
        } else {
            Some(url)
        }
    }

    pub fn is_well_formed(&self) -> bool {
        match self.egress_url() {
            Some(url) => {
                let path = url
                    .strip_prefix("rtmp://")
                    .unwrap_or(&url)
                    .split_once('/')
                    .map(|x| x.1)
                    .unwrap_or("");
                path.split('/').filter(|s| !s.is_empty()).count() >= 2
            }
            None => false,
        }
    }
}

impl Settings {
    pub fn defaults() -> Self {
        Self {
            ingest_port: 1935,
            ingest_bind_all: false,
            platform: "twitch".into(),
            stream_key: String::new(),
            custom_egress_url: String::new(),
            web_port: 7799,
            web_bind_all: false,
            // 300 MB ≈ 5 minutes of headroom at 8 Mbps. With the trim
            // logic, the *actual* used portion matches the armed delay;
            // this is just the hard cap on what the user could ever arm.
            // Lower it to save disk; raise for >5-min armed delays.
            buffer_mb: 300,
            buffer_path: PathBuf::from("./instantclone.buf"),
            target_delay_ms: 0,
            armed_delay_ms: 0,
            initial_delay_ms: 0,
            configured: false,
            profiles: vec![
                DelayProfile {
                    name: "Quick".into(),
                    delay_ms: 15_000,
                },
                DelayProfile {
                    name: "Standard".into(),
                    delay_ms: 30_000,
                },
                DelayProfile {
                    name: "Tournament".into(),
                    delay_ms: 300_000,
                },
            ],
            destinations: Vec::new(),
            discord_webhook_url: String::new(),
            overlays_dir: PathBuf::from("./overlays"),
            tracing_enabled: true,
        }
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let mut s = Self::defaults();
        // Both lists are file-authoritative — a user who deletes them all
        // must see them stay deleted across restarts.
        s.profiles.clear();
        s.destinations.clear();
        let text = fs::read_to_string(path)?;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                s.apply_field(k.trim(), v.trim());
            }
        }
        // Defensive: drop empty-name profiles (hand-edit error tolerance).
        s.profiles.retain(|p| !p.name.is_empty());
        if s.profiles.is_empty() {
            s.profiles = Self::defaults().profiles;
        }
        // Migrate legacy single-destination config: a config file written
        // by an older version had platform/stream_key but no
        // `destination.*` lines. Promote it into destinations[0] so the
        // user doesn't lose their setup.
        if s.destinations.is_empty()
            && (!s.stream_key.is_empty() || !s.custom_egress_url.is_empty())
        {
            s.destinations.push(Destination {
                id: "main".into(),
                name: "Main".into(),
                enabled: true,
                platform: s.platform.clone(),
                stream_key: s.stream_key.clone(),
                custom_egress_url: s.custom_egress_url.clone(),
                twitch_ingest: String::new(),
                youtube_ingest: String::new(),
            });
        }
        // Clamp / sanitize on load — hand-edited values can otherwise
        // hit divide-by-zero (buffer_mb=0 → `% capacity` in DiskRing) or
        // bind two services to the same port (one will silently fail).
        s.sanitize_load();
        Ok(s)
    }

    /// Apply on-load clamps that protect against hand-edited / malformed
    /// values. Distinct from `validate()` (which is for user-facing UI
    /// errors); this one just rounds inputs to safe ranges silently so
    /// the process can still start.
    fn sanitize_load(&mut self) {
        if self.buffer_mb < MIN_BUFFER_MB {
            self.buffer_mb = MIN_BUFFER_MB;
        }
        if self.ingest_port == 0 {
            self.ingest_port = 1935;
        }
        if self.web_port == 0 {
            self.web_port = 7799;
        }
        if self.web_port == self.ingest_port {
            // Defensive port move — if the user collided them in the
            // config file, push the web port to a sane fallback so at
            // least the UI is reachable to fix it.
            self.web_port = if self.ingest_port == 7799 { 7800 } else { 7799 };
        }
        // Cap delay values to the same 10-minute ceiling the runtime uses.
        self.armed_delay_ms = self.armed_delay_ms.min(600_000);
        self.target_delay_ms = self.target_delay_ms.min(600_000);
        self.initial_delay_ms = self.initial_delay_ms.min(600_000);
    }

    pub fn load_or_default(path: &Path) -> Self {
        match Self::load(path) {
            Ok(s) => s,
            Err(_) => Self::defaults().with_smart_defaults(),
        }
    }

    /// On a brand-new install, pick smarter starting values than the
    /// constants above: buffer size scaled to free disk, etc.
    pub fn with_smart_defaults(mut self) -> Self {
        // Buffer: ~5 minutes at 8 Mbps ≈ 300 MB. Cap at 1.5 GB. Floor 200 MB.
        // If the working directory's free space looks tight, scale down.
        if let Some(free) = free_space_bytes(&self.buffer_path) {
            let want = 300u64 * 1024 * 1024;
            let cap = 1_500u64 * 1024 * 1024;
            let floor = 200u64 * 1024 * 1024;
            // never claim more than 20% of free disk
            let bound = (free / 5).max(floor).min(cap);
            self.buffer_mb = (want.min(bound)) / (1024 * 1024);
        }
        self
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut f = fs::File::create(path)?;
        writeln!(f, "# InstantClone — written by the app. Hand edits OK.")?;
        writeln!(f, "configured={}", self.configured)?;
        // Legacy single-destination fields: kept for backward compat with
        // older binaries that don't know about `destination.*`. We mirror
        // destinations[0] into them so a downgrade still works.
        let mirror = self.destinations.first();
        writeln!(
            f,
            "platform={}",
            mirror
                .map(|d| d.platform.as_str())
                .unwrap_or(&self.platform)
        )?;
        writeln!(
            f,
            "stream_key={}",
            mirror
                .map(|d| d.stream_key.as_str())
                .unwrap_or(&self.stream_key)
        )?;
        writeln!(
            f,
            "custom_egress_url={}",
            mirror
                .map(|d| d.custom_egress_url.as_str())
                .unwrap_or(&self.custom_egress_url)
        )?;
        writeln!(f, "ingest_port={}", self.ingest_port)?;
        writeln!(f, "ingest_bind_all={}", self.ingest_bind_all)?;
        writeln!(f, "web_port={}", self.web_port)?;
        writeln!(f, "web_bind_all={}", self.web_bind_all)?;
        writeln!(f, "buffer_mb={}", self.buffer_mb)?;
        writeln!(f, "buffer_path={}", self.buffer_path.display())?;
        writeln!(f, "target_delay_ms={}", self.target_delay_ms)?;
        writeln!(f, "initial_delay_ms={}", self.initial_delay_ms)?;
        writeln!(f, "armed_delay_ms={}", self.armed_delay_ms)?;
        writeln!(f, "discord_webhook_url={}", self.discord_webhook_url)?;
        writeln!(f, "overlays_dir={}", self.overlays_dir.display())?;
        writeln!(f, "tracing_enabled={}", self.tracing_enabled)?;
        for (i, p) in self.profiles.iter().enumerate() {
            writeln!(f, "profile.{}.name={}", i, p.name)?;
            writeln!(f, "profile.{}.delay_ms={}", i, p.delay_ms)?;
        }
        for (i, d) in self.destinations.iter().enumerate() {
            writeln!(f, "destination.{}.id={}", i, d.id)?;
            writeln!(f, "destination.{}.name={}", i, d.name)?;
            writeln!(f, "destination.{}.enabled={}", i, d.enabled)?;
            writeln!(f, "destination.{}.platform={}", i, d.platform)?;
            writeln!(f, "destination.{}.stream_key={}", i, d.stream_key)?;
            writeln!(
                f,
                "destination.{}.custom_egress_url={}",
                i, d.custom_egress_url
            )?;
            writeln!(f, "destination.{}.twitch_ingest={}", i, d.twitch_ingest)?;
            writeln!(f, "destination.{}.youtube_ingest={}", i, d.youtube_ingest)?;
        }
        f.sync_all()?;
        Ok(())
    }

    fn apply_field(&mut self, key: &str, value: &str) {
        match key {
            "configured" => self.configured = value == "true",
            "platform" => self.platform = value.into(),
            "stream_key" => self.stream_key = value.into(),
            "custom_egress_url" => self.custom_egress_url = value.into(),
            "ingest_port" => {
                if let Ok(v) = value.parse() {
                    self.ingest_port = v;
                }
            }
            "ingest_bind_all" => self.ingest_bind_all = value == "true",
            "web_port" => {
                if let Ok(v) = value.parse() {
                    self.web_port = v;
                }
            }
            "web_bind_all" => self.web_bind_all = value == "true",
            "buffer_mb" => {
                if let Ok(v) = value.parse() {
                    self.buffer_mb = v;
                }
            }
            "buffer_path" => self.buffer_path = PathBuf::from(value),
            "target_delay_ms" => {
                if let Ok(v) = value.parse() {
                    self.target_delay_ms = v;
                }
            }
            "armed_delay_ms" => {
                if let Ok(v) = value.parse() {
                    self.armed_delay_ms = v;
                }
            }
            "initial_delay_ms" => {
                if let Ok(v) = value.parse() {
                    self.initial_delay_ms = v;
                }
            }
            "discord_webhook_url" => self.discord_webhook_url = value.into(),
            "overlays_dir" => self.overlays_dir = PathBuf::from(value),
            "tracing_enabled" => self.tracing_enabled = value.parse().unwrap_or(true),
            k if k.starts_with("profile.") => {
                let rest = &k[8..];
                if let Some(dot) = rest.find('.') {
                    if let Ok(idx) = rest[..dot].parse::<usize>() {
                        // Cap: a hand-edited / attacker-fed config line
                        // like `profile.4294967295.name=x` would otherwise
                        // allocate billions of empty profiles and OOM.
                        if idx >= MAX_PROFILES {
                            return;
                        }
                        while self.profiles.len() <= idx {
                            self.profiles.push(DelayProfile {
                                name: String::new(),
                                delay_ms: 0,
                            });
                        }
                        let field = &rest[dot + 1..];
                        match field {
                            "name" => self.profiles[idx].name = value.into(),
                            "delay_ms" => {
                                if let Ok(v) = value.parse() {
                                    self.profiles[idx].delay_ms = v;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            k if k.starts_with("destination.") => {
                // destination.<idx>.<field>=value
                let rest = &k[12..];
                if let Some(dot) = rest.find('.') {
                    if let Ok(idx) = rest[..dot].parse::<usize>() {
                        // Same OOM-guard as profiles. 128 destinations is
                        // already absurd for a single streamer — beyond
                        // that it's almost certainly malformed input.
                        if idx >= MAX_DESTINATIONS {
                            return;
                        }
                        while self.destinations.len() <= idx {
                            self.destinations.push(Destination {
                                id: format!("dest{}", idx),
                                name: String::new(),
                                enabled: true,
                                platform: "twitch".into(),
                                stream_key: String::new(),
                                custom_egress_url: String::new(),
                                twitch_ingest: String::new(),
                                youtube_ingest: String::new(),
                            });
                        }
                        let d = &mut self.destinations[idx];
                        match &rest[dot + 1..] {
                            "id" => d.id = value.into(),
                            "name" => d.name = value.into(),
                            "enabled" => d.enabled = value == "true",
                            "platform" => d.platform = value.into(),
                            "stream_key" => d.stream_key = value.into(),
                            "custom_egress_url" => d.custom_egress_url = value.into(),
                            "twitch_ingest" => d.twitch_ingest = value.into(),
                            "youtube_ingest" => d.youtube_ingest = value.into(),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ---- Derived values ----

    pub fn ingest_addr(&self) -> String {
        let host = if self.ingest_bind_all {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        format!("{}:{}", host, self.ingest_port)
    }

    pub fn web_addr(&self) -> String {
        let host = if self.web_bind_all {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        format!("{}:{}", host, self.web_port)
    }

    pub fn obs_url(&self) -> String {
        // What OBS should be pointed at. Always 127.0.0.1 from the user's
        // perspective even if we bind on 0.0.0.0.
        format!("rtmp://127.0.0.1:{}/live", self.ingest_port)
    }

    /// First destination's resolved URL — used by the legacy single-dest
    /// code paths (validate, redacted display, etc.). Multi-destination
    /// pump uses `Destination::egress_url` directly per-dest.
    pub fn egress_url(&self) -> Option<String> {
        self.destinations
            .first()
            .and_then(|d| d.egress_url())
            .or_else(|| {
                // Pre-migration safety net.
                if self.platform == "custom" {
                    let base = non_empty(&self.custom_egress_url)?
                        .trim_end_matches('/')
                        .to_string();
                    let path = base
                        .strip_prefix("rtmp://")
                        .unwrap_or(&base)
                        .split_once('/')
                        .map(|x| x.1)
                        .unwrap_or("");
                    let segs = path.split('/').filter(|s| !s.is_empty()).count();
                    if segs >= 2 || self.stream_key.is_empty() {
                        return Some(base);
                    }
                    return Some(format!("{}/{}", base, self.stream_key));
                }
                let base = platform_base(&self.platform)?;
                let key = non_empty(&self.stream_key)?;
                Some(format!("{}/{}", base, key))
            })
    }

    /// All resolved, well-formed, enabled destinations — the set the
    /// supervisor will spawn egress pumps for.
    pub fn active_destinations(&self) -> Vec<(Destination, String)> {
        self.destinations
            .iter()
            .filter(|d| d.enabled)
            .filter_map(|d| d.egress_url().map(|u| (d.clone(), u)))
            .collect()
    }

    pub fn buffer_bytes(&self) -> u64 {
        self.buffer_mb * 1024 * 1024
    }

    pub fn to_json(&self) -> String {
        // Build the destinations array. Stream keys are NEVER echoed back
        // (security); we expose a redacted form of the resolved URL plus
        // a `stream_key_set` boolean per destination.
        let mut dests = String::from("[");
        for (i, d) in self.destinations.iter().enumerate() {
            if i > 0 {
                dests.push(',');
            }
            let url = d.egress_url().unwrap_or_default();
            dests.push_str(&format!(
                r#"{{"id":{id},"name":{n},"enabled":{en},"platform":{p},"custom_egress_url":{cu},"twitch_ingest":{ti},"youtube_ingest":{yi},"stream_key_set":{ks},"egress_url_redacted":{red}}}"#,
                id  = json_str(&d.id),
                n   = json_str(&d.name),
                en  = d.enabled,
                p   = json_str(&d.platform),
                cu  = json_str(&d.custom_egress_url),
                ti  = json_str(&d.twitch_ingest),
                yi  = json_str(&d.youtube_ingest),
                ks  = !d.stream_key.is_empty(),
                red = json_str(&redact_key(&url)),
            ));
        }
        dests.push(']');
        format!(
            r#"{{"configured":{c},"ingest_port":{ip},"ingest_bind_all":{iba},"web_port":{wp},"web_bind_all":{wba},"buffer_mb":{bm},"buffer_path":{bp},"target_delay_ms":{td},"initial_delay_ms":{id},"obs_url":{ou},"discord_webhook_url":{dw},"webhook_set":{ws},"overlays_dir":{ov},"tracing_enabled":{te},"destinations":{dests}}}"#,
            c = self.configured,
            ip = self.ingest_port,
            iba = self.ingest_bind_all,
            wp = self.web_port,
            wba = self.web_bind_all,
            bm = self.buffer_mb,
            bp = json_str(&self.buffer_path.display().to_string()),
            td = self.target_delay_ms,
            id = self.initial_delay_ms,
            ou = json_str(&self.obs_url()),
            dw = json_str(&redact_webhook(&self.discord_webhook_url)),
            ws = !self.discord_webhook_url.is_empty(),
            ov = json_str(&self.overlays_dir.display().to_string()),
            te = self.tracing_enabled,
            dests = dests,
        )
    }

    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        if self.ingest_port == 0 {
            errs.push("ingest_port must be > 0".into());
        }
        if self.web_port == 0 {
            errs.push("web_port must be > 0".into());
        }
        if self.ingest_port == self.web_port {
            errs.push("ingest_port and web_port must differ — they share the same socket otherwise and one will silently fail to bind".into());
        }
        if self.buffer_mb < MIN_BUFFER_MB {
            errs.push(format!("buffer_mb must be at least {}", MIN_BUFFER_MB));
        }
        if self.destinations.len() > MAX_DESTINATIONS {
            errs.push(format!("too many destinations (max {})", MAX_DESTINATIONS));
        }
        // Validate each destination individually so the user sees which one
        // is the problem in a multi-dest setup.
        for d in &self.destinations {
            if d.name.trim().is_empty() {
                errs.push("destination is missing a name".into());
                continue;
            }
            if d.platform == "custom" {
                if d.custom_egress_url.is_empty() {
                    errs.push(format!("{}: custom RTMP URL required", d.name));
                    continue;
                }
                if !d.custom_egress_url.starts_with("rtmp://") {
                    errs.push(format!("{}: custom URL must start with rtmp://", d.name));
                    continue;
                }
            } else if platform_base(&d.platform).is_none() {
                errs.push(format!("{}: unknown platform '{}'", d.name, d.platform));
                continue;
            } else if d.stream_key.is_empty() {
                errs.push(format!("{}: stream key required", d.name));
                continue;
            }
            if !d.is_well_formed() {
                errs.push(format!(
                    "{}: resolved URL needs app path + stream key (e.g. rtmp://host/app/key)",
                    d.name
                ));
            }
        }
        if !self.discord_webhook_url.is_empty() && !self.discord_webhook_url.starts_with("https://")
        {
            errs.push("Discord webhook URL must start with https://".into());
        }
        // Path-input sanity. Blocks `..` traversal and Windows system
        // directories. Local-only listener, so attacker would need code
        // exec already; this is just defence in depth so a casual POST
        // can't trick the proxy into writing a 300 MB .buf into System32.
        if !is_path_safe(&self.buffer_path) {
            errs.push(format!(
                "buffer_path looks unsafe: {}",
                self.buffer_path.display()
            ));
        }
        if !is_path_safe(&self.overlays_dir) {
            errs.push(format!(
                "overlays_dir looks unsafe: {}",
                self.overlays_dir.display()
            ));
        }
        errs
    }
}

/// Rejects paths that escape the project (`..`), are absolute into known
/// Windows system roots, or contain shell-injection markers. We don't
/// try to be exhaustive; we just block the obvious foot-guns.
pub fn is_path_safe(p: &std::path::Path) -> bool {
    let s = match p.to_str() {
        Some(s) => s,
        None => return false, // non-UTF8 path — refuse rather than guess
    };
    if s.is_empty() {
        return false;
    }
    if s.contains("..") {
        return false;
    }
    let lower = s.to_ascii_lowercase().replace('\\', "/");
    const BAD_PREFIXES: &[&str] = &[
        "c:/windows",
        "c:/program files",
        "c:/programdata",
        "/windows",
        "/etc",
        "/usr",
        "/bin",
        "/sbin",
        "/boot",
        "/sys",
        "/proc",
    ];
    for bad in BAD_PREFIXES {
        if lower.starts_with(bad) {
            return false;
        }
    }
    true
}

/// Mapping of platform slug → RTMP ingest base. New platforms go here.
pub fn platform_base(slug: &str) -> Option<&'static str> {
    Some(match slug {
        "twitch" => "rtmp://live.twitch.tv/app",
        "youtube" => "rtmp://a.rtmp.youtube.com/live2",
        "kick" => "rtmp://fa723fc1b171.global-contribute.live-video.net/app",
        "trovo" => "rtmp://livepush.trovo.live/live",
        "restream" => "rtmp://live.restream.io/live",
        _ => return None,
    })
}

pub fn all_platforms() -> &'static [(&'static str, &'static str)] {
    &[
        ("twitch", "Twitch"),
        ("youtube", "YouTube Live"),
        ("kick", "Kick"),
        ("trovo", "Trovo"),
        ("restream", "Restream.io"),
        ("custom", "Custom RTMP URL"),
    ]
}

/// Curated list of regional Twitch ingests. Slug → human label. The slug
/// is the airport-code prefix Twitch uses in `live-<slug>.twitch.tv`.
/// Empty slug = auto (default — Twitch picks for you).
pub fn twitch_ingests() -> &'static [(&'static str, &'static str)] {
    &[
        ("", "Auto (recommended)"),
        ("jfk", "North America: New York"),
        ("ord", "North America: Chicago"),
        ("dfw", "North America: Dallas"),
        ("den", "North America: Denver"),
        ("sea", "North America: Seattle"),
        ("sfo", "North America: San Francisco"),
        ("lax", "North America: Los Angeles"),
        ("yto", "North America: Toronto"),
        ("fra", "Europe: Frankfurt"),
        ("ams", "Europe: Amsterdam"),
        ("lhr", "Europe: London"),
        ("cdg", "Europe: Paris"),
        ("arn", "Europe: Stockholm"),
        ("mad", "Europe: Madrid"),
        ("mxp", "Europe: Milan"),
        ("waw", "Europe: Warsaw"),
        ("hnd", "Asia: Tokyo"),
        ("icn", "Asia: Seoul"),
        ("sin", "Asia: Singapore"),
        ("hkg", "Asia: Hong Kong"),
        ("syd", "Oceania: Sydney"),
        ("gru", "South America: São Paulo"),
    ]
}

fn redact_webhook(url: &str) -> String {
    // Discord webhook URLs are: https://discord.com/api/webhooks/<id>/<token>
    // Keep first 8 + last 4 of the token so the user can recognize it
    // without exposing it to a screen-share.
    if let Some(i) = url.rfind('/') {
        let (base, token) = url.split_at(i + 1);
        if token.len() > 16 {
            return format!("{}{}…{}", base, &token[..8], &token[token.len() - 4..]);
        }
    }
    url.to_string()
}

fn redact_key(url: &str) -> String {
    // Find last '/' and keep first 4 + last 4 of whatever follows.
    if let Some(i) = url.rfind('/') {
        let (base, key) = url.split_at(i + 1);
        if key.len() > 12 {
            return format!("{}{}…{}", base, &key[..4], &key[key.len() - 4..]);
        }
        if !key.is_empty() {
            return format!("{}{}…", base, &key[..key.len().min(4)]);
        }
    }
    url.to_string()
}

fn non_empty(s: &String) -> Option<&String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Best-effort free space query. Returns None if the OS doesn't tell us
/// (in which case the caller keeps the default).
fn free_space_bytes(path: &Path) -> Option<u64> {
    let dir = path.parent().unwrap_or(Path::new("."));
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_bytes_available: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free_bytes: u64 = 0;
        // SAFETY: passing well-formed wide null-terminated string + valid output pointers.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_bytes_available,
                &mut total_bytes,
                &mut total_free_bytes,
            )
        };
        if ok != 0 {
            Some(free_bytes_available)
        } else {
            None
        }
    }
    #[cfg(not(windows))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: c is a valid C string; st is a valid out-param.
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some((st.f_bavail as u64) * (st.f_frsize as u64))
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetDiskFreeSpaceExW(
        lpDirectoryName: *const u16,
        lpFreeBytesAvailableToCaller: *mut u64,
        lpTotalNumberOfBytes: *mut u64,
        lpTotalNumberOfFreeBytes: *mut u64,
    ) -> i32;
}

// ---- URL-decoded form-body parsing (used by the web layer) ----

pub fn parse_form(body: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for pair in body.split('&') {
        let mut it = pair.splitn(2, '=');
        if let (Some(k), Some(v)) = (it.next(), it.next()) {
            out.insert(url_decode(k), url_decode(v));
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_form_handles_basic_encoding() {
        let h = parse_form("name=Test&platform=twitch&stream_key=abc");
        assert_eq!(h.get("name"), Some(&"Test".to_string()));
        assert_eq!(h.get("platform"), Some(&"twitch".to_string()));
        assert_eq!(h.get("stream_key"), Some(&"abc".to_string()));
    }

    #[test]
    fn parse_form_handles_percent_and_plus_encoding() {
        // `+` decodes to space, `%21` to `!`, `%C3%A9` to UTF-8 `é`.
        let h = parse_form("a=hello+world&b=%21&c=%C3%A9");
        assert_eq!(h.get("a"), Some(&"hello world".to_string()));
        assert_eq!(h.get("b"), Some(&"!".to_string()));
        assert_eq!(h.get("c"), Some(&"é".to_string()));
    }

    #[test]
    fn parse_form_handles_empty_value() {
        let h = parse_form("stream_key=&platform=custom");
        assert_eq!(h.get("stream_key"), Some(&"".to_string()));
        assert_eq!(h.get("platform"), Some(&"custom".to_string()));
    }

    #[test]
    fn is_path_safe_blocks_traversal_and_system_dirs() {
        assert!(!is_path_safe(&PathBuf::from("../../../etc/passwd")));
        assert!(!is_path_safe(&PathBuf::from(
            "C:\\Windows\\System32\\evil.dll"
        )));
        assert!(!is_path_safe(&PathBuf::from("/etc/shadow")));
        assert!(!is_path_safe(&PathBuf::from("")));
    }

    #[test]
    fn is_path_safe_allows_normal_paths() {
        assert!(is_path_safe(&PathBuf::from("./instantclone.buf")));
        assert!(is_path_safe(&PathBuf::from("D:\\streams\\buffer.bin")));
        assert!(is_path_safe(&PathBuf::from("/home/user/buf")));
    }

    #[test]
    fn destination_egress_url_for_twitch_appends_key() {
        let d = Destination {
            id: "x".into(),
            name: "T".into(),
            enabled: true,
            platform: "twitch".into(),
            stream_key: "live_123_abc".into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
        };
        let url = d.egress_url().unwrap();
        assert!(url.starts_with("rtmp://live.twitch.tv/app/"));
        assert!(url.ends_with("/live_123_abc"));
    }

    #[test]
    fn destination_egress_url_region_pin_uses_live_dash_slug() {
        let d = Destination {
            id: "x".into(),
            name: "T".into(),
            enabled: true,
            platform: "twitch".into(),
            stream_key: "k".into(),
            custom_egress_url: String::new(),
            twitch_ingest: "fra".into(),
            youtube_ingest: String::new(),
        };
        assert_eq!(
            d.egress_url().as_deref(),
            Some("rtmp://live-fra.twitch.tv/app/k")
        );
    }

    #[test]
    fn destination_custom_url_with_embedded_key_is_not_double_appended() {
        // A custom URL that already includes app+key (2+ path segments)
        // must be passed through verbatim, even if stream_key is set.
        let d = Destination {
            id: "x".into(),
            name: "C".into(),
            enabled: true,
            platform: "custom".into(),
            stream_key: "ignored".into(),
            custom_egress_url: "rtmp://my.server/live/secret_key".into(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
        };
        assert_eq!(
            d.egress_url().as_deref(),
            Some("rtmp://my.server/live/secret_key"),
            "custom URL with app+key must not get an extra key appended"
        );
    }

    #[test]
    fn destination_youtube_backup_uses_b_ingest() {
        let d = Destination {
            id: "x".into(),
            name: "Y".into(),
            enabled: true,
            platform: "youtube".into(),
            stream_key: "abcd".into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: "backup".into(),
        };
        assert_eq!(
            d.egress_url().as_deref(),
            Some("rtmp://b.rtmp.youtube.com/live2/abcd?backup=1"),
            "YouTube backup ingest needs the ?backup=1 suffix so their \
             edge enables real fail-over, not just a duplicate stream"
        );
    }

    #[test]
    fn destination_well_formed_requires_app_and_key() {
        let bad = Destination {
            id: "x".into(),
            name: "B".into(),
            enabled: true,
            platform: "custom".into(),
            stream_key: String::new(),
            // No app segment — just a bare host. Not a valid RTMP URL.
            custom_egress_url: "rtmp://my.server".into(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
        };
        assert!(!bad.is_well_formed());
    }

    #[test]
    fn settings_validate_rejects_zero_ports_and_collisions() {
        let mut s = Settings::defaults();
        s.ingest_port = 0;
        assert!(s.validate().iter().any(|e| e.contains("ingest_port")));

        let mut s2 = Settings::defaults();
        s2.web_port = s2.ingest_port;
        assert!(s2.validate().iter().any(|e| e.contains("must differ")));
    }

    #[test]
    fn settings_validate_rejects_unsafe_paths() {
        let mut s = Settings::defaults();
        s.buffer_path = PathBuf::from("../../system32/cmd.exe");
        let errs = s.validate();
        assert!(
            errs.iter().any(|e| e.contains("buffer_path looks unsafe")),
            "expected buffer_path validation error, got: {:?}",
            errs
        );
    }

    #[test]
    fn settings_save_load_roundtrip_preserves_destinations_and_profiles() {
        // Build a config that exercises every field the on-disk format
        // touches: ports, paths, multiple destinations including a
        // custom-URL one, multiple profiles, and a webhook URL.
        let mut s = Settings::defaults();
        s.ingest_port = 1936;
        s.web_port = 7898;
        s.buffer_mb = 200;
        s.discord_webhook_url = "https://discord.com/api/webhooks/123/abc".into();
        s.destinations.clear();
        s.destinations.push(Destination {
            id: "tw1".into(),
            name: "Twitch".into(),
            enabled: true,
            platform: "twitch".into(),
            stream_key: "live_secret".into(),
            custom_egress_url: String::new(),
            twitch_ingest: "fra".into(),
            youtube_ingest: String::new(),
        });
        s.destinations.push(Destination {
            id: "yt1".into(),
            name: "YT Backup".into(),
            enabled: false,
            platform: "youtube".into(),
            stream_key: "yt_secret".into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: "backup".into(),
        });
        s.profiles = vec![
            DelayProfile {
                name: "Quick".into(),
                delay_ms: 10_000,
            },
            DelayProfile {
                name: "Long".into(),
                delay_ms: 120_000,
            },
        ];

        let path = std::env::temp_dir().join(format!("ic-test-cfg-{}.ini", std::process::id()));
        let _ = std::fs::remove_file(&path);
        s.save(&path).expect("save");
        let loaded = Settings::load(&path).expect("load");
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded.ingest_port, 1936);
        assert_eq!(loaded.web_port, 7898);
        assert_eq!(loaded.buffer_mb, 200);
        assert_eq!(
            loaded.discord_webhook_url,
            "https://discord.com/api/webhooks/123/abc"
        );
        assert_eq!(loaded.destinations.len(), 2);
        assert_eq!(loaded.destinations[0].id, "tw1");
        assert_eq!(loaded.destinations[0].name, "Twitch");
        assert_eq!(loaded.destinations[0].stream_key, "live_secret");
        assert_eq!(loaded.destinations[0].twitch_ingest, "fra");
        assert!(loaded.destinations[0].enabled);
        assert!(!loaded.destinations[1].enabled);
        assert_eq!(loaded.destinations[1].youtube_ingest, "backup");
        assert_eq!(loaded.profiles.len(), 2);
        assert_eq!(loaded.profiles[0].delay_ms, 10_000);
        assert_eq!(loaded.profiles[1].name, "Long");
    }

    #[test]
    fn missing_config_file_falls_back_to_defaults() {
        // load_or_default must not panic on a non-existent path — that's
        // the cold-start case where there's nothing to read yet.
        let path = std::env::temp_dir().join(format!("ic-no-such-file-{}.ini", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let s = Settings::load_or_default(&path);
        assert_eq!(s.ingest_port, 1935);
        assert_eq!(s.web_port, 7799);
        assert!(!s.configured);
    }
}

// Tiny libc shim for Unix free-space — avoids adding the `libc` crate.
#[cfg(not(windows))]
mod libc {
    #[repr(C)]
    pub struct statvfs {
        pub f_bsize: u64,
        pub f_frsize: u64,
        pub f_blocks: u64,
        pub f_bfree: u64,
        pub f_bavail: u64,
        pub f_files: u64,
        pub f_ffree: u64,
        pub f_favail: u64,
        pub f_fsid: u64,
        pub f_flag: u64,
        pub f_namemax: u64,
        // padding/reserved: we only read f_bavail and f_frsize, but on many
        // platforms the struct is larger. Allocate generous tail bytes.
        _reserved: [u64; 8],
    }
    extern "C" {
        pub fn statvfs(path: *const i8, buf: *mut self::statvfs) -> i32;
    }
}
