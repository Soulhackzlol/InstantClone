//! Persistent configuration: load from disk, save atomically, expose
//! platform presets so users only paste their stream key.
//!
//! File format is intentionally line-based "key=value" so the file is
//! editable by hand and trivially parseable without a serde dependency.
//! For wire transport (web UI), `to_json` emits a small JSON object.

use std::collections::BTreeMap;
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
/// Ceiling on saved dock layouts (each is one OBS browser dock's widget
/// arrangement). Persisted server-side so a layout survives OBS clearing
/// its browser cache; capped so a runaway client can't bloat the config.
pub const MAX_DOCKS: usize = 64;
/// Max serialized length of one dock layout blob. Comfortably fits the
/// widget list; guards the on-disk file against a giant POST body.
pub const MAX_DOCK_LAYOUT_LEN: usize = 8 * 1024;
/// Minimum on-disk buffer size (MB). Below this, the ring is too small
/// to hold a single typical IDR (which can be 100 KB at 1080p60).
/// Also guards against the `% 0` panic in `DiskRing::append` if a
/// hand-edited config sets `buffer_mb=0`.
const MIN_BUFFER_MB: u64 = 50;

/// Ports for the built-in "Local test sink" destination (platform
/// `"sink"`). The egress supervisor spawns `instantclone sink` as a
/// child process on these when such a destination is enabled. Fixed
/// (not configurable) on purpose: the sink's own CLI defaults (1936 /
/// 1937) are what people use for MANUAL sink runs and the e2e script,
/// so the managed instance stays out of their way on a high pair.
pub const SINK_RTMP_PORT: u16 = 19350;
pub const SINK_WEB_PORT: u16 = 19351;

#[derive(Debug, Clone)]
pub struct Settings {
    pub ingest_port: u16,
    pub ingest_bind_all: bool, // true → 0.0.0.0, false → 127.0.0.1
    /// Optional shared secret an RTMP publisher must use as its stream key.
    /// Empty (the default) accepts any key, which is the right behaviour on a
    /// local machine. Set it to lock the ingest down when it is exposed to a
    /// network, so only OBS configured with this exact key can publish.
    pub ingest_key: String,

    // Legacy single-destination fields. Still present on disk for backward
    // compatibility - on load they get migrated into destinations[0] if
    // `destinations` was empty (i.e. config written before multi-dest).
    pub platform: String,
    pub stream_key: String,
    pub custom_egress_url: String,

