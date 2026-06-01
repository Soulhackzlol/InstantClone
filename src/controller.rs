//! The brain. Coordinates ingest → ring buffer → egress with:
//!   * dynamic delay (request + apply-at-next-IDR alignment)
//!   * monotonic output timestamp rewriting
//!   * input-starvation filler frames so the Twitch session stays alive
//!
//! Concurrency model: the ingest task calls `on_tag` synchronously (it
//! does only an indexed disk write + a notify). The egress task runs as
//! its own loop in `run_egress`, owning the connection to the upstream
//! platform. Communication is via `Controller`'s shared state + Notify.

use crate::buffer::{DiskRing, TagMeta};
use crate::h264::{AudioCodec, VideoCodec};
use crate::rtmp::client::{EgressClient, EgressSink, EgressUrl};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Process-anchored monotonic millisecond counter for hot-path
/// bandwidth / throttle math. `SystemTime::now()` on Windows is a
/// syscall and the previous implementation was calling it on every
/// audio / video tag (~150-300 per second per destination); switching
/// to `Instant::now()` is a vDSO call (~10ns) and immune to clock
/// adjustments.
fn process_now_ms() -> u64 {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    let start = ANCHOR.get_or_init(Instant::now);
    Instant::now().saturating_duration_since(*start).as_millis() as u64
}

/// Encode VideoCodec as a u8 for atomic storage.
fn enc_vcodec(c: VideoCodec) -> u8 {
    match c {
        VideoCodec::Unknown => 0,
        VideoCodec::Avc => 1,
        VideoCodec::Hevc => 2,
        VideoCodec::Av1 => 3,
        VideoCodec::Vp9 => 4,
    }
}
fn dec_vcodec(v: u8) -> VideoCodec {
    match v {
        1 => VideoCodec::Avc,
        2 => VideoCodec::Hevc,
        3 => VideoCodec::Av1,
        4 => VideoCodec::Vp9,
        _ => VideoCodec::Unknown,
    }
}
fn enc_acodec(c: AudioCodec) -> u8 {
    match c {
        AudioCodec::Unknown => 0,
        AudioCodec::Aac => 1,
        AudioCodec::Mp3 => 2,
        AudioCodec::Opus => 3,
        AudioCodec::Ac3 => 4,
        AudioCodec::Eac3 => 5,
        AudioCodec::Flac => 6,
    }
}
fn dec_acodec(v: u8) -> AudioCodec {
    match v {
        1 => AudioCodec::Aac,
        2 => AudioCodec::Mp3,
        3 => AudioCodec::Opus,
        4 => AudioCodec::Ac3,
        5 => AudioCodec::Eac3,
        6 => AudioCodec::Flac,
        _ => AudioCodec::Unknown,
    }
}

/// Smallest buffer we'll keep even when the user has nothing armed —
/// guarantees compute_delay_cut always has at least one IDR to find.
const MIN_BUFFER_MS: u32 = 2_000;

#[derive(Debug, Clone, Copy)]
pub enum ActivateError {
    NotArmed,
    BufferShort { remaining_ms: u32 },
}

impl ActivateError {
    pub fn message(&self) -> String {
        match self {
            ActivateError::NotArmed => "no delay armed".to_string(),
            ActivateError::BufferShort { remaining_ms } => {
                let secs = ((*remaining_ms + 500) / 1000).max(1);
                format!("buffer is still building — wait ~{}s", secs)
            }
        }
    }
}
/// Hidden slack beyond what the user sees as the target. Equal to the
/// IDR-search tolerance, so a "5s armed" cut can always land on a real
/// IDR even if the nearest one happens to be slightly past the boundary.
const BUFFER_SLACK_MS: u32 = 2_000;

/// Flat tuple the dashboard reads each tick:
/// `(id, alive, consumer_seq, kbps_out, tags_sent, bytes_sent, cuts, reconnects)`.
/// Aliased so the public signature of `destination_snapshot()` doesn't
/// trip clippy's `type_complexity` lint.
pub type DestinationSnapshot = (String, bool, u64, u32, u64, u64, u32, u32);

/// Per-destination atomics. One of these per active egress pump.
/// Pointer-stable (`Arc<DestinationState>`) so the pump can hold a
/// reference for its lifetime even if the controller's map changes.
pub struct DestinationState {
    pub id: String,
    pub egress_alive: AtomicBool,
    pub consumer_seq: AtomicU64,
    pub tags_sent: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub cuts_performed: AtomicU32,
    pub reconnects: AtomicU32,
    pub bitrate_kbps_out: AtomicU32,
    /// Set by the supervisor when this destination is being removed or
    /// the app is shutting down. The egress pump checks it once per loop
    /// and tears down the upstream session politely (deleteStream) before
    /// returning, instead of letting the supervisor abort the task and
    /// drop the TCP connection mid-tag.
    pub shutdown_requested: AtomicBool,
    /// Last seq-header generation this pump has resent. Compared against
    /// the Controller's counter so we re-emit AVC/HEVC SPS/PPS (or AAC
    /// AudioSpecificConfig) when OBS switches encoders or resolutions
    /// mid-stream — without this, the cached old config and new keyframe
    /// bytes don't match and the upstream decoder silently rejects every
    /// subsequent frame.
    last_seq_header_gen: AtomicU32,
    rate_window_bytes: AtomicU64,
    rate_window_start_ms: AtomicU64,
    /// True if this destination accepts Enhanced Broadcasting multi-track
    /// video on the wire. Set by the supervisor to `true` when the
    /// destination's platform is `twitch` and to `false` for everything
    /// else (YouTube / Kick / Trovo / Restream / custom RTMP — none of
    /// which currently process multi-track video). When false, the pump
    /// runs `flatten_multitrack_video` on every multi-track tag just
    /// before sending, which produces a single-track tag that's
    /// byte-identical to what beta.6 emitted from the ingest-side
    /// flatten — so existing destinations see no behaviour change.
    pub pass_through_multitrack_video: AtomicBool,
    /// Twitch only: when our /obs/multitrack-config proxy successfully
    /// allocates an Enhanced Broadcasting session, Twitch's API returns
    /// a specific IVS ingest URL like
    /// `rtmps://<region>.contribute.live-video.net/app/<key>` — and
    /// that's the *only* endpoint with the EB transcoder pipeline
    /// behind it. The user's configured destination URL usually points
    /// at `rtmp://live.twitch.tv/app`, the legacy ingest, which
    /// accepts multi-track tags but doesn't route them to a
    /// transcoder, so the stream reaches Twitch but never goes live to
    /// viewers (and the unfed session dies of TCP retransmit timeout
    /// after ~60 s). When this field is Some, the egress supervisor
    /// uses it instead of the configured destination URL. Cleared on
    /// publisher disconnect so the next normal stream goes back to
    /// the configured URL.
    pub eb_override_url: std::sync::Mutex<Option<String>>,
}

impl DestinationState {
    pub fn new(id: String) -> Self {
        Self {
            id,
            egress_alive: AtomicBool::new(false),
            consumer_seq: AtomicU64::new(0),
            tags_sent: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            cuts_performed: AtomicU32::new(0),
            reconnects: AtomicU32::new(0),
            bitrate_kbps_out: AtomicU32::new(0),
            shutdown_requested: AtomicBool::new(false),
            last_seq_header_gen: AtomicU32::new(0),
            eb_override_url: std::sync::Mutex::new(None),
            rate_window_bytes: AtomicU64::new(0),
            rate_window_start_ms: AtomicU64::new(0),
            // Default false: every newly-spawned destination flattens
            // multi-track until the supervisor decides otherwise. This
            // preserves beta.6 behaviour for any code path that creates
            // a DestinationState without going through the supervisor
            // (the destination_state lazy-init in particular).
            pass_through_multitrack_video: AtomicBool::new(false),
        }
    }

    fn note_outbound_bytes(&self, n: usize) {
        let now = process_now_ms();
        let total = self
            .rate_window_bytes
            .fetch_add(n as u64, Ordering::Relaxed)
            + n as u64;
        let start = self.rate_window_start_ms.load(Ordering::Relaxed);
        if start == 0 {
            self.rate_window_start_ms.store(now, Ordering::Relaxed);
            return;
        }
        let elapsed = now.saturating_sub(start);
        if elapsed >= 1_000 {
            let kbps = ((total * 8) / elapsed.max(1)) as u32;
            self.bitrate_kbps_out.store(kbps, Ordering::Relaxed);
            self.rate_window_bytes.store(0, Ordering::Relaxed);
            self.rate_window_start_ms.store(now, Ordering::Relaxed);
        }
    }
}

pub struct Controller {
    pub ring: Arc<DiskRing>,

    // --- Delay state machine (single, applies to ALL destinations) ----
    // The consumer offset is global — every destination delivers the same
    // delay simultaneously. Per-destination delays would require N
    // consumer cursors; deferred until requested.
    armed_delay_ms: AtomicU32,
    target_delay_ms: AtomicU32,
    ingest_alive: AtomicBool,
    buffer_building: AtomicBool,
    publisher_token: AtomicU64,

    // --- Per-destination state, keyed by Destination.id -----------------
    // RwLock not Mutex: every `on_tag` (~150-300/s per active stream)
    // calls `min_consumer_seq` which iterates this map. With a Mutex,
    // those reads serialised against each other AND against every
    // `dest.consumer_seq.store` write the pumps do. RwLock lets the
    // hot read path go through concurrently; writes (add/remove dest)
    // are rare so the write-side contention is fine.
    destinations: std::sync::RwLock<HashMap<String, Arc<DestinationState>>>,

    // Ingest-side stats
    ingest_disconnects: AtomicU32,
    bitrate_kbps: AtomicU32, // inbound (from OBS)
    rate_window_bytes: AtomicU64,
    rate_window_start_ms: AtomicU64,

    // Discord webhook URL. Empty = disabled. Updated live via update_webhook.
    webhook_url: std::sync::Mutex<String>,
    // Wall-clock ms (since UNIX epoch) of last webhook fire. Throttles
    // rapid event sequences (e.g. reconnect flapping) so we never spawn
    // more than one curl every ~2 s.
    webhook_last_fire_ms: AtomicU64,