    pub web_port: u16,
    pub web_bind_all: bool,
    /// Optional PBKDF2 hash of the dashboard password. Empty (the default)
    /// means auth is OFF and the dashboard/API are open exactly as before. Set
    /// means a login is required. Never sent to the client; the wire only
    /// carries an `auth_enabled` bool.
    pub dashboard_password_hash: String,
    /// Secret that lets the OBS dock (which cannot show a login form) through
    /// the gate for delay CONTROL only, never settings. Generated when auth is
    /// enabled, regenerable, empty when auth is off.
    pub dock_token: String,
    pub buffer_mb: u64,
    pub buffer_path: PathBuf,
    pub target_delay_ms: u32,
    pub armed_delay_ms: u32,
    pub configured: bool,
    pub profiles: Vec<DelayProfile>,
    pub destinations: Vec<Destination>,
    pub discord_webhook_url: String,
    pub overlays_dir: PathBuf,
    /// Whether the wire-level egress trace (instantclone-trace.log) is
    /// actively writing. Defaults to false (fresh installs ship it off);
    /// toggleable in the System tab so users can turn it on when diagnosing
    /// an issue, then off again so the file stops growing. Configs saved by
    /// early betas with `tracing_enabled=true` are still honoured on load.
    pub tracing_enabled: bool,
    /// Behaviour: when OBS's RTMP publisher handshake completes, auto-arm
    /// the delay at `auto_arm_delay_ms`. Off by default - the
    /// deliberate-arm-then-activate flow stays the explicit one. Turning
    /// this on suits streamers who *always* want delay (tournament
    /// players, IRL streamers behind a delay-for-safety) and don't want
    /// to remember to arm every session.
    pub auto_arm_on_connect: bool,
    /// Behaviour: when the buffer hits the armed amount and phase
    /// transitions to "ready", auto-fire the activate. Independent of
    /// `auto_arm_on_connect`; with this on, every manual arm becomes
    /// a one-step "go live with delay" (you give up the deliberate
    /// click-when-i'm-ready moment, you get zero-touch delayed streaming).
    pub auto_activate_when_ready: bool,
    /// Behaviour: target delay for `auto_arm_on_connect`. Also serves
    /// as the pre-filled default in the manual delay input. Tracks the
    /// last value the user manually armed at (saved every time the
    /// streamer hits Arm with a non-zero value) so "last used" sticks
    /// across sessions without a separate UI knob to maintain it.
    pub auto_arm_delay_ms: u32,
    /// Whether the dashboard checks GitHub for a newer release on load.
    /// Default true - the check is a passive nicety (cached 10 min
    /// server-side, failures silent). Off for the privacy-minded or
    /// air-gapped: it is the app's one routine outbound call, so it gets
    /// an explicit opt-out in System -> About.
    pub update_check_enabled: bool,
    /// Whether a normal launch opens the dashboard in the browser. Default
    /// true. Turning it off suits streamers who keep InstantClone running
    /// in the tray (or autostart it) and don't want a tab every launch -
    /// the same effect as the `--no-browser` CLI flag, but persisted and
    /// reachable from the UI. The flag still forces suppression regardless.
    pub open_dashboard_on_launch: bool,
    /// State: whether the built-in preset overlays have been seeded to the
    /// overlays folder as real files yet. Set true after the dashboard bakes
    /// them on first run, so deleting a seeded overlay doesn't bring it back.
    /// Reset to false (and the overlays wiped) by "Restore default overlays"
    /// and the factory reset, which re-seeds on the next dashboard load.
    pub overlays_seeded: bool,
    /// Saved OBS dock layouts, keyed by the dock's `?dock=<id>` slot. The
    /// value is the dock's own opaque layout JSON (the server never parses
    /// it - the dock owns its schema). Persisted here so a customized dock
    /// survives OBS wiping its browser cache or the machine restarting.
    pub docks: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct DelayProfile {
    pub name: String,
    pub delay_ms: u32,
}

/// One streaming destination - Twitch, YouTube, a private server, etc.
/// Each destination runs its own RTMP egress and is paced/cut/timestamp-
/// rewritten independently from the same shared DiskRing.
#[derive(Debug, Clone)]
pub struct Destination {
    pub id: String,   // stable across renames; UI key
    pub name: String, // user label ("Twitch", "Backup", etc.)
    pub enabled: bool,
    // "twitch" | "youtube" | "kick" | "trovo" | "restream" | "custom"
    // | "sink" (the built-in local test sink - see SINK_RTMP_PORT)
    pub platform: String,
    pub stream_key: String,
    pub custom_egress_url: String, // used when platform == "custom"
    /// Only meaningful when platform == "twitch". Empty = auto-route via
    /// live.twitch.tv (which load-balances regionally). Otherwise: a
    /// Twitch ingest slug from `twitch_ingests()` (e.g. "fra", "jfk").
    /// Pinning a region helps when auto-routing keeps picking a flaky
    /// edge - common for streamers near a region boundary.
    pub twitch_ingest: String,
    /// Only meaningful when platform == "youtube". One of:
    ///   "" / "primary"  → rtmp://a.rtmp.youtube.com/live2 (default)
    ///   "backup"        → rtmp://b.rtmp.youtube.com/live2
    /// YouTube recommends using the backup ingest when the primary is
    /// flaky in the streamer's region - surfacing it as an option saves
    /// users from dropping to a Custom RTMP URL just to switch endpoints.
    pub youtube_ingest: String,
    /// VOD-audio mode (Twitch only). When true, on publisher-connect we
    /// call Twitch's GetClientConfiguration ourselves with
    /// `vod_track_audio: true`, get back an IVS session URL with a
    /// VOD-audio slot allocated, and stash it on `eb_override_url`.
    /// The streamer must also set OBS to Custom RTMP + enable the VOD
    /// Track checkbox in Output settings (we write the unlocking flag
    /// to OBS's global.ini when the toggle is on).
    ///
    /// Trade-off: VOD-audio mode does not use the EB transcoded ladder
    /// by default - the streamer gets the legacy single-resolution
    /// transcode (Affiliate/Partner) or Source-Only (non-Affiliate).
    /// To recover EB on top of VOD-audio, see `vod_audio_inject_eb`.
    pub vod_audio: bool,
    /// EXPERIMENTAL companion to `vod_audio`. When both this and
    /// `vod_audio` are true, the dashboard injects
    /// `multitrack_video_configuration_url` into the user's OBS
    /// rtmp_custom service.json for the active profile. With that key
    /// present, OBS auto-fetches our multitrack-video config even on
    /// Custom RTMP - bypassing OBS's "name == Twitch" gate so VOD-audio
    /// and Enhanced Broadcasting both fire on the same session.
    ///
    /// Marked experimental because the file we touch is OBS-internal
    /// and per-profile; a future OBS update could change the on-disk
    /// shape and break the injection. We write a `.bak` on every edit.
    pub vod_audio_inject_eb: bool,
    /// Which canvas to forward to this destination. Only meaningful for
    /// non-Twitch platforms (Twitch gets the native dual-canvas
    /// passthrough). One of:
    ///   "horizontal" (default) - the primary 16:9 canvas (TrackId 0)
    ///   "vertical"             - the 9:16 canvas Twitch Dual Format /
    ///                            Enhanced Broadcasting produces, reused
    ///                            for YouTube Shorts / Kick mobile / etc.
    ///
    /// Vertical only has data on the wire while Twitch Dual Format (EB)
    /// is active in OBS. With EB off there is no vertical canvas, so a
    /// vertical destination waits (sends no video) until one appears -
    /// see `h264::detect_vertical_primary_track`. Unknown / empty values
    /// fall back to "horizontal".
    pub stream_format: String,
    /// Which OBS audio track(s) this destination receives. Only meaningful
    /// once OBS sends a second audio track (the VOD-unlocker script, or
    /// Enhanced Broadcasting). One of:
    ///   "auto" (default) - Twitch gets every track (so the VOD-audio track
    ///                      keeps flowing); every other platform gets the
    ///                      single live track (wire TrackId 0).
    ///   "both"           - pass every track through. Only Twitch consumes a
    ///                      second audio track, so for other platforms this
    ///                      degrades to the live track.
    ///   "1"              - the live track only (wire TrackId 0).
    ///   "2"              - the second / clean track only (wire TrackId 1),
    ///                      flattened to a single track. Routes copyright-safe
    ///                      audio to YouTube / Kick while Twitch keeps both.
    /// Unknown / empty values fall back to "auto".
    pub audio_track: String,
}

impl Destination {
    /// Build the resolved egress URL for this destination. Same forgiving
    /// rules as the top-level single-dest path: if a custom URL already has
    /// app+key, the key field is ignored; otherwise the key is appended.
    pub fn egress_url(&self) -> Option<String> {
        // Local test sink: fully self-contained. The URL is fixed (the
        // supervisor spawns the matching `instantclone sink` child on
        // SINK_RTMP_PORT) and the stream key is ignored - the sink
        // accepts any key, so requiring one would just add a fake
        // field to fill in.
        if self.platform == "sink" {
            return Some(format!("rtmp://127.0.0.1:{}/live/test", SINK_RTMP_PORT));
        }
        // Custom and Kick both take a user-supplied Server URL and append
        // the stream key if it isn't already embedded. Kick shows each
        // streamer a distinct (masked) Server value in its dashboard
        // (Settings -> Stream) alongside the key, so we can't hardcode one
        // host for everyone - the streamer pastes their own. Kick's URL is
        // rtmps://, which the egress socket upgrades to TLS transparently.
        if self.platform == "custom" || self.platform == "kick" {
            let mut base = non_empty(&self.custom_egress_url)?
                .trim_end_matches('/')
                .to_string();
            // Kick's ingest app is always "app". Streamers sometimes paste
            // just the host (rtmps://<id>.global-contribute.live-video.net)
            // and leave the "/app" off; add it so the resolved URL has the
            // app path Kick needs instead of failing the app+key check.
            if self.platform == "kick" && !strip_rtmp_scheme(&base).contains('/') {
                base.push_str("/app");
            }
            let path = strip_rtmp_scheme(&base)
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
        // drop would still glitch the stream - the worst of both worlds.
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
                let path = strip_rtmp_scheme(&url)
                    .split_once('/')
                    .map(|x| x.1)
                    .unwrap_or("");
                path.split('/').filter(|s| !s.is_empty()).count() >= 2
            }
            None => false,
        }
    }

    /// True when this destination should receive the vertical (9:16)
    /// canvas instead of the horizontal primary. Twitch destinations
    /// always get native dual-canvas passthrough, so the vertical flag
    /// is meaningless there and ignored - this guard is the single
    /// source of truth the egress supervisor reads.
    pub fn wants_vertical(&self) -> bool {
        self.platform != "twitch" && self.stream_format == "vertical"
    }
}

impl Settings {
    pub fn defaults() -> Self {
        Self {
            ingest_port: 1935,
            ingest_bind_all: false,
            ingest_key: String::new(),
            platform: "twitch".into(),
            stream_key: String::new(),
            custom_egress_url: String::new(),
            web_port: 7799,
            web_bind_all: false,
            dashboard_password_hash: String::new(),
            dock_token: String::new(),
            // 500 MB ≈ 6m 50s at 10 Mbps, ~11 min at 6 Mbps. The cap on
            // what the user could ever arm at the current bitrate; with
            // the trim logic the *actual* used portion matches the armed
            // delay. Bumped from 300 after the 2026-05-31 beta.5 test
            // surfaced that 300 MB stalls at ~247 s when arming 5 min at
            // 10 Mbps - the new default leaves headroom for a 5-min arm
            // at any realistic Twitch bitrate. Lower to save disk; raise
            // for >7-min delays at 10 Mbps+.
            buffer_mb: 500,
            buffer_path: PathBuf::from("./instantclone.buf"),
            target_delay_ms: 0,
            armed_delay_ms: 0,
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
            // Off by default for end users - the trace file grows fast
            // (~3000 lines/s at typical bitrate) and 99 % of users
            // never need it. Streamers reporting a bug can toggle it
            // on in System → Advanced diagnostics, reproduce, then
            // send `./instantclone-trace.log`. Saved configs with
            // `tracing_enabled=true` from earlier betas are honoured
            // - only fresh installs default to off.
            tracing_enabled: false,
            // Behaviour toggles default off so the two-phase
            // arm/activate ceremony stays the canonical flow. Streamers
            // who always want delay opt in once via System -> Behavior.
            auto_arm_on_connect: false,
            auto_activate_when_ready: false,
            // 15 s is the same default the wizard suggests and matches
            // the "Quick" delay profile, so a streamer who flips
            // auto_arm_on_connect without otherwise customising lands
            // on a familiar number.
            auto_arm_delay_ms: 15_000,
            // Both default on: the update check is a convenience most users
            // want, and opening the dashboard is the expected first-run
            // behaviour. Each has an explicit opt-out in the System tab.
            update_check_enabled: true,
            open_dashboard_on_launch: true,
            // Fresh install hasn't seeded the preset overlays yet; the
            // dashboard does it on first load, then flips this true.
            overlays_seeded: false,
            docks: BTreeMap::new(),
        }
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let mut s = Self::defaults();
        // Both lists are file-authoritative - a user who deletes them all
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
                vod_audio: false,
                vod_audio_inject_eb: false,
                stream_format: "horizontal".into(),
                audio_track: "auto".into(),
            });
        }
        // Clamp / sanitize on load - hand-edited values can otherwise
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
            // Defensive port move - if the user collided them in the
            // config file, push the web port to a sane fallback so at
            // least the UI is reachable to fix it.
            self.web_port = if self.ingest_port == 7799 { 7800 } else { 7799 };
        }
        // Cap delay values to the same 10-minute ceiling the runtime uses.
        self.armed_delay_ms = self.armed_delay_ms.min(600_000);
        self.target_delay_ms = self.target_delay_ms.min(600_000);
        // auto_arm_delay_ms is the value used when auto-arm fires. Clamp
        // to the same 10-minute ceiling; a 0 from a fresh install or a
        // hand-edited config falls back to the 15 s default so we never
        // auto-arm at "0 s of delay" (which would be a no-op + confusing).
        self.auto_arm_delay_ms = self.auto_arm_delay_ms.min(600_000);
        if self.auto_arm_delay_ms == 0 {
            self.auto_arm_delay_ms = 15_000;
        }
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
        // Write-then-rename: a crash, power-cut, or out-of-disk error
        // mid-write must not be able to leave the canonical config in
        // a half-written state. fs::File::create truncates immediately,
        // so the old "writeln! 20 times" path could leave the user with
        // a 1-line config that wouldn't parse on the next launch. We
        // build the full payload into a sibling `.tmp` file, fsync it,
        // then atomically rename over the canonical path.
        //
        // The rename is atomic on the same filesystem on every OS we
        // ship for: POSIX `rename(2)` is atomic by spec, Windows
        // `MoveFileExW(REPLACE_EXISTING)` (what fs::rename calls) is
        // atomic at the directory-entry level since NTFS journals the
        // op. So a process death between unlink + create is impossible
        // - either the old file is intact or the new one is.
        let tmp = match path.file_name() {
            Some(name) => {
                let mut t = name.to_os_string();
                t.push(".tmp");
                path.with_file_name(t)
            }
            // Pathological: caller handed us a path like `/` with no
            // file name. Fall back to in-place write so we don't
            // silently lose the save.
            None => return self.save_inplace(path),
        };
        self.save_inplace(&tmp)?;
        fs::rename(&tmp, path)
    }

    /// The non-atomic write - split out so the atomic `save` can call
    /// it for the `.tmp` step, and the rare fallback path (no
    /// file_name component) can still produce SOMETHING on disk.
    fn save_inplace(&self, path: &Path) -> io::Result<()> {
        let mut f = fs::File::create(path)?;
        writeln!(f, "# InstantClone - written by the app. Hand edits OK.")?;
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
        // Secret: only written when set, so a default install's config never
        // carries an empty key line and a downgrade stays clean.
        if !self.ingest_key.is_empty() {
            writeln!(f, "ingest_key={}", self.ingest_key)?;
        }
        writeln!(f, "web_port={}", self.web_port)?;
        writeln!(f, "web_bind_all={}", self.web_bind_all)?;
        // Auth secrets: only written when set (auth on), so a default install's
        // config carries neither and a downgrade stays clean.
        if !self.dashboard_password_hash.is_empty() {
            writeln!(f, "dashboard_password_hash={}", self.dashboard_password_hash)?;
        }
        if !self.dock_token.is_empty() {
            writeln!(f, "dock_token={}", self.dock_token)?;
        }
        writeln!(f, "buffer_mb={}", self.buffer_mb)?;
        writeln!(f, "buffer_path={}", self.buffer_path.display())?;
        writeln!(f, "target_delay_ms={}", self.target_delay_ms)?;
        writeln!(f, "armed_delay_ms={}", self.armed_delay_ms)?;
        writeln!(f, "discord_webhook_url={}", self.discord_webhook_url)?;
        writeln!(f, "overlays_dir={}", self.overlays_dir.display())?;
        writeln!(f, "tracing_enabled={}", self.tracing_enabled)?;
        // Behaviour. Only emit when non-default so a downgrade to a
        // pre-v0.2 build that doesn't know these keys keeps parsing
        // (apply_field's `_ => {}` arm drops keys it doesn't recognise,
        // so this is belt-and-braces; but keeping config files lean on
        // disk is worth a couple of conditional writes).
        if self.auto_arm_on_connect {
            writeln!(f, "auto_arm_on_connect=true")?;
        }
        if self.auto_activate_when_ready {
            writeln!(f, "auto_activate_when_ready=true")?;
        }
        if self.auto_arm_delay_ms != 15_000 {
            writeln!(f, "auto_arm_delay_ms={}", self.auto_arm_delay_ms)?;
        }
        // Both default true, so only the opt-out is worth writing.
        if !self.update_check_enabled {
            writeln!(f, "update_check_enabled=false")?;
        }
        if !self.open_dashboard_on_launch {
            writeln!(f, "open_dashboard_on_launch=false")?;
        }
        if self.overlays_seeded {
            writeln!(f, "overlays_seeded=true")?;
        }
        // Dock layouts. The value is single-line JSON (no `=`/newline in
        // compact JSON), so it round-trips through the line-based parser
        // even though it is opaque to us.
        for (id, layout) in &self.docks {
            writeln!(f, "dock.{}={}", id, layout)?;
        }
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
            // VOD audio knobs. Defaults (false/false) are written only
            // when set so a downgrade to a pre-Phase-B build still
            // parses cleanly (apply_field's `_ => {}` arm drops keys
            // it doesn't recognise).
            if d.vod_audio {
                writeln!(f, "destination.{}.vod_audio=true", i)?;
            }
            if d.vod_audio_inject_eb {
                writeln!(f, "destination.{}.vod_audio_inject_eb=true", i)?;
            }
            // Only emit when non-default ("vertical") so a downgrade to a
            // pre-vertical build still parses cleanly (apply_field drops
            // unknown keys), and fresh configs stay lean.
            if d.stream_format == "vertical" {
                writeln!(f, "destination.{}.stream_format=vertical", i)?;
            }
            // Only emit when non-default ("auto"), same downgrade-safe reason.
            if d.audio_track != "auto" && !d.audio_track.is_empty() {
                writeln!(f, "destination.{}.audio_track={}", i, d.audio_track)?;
            }
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
            "ingest_key" => self.ingest_key = value.into(),
            "web_port" => {
                if let Ok(v) = value.parse() {
                    self.web_port = v;
                }
            }
            "web_bind_all" => self.web_bind_all = value == "true",
            "dashboard_password_hash" => self.dashboard_password_hash = value.into(),
            // Only accept a hex token from disk: it is written into a Set-Cookie
            // header, so a hand-edited value with CR/LF (or anything non-hex)
            // must never reach header construction. Generated tokens are always
            // hex, so this rejects only tampering.
            "dock_token" => {
                if !value.is_empty() && value.bytes().all(|b| b.is_ascii_hexdigit()) {
                    self.dock_token = value.into();
                }
            }
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
            "discord_webhook_url" => self.discord_webhook_url = value.into(),
            "overlays_dir" => self.overlays_dir = PathBuf::from(value),
            "tracing_enabled" => self.tracing_enabled = value.parse().unwrap_or(false),
            "auto_arm_on_connect" => self.auto_arm_on_connect = value == "true",
            "auto_activate_when_ready" => self.auto_activate_when_ready = value == "true",
            "update_check_enabled" => self.update_check_enabled = value != "false",
            "open_dashboard_on_launch" => self.open_dashboard_on_launch = value != "false",
            "overlays_seeded" => self.overlays_seeded = value == "true",
            "auto_arm_delay_ms" => {
                if let Ok(v) = value.parse() {
                    self.auto_arm_delay_ms = v;
                }
            }
            k if k.starts_with("dock.") => {
                let id = &k["dock.".len()..];
                if !id.is_empty() && id.len() <= 40 && self.docks.len() < MAX_DOCKS {
                    self.docks.insert(id.to_string(), value.to_string());
                }
            }
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
                        // already absurd for a single streamer - beyond
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
                                vod_audio: false,
                                vod_audio_inject_eb: false,
                                stream_format: "horizontal".into(),
                                audio_track: "auto".into(),
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
                            "vod_audio" => d.vod_audio = value == "true",
                            "vod_audio_inject_eb" => d.vod_audio_inject_eb = value == "true",
                            "stream_format" => {
                                // Only "vertical" is honored; anything else
                                // (incl. "horizontal", empty, or a typo)
                                // falls back to the safe horizontal default.
                                d.stream_format = if value == "vertical" {
                                    "vertical".into()
                                } else {
                                    "horizontal".into()
                                };
                            }
                            "audio_track" => {
                                // Known routing modes only; anything else
                                // (empty, a typo) falls back to "auto".
                                d.audio_track = match value {
                                    "both" | "1" | "2" => value.into(),
                                    _ => "auto".into(),
                                };
                            }
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

    /// First destination's resolved URL - used by the legacy single-dest
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

    /// All resolved, well-formed, enabled destinations - the set the
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

    /// Serialize for the dashboard's `/config` fetch.
    ///
    /// `start_with_windows` is passed in rather than read here because it
    /// does not live in `Settings` at all - the Run-key entry is its own
    /// source of truth (see `crate::autostart`). Taking it as an argument
    /// keeps this function free of hidden I/O.
    pub fn to_json(&self, start_with_windows: bool) -> String {
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
                r#"{{"id":{id},"name":{n},"enabled":{en},"platform":{p},"custom_egress_url":{cu},"twitch_ingest":{ti},"youtube_ingest":{yi},"stream_format":{sf},"stream_key_set":{ks},"egress_url_redacted":{red}}}"#,
                id  = json_str(&d.id),
                n   = json_str(&d.name),
                en  = d.enabled,
                p   = json_str(&d.platform),
                cu  = json_str(&d.custom_egress_url),
                ti  = json_str(&d.twitch_ingest),
                yi  = json_str(&d.youtube_ingest),
                sf  = json_str(&d.stream_format),
                ks  = !d.stream_key.is_empty(),
                red = json_str(&redact_key(&url)),
            ));
        }
        dests.push(']');
        format!(
            r#"{{"configured":{c},"ingest_port":{ip},"ingest_bind_all":{iba},"web_port":{wp},"web_bind_all":{wba},"buffer_mb":{bm},"buffer_path":{bp},"target_delay_ms":{td},"obs_url":{ou},"discord_webhook_url":{dw},"webhook_set":{ws},"overlays_dir":{ov},"tracing_enabled":{te},"auto_arm_on_connect":{aaoc},"auto_activate_when_ready":{aawr},"auto_arm_delay_ms":{aadm},"overlays_seeded":{os},"start_with_windows":{sww},"update_check_enabled":{uce},"open_dashboard_on_launch":{odol},"ingest_key":{ik},"auth_enabled":{ae},"dock_token":{dt},"os":{osname},"version":{ver},"destinations":{dests}}}"#,
            c = self.configured,
            sww = start_with_windows,
            ik = json_str(&self.ingest_key),
            ae = !self.dashboard_password_hash.is_empty(),
            dt = json_str(&self.dock_token),
            osname = json_str(os_name()),
            ver = json_str(crate::update_check::current_version()),
            ip = self.ingest_port,
            iba = self.ingest_bind_all,
            wp = self.web_port,
            wba = self.web_bind_all,
            bm = self.buffer_mb,
            bp = json_str(&self.buffer_path.display().to_string()),
            td = self.target_delay_ms,
            ou = json_str(&self.obs_url()),
            dw = json_str(&redact_webhook(&self.discord_webhook_url)),
            ws = !self.discord_webhook_url.is_empty(),
            ov = json_str(&self.overlays_dir.display().to_string()),
            te = self.tracing_enabled,
            aaoc = self.auto_arm_on_connect,
            aawr = self.auto_activate_when_ready,
            aadm = self.auto_arm_delay_ms,
            os = self.overlays_seeded,
            uce = self.update_check_enabled,
            odol = self.open_dashboard_on_launch,
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
            errs.push("ingest_port and web_port must differ - they share the same socket otherwise and one will silently fail to bind".into());
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
            if d.platform == "custom" || d.platform == "kick" {
                if d.custom_egress_url.is_empty() {
                    let need = if d.platform == "kick" {
                        "Kick Server URL required - copy it from your Kick dashboard (Settings -> Stream)"
                    } else {
                        "custom RTMP URL required"
                    };
                    errs.push(format!("{}: {}", d.name, need));
                    continue;
                }
                if !is_rtmp_scheme(&d.custom_egress_url) {
                    errs.push(format!(
                        "{}: server URL must start with rtmp:// or rtmps://",
                        d.name
                    ));
                    continue;
                }
                // Kick shows Server and Key as separate fields, so its key
                // is required here. A custom URL may embed the key in the
                // path instead, so custom doesn't force a separate key.
                if d.platform == "kick" && d.stream_key.is_empty() {
                    errs.push(format!(
                        "{}: Kick stream key required - it's next to the Server field in your dashboard",
                        d.name
                    ));
                    continue;
                }
            } else if d.platform == "sink" {
                // Self-contained: fixed local URL, no key needed.
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
        None => return false, // non-UTF8 path - refuse rather than guess
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

/// True when the URL uses an RTMP-family scheme the egress can dial
/// (`rtmp://` plaintext or `rtmps://` TLS). Used to validate custom
/// destination URLs - Kick, Facebook and other TLS-only ingests need
/// the `rtmps://` form.
pub fn is_rtmp_scheme(url: &str) -> bool {
    url.starts_with("rtmp://") || url.starts_with("rtmps://")
}

/// Strip an `rtmp://` or `rtmps://` scheme, returning the host/path
/// remainder. Tries the longer `rtmps://` first so it isn't shadowed by
/// the `rtmp://` prefix check.
fn strip_rtmp_scheme(url: &str) -> &str {
    url.strip_prefix("rtmps://")
        .or_else(|| url.strip_prefix("rtmp://"))
        .unwrap_or(url)
}

/// Mapping of platform slug → RTMP ingest base. New platforms go here.
pub fn platform_base(slug: &str) -> Option<&'static str> {
    Some(match slug {
        "twitch" => "rtmp://live.twitch.tv/app",
        "youtube" => "rtmp://a.rtmp.youtube.com/live2",
        // Kick ingests over RTMPS (TLS) only - plain rtmp:// on 1935 is
        // not accepted, which is why the old rtmp:// value never
        // connected. The `global-contribute` host is Kick's shared geo-
        // router (a CNAME for g.contribute.live-video.net), not a per-
        // account endpoint, so one entry works for everyone. A streamer
        // whose dashboard shows a different host can use the Custom RTMP
        // platform with their full rtmps:// URL instead. Port defaults to
        // 443 for the rtmps scheme (see EgressUrl::parse).
        "kick" => "rtmps://fa723fc1b171.global-contribute.live-video.net/app",
        "trovo" => "rtmp://livepush.trovo.live/live",
        "restream" => "rtmp://live.restream.io/live",
        _ => return None,
    })
}

/// Human-facing name for a platform slug. Falls back to the slug itself so
/// an unknown value still renders as something rather than blank. Kept
/// beside `platform_base` because they describe the same table - a new
/// platform needs an entry in both.
pub fn platform_label(slug: &str) -> &'static str {
    match slug {
        "twitch" => "Twitch",
        "youtube" => "YouTube",
        "kick" => "Kick",
        "trovo" => "Trovo",
        "restream" => "Restream",
        "custom" => "Custom RTMP",
        "sink" => "Local test sink",
        _ => "this destination",
    }
}