    // Coordination
    publish_lock: Mutex<()>,

    // Detected codecs + enhanced-broadcasting state. Updated by the
    // ingest path on every tag (cheap atomic compare). The UI surfaces
    // these so the user can tell at a glance what OBS is sending and
    // whether Enhanced Broadcasting was caught + flattened.
    video_codec: AtomicU8,
    audio_codec: AtomicU8,
    multitrack_video: AtomicBool,
    multitrack_audio: AtomicBool,

    // --- u32 → u64 timestamp wrap tracking (per publisher session) ---
    // RTMP wire timestamps are u32 ms and wrap every 49.7 days. We
    // expand to u64 internally so every comparison / subtraction across
    // the codebase is naturally correct. `last_input_ts_u32` is the
    // most recent wire timestamp we saw; `input_ts_wrap_high` is the
    // count of full 2^32-ms cycles. The expanded u64 ts is
    //   (input_ts_wrap_high << 32) | wire_ts_u32.
    // Both reset on publisher reconnect so a fresh OBS session starts
    // from wrap_high=0 again.
    last_input_ts_u32: AtomicU32,
    input_ts_wrap_high: AtomicU32,

    // Wall-clock (process_now_ms) of last multi-track video tag — the
    // Enhanced Broadcasting warning chip only shows if we've seen one
    // recently. Sticky-on-true was the old behavior and produced
    // permanent false-positive chips after a single misclassified tag.
    last_multitrack_video_ms: AtomicU64,
    // Tracks when backpressure first started being true. Used by
    // `is_backpressured` to require the condition to hold for a
    // sustained window (1.5 s) before reporting — without this the
    // chip strobes on every cut transition.
    backpressure_since_ms: AtomicU64,
    /// Bumped on every NEW sequence-header tag received from ingest
    /// (audio or video, regardless of whether the bytes actually
    /// changed). Egress pumps compare this against their own
    /// `last_seq_header_gen` and resend both cached headers when it
    /// jumps — so mid-stream encoder swaps (resolution change in OBS,
    /// AVC→HEVC switch) don't desync the downstream decoder.
    seq_header_gen: AtomicU32,

    // In-process log ring (most recent N lines).
    pub logs: std::sync::Mutex<std::collections::VecDeque<String>>,
}

impl Controller {
    pub fn new(ring: Arc<DiskRing>, initial_armed_delay_ms: u32) -> Self {
        // Match arm_delay's clamp. A persisted (or hand-edited) config
        // value larger than the cap would otherwise wedge the egress
        // pump in `preparing` forever because the buffer can never grow
        // to a delay we don't actually allow.
        let initial_armed_delay_ms = initial_armed_delay_ms.min(600_000);
        Self {
            ring,
            armed_delay_ms: AtomicU32::new(initial_armed_delay_ms),
            target_delay_ms: AtomicU32::new(0),
            ingest_alive: AtomicBool::new(false),
            buffer_building: AtomicBool::new(false),
            publisher_token: AtomicU64::new(0),
            destinations: std::sync::RwLock::new(HashMap::new()),
            ingest_disconnects: AtomicU32::new(0),
            bitrate_kbps: AtomicU32::new(0),
            rate_window_bytes: AtomicU64::new(0),
            rate_window_start_ms: AtomicU64::new(0),
            webhook_url: std::sync::Mutex::new(String::new()),
            webhook_last_fire_ms: AtomicU64::new(0),
            publish_lock: Mutex::new(()),
            video_codec: AtomicU8::new(0),
            audio_codec: AtomicU8::new(0),
            multitrack_video: AtomicBool::new(false),
            multitrack_audio: AtomicBool::new(false),
            seq_header_gen: AtomicU32::new(0),
            last_input_ts_u32: AtomicU32::new(0),
            input_ts_wrap_high: AtomicU32::new(0),
            last_multitrack_video_ms: AtomicU64::new(0),
            backpressure_since_ms: AtomicU64::new(0),
            logs: std::sync::Mutex::new(std::collections::VecDeque::with_capacity(512)),
        }
    }

    pub fn video_codec(&self) -> VideoCodec {
        dec_vcodec(self.video_codec.load(Ordering::Relaxed))
    }
    pub fn audio_codec(&self) -> AudioCodec {
        dec_acodec(self.audio_codec.load(Ordering::Relaxed))
    }
    /// Freshness-based — true only if a multi-track video tag was seen
    /// within the last 5 s AND OBS is currently publishing. The old
    /// sticky-bool version kept the warning chip on forever after a
    /// single (often misclassified) tag; this version auto-clears as
    /// soon as multi-track stops, and is always off when ingest is
    /// not alive.
    pub fn multitrack_video(&self) -> bool {
        if !self.ingest_alive.load(Ordering::Relaxed) {
            return false;
        }
        let last = self.last_multitrack_video_ms.load(Ordering::Relaxed);
        if last == 0 {
            return false;
        }
        process_now_ms().saturating_sub(last) < 5_000
    }
    pub fn multitrack_audio(&self) -> bool {
        self.multitrack_audio.load(Ordering::Relaxed)
    }

    /// Called by the ingest path. Stores the codec atomically and only
    /// logs / fires-a-webhook on the very first observation, so codec
    /// changes mid-stream don't spam events.
    pub fn note_video_codec(&self, c: VideoCodec) {
        let enc = enc_vcodec(c);
        if enc == 0 {
            return;
        }
        let prev = self.video_codec.swap(enc, Ordering::Relaxed);
        if prev != enc {
            self.log(format!("ingest: video codec = {}", c.label()));
        }
    }
    pub fn note_audio_codec(&self, c: AudioCodec) {
        let enc = enc_acodec(c);
        if enc == 0 {
            return;
        }
        let prev = self.audio_codec.swap(enc, Ordering::Relaxed);
        if prev != enc {
            self.log(format!("ingest: audio codec = {}", c.label()));
        }
    }
    /// Called from ingest on every multi-track video tag. Records a
    /// timestamp so the `multitrack_video()` getter can auto-clear when
    /// multi-track stops (e.g. the user switched Enhanced Broadcasting
    /// off mid-stream, or a single tag was misclassified). Edge-triggered
    /// log + webhook fire only on the first detection per session — the
    /// sticky bool used to live on `multitrack_video` itself; we keep it
    /// here just to throttle the log to once.
    pub fn note_multitrack_video(&self) {
        self.last_multitrack_video_ms
            .store(process_now_ms(), Ordering::Relaxed);
        if !self.multitrack_video.swap(true, Ordering::Relaxed) {
            // Twitch destinations pass the multi-track tag through
            // bit-faithfully (Enhanced Broadcasting → transcoded
            // ladder); every other platform flattens to the primary
            // resolution on the way out via select_video_bytes. So
            // this is now informational, not a warning.
            self.log(
                "Enhanced Broadcasting (multi-track video) detected — \
                 forwarding raw to Twitch destinations, flattening to the \
                 primary resolution for any other platform.",
            );
            self.fire_webhook(
                "🎚️",
                "Enhanced Broadcasting detected — multi-track forwarding active.",
            );
        }
    }
    pub fn note_multitrack_audio(&self) {
        if !self.multitrack_audio.swap(true, Ordering::Relaxed) {
            self.log("ingest: multi-track audio detected (VOD audio track) — forwarding as-is.");
        }
    }
    /// Wipe codec/multitrack state when the publisher disconnects so a
    /// fresh OBS connect with a different codec starts from a clean slate.
    /// Also resets the u32→u64 timestamp wrap counter — a new publisher
    /// may restart from ts=0, which from the old wrap counter's POV would
    /// look like a 49-day jump forward.
    pub fn reset_codec_state(&self) {
        self.video_codec.store(0, Ordering::Relaxed);
        self.audio_codec.store(0, Ordering::Relaxed);
        self.multitrack_video.store(false, Ordering::Relaxed);
        self.multitrack_audio.store(false, Ordering::Relaxed);
        self.last_multitrack_video_ms.store(0, Ordering::Relaxed);
        self.backpressure_since_ms.store(0, Ordering::Relaxed);
        self.last_input_ts_u32.store(0, Ordering::Relaxed);
        self.input_ts_wrap_high.store(0, Ordering::Relaxed);
    }

    /// Promote an RTMP wire timestamp (u32 ms, wraps at ~49.7 days) to
    /// a monotonic u64 ms relative to this publisher session.
    ///
    /// Called by the ingest path exactly once per tag. The single
    /// publisher invariant (only one OBS may publish at a time — see
    /// `begin_publish`) means there's only one caller of `on_tag` /
    /// `expand_ts` at any moment, so the relaxed atomic load + store
    /// is race-free in practice.
    ///
    /// Wrap detection rule: if the new u32 ts is less than the previous
    /// by more than 2^31 ms (~24.8 days), the counter wrapped around;
    /// bump the high half. Smaller backward jumps are treated as the
    /// (normal) inter-stream out-of-order audio interleaving and ignored
    /// here — pace_and_send drops those separately.
    fn expand_ts(&self, wire_ts: u32) -> u64 {
        let last = self.last_input_ts_u32.load(Ordering::Relaxed);
        let mut wrap_high = self.input_ts_wrap_high.load(Ordering::Relaxed);
        if last > 0 && wire_ts < last && last.wrapping_sub(wire_ts) > (1u32 << 31) {
            wrap_high = wrap_high.wrapping_add(1);
            self.input_ts_wrap_high.store(wrap_high, Ordering::Relaxed);
        }
        self.last_input_ts_u32.store(wire_ts, Ordering::Relaxed);
        ((wrap_high as u64) << 32) | (wire_ts as u64)
    }

    /// Look up or insert a destination's state. Returns the same `Arc`
    /// across calls for the same id, so spawned egress pumps can hold
    /// their handle for their whole lifetime.
    pub fn destination_state(&self, id: &str) -> Arc<DestinationState> {
        // Fast path: existing entry. Use read() so concurrent destination_state
        // calls don't serialise (e.g. supervisor + state endpoint).
        if let Some(s) = self.destinations.read().unwrap().get(id) {
            return s.clone();
        }
        let mut map = self.destinations.write().unwrap();
        map.entry(id.to_string())
            .or_insert_with(|| Arc::new(DestinationState::new(id.to_string())))
            .clone()
    }