/// Curated list of regional Twitch ingests. Slug → human label. The slug
/// is the airport-code prefix Twitch uses in `live-<slug>.twitch.tv`.
/// Empty slug = auto (default - Twitch picks for you).
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

/// Compile-time OS tag the dashboard reads to pick platform-correct copy
/// (the start-at-login wording, the file-manager name, the tray mention).
fn os_name() -> &'static str {
    #[cfg(windows)]
    {
        "windows"
    }
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "linux"
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
            // Decode `%XX` straight from the raw bytes. NEVER slice `s` here:
            // `&s[i+1..i+3]` panics when the offset lands inside a multi-byte
            // UTF-8 char (e.g. a `%` before `€`), which under panic=abort is a
            // remote process kill from any form field.
            b'%' if i + 2 < bytes.len() => {
                match (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                    (Some(hi), Some(lo)) => {
                        out.push((hi << 4) | lo);
                        i += 3;
                    }
                    // Not a valid `%XX`; keep the literal `%` and move on.
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
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

/// Value of a single ASCII hex digit, or None. Used by `url_decode` so it can
/// decode `%XX` from bytes without ever slicing a `&str` on a non-char boundary.
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// `start_with_windows` is not a `Settings` field - it comes from the
    /// registry via the caller. This guards the wire contract the dashboard
    /// reads, which a signature change could otherwise drop silently.
    #[test]
    fn to_json_reflects_the_passed_autostart_state() {
        let s = Settings::defaults();
        assert!(s.to_json(true).contains(r#""start_with_windows":true"#));
        assert!(s.to_json(false).contains(r#""start_with_windows":false"#));
    }

    /// The auth + ingest secrets must survive a save/load round-trip, and the
    /// hash must NEVER appear in the client-facing JSON (only `auth_enabled`).
    #[test]
    fn secret_fields_round_trip_and_hash_stays_server_side() {
        let dir = std::env::temp_dir().join(format!("ic-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sec.config.json");

        let mut s = Settings::defaults();
        s.ingest_key = "streamsecret".into();
        s.dashboard_password_hash = "pbkdf2-sha256$210000$aa$bb".into();
        s.dock_token = "a1b2c3d4e5f60718".into();
        s.save(&path).unwrap();

        let loaded = Settings::load(&path).unwrap();
        assert_eq!(loaded.ingest_key, "streamsecret");
        assert_eq!(loaded.dashboard_password_hash, "pbkdf2-sha256$210000$aa$bb");
        assert_eq!(loaded.dock_token, "a1b2c3d4e5f60718");

        // Wire JSON: auth_enabled true, ingest_key + dock_token present, but
        // the password hash must not leak.
        let json = loaded.to_json(false);
        assert!(json.contains(r#""auth_enabled":true"#));
        assert!(json.contains(r#""dock_token":"a1b2c3d4e5f60718""#));
        assert!(!json.contains("pbkdf2-sha256"));
        assert!(!json.contains("dashboard_password_hash"));

        // A default install writes neither secret line and reads auth as off.
        let plain = Settings::defaults();
        let plain_path = dir.join("plain.config.json");
        plain.save(&plain_path).unwrap();
        let plain_text = std::fs::read_to_string(&plain_path).unwrap();
        assert!(!plain_text.contains("dashboard_password_hash"));
        assert!(!plain_text.contains("ingest_key"));
        assert!(Settings::load(&plain_path).unwrap().dashboard_password_hash.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The dashboard swaps platform-specific copy off this field, so it must
    /// always be present and non-empty.
    #[test]
    fn to_json_carries_the_os_tag() {
        let json = Settings::defaults().to_json(false);
        assert!(json.contains(r#""os":"windows""#) || json.contains(r#""os":"linux""#) || json.contains(r#""os":"macos""#));
    }

    #[test]
    fn platform_label_covers_every_known_platform() {
        // Any slug with an ingest base must also have a human label, or a
        // compatibility warning would read "this destination expects 2s".
        for slug in ["twitch", "youtube", "kick", "trovo", "restream"] {
            assert!(platform_base(slug).is_some(), "{slug} lost its base");
            assert_ne!(
                platform_label(slug),
                "this destination",
                "{slug} is missing a label"
            );
        }
    }

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

    /// Regression: a bare `%` before a multi-byte UTF-8 char used to slice the
    /// `&str` on a non-char boundary and panic (a remote process abort under
    /// panic=abort). It must now decode losslessly and never panic.
    #[test]
    fn url_decode_survives_percent_before_multibyte_char() {
        // `%€`, a stray `%` at the very end, and a truncated `%A`.
        assert_eq!(url_decode("%€"), "%€");
        assert_eq!(url_decode("abc%"), "abc%");
        assert_eq!(url_decode("x%Ay"), "x%Ay");
        // A real escape still works, and mixed content round-trips.
        assert_eq!(url_decode("a%20b%E2%82%ACc"), "a b€c");
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
    fn sink_destination_is_self_contained() {
        // No stream key, no custom URL - the local test sink needs
        // neither. The URL is the fixed managed-child endpoint, and
        // validate() must not demand a key for it.
        let d = Destination {
            id: "x".into(),
            name: "Test sink".into(),
            enabled: true,
            platform: "sink".into(),
            stream_key: String::new(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        };
        assert_eq!(
            d.egress_url().as_deref(),
            Some(format!("rtmp://127.0.0.1:{}/live/test", SINK_RTMP_PORT).as_str())
        );
        assert!(d.is_well_formed());
        let mut s = Settings::defaults();
        s.destinations.push(d);
        assert!(
            s.validate().is_empty(),
            "sink destination must validate without a key: {:?}",
            s.validate()
        );
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
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
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
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
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
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
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
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        };
        assert_eq!(
            d.egress_url().as_deref(),
            Some("rtmp://b.rtmp.youtube.com/live2/abcd?backup=1"),
            "YouTube backup ingest needs the ?backup=1 suffix so their \
             edge enables real fail-over, not just a duplicate stream"
        );
    }

    #[test]
    fn destination_egress_url_for_kick_uses_pasted_server_plus_key() {
        // Kick gives each streamer a distinct Server URL (rtmps) in the
        // dashboard; we store it in custom_egress_url and append the key.
        // The rtmps scheme makes the egress socket upgrade to TLS - the
        // old hardcoded rtmp:// host connected on 1935 and silently failed.
        let d = Destination {
            id: "x".into(),
            name: "K".into(),
            enabled: true,
            platform: "kick".into(),
            stream_key: "sk_test_key".into(),
            custom_egress_url: "rtmps://fa723fc1b171.global-contribute.live-video.net:443/app"
                .into(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        };
        let url = d.egress_url().unwrap();
        assert!(url.starts_with("rtmps://"), "kick must use rtmps: {url}");
        assert!(url.ends_with("/app/sk_test_key"));
        assert!(d.is_well_formed());
    }

    #[test]
    fn kick_server_url_without_app_path_gets_app_appended() {
        // Streamer pastes just the host (or host with a trailing slash),
        // leaving off "/app". We add Kick's fixed app so it still resolves
        // to a well-formed host/app/key URL instead of erroring.
        for server in [
            "rtmps://fa723fc1b171.global-contribute.live-video.net/",
            "rtmps://fa723fc1b171.global-contribute.live-video.net",
        ] {
            let d = Destination {
                id: "x".into(),
                name: "K".into(),
                enabled: true,
                platform: "kick".into(),
                stream_key: "sk_test_key".into(),
                custom_egress_url: server.into(),
                twitch_ingest: String::new(),
                youtube_ingest: String::new(),
                vod_audio: false,
                vod_audio_inject_eb: false,
                stream_format: "horizontal".into(),
                audio_track: "auto".into(),
            };
            assert_eq!(
                d.egress_url().as_deref(),
                Some("rtmps://fa723fc1b171.global-contribute.live-video.net/app/sk_test_key"),
                "host-only Kick server URL '{server}' should gain /app"
            );
            assert!(d.is_well_formed());
        }
    }

    #[test]
    fn kick_without_server_url_fails_validation() {
        let mut s = Settings::defaults();
        s.destinations.push(Destination {
            id: "k".into(),
            name: "Kick".into(),
            enabled: true,
            platform: "kick".into(),
            stream_key: "sk_test".into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        });
        assert!(
            s.validate().iter().any(|e| e.contains("Kick Server URL")),
            "kick with no server URL must report a clear error: {:?}",
            s.validate()
        );
    }

    #[test]
    fn destination_custom_rtmps_url_is_accepted() {
        // The Custom platform is the escape hatch for any TLS ingest
        // (a non-standard Kick host, Facebook, etc.). rtmps:// must
        // validate and pass through, well-formed with app+key.
        let d = Destination {
            id: "x".into(),
            name: "C".into(),
            enabled: true,
            platform: "custom".into(),
            stream_key: "ignored".into(),
            custom_egress_url: "rtmps://edge.example.net/app/streamkey".into(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        };
        assert_eq!(
            d.egress_url().as_deref(),
            Some("rtmps://edge.example.net/app/streamkey")
        );
        assert!(d.is_well_formed());
        assert!(is_rtmp_scheme("rtmps://edge.example.net/app/k"));
        assert!(is_rtmp_scheme("rtmp://edge.example.net/app/k"));
        assert!(!is_rtmp_scheme("https://edge.example.net/app/k"));
    }

    #[test]
    fn destination_well_formed_requires_app_and_key() {
        let bad = Destination {
            id: "x".into(),
            name: "B".into(),
            enabled: true,
            platform: "custom".into(),
            stream_key: String::new(),
            // No app segment - just a bare host. Not a valid RTMP URL.
            custom_egress_url: "rtmp://my.server".into(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
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
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
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
            vod_audio: false,
            vod_audio_inject_eb: false,
            // Non-default value so the round-trip proves stream_format
            // both writes (only when != "horizontal") and reads back.
            stream_format: "vertical".into(),
            // Non-default: the clean track routed to YouTube (copyright).
            // Proves audio_track writes only when != "auto" and reads back.
            audio_track: "2".into(),
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
        assert_eq!(loaded.destinations[0].stream_format, "horizontal");
        assert_eq!(loaded.destinations[1].stream_format, "vertical");
        // audio_track: default stays "auto", the non-default "2" survives.
        assert_eq!(loaded.destinations[0].audio_track, "auto");
        assert_eq!(loaded.destinations[1].audio_track, "2");
        assert_eq!(loaded.profiles.len(), 2);
        assert_eq!(loaded.profiles[0].delay_ms, 10_000);
        assert_eq!(loaded.profiles[1].name, "Long");
    }

    #[test]
    fn dock_layouts_round_trip_through_save_and_load() {
        // Dock layouts are opaque single-line JSON blobs stored as
        // `dock.<id>=<json>`. Prove a value containing `=`, `:`, and quotes
        // survives the line parser (it splits on the first `=` only, so the
        // JSON body is preserved verbatim).
        let mut s = Settings::defaults();
        s.docks.insert(
            "default".into(),
            r#"{"v":1,"preset":"minimal","w":{"number":{"on":true,"step":5}}}"#.into(),
        );
        s.docks
            .insert("gaming".into(), r#"{"preset":"delay","eq":"a=b"}"#.into());

        let path = std::env::temp_dir().join(format!("ic-test-docks-{}.ini", std::process::id()));
        let _ = std::fs::remove_file(&path);
        s.save(&path).expect("save");
        let loaded = Settings::load(&path).expect("load");
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded.docks.len(), 2);
        assert_eq!(
            loaded.docks.get("default").map(String::as_str),
            Some(r#"{"v":1,"preset":"minimal","w":{"number":{"on":true,"step":5}}}"#)
        );
        assert_eq!(
            loaded.docks.get("gaming").map(String::as_str),
            Some(r#"{"preset":"delay","eq":"a=b"}"#),
            "value after the first = (including embedded =) must survive"
        );
    }

    #[test]
    fn behavior_toggles_round_trip_through_save_and_load() {
        // v0.1.4 added three Settings fields for the auto-arm /
        // auto-activate behaviour. Save them, load them back, assert
        // every field survives. The format::Display writer is
        // conditional (only emits non-default values) so this also
        // verifies the apply_field reader sees those conditional keys.
        let path = std::env::temp_dir().join(format!(
            "ic-test-behavior-roundtrip-{}-{}.ini",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);

        let mut s = Settings::defaults();
        s.auto_arm_on_connect = true;
        s.auto_activate_when_ready = true;
        s.auto_arm_delay_ms = 30_000;
        s.overlays_seeded = true;
        // Need a destination so save() doesn't bail on the
        // configured-but-empty path.
        s.destinations.push(Destination {
            id: "test".into(),
            name: "Test".into(),
            enabled: true,
            platform: "twitch".into(),
            stream_key: "test".into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        });
        s.save(&path).expect("save");

        let loaded = Settings::load(&path).expect("load");
        assert!(
            loaded.auto_arm_on_connect,
            "auto_arm_on_connect must survive round-trip"
        );
        assert!(
            loaded.auto_activate_when_ready,
            "auto_activate_when_ready must survive round-trip"
        );
        assert_eq!(
            loaded.auto_arm_delay_ms, 30_000,
            "auto_arm_delay_ms must survive round-trip"
        );
        assert!(
            loaded.overlays_seeded,
            "overlays_seeded must survive round-trip"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn default_true_toggles_round_trip_when_disabled() {
        // update_check_enabled and open_dashboard_on_launch both default
        // true and are written only when turned off. Flipping them off must
        // survive a save/load, or a privacy opt-out would silently revert.
        let path = std::env::temp_dir().join(format!(
            "ic-test-optout-roundtrip-{}-{}.ini",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);

        let mut s = Settings::defaults();
        assert!(s.update_check_enabled, "defaults on");
        assert!(s.open_dashboard_on_launch, "defaults on");
        s.update_check_enabled = false;
        s.open_dashboard_on_launch = false;
        s.destinations.push(Destination {
            id: "test".into(),
            name: "Test".into(),
            enabled: true,
            platform: "twitch".into(),
            stream_key: "test".into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        });
        s.save(&path).expect("save");

        let loaded = Settings::load(&path).expect("load");
        assert!(
            !loaded.update_check_enabled,
            "update_check_enabled opt-out must survive round-trip"
        );
        assert!(
            !loaded.open_dashboard_on_launch,
            "open_dashboard_on_launch opt-out must survive round-trip"
        );

        // A fresh config with neither key present must read as the on
        // default, not accidentally inherit the false above.
        let fresh = Settings::defaults();
        assert!(fresh.update_check_enabled && fresh.open_dashboard_on_launch);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn behavior_defaults_omitted_from_save_to_keep_config_lean() {
        // The save writer skips default values for the behaviour keys
        // so a fresh install's config file stays clean. Verify by
        // saving a defaults() snapshot and grepping the file.
        let path = std::env::temp_dir().join(format!(
            "ic-test-behavior-defaults-{}-{}.ini",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);

        let mut s = Settings::defaults();
        s.destinations.push(Destination {
            id: "test".into(),
            name: "Test".into(),
            enabled: true,
            platform: "twitch".into(),
            stream_key: "test".into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: "horizontal".into(),
            audio_track: "auto".into(),
        });
        s.save(&path).expect("save");

        let text = std::fs::read_to_string(&path).expect("read");
        assert!(
            !text.contains("auto_arm_on_connect"),
            "default off must not be written"
        );
        assert!(
            !text.contains("auto_activate_when_ready"),
            "default off must not be written"
        );
        assert!(
            !text.contains("auto_arm_delay_ms"),
            "default 15000 must not be written"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn behavior_sanitize_clamps_and_fills_zero() {
        // sanitize_load() runs after load(); it should clamp the
        // delay to 600s and replace a zero (e.g. from a hand-edited
        // legacy config or downgrade roundtrip) with the 15 s default
        // so auto-arm never tries to arm at 0 s (= disarm, confusing).
        let mut s = Settings::defaults();

        s.auto_arm_delay_ms = 0;
        s.sanitize_load();
        assert_eq!(
            s.auto_arm_delay_ms, 15_000,
            "zero must fall back to 15 s default"
        );

        s.auto_arm_delay_ms = 9_999_999;
        s.sanitize_load();
        assert_eq!(
            s.auto_arm_delay_ms, 600_000,
            "above ceiling must clamp to 10 min"
        );
    }

    #[test]
    fn save_replaces_existing_file_and_leaves_no_tmp_sibling() {
        // Atomic save invariant: after a successful save() the canonical
        // file holds the new contents AND no `.tmp` sibling is left on
        // disk. The latter is the visible sign of a successful rename
        // (vs. a hypothetical fallback path that wrote in place).
        let path = std::env::temp_dir().join(format!(
            "ic-test-atomic-save-{}-{}.ini",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);

        // First save: file didn't exist beforehand.
        let mut a = Settings::defaults();
        a.web_port = 9001;
        a.save(&path).expect("first save");
        let on_disk_a = std::fs::read_to_string(&path).expect("read after first save");
        assert!(on_disk_a.contains("web_port=9001"));

        // Second save over the same path with different contents.
        // After the atomic rename, the canonical file must reflect
        // the SECOND save, not the first.
        let mut b = Settings::defaults();
        b.web_port = 9002;
        b.save(&path).expect("second save");
        let on_disk_b = std::fs::read_to_string(&path).expect("read after second save");
        assert!(on_disk_b.contains("web_port=9002"));
        assert!(
            !on_disk_b.contains("web_port=9001"),
            "second save did not replace the first - file still mentions :9001"
        );

        // The .tmp sibling that save() writes to internally must be
        // renamed-away, never left on disk after a successful save.
        let tmp = {
            let mut name = path.file_name().unwrap().to_os_string();
            name.push(".tmp");
            path.with_file_name(name)
        };
        assert!(
            !tmp.exists(),
            "save() left a stale {} on disk",
            tmp.display()
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_config_file_falls_back_to_defaults() {
        // load_or_default must not panic on a non-existent path - that's
        // the cold-start case where there's nothing to read yet.
        let path = std::env::temp_dir().join(format!("ic-no-such-file-{}.ini", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let s = Settings::load_or_default(&path);
        assert_eq!(s.ingest_port, 1935);
        assert_eq!(s.web_port, 7799);
        assert!(!s.configured);
    }
}

// Tiny libc shim for Unix free-space - avoids adding the `libc` crate.
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