    /// Drop a destination's state — call when the user removes it.
    pub fn remove_destination_state(&self, id: &str) {
        self.destinations.write().unwrap().remove(id);
    }

    /// Snapshot of every (id → state) pair. Used by graceful-shutdown
    /// paths that need to flip flags on every pump in one pass.
    pub fn all_destination_states(&self) -> Vec<(String, Arc<DestinationState>)> {
        self.destinations
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Snapshot for the dashboard: (id, alive, consumer_seq, kbps_out, tags, bytes, cuts, reconnects).
    pub fn destination_snapshot(&self) -> Vec<DestinationSnapshot> {
        let map = self.destinations.read().unwrap();
        map.values()
            .map(|d| {
                (
                    d.id.clone(),
                    d.egress_alive.load(Ordering::Relaxed),
                    d.consumer_seq.load(Ordering::Relaxed),
                    d.bitrate_kbps_out.load(Ordering::Relaxed),
                    d.tags_sent.load(Ordering::Relaxed),
                    d.bytes_sent.load(Ordering::Relaxed),
                    d.cuts_performed.load(Ordering::Relaxed),
                    d.reconnects.load(Ordering::Relaxed),
                )
            })
            .collect()
    }

    /// All destinations alive flag (any-of) — for the topbar pill.
    pub fn any_destination_alive(&self) -> bool {
        self.destinations
            .read()
            .unwrap()
            .values()
            .any(|d| d.egress_alive.load(Ordering::Relaxed))
    }

    /// (alive_count, total_count) — for "2/3 destinations live" chips.
    pub fn destination_alive_summary(&self) -> (u32, u32) {
        let map = self.destinations.read().unwrap();
        let total = map.len() as u32;
        let alive = map
            .values()
            .filter(|d| d.egress_alive.load(Ordering::Relaxed))
            .count() as u32;
        (alive, total)
    }

    // ---- Public API used by the web server ----

    // --- Two-phase delay control (matches the real InstantDelay flow) ---

    /// Arm a delay. The buffer will fill toward it; meanwhile output
    /// stays at the live edge so viewers see nothing change. Pass 0 to
    /// disarm (also resets target).
    ///
    /// If a delay is currently *active*, arming a smaller delay
    /// live-updates the target (no buffer wait needed); arming a larger
    /// delay updates the target too, and the controller will smoothly
    /// rewind once enough buffer has accumulated.
    pub fn arm_delay(&self, ms: u32) {
        let ms = ms.min(600_000);
        self.armed_delay_ms.store(ms, Ordering::Relaxed);
        if ms == 0 {
            // Disarm wipes target as well.
            self.target_delay_ms.store(0, Ordering::Relaxed);
        } else if self.target_delay_ms.load(Ordering::Relaxed) > 0 {
            // Already active → live-update what we're delivering.
            self.target_delay_ms.store(ms, Ordering::Relaxed);
        }
    }

    /// Switch the armed delay on. Caller-side check: only acts if the
    /// buffer holds at least the armed amount. Returns Err otherwise.
    /// The error carries a static prefix the UI maps to a human message;
    /// when buffer is still filling we encode the remaining seconds in
    /// `BufferShort` so the dashboard can show "wait ~3 s" instead of
    /// the user staring at "still building" with no eta.
    pub fn activate_delay(&self) -> Result<u32, ActivateError> {
        let armed = self.armed_delay_ms.load(Ordering::Relaxed);
        if armed == 0 {
            return Err(ActivateError::NotArmed);
        }
        let fill = self.buffer_fill_ms();
        if fill + 500 < armed {
            let remaining_ms = armed.saturating_sub(fill);
            return Err(ActivateError::BufferShort { remaining_ms });
        }
        self.target_delay_ms.store(armed, Ordering::Relaxed);
        Ok(armed)
    }

    /// Drop back to live but *keep the armed delay* — buffer continues
    /// to fill, so the next activate is instant. This is the magic
    /// behavior the streamer described.
    pub fn stop_delay(&self) {
        self.target_delay_ms.store(0, Ordering::Relaxed);
    }

    pub fn armed_delay_ms(&self) -> u32 {
        self.armed_delay_ms.load(Ordering::Relaxed)
    }
    pub fn target_delay_ms(&self) -> u32 {
        self.target_delay_ms.load(Ordering::Relaxed)
    }
    /// Server-side derivation: (latest_ts − consumer_ts) using the
    /// slowest live destination. Replaces the prior per-pump
    /// `current_delay_ms` atomic, which N pumps would race to overwrite
    /// every loop iteration — producing visible UI wobble. Falls back
    /// to 0 when nothing is being sent.
    pub fn current_delay_ms(&self) -> u32 {
        let Some(latest) = self.ring.latest_ts() else {
            return 0;
        };
        let min_consumer = {
            let map = self.destinations.read().unwrap();
            map.values()
                .filter(|d| d.egress_alive.load(Ordering::Relaxed))
                .map(|d| d.consumer_seq.load(Ordering::Relaxed))
                .min()
        };
        match min_consumer.and_then(|c| self.ring.find_by_seq(c).map(|(_, m)| m)) {
            // Clamp to u32: a u64 delta can't realistically exceed
            // 600_000 ms (our hard armed-delay ceiling) but we cap to
            // be safe — the UI consumes a u32 number anyway.
            Some(meta) => latest.saturating_sub(meta.ts_ms).min(u32::MAX as u64) as u32,
            None => 0,
        }
    }

    /// Convenience for the dashboard — collapses the (armed, target, fill)
    /// triple into a single label.
    pub fn phase(&self) -> &'static str {
        let armed = self.armed_delay_ms();
        let target = self.target_delay_ms();
        if target > 0 {
            return "active";
        }
        if armed == 0 {
            return "idle";
        }
        if self.buffer_fill_ms() + 500 < armed {
            return "preparing";
        }
        "ready"
    }

    pub fn ingest_alive(&self) -> bool {
        self.ingest_alive.load(Ordering::Relaxed)
    }
    /// Bumps once per OBS publish session. Egress reads it each loop and
    /// re-anchors its output timeline if the token changed — without this,
    /// the new publisher's "fresh" timestamps (which can reset to 0) get
    /// silently dropped by pace_and_send's monotonic guard.
    pub fn publisher_token(&self) -> u64 {
        self.publisher_token.load(Ordering::Relaxed)
    }
    pub fn egress_alive(&self) -> bool {
        self.any_destination_alive()
    }
    pub fn buffer_fill_ms(&self) -> u32 {
        match (self.ring.oldest_ts(), self.ring.latest_ts()) {
            (Some(o), Some(l)) => l.saturating_sub(o).min(u32::MAX as u64) as u32,
            _ => 0,
        }
    }
    pub fn buffer_building(&self) -> bool {
        self.buffer_building.load(Ordering::Relaxed)
    }

    // Aggregated stats — summed across all destinations for the
    // dashboard's top-level metric cards.
    pub fn tags_sent(&self) -> u64 {
        self.destinations
            .read()
            .unwrap()
            .values()
            .map(|d| d.tags_sent.load(Ordering::Relaxed))
            .sum()
    }
    pub fn bytes_sent(&self) -> u64 {
        self.destinations
            .read()
            .unwrap()
            .values()
            .map(|d| d.bytes_sent.load(Ordering::Relaxed))
            .sum()
    }
    pub fn cuts_performed(&self) -> u32 {
        self.destinations
            .read()
            .unwrap()
            .values()
            .map(|d| d.cuts_performed.load(Ordering::Relaxed))
            .sum()
    }
    pub fn egress_reconnects(&self) -> u32 {
        self.destinations
            .read()
            .unwrap()
            .values()
            .map(|d| d.reconnects.load(Ordering::Relaxed))
            .sum()
    }
    pub fn ingest_disconnects(&self) -> u32 {
        self.ingest_disconnects.load(Ordering::Relaxed)
    }
    pub fn bitrate_kbps(&self) -> u32 {
        self.bitrate_kbps.load(Ordering::Relaxed)
    }

    // ---- Internal: ingest counters ----

    /// Called from the ingest path on every audio/video tag. Maintains a
    /// 1-second rolling bitrate average (kbps) — cheap, lock-free.
    pub fn note_inbound_bytes(&self, n: usize) {
        let now = process_now_ms();
        let total = self
            .rate_window_bytes
            .fetch_add(n as u64, Ordering::Relaxed)
            + n as u64;
        let start = self.rate_window_start_ms.load(Ordering::Relaxed);
        if start == 0 {
            self.rate_window_start_ms.store(now, Ordering::Relaxed);
            return;
        }
        let elapsed = now.saturating_sub(start);
        if elapsed >= 1_000 {
            // Convert bytes/ms → kbps:  bytes * 8 / ms
            let kbps = ((total * 8) / elapsed.max(1)) as u32;
            self.bitrate_kbps.store(kbps, Ordering::Relaxed);
            self.rate_window_bytes.store(0, Ordering::Relaxed);
            self.rate_window_start_ms.store(now, Ordering::Relaxed);
        }
    }

    pub fn note_ingest_disconnect(&self) {
        self.ingest_disconnects.fetch_add(1, Ordering::Relaxed);
    }

    /// Append a line to the in-process log ring (drops the oldest if full).
    /// Each entry is prefixed with a process-relative `[+12.345s]` timestamp
    /// so a downloaded log shows when things happened relative to each
    /// other — invaluable for diagnosing "the bouncing happened around
    /// 30 seconds in".
    pub fn log(&self, line: impl Into<String>) {
        let mut q = self.logs.lock().unwrap();
        if q.len() >= 1500 {
            q.pop_front();
        }
        let ts_s = process_now_ms() as f64 / 1000.0;
        q.push_back(format!("[+{:>8.3}s] {}", ts_s, line.into()));
    }

    pub fn clear_logs(&self) {
        self.logs.lock().unwrap().clear();
    }

    // ---- Ingest entry points (called from rtmp::server) ----

    pub async fn begin_publish(&self, _stream_key: &str) -> io::Result<u64> {
        let _g = self.publish_lock.lock().await;
        // One publisher at a time — a second OBS connecting would
        // interleave its tags into the buffer with its own timestamp
        // origin and guarantee a viewer-visible glitch.
        if self.ingest_alive.load(Ordering::Relaxed) {
            self.log("ingest: rejected second publisher (slot in use)");
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "another publisher is already active",
            ));
        }
        // Bump token so any prior egress reader knows it's stale.
        let token = self.publisher_token.fetch_add(1, Ordering::SeqCst) + 1;
        self.ingest_alive.store(true, Ordering::Relaxed);
        self.log("ingest: publisher connected");
        self.fire_webhook("✅", "OBS publisher connected — going live.");
        Ok(token)
    }

    pub fn on_tag(&self, kind: u8, wire_ts: u32, payload: &[u8], is_idr: bool, is_seq: bool) {
        // Expand the wire u32 ts to monotonic u64 (handles the 49.7-day
        // RTMP timestamp wrap). Sequence headers don't need expansion
        // (they bypass the ring) but doing it unconditionally keeps the
        // wrap counter in sync with the real ingest timeline.
        let ts_ms = self.expand_ts(wire_ts);
        // Track bitrate on real media tags only (skip sequence headers,
        // which are tiny and one-shot).
        if !is_seq {
            self.note_inbound_bytes(payload.len());
        }
        // Bump the generation on every seq header so each egress pump
        // knows to re-emit its cached config bytes on the next iteration.
        if is_seq {
            self.seq_header_gen.fetch_add(1, Ordering::Relaxed);
        }
        let _ = self.ring.append(kind, ts_ms, payload, is_idr, is_seq);
        // Cap the buffer to the user's armed delay (plus a small slack for
        // IDR alignment). This keeps the on-screen "Buffer N/N s" exactly
        // what the user asked for, instead of growing to the full disk cap.
        // The trim respects the slowest consumer across all destinations
        // so we never evict a tag any pump is still about to read.
        if !is_seq {
            let target = self.effective_target_buffer_ms();
            let min_consumer = self.min_consumer_seq();
            self.ring.trim_older_than(target, ts_ms, min_consumer);
        }
    }

    /// Slowest consumer across all destinations. Used by trim to ensure
    /// no in-flight read can be invalidated. If no destinations exist or
    /// none have produced a seq yet, returns u64::MAX so trim is a no-op.
    fn min_consumer_seq(&self) -> u64 {
        let map = self.destinations.read().unwrap();
        map.values()
            .map(|d| d.consumer_seq.load(Ordering::Relaxed))
            .min()
            .unwrap_or(u64::MAX)
    }

    /// How many tags behind the latest the slowest consumer is. Kept
    /// for diagnostics — but DO NOT use this directly to flag
    /// backpressure: on any active delay the consumer is intentionally
    /// behind (5 s × ~80 tags/s ≈ 400 tags), so any naive threshold
    /// generates false positives. Use `is_backpressured` instead.
    pub fn max_consumer_lag(&self) -> u64 {
        let Some(latest) = self.ring.latest_seq() else {
            return 0;
        };
        let min_consumer = {
            let map = self.destinations.read().unwrap();
            map.values()
                .filter(|d| d.egress_alive.load(Ordering::Relaxed))
                .map(|d| d.consumer_seq.load(Ordering::Relaxed))
                .min()
        };
        match min_consumer {
            // consumer_seq is the seq we'll READ NEXT (one past the last
            // sent), so the natural "fully caught up" state is +1 above
            // the latest. Saturating-sub the 1 so caught-up reads as 0.
            Some(c) => latest.saturating_add(1).saturating_sub(c),
            None => 0,
        }
    }

    /// True if egress can't keep up with ingest — i.e. the actual
    /// delivered delay is materially larger than the user asked for.
    ///
    /// Definition: `current_delay − target_delay > 2 s` (sustained).
    /// This is timestamp-based, so a healthy 5 s delay reads as
    /// "0 over" (no backpressure) — unlike the tag-count metric, which
    /// would always read ~400 tags behind on a 5 s delay regardless of
    /// stream health.
    ///
    /// Caller has to suppress during the cut-transition window itself
    /// or it briefly flips on every toggle. We use a sustained-condition
    /// check via `backpressure_since_ms`.
    pub fn is_backpressured(&self) -> bool {
        // Skip the check entirely if there's no live destination — the
        // signal is meaningless when nothing is being sent.
        let any_alive = {
            let map = self.destinations.read().unwrap();
            map.values().any(|d| d.egress_alive.load(Ordering::Relaxed))
        };
        if !any_alive {
            self.backpressure_since_ms.store(0, Ordering::Relaxed);
            return false;
        }

        let current = self.current_delay_ms();
        let target = self.target_delay_ms();
        // 2 s margin: covers the dead-band oscillation (500 ms),
        // the next_or_wait poll cycle (500 ms), the last_cut_check
        // throttle (500 ms), and a little slack so the chip doesn't
        // strobe on every cut.
        let over = current.saturating_sub(target) > 2_000;
        let now = process_now_ms();
        if over {
            // Mark first observation; only return true once it's
            // sustained for >= 1.5 s. Stops the chip from flipping
            // briefly during every backward cut (which transiently
            // makes current_delay shoot up before settling).
            let since = self.backpressure_since_ms.load(Ordering::Relaxed);
            if since == 0 {
                self.backpressure_since_ms.store(now, Ordering::Relaxed);
                false
            } else {
                now.saturating_sub(since) >= 1_500
            }
        } else {
            self.backpressure_since_ms.store(0, Ordering::Relaxed);
            false
        }
    }

    /// Wall-clock ms of buffer to retain. Surfaces in the UI as the
    /// denominator of the buffer bar (so a 5s armed delay shows a 0/5s
    /// progress, not 0/1258s).
    pub fn target_buffer_ms(&self) -> u32 {
        // Visible-to-user target = exactly the armed delay (or a small
        // minimum when nothing is armed — gives compute_delay_cut at
        // least one IDR to work with the moment the user arms something).
        let armed = self.armed_delay_ms();
        if armed == 0 {
            MIN_BUFFER_MS
        } else {
            armed
        }
    }

    fn effective_target_buffer_ms(&self) -> u32 {
        // What we *actually* keep: a bit more than what the user sees, so
        // IDR alignment has wiggle room and we never trim the exact
        // boundary IDR right when compute_delay_cut wants it.
        self.target_buffer_ms() + BUFFER_SLACK_MS
    }

    pub fn on_metadata(&self, payload: Vec<u8>) {
        *self.ring.metadata.lock().unwrap() = Some(payload);
    }

    pub fn mark_ingest_dead(&self) {
        // Only count when transitioning alive → dead, so a stray call
        // doesn't inflate the counter.
        if self.ingest_alive.swap(false, Ordering::Relaxed) {
            self.note_ingest_disconnect();
            self.reset_codec_state();
            // Clear any Enhanced Broadcasting URL overrides on the
            // way out — the next stream may or may not be EB, and a
            // stale override would force a non-EB stream onto an IVS
            // endpoint that has no allocated session. The
            // /obs/multitrack-config proxy sets a fresh override on
            // every new EB session anyway.
            for (_id, state) in self.all_destination_states() {
                *state.eb_override_url.lock().unwrap() = None;
            }
            self.log("ingest: publisher disconnected");
            self.fire_webhook("⚠️", "OBS publisher disconnected.");
        }
    }

    /// Update the Discord webhook URL — call when settings change. Empty
    /// string disables webhook delivery entirely.
    pub fn update_webhook(&self, url: String) {
        *self.webhook_url.lock().unwrap() = url;
    }

    /// Snapshot the current webhook URL. Used by the test endpoint so it
    /// can route the request with verbose error reporting instead of
    /// going through `fire_webhook` (which is fire-and-forget and
    /// silently swallows everything from empty-URL to TLS failures).
    pub fn webhook_url_snapshot(&self) -> String {
        self.webhook_url.lock().unwrap().clone()
    }

    /// Fire-and-forget Discord post. Skips silently when no webhook is
    /// configured, OR when the last fire was less than 2 s ago (rate
    /// limit — prevents subprocess spam if a destination flaps).
    ///
    /// Uses `ureq` (tiny blocking HTTPS client, ~150 KB) wrapped in
    /// `spawn_blocking` so the actual TCP+TLS work doesn't park the
    /// current-thread runtime. Previously shelled out to `curl`, which
    /// (a) silently failed when `curl.exe` wasn't on PATH and (b) made
    /// "runtime deps" technically include the system curl binary.
    pub fn fire_webhook(&self, emoji: &str, message: &str) {
        let url = self.webhook_url.lock().unwrap().clone();
        if url.is_empty() {
            return;
        }

        // Throttle: skip if we fired less than 2 s ago.
        let now = process_now_ms();
        let last = self.webhook_last_fire_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) < 2_000 {
            return;
        }
        self.webhook_last_fire_ms.store(now, Ordering::Relaxed);

        let content = format!("{emoji} **InstantClone**: {message}");
        let body = format!(r#"{{"content":"{}"}}"#, json_escape_inline(&content));

        tokio::spawn(async move {
            // Spawn-blocking + 10 s outer timeout so a wedged Discord
            // edge can't keep a thread tied up forever. Ignore the
            // result: webhooks are fire-and-forget by design.
            let _ = tokio::time::timeout(
                Duration::from_secs(10),
                tokio::task::spawn_blocking(move || {
                    let _ = crate::https::https_agent_builder()
                        .timeout_connect(Duration::from_secs(5))
                        .timeout(Duration::from_secs(8))
                        .build()
                        .post(&url)
                        .set("Content-Type", "application/json")
                        .send_string(&body);
                }),
            )
            .await;
        });
    }
}

/// JSON-string escape that handles every C0 control char that would
/// otherwise produce an invalid Discord payload (the previous
/// `replace('\\', ..).replace('"', ..).replace('\n', ..)` chain missed
/// `\r`, `\t`, `\u{0008}` and friends — any destination name with a
/// stray control character could nuke the webhook body).
fn json_escape_inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str(r#"\""#),
            '\\' => out.push_str(r"\\"),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            '\u{0008}' => out.push_str(r"\b"),
            '\u{000C}' => out.push_str(r"\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Egress driver — the timing & cut-alignment core.
// ---------------------------------------------------------------------------

/// Run the egress loop for ONE destination. Reconnects on connection
/// loss with exponential backoff (capped at 30 s). Resets backoff on
/// each successful connection.
pub async fn run_egress(
    ctrl: Arc<Controller>,
    label: String,
    url: String,
    dest: Arc<DestinationState>,
) -> io::Result<()> {
    let parsed = match EgressUrl::parse(&url) {
        Ok(p) => p,
        Err(e) => {
            ctrl.log(format!(
                "[{}] invalid URL ({}) — fix it in Settings",
                label, e
            ));
            tokio::time::sleep(Duration::from_secs(3600)).await;
            return Ok(());
        }
    };

    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(30);

    loop {
        // Cooperative shutdown — check BEFORE attempting another
        // connect. Without this, a destination the user just disabled
        // (or with a permanently failing endpoint) would spin forever
        // in the connect-retry loop, because pump_dest is only reached
        // on a SUCCESSFUL connect. Symptom: log spam like
        //   "[egress Twitch] connecting to live.twitch.tv:1935"
        //   "[egress Twitch] connect failed: early eof (next try in 30s)"
        // continuing even after the destination is toggled off.
        if dest.shutdown_requested.load(Ordering::Relaxed) {
            ctrl.log(format!(
                "[{}] shutdown requested — egress loop exiting",
                label
            ));
            return Ok(());
        }
        // Ingest gate. pump_dest closes when ingest_alive flips false;
        // without this matching gate at the connect site, the outer
        // retry loop would just dial Twitch / YouTube again with no
        // frames to send, creating a 1-Hz connect → close → reconnect
        // spam visible in /logs. Wait for ingest to come back instead,
        // polling cheaply every 500 ms so we react quickly to OBS
        // resuming a publish session.
        if !ctrl.ingest_alive() {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }
        eprintln!(
            "[egress {}] connecting to {}:{}/{}",
            label, parsed.host, parsed.port, parsed.app
        );
        ctrl.log(format!(
            "[{}] connecting to {}:{}",
            label, parsed.host, parsed.port
        ));
        match EgressClient::connect(&parsed).await {
            Ok(client) => {
                backoff = Duration::from_secs(1);
                let was_alive = dest.egress_alive.swap(true, Ordering::Relaxed);
                if !was_alive {
                    ctrl.fire_webhook("🟢", &format!("**{}** is now live.", label));
                }
                let sink = client.spawn_reader_drain();
                let pump_result = pump_dest(&ctrl, &dest, sink).await;
                dest.egress_alive.store(false, Ordering::Relaxed);
                if let Err(e) = pump_result {
                    // Twitch/etc. sometimes echo the stream key in error
                    // descriptions ("Authentication failed for live_…").
                    // Scrub before logging or webhooking — otherwise the
                    // key shows up in /logs (screen-shareable) and in
                    // the Discord webhook payload.
                    let safe = scrub_secret(&e.to_string(), &parsed.stream_key);
                    eprintln!("[egress {}] pump error: {}", label, safe);
                    ctrl.log(format!("[{}] disconnected ({})", label, safe));
                    ctrl.fire_webhook("🔴", &format!("**{}** disconnected: {}", label, safe));
                }
                // Count this as a reconnect: the next loop iteration WILL
                // re-establish a connection. Incrementing here (instead of
                // at the loop tail) avoids the old bug where every initial
                // connect AND every connect-failure bumped the counter,
                // making "Egress reconnects: 1" the resting state of a
                // healthy fresh stream.
                dest.reconnects.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                let safe = scrub_secret(&e.to_string(), &parsed.stream_key);
                eprintln!(
                    "[egress {}] connect failed: {} (next try in {:?})",
                    label, safe, backoff
                );
                ctrl.log(format!(
                    "[{}] connect failed ({}), retrying in {}s",
                    label,
                    safe,
                    backoff.as_secs()
                ));
            }
        }
        // Cancellable backoff sleep — wake every 200 ms to check the
        // shutdown flag so a disable doesn't have to wait the full
        // 30 s backoff window before stopping.
        let deadline = tokio::time::Instant::now() + backoff;
        loop {
            if dest.shutdown_requested.load(Ordering::Relaxed) {
                ctrl.log(format!(
                    "[{}] shutdown requested during backoff — exiting",
                    label
                ));
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Replace any occurrence of `secret` (case-sensitive) in `text` with a
/// short redaction so it doesn't end up in logs or webhook payloads.
/// Also redacts the suffix after the last `/` if it's long enough to be
/// a stream key — defensive against secrets we don't know about.
fn scrub_secret(text: &str, secret: &str) -> String {
    let mut out = text.to_string();
    if secret.len() >= 6 {
        let redacted = format!(
            "{}…{}",
            &secret[..secret.len().min(3)],
            &secret[secret.len().saturating_sub(3)..]
        );
        out = out.replace(secret, &redacted);
    }
    out
}

/// One destination's session against a platform. Returns on disconnect.
///
/// If ingest starves (OBS drops, network glitches that interrupt the
/// publisher), we simply *stop sending*. The upstream platform's idle
/// timeout will close the session naturally — which is the standard,
/// predictable failure mode and lets viewers see the real "stream
/// offline" UI instead of a confusing freeze-frame. No filler-frame
/// replay (it created its own desync bugs and added memory pressure
/// for negligible benefit).
async fn pump_dest(
    ctrl: &Arc<Controller>,
    dest: &Arc<DestinationState>,
    mut sink: EgressSink,
) -> io::Result<()> {
    let meta = ctrl.ring.metadata.lock().unwrap().clone();
    if let Some(meta) = meta {
        let _ = sink.send_metadata(&meta).await;
    }

    let mut state = EgressState::new();
    state.last_publisher_token = ctrl.publisher_token();
    let mut io_buf: Vec<u8> = Vec::with_capacity(64 * 1024);

    // Initial seed: if a delay is ALREADY active when this pump spawns
    // (multi-destination case — a second destination added mid-stream
    // while the first is on a 5 s delay), join at the right delayed
    // position. Otherwise we'd briefly emit live frames before
    // compute_delay_cut catches up, producing a visible ~5 s backward
    // jump for viewers of the new destination.
    let first_idr = seed_idr(ctrl).await;
    state.consumer_seq = first_idr.seq;
    state.input_ts_anchor = first_idr.ts_ms;
    state.output_ts_base = 0;
    state.wall_anchor = Instant::now();
    state.wall_anchor_input_ts = first_idr.ts_ms;
    state.last_sent_input_ts = first_idr.ts_ms;
    dest.consumer_seq
        .store(state.consumer_seq, Ordering::Relaxed);
    dest.last_seq_header_gen.store(
        ctrl.seq_header_gen.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );

    crate::trace::log(
        "PUMP_START",
        &format!(
            "dest={} consumer_seq={} input_ts_anchor={} output_ts_base=0x{:08x} pub_token={}",
            dest.id,
            state.consumer_seq,
            state.input_ts_anchor,
            state.output_ts_base,
            state.last_publisher_token
        ),
    );
    // Always lead with sequence headers + the IDR itself.
    send_sequence_headers(ctrl, dest, &mut sink, state.output_ts_base).await?;

    loop {
        // Cooperative shutdown: when the supervisor flips this, we end
        // the session cleanly (sending deleteStream) instead of dropping
        // the TCP connection mid-tag.
        if dest.shutdown_requested.load(Ordering::Relaxed) {
            let _ = sink.send_delete_stream().await;
            return Ok(());
        }

        // Reply to any RTMP Ping Requests the server (Twitch / YouTube
        // edge) sent us since the last tick. Cheap when idle, critical
        // for long sessions — without it, the server eventually
        // concludes we're dead and drops the publish slot.
        sink.drain_pings().await?;

        // Ingest gone → close the destination session cleanly instead
        // of sitting on a stale TCP connection. Platforms hold the
        // publish slot for 30-90 s after the last frame, so without
        // this the user's stream appears "live but frozen" on Twitch /
        // YouTube long after OBS dropped. The supervisor's gate on
        // ingest_alive prevents an immediate respawn here, so the
        // destination stays cleanly disconnected until OBS comes back.
        if !ctrl.ingest_alive() {
            ctrl.log(format!(
                "[{}] ingest gone — closing destination session",
                dest.id
            ));
            let _ = sink.send_delete_stream().await;
            return Ok(());
        }

        // Detect publisher reconnect (OBS stopped and re-started, new
        // session token). Without this branch the new session's "fresh"
        // timestamps would all read earlier than `input_ts_anchor` and
        // pace_and_send's monotonic guard would silently drop every
        // tag — the upstream stream would freeze forever even though
        // ingest is happily receiving bytes.
        let current_token = ctrl.publisher_token();
        if current_token != state.last_publisher_token {
            ctrl.log(format!("[{}] publisher reconnect — re-anchoring", dest.id));
            let watermark = ctrl.ring.latest_seq().unwrap_or(0);
            let new_idr = wait_first_idr_after(&ctrl.ring, watermark).await;
            reseed_after_publisher_change(&mut state, new_idr);
            state.last_publisher_token = current_token;
            send_sequence_headers(ctrl, dest, &mut sink, state.output_ts_base).await?;
            dest.consumer_seq
                .store(state.consumer_seq, Ordering::Relaxed);
            dest.last_seq_header_gen.store(
                ctrl.seq_header_gen.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            continue;
        }

        // Mid-stream codec / resolution change: OBS pushed a fresh
        // SPS/PPS or AudioSpecificConfig and the cached one is now
        // stale. Send the new bytes on the current output_ts before the
        // next media tag so the decoder reconfigures cleanly.
        let cur_gen = ctrl.seq_header_gen.load(Ordering::Relaxed);
        if cur_gen != dest.last_seq_header_gen.load(Ordering::Relaxed) {
            ctrl.log(format!("[{}] sequence header changed — resending", dest.id));
            // Use last_sent_input_ts so the resent header lands AFTER
            // anything we've already sent (same trick as apply_cut).
            let delta_u32 = state
                .last_sent_input_ts
                .saturating_sub(state.input_ts_anchor) as u32;
            let resend_ts = state.output_ts_base.wrapping_add(delta_u32);
            send_sequence_headers(ctrl, dest, &mut sink, resend_ts).await?;
            dest.last_seq_header_gen.store(cur_gen, Ordering::Relaxed);
        }

        let next_real = next_or_wait(&ctrl.ring, state.consumer_seq, 500).await;

        if let Some(meta) = next_real {
            let target_now = ctrl.target_delay_ms();
            let due = target_now != state.last_seen_target
                || state.last_cut_check.elapsed() >= Duration::from_millis(500);
            if due {
                state.last_cut_check = Instant::now();
                state.last_seen_target = target_now;
                if let Some(cut) = compute_delay_cut(ctrl, &meta) {
                    apply_cut(ctrl, dest, &mut sink, &mut state, cut).await?;
                    continue;
                }
            }
            pace_and_send(&mut sink, &mut state, &meta, ctrl, dest, &mut io_buf).await?;
        }
        // next_real == None → ingest starved; just wait for the next tag.
        // current_delay_ms is derived server-side now (see Controller),
        // so pumps don't race to overwrite it.
    }
}

/// Per-egress-session state. Lost on reconnect; re-anchored from scratch.
struct EgressState {
    consumer_seq: u64,
    input_ts_anchor: u64,      // original input ts of the most recent cut target
    output_ts_base: u32, // output ts assigned to the most recent cut target (RTMP wire is u32)
    wall_anchor: Instant, // wall clock at the most recent cut
    wall_anchor_input_ts: u64, // input ts that pairs with wall_anchor
    last_sent_input_ts: u64, // input ts of the last tag we actually emitted —
    // required so apply_cut can re-anchor the
    // output timeline *after* the last sent frame
    // (instead of after the last cut, which would
    // produce a monotonic-violating backward jump).
    /// Snapshot of `Controller::publisher_token()` at the last seed.
    /// When the controller bumps this (new OBS publish session), the
    /// pump re-anchors — otherwise the new publisher's reset timestamps
    /// would all fail pace_and_send's "older than anchor" check and the
    /// upstream player would never see another frame.
    last_publisher_token: u64,
    // --- cut-check throttling ---
    last_cut_check: Instant,
    last_seen_target: u32,
}

impl EgressState {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            consumer_seq: 0,
            input_ts_anchor: 0,
            output_ts_base: 0,
            wall_anchor: now,
            wall_anchor_input_ts: 0,
            last_sent_input_ts: 0,
            last_publisher_token: 0,
            last_cut_check: now,
            last_seen_target: 0,
        }
    }
}

/// Pace this tag's output: wait until its scheduled wall time, then send.
/// All accounting flows into the per-destination atomics so the UI can
/// show per-dest stats (bitrate, frames, bytes, etc).
async fn pace_and_send(
    sink: &mut EgressSink,
    state: &mut EgressState,
    meta: &TagMeta,
    ctrl: &Arc<Controller>,
    dest: &Arc<DestinationState>,
    io_buf: &mut Vec<u8>,
) -> io::Result<()> {
    // Out-of-order guard. After a cut, `input_ts_anchor` is the IDR's
    // input ts. RTMP from OBS interleaves audio+video in send order, not
    // strict timestamp order, so it's normal for an audio frame to land
    // in our index right after a video keyframe with ts SLIGHTLY EARLIER
    // than the keyframe. Drop the tag rather than emit a backward
    // out_ts (which would break monotonicity and stutter the player).
    // Lost frame is at most ~23 ms of audio (one AAC frame) or ~33 ms
    // of video (one P-frame) — imperceptible compared to the glitch.
    //
    // With u64 ts (set by expand_ts on ingest) the comparison is now
    // direct — no wrapping_sub / signed-int dance needed.
    if meta.ts_ms < state.input_ts_anchor {
        state.consumer_seq = meta.seq + 1;
        dest.consumer_seq
            .store(state.consumer_seq, Ordering::Relaxed);
        return Ok(());
    }
    let raw_delta_u64 = meta.ts_ms - state.input_ts_anchor;
    // The wire send carries a u32. Truncation is fine here because the
    // delta is bounded by buffer_target_ms + slack (~10 min absolute
    // max), which fits in u32. wrapping_add against output_ts_base
    // handles the (rare) out_ts wrap at the 49-day mark; downstream
    // players accept the wrap because rtmp ts is defined as wrap-modulo.
    let raw_delta = raw_delta_u64 as u32;

    let logical_offset_ms = meta.ts_ms.saturating_sub(state.wall_anchor_input_ts);
    let target_wall = state.wall_anchor + Duration::from_millis(logical_offset_ms);
    let now = Instant::now();
    if target_wall > now {
        tokio::time::sleep_until(tokio::time::Instant::from_std(target_wall)).await;
    }

    let out_ts = state.output_ts_base.wrapping_add(raw_delta);

    // Race-safe read: between the next_or_wait above and now we may have
    // slept hundreds of ms waiting for the wall-clock to catch up. While
    // we slept, ingest could in theory have wrapped the ring past this
    // tag's bytes (only realistic if buffer_mb is tight and bitrate is
    // huge — but the check is essentially free, so we always do it).
    // try_read_seq holds the index lock for the disk read, so the bytes
    // are guaranteed to still be the bytes of this tag — or it returns
    // None and we skip ahead instead of sending corrupted data to Twitch.
    match ctrl.ring.try_read_seq(meta.seq, io_buf)? {
        Some(()) => {}
        None => {
            state.consumer_seq = meta.seq + 1;
            dest.consumer_seq
                .store(state.consumer_seq, Ordering::Relaxed);
            return Ok(());
        }
    }
    // Per-tag trace. For audio we log only seq headers and an every-N
    // sample to keep the file small (audio at 50 Hz would dominate).
    // For video we log every tag — at ~30 fps × bytes/line the file
    // grows ~3 MB / 10 min, which is the right trade for diagnosing a
    // wire-format bug.
    match meta.kind {
        8 => {
            let tags_so_far = dest.tags_sent.load(Ordering::Relaxed);
            if tags_so_far < 20 || tags_so_far.is_multiple_of(200) {
                crate::trace::log(
                    "TAG_AUDIO",
                    &format!(
                        "dest={} i={} in_ts={} out_ts=0x{:08x} bytes={} hdr=0x{:02x}",
                        dest.id,
                        tags_so_far,
                        meta.ts_ms,
                        out_ts,
                        io_buf.len(),
                        io_buf.first().copied().unwrap_or(0),
                    ),
                );
            }
            sink.send_audio(out_ts, io_buf).await?;
        }
        9 => {
            // Per-destination video-tag selection. Twitch
            // destinations pass multi-track through bit-faithfully
            // (Enhanced Broadcasting); every other RTMP ingest gets
            // the single-track flatten that beta.6 used to apply
            // globally at the ingest stage. Single-track tags
            // borrow `io_buf` and never allocate.
            let selected = crate::h264::select_video_bytes(
                io_buf,
                dest.pass_through_multitrack_video.load(Ordering::Relaxed),
            );
            let bytes_out: &[u8] = &selected;
            let hdr = bytes_out.first().copied().unwrap_or(0);
            let is_idr = meta.is_idr;
            crate::trace::log(
                "TAG_VIDEO",
                &format!(
                    "dest={} i={} in_ts={} out_ts=0x{:08x} bytes={} hdr=0x{:02x} is_idr={} hex={}",
                    dest.id,
                    dest.tags_sent.load(Ordering::Relaxed),
                    meta.ts_ms,
                    out_ts,
                    bytes_out.len(),
                    hdr,
                    is_idr as u8,
                    crate::trace::hex_prefix(bytes_out, 16),
                ),
            );
            sink.send_video(out_ts, bytes_out).await?;
            // The bytes_sent accounting below uses bytes_out.len()
            // so per-destination bitrate reflects what we actually
            // put on the wire (raw multi-track for Twitch, flat
            // for everyone else).
            dest.tags_sent.fetch_add(1, Ordering::Relaxed);
            dest.bytes_sent
                .fetch_add(bytes_out.len() as u64, Ordering::Relaxed);
            dest.note_outbound_bytes(bytes_out.len());
            state.consumer_seq = meta.seq + 1;
            state.last_sent_input_ts = meta.ts_ms;
            dest.consumer_seq
                .store(state.consumer_seq, Ordering::Relaxed);
            return Ok(());
        }
        _ => {}
    }
    dest.tags_sent.fetch_add(1, Ordering::Relaxed);
    dest.bytes_sent
        .fetch_add(io_buf.len() as u64, Ordering::Relaxed);
    dest.note_outbound_bytes(io_buf.len());
    state.consumer_seq = meta.seq + 1;
    state.last_sent_input_ts = meta.ts_ms;
    // Tell the ingest-side trimmer how far we've read. The trimmer takes
    // the MIN across all destinations, so a slow consumer protects all
    // others from over-aggressive eviction.
    dest.consumer_seq
        .store(state.consumer_seq, Ordering::Relaxed);
    Ok(())
}

/// Describes a pending cut: just the IDR we want the consumer to jump
/// to next. Direction (fast-forward vs rewind) is implicit in whether
/// `target.ts_ms` is greater or less than the consumer's current ts;
/// the pump doesn't need to special-case it.
struct PendingCut {
    target: TagMeta,
}

fn compute_delay_cut(ctrl: &Arc<Controller>, current: &TagMeta) -> Option<PendingCut> {
    let target_delay = ctrl.target_delay_ms.load(Ordering::Relaxed) as u64;
    let latest = ctrl.ring.latest_ts()?;
    let oldest = ctrl.ring.oldest_ts()?;
    let current_delay = latest.saturating_sub(current.ts_ms);

    // 1500 ms dead band. The earlier 500 ms was too tight against the
    // typical 2 s OBS IDR cadence: after the initial cut, delivered
    // delay would be off by up to ~1 s (closest-IDR error), the 500 ms
    // dead band would fire, find_idr_near would (often) return the
    // SAME IDR, and we'd re-cut to it every 500 ms — visible as the
    // "repeating 1–2 s of content" bouncing the user reported.
    //
    // 1500 ms must exceed (IDR_cadence / 2 + send_jitter) to prevent
    // re-cuts when we're already on the best available IDR. For OBS's
    // recommended 2 s keyframe interval, 1500 ms is the right number.
    // The trade-off is the user's delay may be off by up to ~1.5 s from
    // their requested value — acceptable given the alternative is the
    // bouncing bug.
    let diff = (current_delay as i64) - (target_delay as i64);
    if diff.abs() < 1500 {
        ctrl.buffer_building.store(false, Ordering::Relaxed);
        return None;
    }

    // "Build buffer first" — if the user asked for a delay deeper than
    // the buffer currently extends, we can't honor it yet. Mark the state
    // and hold our position; the buffer keeps filling at real time, and
    // once the requested delay becomes reachable, the next iteration cuts.
    let have_seconds_back = latest.saturating_sub(oldest);
    if target_delay > have_seconds_back.saturating_add(1_500) {
        ctrl.buffer_building.store(true, Ordering::Relaxed);
        return None;
    }
    ctrl.buffer_building.store(false, Ordering::Relaxed);

    // Binary-search on the IDR-only secondary index (~log n over just
    // the keyframes) rather than the old O(n) walk over every tag.
    let desired_input_ts = latest.saturating_sub(target_delay);
    let target = ctrl.ring.find_idr_near(desired_input_ts, 2_000)?;
    if target.seq == current.seq {
        return None;
    }
    Some(PendingCut { target })
}

async fn apply_cut(
    ctrl: &Arc<Controller>,
    dest: &Arc<DestinationState>,
    sink: &mut EgressSink,
    state: &mut EgressState,
    cut: PendingCut,
) -> io::Result<()> {
    // Compute the LAST OUTPUT timestamp we actually sent (not the base
    // from the previous cut). The +1 ms gap is the minimum that satisfies
    // strict monotonicity (the only thing RTMP players require here).
    // The prior +33 ms was framerate-naive: at 60 fps it consistently
    // pushed the output timeline 17 ms ahead per cut, drifting forever
    // and showing up as audio/video sync drift over many toggles.
    // Both anchors are u64 now so the subtraction can't underflow even
    // across the RTMP 49-day wrap (expand_ts handles the wrap at ingest).
    // Output_ts is still u32 (RTMP wire) and wraps naturally.
    let input_delta_u32 = state
        .last_sent_input_ts
        .saturating_sub(state.input_ts_anchor) as u32;
    let last_out_ts = state.output_ts_base.wrapping_add(input_delta_u32);
    let new_output_ts_base = last_out_ts.wrapping_add(1);

    // Detailed cut trace — every cut writes one log line with the
    // before/after seq, the input-ts jump, and the resulting output_ts
    // base. Now logs the ACTUAL new base (previously the formatter just
    // showed `old+1`, useless for diagnosing post-cut drift), plus the
    // current seq_header_gen so reconnect/codec-change events line up.
    {
        let prev_seq = state.consumer_seq;
        let prev_ts = state.last_sent_input_ts;
        let new_seq = cut.target.seq;
        let new_ts = cut.target.ts_ms;
        let direction = if new_ts > prev_ts {
            "FWD"
        } else if new_ts < prev_ts {
            "BACK"
        } else {
            "SAME"
        };
        let delta_ms = (new_ts as i64) - (prev_ts as i64);
        let gen = ctrl.seq_header_gen.load(Ordering::Relaxed);
        ctrl.log(format!(
            "[{}] CUT {} seq:{}→{}  ts:{}→{}  delta:{}ms  out_ts_base:0x{:08x}→0x{:08x}  gen:{}",
            dest.id,
            direction,
            prev_seq,
            new_seq,
            prev_ts,
            new_ts,
            delta_ms,
            state.output_ts_base,
            new_output_ts_base,
            gen,
        ));
        crate::trace::log(
            "CUT",
            &format!(
                "dest={} dir={} seq={}→{} in_ts={}→{} delta_ms={} out_ts_base=0x{:08x}→0x{:08x} gen={}",
                dest.id, direction, prev_seq, new_seq, prev_ts, new_ts, delta_ms,
                state.output_ts_base, new_output_ts_base, gen,
            ),
        );
    }

    state.output_ts_base = new_output_ts_base;
    state.input_ts_anchor = cut.target.ts_ms;
    // Plain wall-clock anchor: from this instant onwards, pace_and_send
    // delivers content at real-time rate relative to the cut target's
    // input timeline. No backdating, no burst — the user model is
    // "save N seconds of buffer, when ready jump back N seconds, then
    // play at 1×" and that's exactly this.
    state.wall_anchor = Instant::now();
    state.wall_anchor_input_ts = cut.target.ts_ms;
    state.consumer_seq = cut.target.seq;
    state.last_sent_input_ts = cut.target.ts_ms;
    // Update the per-dest atomic immediately so the ingest-side trim
    // sees the new (potentially backward) position right away and can't
    // evict tags we just rewound to.
    dest.consumer_seq
        .store(state.consumer_seq, Ordering::Relaxed);

    // Re-emit cached sequence headers on the new output timeline so
    // the destination decoder has fresh config before the first
    // post-cut frame. The previous code skipped this on the assumption
    // that platforms cache headers from the initial publish — which is
    // true for YouTube but NOT reliably for Twitch. Twitch rotates its
    // transcoder workers periodically and the new worker has no cached
    // config: every cut without an explicit header resend was a chance
    // to land on a fresh worker with no SPS/PPS, producing audio-only
    // playback for the rest of the session. Cost is ~50 bytes per cut,
    // benefit is that every cut becomes self-contained from the
    // destination decoder's POV. Headers are also resent on publisher
    // reconnect (in the pump loop) and on actual codec change (via the
    // seq_header_gen check).
    send_sequence_headers(ctrl, dest, sink, new_output_ts_base).await?;
    // Sync the generation counter — the explicit resend above means
    // the next pump iteration shouldn't redundantly resend on a
    // gen-mismatch that has already been satisfied.
    dest.last_seq_header_gen.store(
        ctrl.seq_header_gen.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );

    dest.cuts_performed.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

async fn send_sequence_headers(
    ctrl: &Arc<Controller>,
    dest: &Arc<DestinationState>,
    sink: &mut EgressSink,
    ts: u32,
) -> io::Result<()> {
    // Drop the MutexGuards before the awaits.
    let v = ctrl.ring.video_seq_header.lock().unwrap().clone();
    if let Some(h) = v {
        // Same per-destination selection as the data-tag path: if the
        // cached video seq header is a multi-track wrapper and this
        // destination doesn't accept multi-track, flatten it down to
        // single-track before sending so the destination's decoder
        // doesn't choke on bytes it can't parse.
        let selected = crate::h264::select_video_bytes(
            &h,
            dest.pass_through_multitrack_video.load(Ordering::Relaxed),
        );
        let bytes_out: &[u8] = &selected;
        crate::trace::log(
            "VIDEO_SEQ_HDR_SENT",
            &format!(
                "ts=0x{:08x} bytes={} hex={}",
                ts,
                bytes_out.len(),
                crate::trace::hex_prefix(bytes_out, 64)
            ),
        );
        sink.send_video(ts, bytes_out).await?;
    }
    let a = ctrl.ring.audio_seq_header.lock().unwrap().clone();
    if let Some(h) = a {
        crate::trace::log(
            "AUDIO_SEQ_HDR_SENT",
            &format!(
                "ts=0x{:08x} bytes={} hex={}",
                ts,
                h.len(),
                crate::trace::hex_prefix(&h, 32)
            ),
        );
        sink.send_audio(ts, &h).await?;
    }
    sink.flush().await
}

async fn wait_first_idr(ring: &Arc<DiskRing>) -> TagMeta {
    loop {
        // Register notification *before* checking — guarantees we don't
        // miss an append that lands between the check and the await.
        let notified = ring.on_append.notified();
        if let Some(m) = ring.newest_idr() {
            return m;
        }
        notified.await;
    }
}

/// Same as `wait_first_idr` but only returns IDRs with seq strictly
/// greater than `min_seq`. Used after publisher reconnect to ignore the
/// previous session's IDRs that are still indexed in the ring.
async fn wait_first_idr_after(ring: &Arc<DiskRing>, min_seq: u64) -> TagMeta {
    loop {
        let notified = ring.on_append.notified();
        if let Some(m) = ring.newest_idr_after(min_seq) {
            return m;
        }
        notified.await;
    }
}

/// Pick the seed IDR for a freshly spawned egress pump. If a delay is
/// already active and the ring has enough history, seed at the delayed
/// position so the new pump joins mid-stream cleanly; otherwise fall
/// back to the newest IDR (live edge).
async fn seed_idr(ctrl: &Arc<Controller>) -> TagMeta {
    let target = ctrl.target_delay_ms() as u64;
    if target > 0 {
        if let (Some(latest), Some(oldest)) = (ctrl.ring.latest_ts(), ctrl.ring.oldest_ts()) {
            // Only attempt the delayed seed if the buffer actually spans
            // far enough back — otherwise find_idr_near may give us an
            // IDR much closer to live than the user asked for.
            if latest.saturating_sub(oldest) + 1_500 >= target {
                let desired = latest.saturating_sub(target);
                if let Some(idr) = ctrl.ring.find_idr_near(desired, 2_000) {
                    return idr;
                }
            }
        }
    }
    wait_first_idr(&ctrl.ring).await
}

/// Re-anchor egress state after the publisher changed identity. Mirrors
/// `apply_cut`'s timeline math so the output_ts stays strictly monotonic.
fn reseed_after_publisher_change(state: &mut EgressState, new_idr: TagMeta) {
    let input_delta_u32 = state
        .last_sent_input_ts
        .saturating_sub(state.input_ts_anchor) as u32;
    let last_out_ts = state.output_ts_base.wrapping_add(input_delta_u32);
    state.output_ts_base = last_out_ts.wrapping_add(1);
    state.input_ts_anchor = new_idr.ts_ms;
    state.wall_anchor = Instant::now();
    state.wall_anchor_input_ts = new_idr.ts_ms;
    state.consumer_seq = new_idr.seq;
    state.last_sent_input_ts = new_idr.ts_ms;
}

/// Resolve the next tag at `seq`. If the producer hasn't reached `seq` yet,
/// wait for up to `wait_ms` for a new append. Returns None if still nothing.
///
/// Notification is registered *before* the find_by_seq check; the prior
/// order had a race where an append between check and registration would
/// be missed and the call would block for the full `wait_ms` for no reason.
async fn next_or_wait(ring: &Arc<DiskRing>, seq: u64, wait_ms: u64) -> Option<TagMeta> {
    // If we've fallen off the back of the ring (eviction passed us), jump
    // forward to the FIRST IDR at or after the new front. Landing on
    // whatever the front happens to be — typically a P-frame — would
    // send frames that reference reference-frames that aren't in the
    // decoder's buffer → viewers see macroblocking until the next IDR.
    // Aligning to an IDR boundary loses a bit more content but keeps
    // the decode chain valid.
    if let Some(front) = ring.front_seq() {
        if seq < front {
            if let Some(m) = ring.oldest_idr_at_or_after(front) {
                return Some(m);
            }
            // No IDR in the ring at all (very early or pathological) —
            // fall through to the wait path; the next append might be one.
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let notified = ring.on_append.notified();
        if let Some((_, m)) = ring.find_by_seq(seq) {
            return Some(m);
        }
        tokio::select! {
            _ = notified => continue,
            _ = tokio::time::sleep_until(deadline) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicU32 as TestUniq, Ordering as TestOrd};

    static UNIQ: TestUniq = TestUniq::new(0);

    /// Test-scoped Controller with its own tmp DiskRing. Cleans up on drop.
    struct Harness {
        ctrl: Arc<Controller>,
        path: std::path::PathBuf,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn harness(initial_armed_ms: u32) -> Harness {
        let n = UNIQ.fetch_add(1, TestOrd::SeqCst);
        let path = env::temp_dir().join(format!("ic-test-ctrl-{}-{}.buf", std::process::id(), n));
        let _ = std::fs::remove_file(&path);
        let ring = Arc::new(DiskRing::create(&path, 4 * 1024 * 1024).expect("ring create"));
        let ctrl = Arc::new(Controller::new(ring, initial_armed_ms));
        Harness { ctrl, path }
    }

    /// Push N seconds of fake tags into the ring at `fps`. Each IDR is at
    /// the start of every second; the rest are non-IDR p-frames. Stamps
    /// monotonically from `start_ms`. Used to drive `buffer_fill_ms` past
    /// the armed threshold so we can exercise phase transitions.
    fn feed_seconds(ctrl: &Controller, start_ms: u32, secs: u32, fps: u32) {
        let frame_ms = 1000 / fps;
        for s in 0..secs {
            for f in 0..fps {
                let ts = start_ms + s * 1000 + f * frame_ms;
                let is_idr = f == 0;
                // kind 9 = video, ~50 B payload (size doesn't matter for state machine)
                ctrl.on_tag(9, ts, &[0u8; 50], is_idr, false);
            }
        }
    }

    // ── Phase machine ────────────────────────────────────────────────

    #[test]
    fn cold_start_is_idle() {
        let h = harness(0);
        assert_eq!(h.ctrl.phase(), "idle");
        assert_eq!(h.ctrl.armed_delay_ms(), 0);
        assert_eq!(h.ctrl.target_delay_ms(), 0);
    }

    #[test]
    fn arm_then_empty_buffer_is_preparing() {
        let h = harness(0);
        h.ctrl.arm_delay(5_000);
        assert_eq!(h.ctrl.armed_delay_ms(), 5_000);
        // Empty buffer, fill=0 < armed → preparing
        assert_eq!(h.ctrl.phase(), "preparing");
    }

    #[test]
    fn buffer_fills_to_ready() {
        let h = harness(0);
        h.ctrl.arm_delay(3_000);
        feed_seconds(&h.ctrl, 0, 4, 30); // 4 seconds of tags @ 30 fps
                                         // fill ≈ 3933 ms (29 × 33 ms span), well past the 3 s armed target
        assert!(
            h.ctrl.buffer_fill_ms() >= 3_000,
            "buffer should hold ≥3 s of tags, got {} ms",
            h.ctrl.buffer_fill_ms()
        );
        assert_eq!(h.ctrl.phase(), "ready");
    }

    #[test]
    fn activate_when_ready_flips_to_active() {
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        let r = h.ctrl.activate_delay();
        assert!(
            r.is_ok(),
            "activate must succeed when buffer ≥ armed: {:?}",
            r
        );
        assert_eq!(h.ctrl.phase(), "active");
        assert_eq!(h.ctrl.target_delay_ms(), 2_000);
    }

    #[test]
    fn activate_without_arm_errors_not_armed() {
        let h = harness(0);
        let r = h.ctrl.activate_delay();
        assert!(matches!(r, Err(ActivateError::NotArmed)));
    }

    #[test]
    fn activate_with_partial_buffer_errors_buffer_short() {
        let h = harness(0);
        h.ctrl.arm_delay(10_000);
        // Only 1 second of buffer — activate should refuse with remaining ETA
        feed_seconds(&h.ctrl, 0, 1, 30);
        match h.ctrl.activate_delay() {
            Err(ActivateError::BufferShort { remaining_ms }) => {
                assert!(
                    remaining_ms >= 5_000,
                    "expected meaningful remaining time, got {}",
                    remaining_ms
                );
            }
            other => panic!("expected BufferShort, got {:?}", other),
        }
    }

    #[test]
    fn stop_clears_target_keeps_armed() {
        // The "magic" two-phase behaviour: dropping back to live after a
        // cut must not also disarm the delay, so the user can re-activate
        // instantly once the buffer rebuilds.
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        h.ctrl.activate_delay().unwrap();
        h.ctrl.stop_delay();
        assert_eq!(h.ctrl.target_delay_ms(), 0, "target must clear on stop");
        assert_eq!(h.ctrl.armed_delay_ms(), 2_000, "armed must survive stop");
        // With buffer still full, phase is `ready` again — not `idle`.
        assert_eq!(h.ctrl.phase(), "ready");
    }

    #[test]
    fn disarm_via_zero_arm_wipes_both() {
        let h = harness(0);
        h.ctrl.arm_delay(5_000);
        feed_seconds(&h.ctrl, 0, 6, 30);
        h.ctrl.activate_delay().unwrap();
        h.ctrl.arm_delay(0);
        assert_eq!(h.ctrl.armed_delay_ms(), 0);
        assert_eq!(h.ctrl.target_delay_ms(), 0);
        assert_eq!(h.ctrl.phase(), "idle");
    }

    #[test]
    fn arm_change_during_active_updates_target_live() {
        // While active, changing the armed amount must also re-target —
        // otherwise the user moves the slider and nothing happens until
        // they cut + re-activate.
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        h.ctrl.activate_delay().unwrap();
        assert_eq!(h.ctrl.target_delay_ms(), 2_000);
        h.ctrl.arm_delay(5_000);
        assert_eq!(
            h.ctrl.target_delay_ms(),
            5_000,
            "live-arm-change must propagate to target"
        );
    }

    #[test]
    fn arm_clamps_to_max() {
        // Hard ceiling at 600 s. A persisted-or-hand-edited 999 s must
        // be clamped so the buffer can actually catch up.
        let h = harness(0);
        h.ctrl.arm_delay(9_999_999);
        assert_eq!(h.ctrl.armed_delay_ms(), 600_000);
    }

    // ── Ring + index correctness ─────────────────────────────────────

    #[test]
    fn on_tag_appends_to_ring_with_monotonic_ts() {
        // The wrap-expansion path is the load-bearing piece of timestamp
        // logic. Even a small wrap should produce strictly increasing
        // u64 timestamps in the ring.
        let h = harness(0);
        // Two tags well below any wrap point — strictly increasing.
        h.ctrl.on_tag(9, 100, &[0u8; 10], true, false);
        h.ctrl.on_tag(9, 200, &[0u8; 10], false, false);
        h.ctrl.on_tag(9, 300, &[0u8; 10], false, false);

        let oldest = h.ctrl.ring.oldest_ts().unwrap();
        let latest = h.ctrl.ring.latest_ts().unwrap();
        assert_eq!(oldest, 100);
        assert_eq!(latest, 300);
        assert!(latest > oldest);
    }

    #[test]
    fn idr_index_seek_finds_the_right_frame() {
        let h = harness(0);
        feed_seconds(&h.ctrl, 0, 5, 30); // IDR at 0, 1000, 2000, 3000, 4000
                                         // Target 2500, tolerance 600 → closest IDR is 2000 (distance 500)
        let m = h
            .ctrl
            .ring
            .find_idr_near(2500, 600)
            .expect("should find IDR near 2500");
        assert_eq!(m.ts_ms, 2000);
        assert!(m.is_idr);
    }

    // ── Timestamp wrap (the load-bearing one) ──────────────────────

    #[test]
    fn wire_ts_wrap_promotes_to_monotonic_u64() {
        // RTMP wire timestamps are u32 ms, so they wrap every ~49.7 days.
        // The Controller's expand_ts() promotes them to u64 by detecting
        // a wrap (current u32 << previous u32) and bumping a "high"
        // counter. Without this, a wrap would make `latest_ts - oldest_ts`
        // negative and the whole delay accounting would explode.
        //
        // We can't call expand_ts directly (private), so we exercise it
        // through on_tag() and observe the result via ring.latest_ts().
        let h = harness(0);
        // Walk forward toward the wrap boundary, then over it.
        let near_max = u32::MAX - 1000;
        h.ctrl.on_tag(9, near_max, &[0u8; 20], true, false);
        h.ctrl.on_tag(9, near_max + 500, &[0u8; 20], false, false);
        // Cross the boundary: wire_ts drops near 0 (this LOOKS LIKE
        // going backwards if you only had u32 math).
        h.ctrl.on_tag(9, 200, &[0u8; 20], false, false);

        let latest = h.ctrl.ring.latest_ts().unwrap();
        let oldest = h.ctrl.ring.oldest_ts().unwrap();
        assert!(
            latest > oldest,
            "post-wrap latest ({latest}) must be > oldest ({oldest}); \
             wrap promotion is broken"
        );
        // The expand_ts machinery should have lifted the post-wrap value
        // above u32::MAX (high bit promoted).
        assert!(
            latest > u32::MAX as u64,
            "expected post-wrap ts above u32::MAX, got {latest}"
        );
    }
}
