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
use crate::compat::StreamParams;
use crate::h264::{AudioCodec, VideoCodec};
use crate::rtmp::client::{EgressClient, EgressSink, EgressUrl};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};

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

/// Smallest buffer we'll keep even when the user has nothing armed -
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
                format!("buffer is still building - wait ~{}s", secs)
            }
        }
    }
}
/// Hidden slack beyond what the user sees as the target. Equal to the
/// IDR-search tolerance, so a "5s armed" cut can always land on a real
/// IDR even if the nearest one happens to be slightly past the boundary.
const BUFFER_SLACK_MS: u32 = 2_000;

/// How many IDR-to-IDR gaps to average before freezing the keyframe-interval
/// measurement. Five gaps is ~10 s of a normally-configured stream: long
/// enough that one odd GOP at stream start can't skew the mean, short enough
/// that the reading is settled before anyone looks at the dashboard.
const KEYFRAME_SAMPLE_GAPS: u32 = 5;

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
    /// mid-stream - without this, the cached old config and new keyframe
    /// bytes don't match and the upstream decoder silently rejects every
    /// subsequent frame.
    last_seq_header_gen: AtomicU32,
    rate_window_bytes: AtomicU64,
    rate_window_start_ms: AtomicU64,
    /// True if this destination accepts Enhanced Broadcasting multi-track
    /// video on the wire. Set by the supervisor to `true` when the
    /// destination's platform is `twitch` and to `false` for everything
    /// else (YouTube / Kick / Trovo / Restream / custom RTMP - none of
    /// which currently process multi-track video). When false, the pump
    /// runs `flatten_multitrack_video` on every multi-track tag just
    /// before sending, which produces a single-track tag that's
    /// byte-identical to what beta.6 emitted from the ingest-side
    /// flatten - so existing destinations see no behaviour change.
    pub pass_through_multitrack_video: AtomicBool,
    /// When true, multi-track AUDIO tags pass through to this destination
    /// bit-faithfully - a Twitch destination keeping the live track (wire
    /// TrackId 0) and the VOD-audio track (TrackId 1, OBS's "Pista VOD de
    /// Twitch") together. Twitch's regular ingest has supported the VOD
    /// audio track for years, predating Enhanced Broadcasting, so this is
    /// the default for any enabled Twitch destination, not gated on an EB
    /// session like the video flag above. When false, the pump keeps only
    /// `audio_track` (below), flattened - so a simulcast YouTube / Kick
    /// gets exactly one audio track it can decode. Set by the supervisor
    /// from the destination's `audio_track` setting + platform; see
    /// `audio_egress`.
    pub audio_passthrough: AtomicBool,
    /// The single wire TrackId this destination keeps when `audio_passthrough`
    /// is false. `0` = the live track (default for every non-Twitch
    /// platform); `1` = the second / clean track (copyright-safe audio for
    /// YouTube / Kick). Only read when `audio_passthrough` is false.
    pub audio_track: AtomicU8,
    /// True when this destination wants the vertical (9:16) canvas
    /// instead of the horizontal primary. Set by the supervisor from the
    /// destination's `stream_format == "vertical"` (non-Twitch only;
    /// Twitch always gets native dual-canvas passthrough). When true the
    /// pump forwards only `vertical_primary_track`, flattened.
    pub egress_vertical: AtomicBool,
    /// The OneTrack TrackId of the vertical-canvas primary, discovered by
    /// `h264::detect_vertical_primary_track` from the per-track seq-header
    /// cache and refreshed whenever that cache changes. `0xFF` means
    /// "not resolved yet" (Twitch Dual Format isn't active, or no portrait
    /// track has been seen): a vertical destination then sends no video
    /// and surfaces a "waiting for Dual Format" status, while every other
    /// destination is unaffected.
    pub vertical_primary_track: AtomicU8,
    /// Twitch only: when our /obs/multitrack-config proxy successfully
    /// allocates an Enhanced Broadcasting session, Twitch's API returns
    /// a specific IVS ingest URL like
    /// `rtmps://<region>.contribute.live-video.net/app/<key>` - and
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
    pub eb_override_url: crate::sync::Mutex<Option<String>>,
    /// In-flight latch for the VOD-audio IVS session fetch. Invariant:
    /// `true` for exactly as long as one `fetch_twitch_vod_session` task is
    /// running, `false` otherwise. The supervisor fires every ~2 s; without
    /// this latch every tick while `eb_override_url` is None would launch a
    /// fresh Twitch API request, each allocating a *different* IVS session
    /// and forcing an extra egress restart (the multi-session / Source-Only
    /// symptom). Claimed via `try_claim_vod_fetch`; the fetch task clears it
    /// when it finishes, success or failure. Deliberately decoupled from
    /// `eb_override_url`'s lifecycle - the override may be cleared from
    /// several places (publisher disconnect, the multitrack-config proxy's
    /// stale-override cleanup), and none of them need to know this latch
    /// exists. The override being Some is what blocks a re-fetch; this latch
    /// only prevents concurrent ones.
    pub vod_fetch_pending: AtomicBool,
    /// Generation counter for the publisher session this destination's
    /// override belongs to. Bumped (under `eb_override_url`'s mutex) every
    /// time the publisher disconnects. A VOD-session fetch can take up to
    /// 15 s; if OBS disconnects while one is in flight, the IVS session it
    /// returns is bound to the now-dead publisher session and pointing the
    /// next stream at it would land on an endpoint with no live session
    /// (the Source-Only failure). The fetch captures this epoch when it is
    /// spawned and, at apply time, writes the override only if the epoch
    /// still matches - both reads happen under the override mutex so the
    /// check can't race the disconnect's clear+bump.
    pub session_epoch: AtomicU64,
}

/// Outcome of finishing a VOD-session fetch, returned by
/// `complete_vod_fetch` so the supervisor can log it without re-deriving
/// what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VodFetchOutcome {
    /// Session applied - egress will restart onto the IVS endpoint.
    Applied,
    /// Result dropped: the publisher session it was for ended mid-fetch.
    DiscardedStale,
    /// Twitch's API returned nothing; the next supervisor tick retries.
    Failed,
}

impl DestinationState {
    pub fn new(id: String) -> Self {
        Self {
            id,
            egress_alive: AtomicBool::new(false),
            // Sentinel: a freshly-registered destination whose pump
            // hasn't seeded yet must NOT pin the ring's trim to seq 0.
            // min_consumer_seq treats u64::MAX as "no constraint", so
            // until PUMP_START stores the real seed seq, on_tag's trim
            // can evict freely. Otherwise adding a new destination
            // mid-stream would briefly stop the ring from trimming -
            // ballooning the buffer by ~bitrate × seed_idr wait.
            consumer_seq: AtomicU64::new(u64::MAX),
            tags_sent: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            cuts_performed: AtomicU32::new(0),
            reconnects: AtomicU32::new(0),
            bitrate_kbps_out: AtomicU32::new(0),
            shutdown_requested: AtomicBool::new(false),
            last_seq_header_gen: AtomicU32::new(0),
            eb_override_url: crate::sync::Mutex::new(None),
            rate_window_bytes: AtomicU64::new(0),
            rate_window_start_ms: AtomicU64::new(0),
            // Default false: every newly-spawned destination flattens
            // multi-track until the supervisor decides otherwise. This
            // preserves beta.6 behaviour for any code path that creates
            // a DestinationState without going through the supervisor
            // (the destination_state lazy-init in particular).
            pass_through_multitrack_video: AtomicBool::new(false),
            // Default: single live track (TrackId 0), the safe non-Twitch
            // behaviour until the supervisor sets the real policy.
            audio_passthrough: AtomicBool::new(false),
            audio_track: AtomicU8::new(0),
            egress_vertical: AtomicBool::new(false),
            // 0xFF = unresolved until a portrait track is detected.
            vertical_primary_track: AtomicU8::new(0xFF),
            vod_fetch_pending: AtomicBool::new(false),
            session_epoch: AtomicU64::new(0),
        }
    }

    /// The video egress policy for this destination right now. Read by
    /// both the live send path and the seq-header replay so they always
    /// agree on which canvas to forward. Returns `None` when a vertical
    /// destination has no resolved canvas yet (Twitch Dual Format isn't
    /// active): the caller drops all video and the dest waits, leaving
    /// every other destination untouched.
    pub fn video_egress(&self) -> Option<crate::h264::VideoEgress> {
        use crate::h264::VideoEgress;
        if self.pass_through_multitrack_video.load(Ordering::Relaxed) {
            return Some(VideoEgress::Passthrough);
        }
        if self.egress_vertical.load(Ordering::Relaxed) {
            let track = self.vertical_primary_track.load(Ordering::Relaxed);
            if track == 0xFF {
                return None;
            }
            return Some(VideoEgress::Track(track));
        }
        Some(VideoEgress::Track(0))
    }

    /// The audio egress policy for this destination right now, the audio
    /// twin of `video_egress`. Read by both the live send path and the
    /// seq-header replay so they agree on which track(s) to forward.
    pub fn audio_egress(&self) -> crate::h264::AudioEgress {
        use crate::h264::AudioEgress;
        if self.audio_passthrough.load(Ordering::Relaxed) {
            AudioEgress::Passthrough
        } else {
            AudioEgress::Track(self.audio_track.load(Ordering::Relaxed))
        }
    }

    /// Try to claim the right to fetch this destination's VOD-audio IVS
    /// session. Returns `true` for at most one caller per in-flight fetch.
    ///
    /// Two checks, in order:
    /// 1. Atomically latch `vod_fetch_pending`. If it was already set, a
    ///    fetch is in flight - bail.
    /// 2. Re-read `eb_override_url` *through its mutex* (the authoritative
    ///    source of truth for "do we have a session"). The supervisor reads
    ///    the override at the top of its loop, a moment before this claim;
    ///    a fetch that started on an earlier tick may have completed in that
    ///    gap. If a session now exists we release the latch and bail so we
    ///    never allocate a redundant one.
    ///
    /// The caller MUST clear `vod_fetch_pending` when the fetch finishes
    /// (both on success and failure) to preserve the latch's invariant.
    pub fn try_claim_vod_fetch(&self) -> bool {
        if self.vod_fetch_pending.swap(true, Ordering::Relaxed) {
            return false;
        }
        if self.eb_override_url.lock().is_some() {
            self.vod_fetch_pending.store(false, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// Read the current session epoch. Capture this when spawning a VOD
    /// session fetch and pass it back to `apply_vod_session_if_current`.
    pub fn session_epoch(&self) -> u64 {
        self.session_epoch.load(Ordering::Relaxed)
    }

    /// Apply a freshly-fetched VOD-audio IVS session URL, but only if the
    /// publisher session that requested it is still the current one.
    /// `captured_epoch` is `session_epoch()` read when the fetch was spawned.
    /// If a disconnect has since bumped the epoch, the fetched session is
    /// bound to a dead publisher session - writing it would point the next
    /// stream at a stale IVS endpoint - so we discard it and return false.
    /// The epoch is read under the override mutex, so it cannot race
    /// `invalidate_session_override`'s clear+bump.
    pub fn apply_vod_session_if_current(&self, url: String, captured_epoch: u64) -> bool {
        let mut guard = self.eb_override_url.lock();
        if self.session_epoch.load(Ordering::Relaxed) != captured_epoch {
            return false;
        }
        *guard = Some(url);
        true
    }

    /// Invalidate this destination's VOD/EB override on publisher
    /// disconnect: clear the URL and bump the session epoch in one locked
    /// section, so a fetch still in flight discards its (now-stale) result
    /// rather than writing it into the next session.
    pub fn invalidate_session_override(&self) {
        let mut guard = self.eb_override_url.lock();
        *guard = None;
        self.session_epoch.fetch_add(1, Ordering::Relaxed);
    }

    /// Finish a VOD-session fetch: apply the result if our publisher
    /// session is still current, then release the in-flight latch no
    /// matter what. Keeping the release here - not in the supervisor
    /// closure - puts the whole latch lifecycle (claim in
    /// `try_claim_vod_fetch`, release here) on one type, so a fetch can
    /// never leave the latch stuck. Returns the outcome for the caller
    /// to log.
    pub fn complete_vod_fetch(
        &self,
        result: Option<String>,
        captured_epoch: u64,
    ) -> VodFetchOutcome {
        let outcome = match result {
            Some(url) => {
                if self.apply_vod_session_if_current(url, captured_epoch) {
                    VodFetchOutcome::Applied
                } else {
                    VodFetchOutcome::DiscardedStale
                }
            }
            None => VodFetchOutcome::Failed,
        };
        self.vod_fetch_pending.store(false, Ordering::Relaxed);
        outcome
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

/// What the main loop should do after a graceful shutdown: exit for good, or
/// relaunch a fresh process in place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShutdownKind {
    Quit,
    Restart,
}

pub struct Controller {
    pub ring: Arc<DiskRing>,

    // --- Delay state machine (single, applies to ALL destinations) ----
    // The consumer offset is global - every destination delivers the same
    // delay simultaneously. Per-destination delays would require N
    // consumer cursors; deferred until requested.
    armed_delay_ms: AtomicU32,
    target_delay_ms: AtomicU32,
    /// "There is an outstanding arm action that hasn't been activated
    /// yet, and the user hasn't cut since arming." Set true by
    /// `arm_delay` when arming from a non-active state (target was 0).
    /// Cleared by `activate_delay` on success, by `stop_delay` (cut),
    /// and by `begin_publish` (fresh publisher session).
    ///
    /// Used by `supervise_egress` to gate the auto-activate-when-ready
    /// behaviour: fires once per arm action, not once per publisher
    /// session. So after Cut, this stays false until the next arm
    /// action - the cut sticks. Re-arming sets it true again, so a
    /// fresh arm (manual, profile activation, or auto-pre-arm on
    /// reconnect) gets its own auto-activate. Live-update arms (arming
    /// a new value while target > 0) do NOT set this, because the
    /// streamer is already in the active state and a subsequent cut
    /// shouldn't snap back to "active" via auto-activate.
    auto_activate_pending: AtomicBool,
    /// Input-timeline timestamp (u64, expand_ts domain) of a scheduled
    /// "cut after this airs" mark. 0 = none pending. Set by
    /// `schedule_safe_cut` to the live-edge ts at the moment the
    /// streamer pressed the button; the pump loops watch the slowest
    /// live consumer and fire `stop_delay` once every destination has
    /// aired past the mark. This is the "match-end reaction" cut: the
    /// streamer marks the moment their reaction ends (on their live
    /// timeline) instead of counting the delay down in their head, and
    /// nothing before the mark ever gets clipped. Cleared by
    /// `cancel_safe_cut`, by a manual `stop_delay`, by disarm
    /// (`arm_delay(0)`), and by `begin_publish` (the mark belongs to
    /// the old session's timeline - a fresh publisher restarts ts near
    /// 0, so a stale mark could never be reached).
    safe_cut_input_ts: AtomicU64,
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
    destinations: crate::sync::RwLock<HashMap<String, Arc<DestinationState>>>,

    // Ingest-side stats
    ingest_disconnects: AtomicU32,
    bitrate_kbps: AtomicU32, // inbound (from OBS)
    rate_window_bytes: AtomicU64,
    rate_window_start_ms: AtomicU64,

    // Discord webhook URL. Empty = disabled. Updated live via update_webhook.
    webhook_url: crate::sync::Mutex<String>,
    // Required RTMP stream key. Empty = accept any publisher (local default).
    // Mirrored from Settings via update_ingest_key so `begin_publish` can
    // enforce it without the ingest task needing a settings handle.
    ingest_key: crate::sync::Mutex<String>,
    // Per-peer wrong-key throttle so a weak ingest key can't be brute-forced at
    // wire speed. Only engages once a key is configured.
    ingest_limiter: crate::auth::RateLimiter,
    // Stream keys from Enhanced Broadcasting sessions we brokered via
    // /obs/multitrack-config. OBS publishes an EB stream with the Twitch session
    // token (not the configured ingest key), and we are the one that handed that
    // token out, so `begin_publish` trusts it alongside the ingest key. Bounded
    // and TTL'd (see remember_eb_key) so tokens never accumulate.
    eb_keys: crate::sync::Mutex<Vec<(String, Instant)>>,
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

    // --- Measured encoder parameters (per publisher session) ---
    // Feed `compat::compat_warning`, which compares them against the
    // enabled destinations. Decoded from the AVC sequence header; 0 when
    // the codec isn't AVC or the header hasn't arrived yet.
    // Packed (width << 32 | height). One atomic, not two, so a dashboard
    // read always pairs a width with ITS height: storing them separately let
    // a read land between the two writes and report a new width beside a
    // stale height for one frame.
    video_dims: AtomicU64,
    // Keyframe interval is measured, not configured, so it has to be
    // sampled from the stream. We take the MEAN spacing across the first
    // `KEYFRAME_SAMPLE_GAPS` IDR gaps rather than a running average: it
    // is jitter-tolerant, needs no decay tuning, and - the point - it
    // FREEZES once the sample budget is spent. A value that keeps moving
    // is what made the old buffer-capacity gate strobe (see 0.1.10), and
    // a warning line that flickers on and off mid-stream is worse than
    // no warning at all.
    //
    // The first/last/gaps triple is writer-private accounting (only the
    // single ingest task touches it). After each gap the writer republishes
    // the mean into `keyframe_interval_cached`, so every reader does one
    // atomic load instead of a (last, first, gaps) read that could tear.
    //
    // `idr_window_open` cannot be folded into `first_idr_ts_ms == 0`: an
    // RTMP session normally starts at timestamp 0, so a 0 sentinel would
    // discard the real first keyframe and re-open the window on the
    // second one, skewing every subsequent mean.
    idr_window_open: AtomicBool,
    first_idr_ts_ms: AtomicU64,
    last_idr_ts_ms: AtomicU64,
    idr_gaps: AtomicU32,
    keyframe_interval_cached: AtomicU32,

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

    // Wall-clock (process_now_ms) of last multi-track video tag - the
    // Enhanced Broadcasting warning chip only shows if we've seen one
    // recently. Sticky-on-true was the old behavior and produced
    // permanent false-positive chips after a single misclassified tag.
    last_multitrack_video_ms: AtomicU64,
    // Tracks when backpressure first started being true. Used by
    // `is_backpressured` to require the condition to hold for a
    // sustained window (1.5 s) before reporting - without this the
    // chip strobes on every cut transition.
    backpressure_since_ms: AtomicU64,
    /// Bumped on every NEW sequence-header tag received from ingest
    /// (audio or video, regardless of whether the bytes actually
    /// changed). Egress pumps compare this against their own
    /// `last_seq_header_gen` and resend both cached headers when it
    /// jumps - so mid-stream encoder swaps (resolution change in OBS,
    /// AVC→HEVC switch) don't desync the downstream decoder.
    seq_header_gen: AtomicU32,

    // --- Graceful shutdown signal -------------------------------------
    // Fired by the web Quit/Restart routes and (on Windows) the tray Quit.
    // The main loop parks on `shutdown_notify`; `shutdown_kind` carries the
    // intent (0 = none, 1 = quit, 2 = restart). Unified here so every exit
    // path runs the same clean egress teardown.
    shutdown_notify: Notify,
    shutdown_kind: AtomicU8,

    // Process start, for the dashboard's uptime readout.
    started: std::time::Instant,

    // In-process log ring (most recent N lines).
    pub logs: crate::sync::Mutex<std::collections::VecDeque<String>>,

    // Shared MIDI state (bindings mirror + learn mode + device list),
    // driven by the Windows winmm listener thread and read/written by the
    // web layer. Present on every platform; inert where there is no MIDI
    // backend (`available` stays false).
    midi: Arc<crate::midi::MidiState>,
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
            auto_activate_pending: AtomicBool::new(false),
            safe_cut_input_ts: AtomicU64::new(0),
            ingest_alive: AtomicBool::new(false),
            buffer_building: AtomicBool::new(false),
            publisher_token: AtomicU64::new(0),
            destinations: crate::sync::RwLock::new(HashMap::new()),
            ingest_disconnects: AtomicU32::new(0),
            bitrate_kbps: AtomicU32::new(0),
            rate_window_bytes: AtomicU64::new(0),
            rate_window_start_ms: AtomicU64::new(0),
            webhook_url: crate::sync::Mutex::new(String::new()),
            ingest_key: crate::sync::Mutex::new(String::new()),
            // 5 wrong keys then a short exponential lockout. A legit OBS uses
            // the right key and clears its record on the first accept, so this
            // only ever bites a guesser.
            ingest_limiter: crate::auth::RateLimiter::new(
                5,
                std::time::Duration::from_secs(10),
                std::time::Duration::from_secs(10 * 60),
                std::time::Duration::from_secs(10 * 60),
            ),
            eb_keys: crate::sync::Mutex::new(Vec::new()),
            webhook_last_fire_ms: AtomicU64::new(0),
            publish_lock: Mutex::new(()),
            video_codec: AtomicU8::new(0),
            audio_codec: AtomicU8::new(0),
            video_dims: AtomicU64::new(0),
            idr_window_open: AtomicBool::new(false),
            first_idr_ts_ms: AtomicU64::new(0),
            last_idr_ts_ms: AtomicU64::new(0),
            idr_gaps: AtomicU32::new(0),
            keyframe_interval_cached: AtomicU32::new(0),
            multitrack_video: AtomicBool::new(false),
            multitrack_audio: AtomicBool::new(false),
            seq_header_gen: AtomicU32::new(0),
            last_input_ts_u32: AtomicU32::new(0),
            input_ts_wrap_high: AtomicU32::new(0),
            last_multitrack_video_ms: AtomicU64::new(0),
            backpressure_since_ms: AtomicU64::new(0),
            shutdown_notify: Notify::new(),
            shutdown_kind: AtomicU8::new(0),
            started: std::time::Instant::now(),
            logs: crate::sync::Mutex::new(std::collections::VecDeque::with_capacity(512)),
            midi: Arc::new(crate::midi::MidiState::new()),
        }
    }

    /// Shared MIDI state, for the listener thread and the web endpoints.
    pub fn midi(&self) -> &Arc<crate::midi::MidiState> {
        &self.midi
    }

    /// Seconds since the process started, for the dashboard uptime readout.
    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Ask the main loop to shut down cleanly and then exit for good.
    /// Idempotent: a second call before the loop wakes just re-arms the same
    /// notification. Safe to call from any thread (tray) or task (web).
    pub fn request_quit(&self) {
        self.shutdown_kind.store(1, Ordering::SeqCst);
        self.shutdown_notify.notify_one();
    }

    /// Ask the main loop to shut down cleanly and then relaunch in place.
    pub fn request_restart(&self) {
        self.shutdown_kind.store(2, Ordering::SeqCst);
        self.shutdown_notify.notify_one();
    }

    /// Park until a quit/restart is requested, then report which. `notify_one`
    /// stores a permit if it fires before this is awaited, so the signal can
    /// never be lost to a race with the main loop entering its select.
    pub async fn wait_shutdown(&self) -> ShutdownKind {
        self.shutdown_notify.notified().await;
        match self.shutdown_kind.load(Ordering::SeqCst) {
            2 => ShutdownKind::Restart,
            _ => ShutdownKind::Quit,
        }
    }

    pub fn video_codec(&self) -> VideoCodec {
        dec_vcodec(self.video_codec.load(Ordering::Relaxed))
    }
    pub fn audio_codec(&self) -> AudioCodec {
        dec_acodec(self.audio_codec.load(Ordering::Relaxed))
    }
    /// Freshness-based - true only if a multi-track video tag was seen
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
    /// Decode primary-track dimensions from an AVC sequence header.
    ///
    /// Multi-track (Enhanced Broadcasting) headers are skipped: under EB
    /// it is Twitch that picks the encode ladder, so "your resolution is
    /// wrong" would be both wrong and unactionable. `sps_dimensions`
    /// already returns None for non-AVC codecs, so HEVC/AV1 simply leave
    /// the dimensions at 0 and the resolution check stays quiet.
    pub fn note_video_dimensions(&self, seq_header: &[u8]) {
        if crate::h264::seq_header_track_id(seq_header) != 0 {
            return;
        }
        if let Some((w, h)) = crate::h264::sps_dimensions(seq_header) {
            let packed = ((w as u64) << 32) | h as u64;
            let prev = self.video_dims.swap(packed, Ordering::Relaxed);
            if prev != packed {
                self.log(format!("ingest: video is {}x{}", w, h));
            }
        }
    }

    /// Feed one IDR timestamp into the keyframe-interval measurement.
    ///
    /// Stops sampling after `KEYFRAME_SAMPLE_GAPS` gaps so the reported
    /// interval is stable for the rest of the session - see the field
    /// comments for why a frozen value matters here.
    fn sample_keyframe_interval(&self, ts_ms: u64) {
        if self.idr_gaps.load(Ordering::Relaxed) >= KEYFRAME_SAMPLE_GAPS {
            return;
        }
        if !self.idr_window_open.swap(true, Ordering::Relaxed) {
            // First IDR of the session: it opens the window, it is not a gap.
            self.first_idr_ts_ms.store(ts_ms, Ordering::Relaxed);
            self.last_idr_ts_ms.store(ts_ms, Ordering::Relaxed);
            return;
        }
        // Guard against a non-advancing timestamp (a seek or a duplicated
        // tag) turning into a zero-width gap that drags the mean down.
        if ts_ms <= self.last_idr_ts_ms.load(Ordering::Relaxed) {
            return;
        }
        self.last_idr_ts_ms.store(ts_ms, Ordering::Relaxed);
        let gaps = self.idr_gaps.fetch_add(1, Ordering::Relaxed) + 1;
        // Republish the mean on the writer thread so readers load one atomic.
        // gaps >= 1 here, so the division can never be by zero.
        let first = self.first_idr_ts_ms.load(Ordering::Relaxed);
        let mean = (ts_ms.saturating_sub(first) / gaps as u64).min(u32::MAX as u64) as u32;
        self.keyframe_interval_cached.store(mean, Ordering::Relaxed);
    }

    /// Mean IDR spacing in ms, or 0 until at least one gap is measured.
    pub fn keyframe_interval_ms(&self) -> u32 {
        self.keyframe_interval_cached.load(Ordering::Relaxed)
    }

    /// Snapshot of the measured encoder parameters for `compat_warning`.
    pub fn stream_params(&self) -> StreamParams {
        let dims = self.video_dims.load(Ordering::Relaxed);
        StreamParams {
            width: (dims >> 32) as u32,
            height: dims as u32,
            keyframe_interval_ms: self.keyframe_interval_ms(),
            codec: self.video_codec(),
        }
    }

    /// Called from ingest on every multi-track video tag. Records a
    /// timestamp so the `multitrack_video()` getter can auto-clear when
    /// multi-track stops (e.g. the user switched Enhanced Broadcasting
    /// off mid-stream, or a single tag was misclassified). Edge-triggered
    /// log + webhook fire only on the first detection per session - the
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
                "Enhanced Broadcasting (multi-track video) detected - \
                 forwarding raw to Twitch destinations, flattening to the \
                 primary resolution for any other platform.",
            );
            self.fire_webhook(
                "🎚️",
                "Enhanced Broadcasting detected - multi-track forwarding active.",
            );
        }
    }
    pub fn note_multitrack_audio(&self) {
        if !self.multitrack_audio.swap(true, Ordering::Relaxed) {
            self.log("ingest: multi-track audio detected (VOD audio track) - forwarding as-is.");
        }
    }
    /// Wipe codec/multitrack state when the publisher disconnects so a
    /// fresh OBS connect with a different codec starts from a clean slate.
    /// Also resets the u32→u64 timestamp wrap counter - a new publisher
    /// may restart from ts=0, which from the old wrap counter's POV would
    /// look like a 49-day jump forward.
    pub fn reset_codec_state(&self) {
        self.video_codec.store(0, Ordering::Relaxed);
        self.audio_codec.store(0, Ordering::Relaxed);
        // Measured encoder params belong to the session that produced
        // them - a reconnect may bring a different OBS profile entirely,
        // and a stale interval would warn about settings nobody is using.
        self.video_dims.store(0, Ordering::Relaxed);
        self.idr_window_open.store(false, Ordering::Relaxed);
        self.first_idr_ts_ms.store(0, Ordering::Relaxed);
        self.last_idr_ts_ms.store(0, Ordering::Relaxed);
        self.idr_gaps.store(0, Ordering::Relaxed);
        self.keyframe_interval_cached.store(0, Ordering::Relaxed);
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
    /// publisher invariant (only one OBS may publish at a time - see
    /// `begin_publish`) means there's only one caller of `on_tag` /
    /// `expand_ts` at any moment, so the relaxed atomic load + store
    /// is race-free in practice.
    ///
    /// Wrap detection rule: if the new u32 ts is less than the previous
    /// by more than 2^31 ms (~24.8 days), the counter wrapped around;
    /// bump the high half. Smaller backward jumps are treated as the
    /// (normal) inter-stream out-of-order audio interleaving and ignored
    /// here - pace_and_send drops those separately.
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
        if let Some(s) = self.destinations.read().get(id) {
            return s.clone();
        }
        let mut map = self.destinations.write();
        map.entry(id.to_string())
            .or_insert_with(|| Arc::new(DestinationState::new(id.to_string())))
            .clone()
    }

    /// Drop a destination's state - call when the user removes it.
    pub fn remove_destination_state(&self, id: &str) {
        self.destinations.write().remove(id);
    }

    /// Snapshot of every (id → state) pair. Used by graceful-shutdown
    /// paths that need to flip flags on every pump in one pass.
    pub fn all_destination_states(&self) -> Vec<(String, Arc<DestinationState>)> {
        self.destinations
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Snapshot for the dashboard: (id, alive, consumer_seq, kbps_out, tags, bytes, cuts, reconnects).
    pub fn destination_snapshot(&self) -> Vec<DestinationSnapshot> {
        let map = self.destinations.read();
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

    /// All destinations alive flag (any-of) - for the topbar pill.
    pub fn any_destination_alive(&self) -> bool {
        self.destinations
            .read()
            .values()
            .any(|d| d.egress_alive.load(Ordering::Relaxed))
    }

    /// (alive_count, total_count) - for "2/3 destinations live" chips.
    pub fn destination_alive_summary(&self) -> (u32, u32) {
        let map = self.destinations.read();
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
    /// delay updates the target too, and the controller holds position
    /// until the ring has enough history, then performs ONE IDR-aligned
    /// jump back to it (there is no gradual rewind - see
    /// `compute_delay_cut`'s build-buffer-first branch).
    pub fn arm_delay(&self, ms: u32) {
        let ms = ms.min(600_000);
        let previous_target = self.target_delay_ms.load(Ordering::Relaxed);
        self.armed_delay_ms.store(ms, Ordering::Relaxed);
        if ms == 0 {
            // Disarm wipes target as well.
            self.target_delay_ms.store(0, Ordering::Relaxed);
            // Disarm clears any pending auto-activate too - there's
            // nothing to auto-activate into anymore.
            self.auto_activate_pending.store(false, Ordering::Relaxed);
            // A pending "cut after this airs" mark dies with the delay
            // it was going to cut.
            self.safe_cut_input_ts.store(0, Ordering::Relaxed);
        } else if previous_target > 0 {
            // Already active → live-update what we're delivering. This
            // is NOT a fresh arm action; the streamer is mid-stream and
            // adjusting the delay value. Leave auto_activate_pending
            // alone so a subsequent cut doesn't snap back to active via
            // auto-activate-when-ready.
            self.target_delay_ms.store(ms, Ordering::Relaxed);
        } else {
            // Fresh arm: target was 0 (we were disarmed, in cut-hold,
            // or in passthrough). Mark the auto-activate slot eligible
            // - one shot when the buffer hits ready.
            self.auto_activate_pending.store(true, Ordering::Relaxed);
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
        // Successful activate consumes the pending slot. Both manual
        // and auto-activate share this path; either way, the slot is
        // used up and won't re-fire until the next arm event refills
        // it. Matters for auto-activate: after Cut, target drops to 0
        // and phase reverts to "ready", which would otherwise look
        // like a fresh "*->ready" transition to the supervisor.
        self.auto_activate_pending.store(false, Ordering::Relaxed);
        Ok(armed)
    }

    /// Drop back to live but *keep the armed delay* - buffer continues
    /// to fill, so the next activate is instant. This is the magic
    /// behavior the streamer described.
    pub fn stop_delay(&self) {
        self.target_delay_ms.store(0, Ordering::Relaxed);
        // Cut consumes the pending slot. The streamer made a
        // deliberate "go live without delay" decision; auto-activate
        // mustn't override it. The slot stays consumed until the next
        // arm event (re-arm at a non-zero value with target = 0)
        // refills it.
        self.auto_activate_pending.store(false, Ordering::Relaxed);
        // A manual cut supersedes any scheduled "cut after this airs" -
        // the streamer chose "now" over "when the mark airs".
        self.safe_cut_input_ts.store(0, Ordering::Relaxed);
    }

    // --- "Cut after this airs" (scheduled safe cut) -------------------
    //
    // The competitive-streamer workflow: a match ends on a 30 s delay,
    // the streamer reacts, and the moment the reaction is over they
    // want to snap back to live WITHOUT clipping the reaction off the
    // delayed output - which today means counting the delay in their
    // head. Instead they press one button at the safe moment; we record
    // the live-edge input timestamp and fire the normal cut machinery
    // once the slowest destination has aired past it.

    /// Schedule a cut for the moment the CURRENT live edge has aired on
    /// every destination. Returns the estimated wait (ms) for the UI
    /// countdown. Only meaningful while a delay is active - refuses
    /// otherwise so the button can't arm a mark that fires surprisingly
    /// on some future activate.
    pub fn schedule_safe_cut(&self) -> Result<u32, &'static str> {
        if self.target_delay_ms.load(Ordering::Relaxed) == 0 {
            return Err("no delay is active - nothing to schedule");
        }
        let Some(latest) = self.ring.latest_ts() else {
            return Err("no stream data yet");
        };
        // 0 is the "none pending" sentinel; a genuine ts of 0 (first
        // tag of a session) shifts by 1 ms, which is far below the
        // IDR-quantisation the cut lands on anyway.
        self.safe_cut_input_ts
            .store(latest.max(1), Ordering::Relaxed);
        Ok(self.safe_cut_remaining_ms())
    }

    /// Drop a pending scheduled cut. No-op if none is pending.
    pub fn cancel_safe_cut(&self) {
        self.safe_cut_input_ts.store(0, Ordering::Relaxed);
    }

    /// Run a named delay action. Shared by the keyboard hotkeys and MIDI
    /// bindings so both trigger identical behaviour. `default_ms` is the
    /// delay armed when starting from nothing; `source` tags the log line
    /// ("hotkey" / "midi"). Unknown action names are ignored. Every call
    /// here is atomic-only, so it is safe to invoke straight from an OS
    /// callback thread with no runtime.
    ///
    /// Windows-only today: both callers (the tray hotkeys and the winmm MIDI
    /// listener) are Windows-only. On other platforms the delay is driven
    /// through the HTTP endpoints instead.
    #[cfg(windows)]
    pub fn run_named_action(&self, action: &str, default_ms: u32, source: &str) {
        match action {
            // Toggle: cut to live when a delay is live (or a safe-cut is
            // pending), otherwise arm at the default delay and go delayed.
            "toggle" => {
                if self.target_delay_ms() > 0 || self.safe_cut_pending() {
                    self.stop_delay();
                    self.log(format!("[{source}] delay off - cut to live"));
                } else {
                    let ms = if self.armed_delay_ms() > 0 {
                        self.armed_delay_ms()
                    } else {
                        default_ms
                    };
                    self.arm_delay(ms);
                    match self.activate_delay() {
                        Ok(d) => self.log(format!("[{source}] delay on - {} s", d / 1000)),
                        Err(_) => self.log(format!(
                            "[{source}] delay arming {} s - goes live once the buffer fills",
                            ms / 1000
                        )),
                    }
                }
            }
            "arm" => {
                self.arm_delay(default_ms);
                self.log(format!("[{source}] armed {} s", default_ms / 1000));
            }
            "activate" => match self.activate_delay() {
                Ok(d) => self.log(format!("[{source}] activated - {} s delay", d / 1000)),
                Err(e) => self.log(format!("[{source}] activate: {}", e.message())),
            },
            "cut" => {
                self.stop_delay();
                self.log(format!("[{source}] cut to live"));
            }
            // Toggle: first press schedules the safe cut, a second cancels
            // the pending one, so a mistaken press stays reversible.
            "cut_after" => {
                if self.safe_cut_pending() {
                    self.cancel_safe_cut();
                    self.log(format!("[{source}] cut after this airs - cancelled"));
                } else {
                    match self.schedule_safe_cut() {
                        Ok(_) => self.log(format!("[{source}] cut after this airs - scheduled")),
                        Err(e) => self.log(format!("[{source}] cut after: {e}")),
                    }
                }
            }
            _ => {}
        }
    }

    pub fn safe_cut_pending(&self) -> bool {
        self.safe_cut_input_ts.load(Ordering::Relaxed) != 0
    }

    /// How long until the pending mark has aired, for the dashboard
    /// countdown. 0 when nothing is pending. When no destination is
    /// live to measure against, fall back to the target delay - the
    /// honest "roughly this long" number the streamer armed.
    pub fn safe_cut_remaining_ms(&self) -> u32 {
        let mark = self.safe_cut_input_ts.load(Ordering::Relaxed);
        if mark == 0 {
            return 0;
        }
        match self.slowest_live_consumer_ts() {
            Some(ts) => mark.saturating_sub(ts).min(u32::MAX as u64) as u32,
            None => self.target_delay_ms.load(Ordering::Relaxed),
        }
    }

    /// Called from each pump's throttled cut-check. If a mark is pending
    /// and the SLOWEST live destination has aired past it, fire the
    /// normal cut (stop_delay → compute_delay_cut sees target 0 on the
    /// same pump iteration and seeks to live). Gating on the slowest
    /// consumer is what makes the promise hold per-destination: a
    /// faster pump must not cut a slower one short of the mark. The
    /// compare_exchange makes exactly one pump the firing pump, so the
    /// log line and the stop_delay don't multiply across destinations.
    pub fn maybe_fire_safe_cut(&self) {
        let mark = self.safe_cut_input_ts.load(Ordering::Relaxed);
        if mark == 0 {
            return;
        }
        let Some(consumer_ts) = self.slowest_live_consumer_ts() else {
            return;
        };
        // Strictly greater: the next-to-send tag being AT the mark
        // means the mark's own frame hasn't gone out yet.
        if consumer_ts <= mark {
            return;
        }
        if self
            .safe_cut_input_ts
            .compare_exchange(mark, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            self.stop_delay();
            self.log("scheduled cut: marked moment has aired everywhere - cutting to live");
        }
    }

    /// Input-timeline position of the slowest live destination: the ts
    /// of the next tag it will send. `None` when no destination is
    /// alive, or the cursor fell behind the ring front (transient -
    /// next_or_wait realigns it). A cursor PAST the newest tag means
    /// fully caught up, which reads as the live edge.
    fn slowest_live_consumer_ts(&self) -> Option<u64> {
        let min_consumer = {
            let map = self.destinations.read();
            map.values()
                .filter(|d| d.egress_alive.load(Ordering::Relaxed))
                .map(|d| d.consumer_seq.load(Ordering::Relaxed))
                .min()
        }?;
        if let Some((_, m)) = self.ring.find_by_seq(min_consumer) {
            return Some(m.ts_ms);
        }
        match self.ring.latest_seq() {
            Some(latest_seq) if min_consumer > latest_seq => self.ring.latest_ts(),
            _ => None,
        }
    }

    /// Snapshot read of the auto-activate-pending slot. Used by the
    /// supervisor to gate the auto-activate-when-ready behaviour.
    pub fn auto_activate_pending(&self) -> bool {
        self.auto_activate_pending.load(Ordering::Relaxed)
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
    /// every loop iteration - producing visible UI wobble. Falls back
    /// to 0 when nothing is being sent.
    pub fn current_delay_ms(&self) -> u32 {
        let Some(latest) = self.ring.latest_ts() else {
            return 0;
        };
        match self.slowest_live_consumer_ts() {
            // Clamp to u32: a u64 delta can't realistically exceed
            // 600_000 ms (our hard armed-delay ceiling) but we cap to
            // be safe - the UI consumes a u32 number anyway.
            Some(ts) => latest.saturating_sub(ts).min(u32::MAX as u64) as u32,
            None => 0,
        }
    }

    /// Convenience for the dashboard - collapses the (armed, target, fill)
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
    /// re-anchors its output timeline if the token changed - without this,
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

    // Aggregated stats - summed across all destinations for the
    // dashboard's top-level metric cards.
    pub fn tags_sent(&self) -> u64 {
        self.destinations
            .read()
            .values()
            .map(|d| d.tags_sent.load(Ordering::Relaxed))
            .sum()
    }
    pub fn bytes_sent(&self) -> u64 {
        self.destinations
            .read()
            .values()
            .map(|d| d.bytes_sent.load(Ordering::Relaxed))
            .sum()
    }
    pub fn cuts_performed(&self) -> u32 {
        self.destinations
            .read()
            .values()
            .map(|d| d.cuts_performed.load(Ordering::Relaxed))
            .sum()
    }
    pub fn egress_reconnects(&self) -> u32 {
        self.destinations
            .read()
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
    /// 1-second rolling bitrate average (kbps) - cheap, lock-free.
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
    /// other - invaluable for diagnosing "the bouncing happened around
    /// 30 seconds in".
    pub fn log(&self, line: impl Into<String>) {
        let mut q = self.logs.lock();
        if q.len() >= 1500 {
            q.pop_front();
        }
        let ts_s = process_now_ms() as f64 / 1000.0;
        q.push_back(format!("[+{:>8.3}s] {}", ts_s, line.into()));
    }

    pub fn clear_logs(&self) {
        self.logs.lock().clear();
    }

    // ---- Ingest entry points (called from rtmp::server) ----

    pub async fn begin_publish(&self, stream_key: &str, peer_ip: &str) -> io::Result<u64> {
        // OBS's multitrack / Enhanced Broadcasting output appends query params to
        // the stream key before publishing: always `?clientConfigId=<id>`, plus
        // any query the user typed into their Stream Key field (e.g.
        // `?bandwidthtest=1`). See create_service() in OBS's
        // MultitrackVideoOutput.cpp. An RTMP playpath query is not part of the
        // stream-key identity, so strip it here - otherwise the exact ingest key
        // arrives as `mykey?clientConfigId=...` and gets rejected as a wrong key.
        let stream_key = stream_key.split('?').next().unwrap_or(stream_key);
        // Ingest auth: when a key is configured, only a publisher using that
        // exact key gets in. Empty key (the default) accepts anyone, which is
        // the right behaviour on a local machine. Checked before the slot lock
        // so a wrong key never even contends for the publisher slot.
        {
            let required = self.ingest_key.lock().clone();
            if !required.is_empty() {
                // The wrong-key throttle defends a network-exposed ingest port.
                // A loopback publisher is a local process - RTMP ingest is never
                // behind an HTTP reverse proxy that could mask its IP - so it is
                // no brute-force threat and must not be locked out of its own
                // machine for a mistyped key. Apply the limiter to remote peers
                // only; an unparseable IP is treated as remote (fail-safe).
                let remote = !peer_ip
                    .parse::<std::net::IpAddr>()
                    .map(|a| a.is_loopback())
                    .unwrap_or(false);
                // Throttle first so a locked-out guesser burns no work.
                if remote && self.ingest_limiter.check(peer_ip).is_err() {
                    self.log("ingest: rejected publisher (rate limited)");
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "too many attempts",
                    ));
                }
                // Accept the configured ingest key (constant-time compare, same
                // as the dashboard password / dock token: a plain `!=` early
                // exits on the first differing byte and leaks the match length)
                // OR a stream key from an Enhanced Broadcasting session we
                // brokered. In EB, OBS publishes with the Twitch session token,
                // not the ingest key; we handed that token out via
                // /obs/multitrack-config, so it is trusted.
                let accepted =
                    crate::crypto::constant_time_eq(stream_key.as_bytes(), required.as_bytes())
                        || self.is_brokered_eb_key(stream_key);
                if !accepted {
                    if remote {
                        self.ingest_limiter.record_failure(peer_ip);
                    }
                    self.log("ingest: rejected publisher (wrong stream key)");
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "invalid stream key",
                    ));
                }
                if remote {
                    self.ingest_limiter.record_success(peer_ip);
                }
            }
        }
        let _g = self.publish_lock.lock().await;
        // One publisher at a time - a second OBS connecting would
        // interleave its tags into the buffer with its own timestamp
        // origin and guarantee a viewer-visible glitch.
        if self.ingest_alive.load(Ordering::Relaxed) {
            self.log("ingest: rejected second publisher (slot in use)");
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "another publisher is already active",
            ));
        }

        // Wipe every per-session cache that survives mark_ingest_dead.
        // The seq-header caches deliberately persist across an in-session
        // egress restart (see `eb_seq_headers_survive_egress_restart`) so
        // we clear them only when a new publisher takes the slot -
        // otherwise stale multi-track SPS/PPS leak into a non-EB
        // session and freeze the destination's decoder.
        self.ring.video_seq_headers.lock().clear();
        self.ring.audio_seq_headers.lock().clear();
        // onMetaData leaks the same way: the prior publisher's
        // resolution / fps / encoder fields would be replayed at every
        // pump start until the new publisher's first onMetaData arrives.
        *self.ring.metadata.lock() = None;
        // The ring's indexed tags also have to go. OBS's RTMP wire
        // timestamps restart from ~0 on every fresh stream session
        // (Start Streaming -> Stop -> Start), but the ring still holds
        // the prior session's tags at much higher ts_ms values. Leaving
        // them in place makes oldest_ts() return the stale front and
        // latest_ts() the fresh back, so buffer_fill_ms saturates to 0
        // forever in the new session - the delay bar never fills.
        // trim_older_than cannot recover from this either: its cutoff
        // also saturates to 0 against the new session's low current_ts.
        self.ring.clear();

        // Defensive: clear any stale auto-activate slot. A fresh
        // publisher session is a clean slate; the prior session's
        // arm-or-not state shouldn't leak into the new one.
        self.auto_activate_pending.store(false, Ordering::Relaxed);
        // Same for a pending "cut after this airs" mark: it's an input
        // timestamp on the OLD session's timeline. The new session
        // restarts near ts 0, so the mark would sit unreachable and
        // fire hours later (or never) - clear it with the session.
        self.safe_cut_input_ts.store(0, Ordering::Relaxed);

        // Bump token so any prior egress reader knows it's stale.
        let token = self.publisher_token.fetch_add(1, Ordering::SeqCst) + 1;
        self.ingest_alive.store(true, Ordering::Relaxed);
        self.log("ingest: publisher connected");
        self.fire_webhook("✅", "OBS publisher connected - going live.");
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
        // Sample the encoder parameters the compatibility check compares
        // against the enabled destinations. Both are cheap: the dimension
        // decode runs once per sequence header, and the keyframe sampler
        // stops touching anything after its first few gaps.
        if kind == 9 {
            if is_seq {
                self.note_video_dimensions(payload);
            } else if is_idr {
                self.sample_keyframe_interval(ts_ms);
            }
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
        let map = self.destinations.read();
        map.values()
            .map(|d| d.consumer_seq.load(Ordering::Relaxed))
            .min()
            .unwrap_or(u64::MAX)
    }

    /// How many tags behind the latest the slowest consumer is. Kept
    /// for diagnostics - but DO NOT use this directly to flag
    /// backpressure: on any active delay the consumer is intentionally
    /// behind (5 s × ~80 tags/s ≈ 400 tags), so any naive threshold
    /// generates false positives. Use `is_backpressured` instead.
    pub fn max_consumer_lag(&self) -> u64 {
        let Some(latest) = self.ring.latest_seq() else {
            return 0;
        };
        let min_consumer = {
            let map = self.destinations.read();
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

    /// True if egress can't keep up with ingest - i.e. the actual
    /// delivered delay is materially larger than the user asked for.
    ///
    /// Definition: `current_delay − target_delay > 2 s` (sustained).
    /// This is timestamp-based, so a healthy 5 s delay reads as
    /// "0 over" (no backpressure) - unlike the tag-count metric, which
    /// would always read ~400 tags behind on a 5 s delay regardless of
    /// stream health.
    ///
    /// Caller has to suppress during the cut-transition window itself
    /// or it briefly flips on every toggle. We use a sustained-condition
    /// check via `backpressure_since_ms`.
    pub fn is_backpressured(&self) -> bool {
        // Skip the check entirely if there's no live destination - the
        // signal is meaningless when nothing is being sent.
        let any_alive = {
            let map = self.destinations.read();
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
        // minimum when nothing is armed - gives compute_delay_cut at
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
        *self.ring.metadata.lock() = Some(payload);
    }

    pub fn mark_ingest_dead(&self) {
        // Only count when transitioning alive → dead, so a stray call
        // doesn't inflate the counter.
        if self.ingest_alive.swap(false, Ordering::Relaxed) {
            self.note_ingest_disconnect();
            self.reset_codec_state();
            // Clear any Enhanced Broadcasting URL overrides on the
            // way out - the next stream may or may not be EB, and a
            // stale override would force a non-EB stream onto an IVS
            // endpoint that has no allocated session. The
            // /obs/multitrack-config proxy sets a fresh override on
            // every new EB session anyway.
            for (_id, state) in self.all_destination_states() {
                // Clear the override AND bump the session epoch atomically,
                // so a VOD-session fetch still in flight discards its stale
                // result instead of writing a dead-session IVS URL into the
                // next stream (the late-completion race).
                state.invalidate_session_override();
                // Note: we deliberately do NOT touch `vod_fetch_pending`
                // here. It's owned solely by the fetch lifecycle (claim sets
                // it, the task clears it on completion within ~15 s). If a
                // fetch is in flight across a disconnect/reconnect, leaving
                // the latch set is what stops a second concurrent fetch from
                // being spawned for the same destination - clearing it here
                // would reintroduce the multi-session bug on a fast restart.
            }
            self.log("ingest: publisher disconnected");
            self.fire_webhook("⚠️", "OBS publisher disconnected.");
        }
    }

    /// Update the Discord webhook URL - call when settings change. Empty
    /// string disables webhook delivery entirely.
    pub fn update_webhook(&self, url: String) {
        *self.webhook_url.lock() = url;
    }

    /// Mirror the required ingest stream key from Settings. Empty disables the
    /// check (any key accepted). Called on startup and on every settings edit.
    pub fn update_ingest_key(&self, key: String) {
        *self.ingest_key.lock() = key;
    }

    /// Remember a stream key from an Enhanced Broadcasting session we just
    /// brokered - the Twitch session token OBS will publish with. `begin_publish`
    /// accepts these alongside the configured ingest key, so an EB publish is not
    /// rejected as a "wrong stream key" when an ingest key is set. Bounded and
    /// TTL'd so tokens do not accumulate; only ever populated while EB is in use.
    pub fn remember_eb_key(&self, key: String) {
        if key.is_empty() {
            return;
        }
        let now = Instant::now();
        let ttl = Duration::from_secs(600);
        let mut v = self.eb_keys.lock();
        // Drop expired entries and any prior copy of this key, then re-add it.
        v.retain(|(k, t)| k != &key && now.duration_since(*t) < ttl);
        v.push((key, now));
        const MAX_EB_KEYS: usize = 8;
        if v.len() > MAX_EB_KEYS {
            let drop = v.len() - MAX_EB_KEYS;
            v.drain(0..drop);
        }
    }

    /// True if `key` is a still-valid EB session token we brokered. Constant-time
    /// compare per candidate, matching the ingest-key / password / dock-token
    /// paths (a plain `==` leaks the match length to a timing attacker).
    fn is_brokered_eb_key(&self, key: &str) -> bool {
        let now = Instant::now();
        let ttl = Duration::from_secs(600);
        self.eb_keys.lock().iter().any(|(k, t)| {
            now.duration_since(*t) < ttl
                && crate::crypto::constant_time_eq(k.as_bytes(), key.as_bytes())
        })
    }

    /// Snapshot the current webhook URL. Used by the test endpoint so it
    /// can route the request with verbose error reporting instead of
    /// going through `fire_webhook` (which is fire-and-forget and
    /// silently swallows everything from empty-URL to TLS failures).
    pub fn webhook_url_snapshot(&self) -> String {
        self.webhook_url.lock().clone()
    }

    /// Fire-and-forget Discord post. Skips silently when no webhook is
    /// configured, OR when the last fire was less than 2 s ago (rate
    /// limit - prevents subprocess spam if a destination flaps).
    ///
    /// Uses `ureq` (tiny blocking HTTPS client, ~150 KB) wrapped in
    /// `spawn_blocking` so the actual TCP+TLS work doesn't park the
    /// current-thread runtime. Previously shelled out to `curl`, which
    /// (a) silently failed when `curl.exe` wasn't on PATH and (b) made
    /// "runtime deps" technically include the system curl binary.
    pub fn fire_webhook(&self, emoji: &str, message: &str) {
        let url = self.webhook_url.lock().clone();
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
                    let _ = crate::https::https_agent()
                        .post(&url)
                        .config()
                        .timeout_connect(Some(Duration::from_secs(5)))
                        .timeout_global(Some(Duration::from_secs(8)))
                        .build()
                        .header("Content-Type", "application/json")
                        .send(&body);
                }),
            )
            .await;
        });
    }
}

/// JSON-string escape that handles every C0 control char that would
/// otherwise produce an invalid Discord payload (the previous
/// `replace('\\', ..).replace('"', ..).replace('\n', ..)` chain missed
/// `\r`, `\t`, `\u{0008}` and friends - any destination name with a
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
// Egress driver - the timing & cut-alignment core.
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
                "[{}] invalid URL ({}) - fix it in Settings",
                label, e
            ));
            tokio::time::sleep(Duration::from_secs(3600)).await;
            return Ok(());
        }
    };

    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(30);

    loop {
        // Cooperative shutdown - check BEFORE attempting another
        // connect. Without this, a destination the user just disabled
        // (or with a permanently failing endpoint) would spin forever
        // in the connect-retry loop, because pump_dest is only reached
        // on a SUCCESSFUL connect. Symptom: log spam like
        //   "[egress Twitch] connecting to live.twitch.tv:1935"
        //   "[egress Twitch] connect failed: early eof (next try in 30s)"
        // continuing even after the destination is toggled off.
        if dest.shutdown_requested.load(Ordering::Relaxed) {
            ctrl.log(format!(
                "[{}] shutdown requested - egress loop exiting",
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
                    // Scrub before logging or webhooking - otherwise the
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
        // Cancellable backoff sleep - wake every 200 ms to check the
        // shutdown flag so a disable doesn't have to wait the full
        // 30 s backoff window before stopping.
        let deadline = tokio::time::Instant::now() + backoff;
        loop {
            if dest.shutdown_requested.load(Ordering::Relaxed) {
                ctrl.log(format!(
                    "[{}] shutdown requested during backoff - exiting",
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
/// a stream key - defensive against secrets we don't know about.
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
/// timeout will close the session naturally - which is the standard,
/// predictable failure mode and lets viewers see the real "stream
/// offline" UI instead of a confusing freeze-frame. No filler-frame
/// replay (it created its own desync bugs and added memory pressure
/// for negligible benefit).
async fn pump_dest(
    ctrl: &Arc<Controller>,
    dest: &Arc<DestinationState>,
    mut sink: EgressSink,
) -> io::Result<()> {
    let meta = ctrl.ring.metadata.lock().clone();
    if let Some(meta) = meta {
        let _ = sink.send_metadata(&meta).await;
    }

    let mut state = EgressState::new();
    state.last_publisher_token = ctrl.publisher_token();
    let mut io_buf: Vec<u8> = Vec::with_capacity(64 * 1024);

    // Initial seed: if a delay is ALREADY active when this pump spawns
    // (multi-destination case - a second destination added mid-stream
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
        // for long sessions - without it, the server eventually
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
                "[{}] ingest gone - closing destination session",
                dest.id
            ));
            let _ = sink.send_delete_stream().await;
            return Ok(());
        }

        // Detect publisher reconnect (OBS stopped and re-started, new
        // session token). Without this branch the new session's "fresh"
        // timestamps would all read earlier than `input_ts_anchor` and
        // pace_and_send's monotonic guard would silently drop every
        // tag - the upstream stream would freeze forever even though
        // ingest is happily receiving bytes.
        let current_token = ctrl.publisher_token();
        if current_token != state.last_publisher_token {
            ctrl.log(format!("[{}] publisher reconnect - re-anchoring", dest.id));
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
            ctrl.log(format!("[{}] sequence header changed - resending", dest.id));
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
                // Scheduled "cut after this airs": if the slowest live
                // destination has aired past the mark, this flips target
                // to 0 - and compute_delay_cut below reads the atomic
                // fresh, so the cut lands on this same iteration.
                ctrl.maybe_fire_safe_cut();
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
    last_sent_input_ts: u64, // input ts of the last tag we actually emitted -
    // required so apply_cut can re-anchor the
    // output timeline *after* the last sent frame
    // (instead of after the last cut, which would
    // produce a monotonic-violating backward jump).
    /// Snapshot of `Controller::publisher_token()` at the last seed.
    /// When the controller bumps this (new OBS publish session), the
    /// pump re-anchors - otherwise the new publisher's reset timestamps
    /// would all fail pace_and_send's "older than anchor" check and the
    /// upstream player would never see another frame.
    last_publisher_token: u64,
    // --- cut-check throttling ---
    last_cut_check: Instant,
    last_seen_target: u32,
    /// Vertical egress only: after a (re)seed we may be pointed mid-GOP of
    /// the vertical canvas (the cut/seed index is built from the HORIZONTAL
    /// primary's keyframes). Emitting the vertical canvas's P-frames before
    /// its first IDR makes strict ingests (YouTube) drop the stream ~10 s in,
    /// waiting for a keyframe our GOP never leads with. While this is set we
    /// hold vertical video until its first IDR, then stream normally.
    awaiting_keyframe: bool,
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
            awaiting_keyframe: true,
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
    // of video (one P-frame) - imperceptible compared to the glitch.
    //
    // With u64 ts (set by expand_ts on ingest) the comparison is now
    // direct - no wrapping_sub / signed-int dance needed.
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
    // huge - but the check is essentially free, so we always do it).
    // try_read_seq holds the index lock for the disk read, so the bytes
    // are guaranteed to still be the bytes of this tag - or it returns
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
    // For video we log every tag - at ~30 fps × bytes/line the file
    // grows ~3 MB / 10 min, which is the right trade for diagnosing a
    // wire-format bug.
    match meta.kind {
        8 => {
            // A vertical destination whose 9:16 canvas isn't on the wire yet
            // (Dual Format off) has `video_egress() == None`. Drop its AUDIO
            // too - otherwise we'd feed the platform an audio-only stream
            // with no video, which reads as a broken/black broadcast. It
            // should send nothing until the canvas appears.
            if dest.video_egress().is_none() {
                state.consumer_seq = meta.seq + 1;
                dest.consumer_seq
                    .store(state.consumer_seq, Ordering::Relaxed);
                return Ok(());
            }
            // Mirror the per-destination video selection. Twitch
            // destinations get multi-track audio passthrough (VOD-audio
            // session); every other destination keeps a single track
            // (`audio_egress`), flattened, so a simulcast YouTube / Kick
            // gets exactly one audio track it can decode. Single-track
            // audio borrows through unchanged.
            let Some(selected) = crate::h264::select_audio_bytes(io_buf, dest.audio_egress())
            else {
                state.consumer_seq = meta.seq + 1;
                dest.consumer_seq
                    .store(state.consumer_seq, Ordering::Relaxed);
                return Ok(());
            };
            let bytes_out: &[u8] = &selected;
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
                        bytes_out.len(),
                        bytes_out.first().copied().unwrap_or(0),
                    ),
                );
            }
            sink.send_audio(out_ts, bytes_out).await?;
        }
        9 => {
            // Per-destination video-tag selection. Twitch
            // destinations pass multi-track through bit-faithfully
            // (Enhanced Broadcasting); every other RTMP ingest gets
            // single-track tags (legacy AVC / Enhanced single-track)
            // unchanged plus a *filtered* view of any multi-track
            // simulcast: OneTrack TrackId != 0 tags are dropped to
            // avoid the multi-frame-per-PTS storm that crashes
            // YouTube's decoder. See `select_video_bytes` for the
            // full rationale. Single-track tags borrow `io_buf`. A
            // vertical destination with no resolved canvas yet
            // (`video_egress` returns None) drops all video and waits.
            let egress = dest.video_egress();
            let dropped = match egress {
                Some(e) => crate::h264::select_video_bytes(io_buf, e),
                None => None,
            };
            // Vertical keyframe-lead: after a (re)seed on the horizontal IDR
            // index, hold this vertical canvas's P-frames until its first
            // IDR, so YouTube et al. always get a keyframe-led stream and
            // don't drop the socket ~10 s in. Only vertical tracks (t != 0)
            // gate; the horizontal primary already seeds on its own IDR.
            if state.awaiting_keyframe {
                match egress {
                    // Vertical canvas: hold its P-frames until the first IDR.
                    Some(crate::h264::VideoEgress::Track(t)) if t != 0 => {
                        if dropped.is_some() {
                            // `meta.is_idr` is the any-track classification
                            // (set from classify_video_tag on ingest), which
                            // is exactly what we need here - it's true for the
                            // vertical track's own IDR, not just track 0.
                            if meta.is_idr {
                                state.awaiting_keyframe = false;
                            } else {
                                state.consumer_seq = meta.seq + 1;
                                dest.consumer_seq
                                    .store(state.consumer_seq, Ordering::Relaxed);
                                return Ok(());
                            }
                        }
                        // Not our track: falls through, dropped below.
                    }
                    // Horizontal (seeds on its own IDR) or Twitch passthrough:
                    // nothing to hold.
                    Some(_) => state.awaiting_keyframe = false,
                    // Vertical canvas not resolved yet: keep waiting; the
                    // video is dropped below regardless.
                    None => {}
                }
            }
            let Some(selected) = dropped else {
                // Multi-track ladder tag deliberately dropped; advance
                // the consumer cursor so we don't replay it next call
                // but skip every per-tag side-effect (send, byte
                // accounting, last_sent_input_ts update).
                state.consumer_seq = meta.seq + 1;
                dest.consumer_seq
                    .store(state.consumer_seq, Ordering::Relaxed);
                return Ok(());
            };
            let bytes_out: &[u8] = &selected;
            // Hottest path in the whole binary - ~300 events/s on a
            // 5-rung EB stream × 2 destinations. Skip the format!
            // entirely when tracing is disabled (the default).
            if crate::trace::is_enabled() {
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
            }
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

/// Baseline re-cut dead band, tuned for OBS's default 2 s keyframe interval.
const RECUT_DEAD_BAND_FLOOR_MS: u64 = 1_500;
/// Baseline IDR-search tolerance, enough to always find a keyframe at a 2 s
/// (or tighter) cadence.
const IDR_SEARCH_FLOOR_MS: u32 = 2_000;

/// Re-cut hysteresis as a function of the *measured* keyframe interval.
///
/// The dead band must exceed `IDR_cadence / 2 + send_jitter`, or once we are
/// already parked on the best available IDR the delay error (up to half a GOP)
/// keeps re-tripping the gate and we re-cut to the same keyframe every tick -
/// the "repeating 1-2 s of content" bounce. At OBS's default 2 s GOP the
/// tuned floor of 1500 ms covers this. A long-GOP encoder (3-4 s) has a
/// larger half-GOP error, so the band has to widen with it or the exact same
/// bounce returns - just triggered by keyframe interval instead of dead-band
/// size. `keyframe_interval_ms == 0` (not yet measured) keeps the floor, so a
/// fresh stream behaves identically until it reveals a cadence.
fn recut_dead_band_ms(keyframe_interval_ms: u32) -> u64 {
    RECUT_DEAD_BAND_FLOOR_MS.max(keyframe_interval_ms as u64 / 2 + 500)
}

/// How far from the ideal input timestamp we accept an IDR when cutting.
/// Grows to half a GOP for long-keyframe streams so a cut can still land on a
/// real keyframe; never below the 2 s that served the default cadence.
fn idr_search_tolerance_ms(keyframe_interval_ms: u32) -> u32 {
    IDR_SEARCH_FLOOR_MS.max(keyframe_interval_ms / 2)
}

fn compute_delay_cut(ctrl: &Arc<Controller>, current: &TagMeta) -> Option<PendingCut> {
    let target_delay = ctrl.target_delay_ms.load(Ordering::Relaxed) as u64;
    let latest = ctrl.ring.latest_ts()?;
    let oldest = ctrl.ring.oldest_ts()?;
    let current_delay = latest.saturating_sub(current.ts_ms);

    // Dead band scales with the measured GOP (see recut_dead_band_ms). At the
    // default 2 s cadence this is the tuned 1500 ms; the trade-off is the
    // delivered delay may sit up to one dead-band away from the requested
    // value, which is exactly the price of not re-cutting to the same IDR.
    let keyframe_interval = ctrl.keyframe_interval_ms();
    let dead_band = recut_dead_band_ms(keyframe_interval);
    let diff = (current_delay as i64) - (target_delay as i64);
    if diff.abs() < dead_band as i64 {
        ctrl.buffer_building.store(false, Ordering::Relaxed);
        return None;
    }

    // "Build buffer first" - if the user asked for a delay deeper than
    // the buffer currently extends, we can't honor it yet. Mark the state
    // and hold our position; the buffer keeps filling at real time, and
    // once the requested delay becomes reachable, the next iteration cuts.
    // Same dead band as the re-cut gate so the two agree on "close enough".
    let have_seconds_back = latest.saturating_sub(oldest);
    if target_delay > have_seconds_back.saturating_add(dead_band) {
        ctrl.buffer_building.store(true, Ordering::Relaxed);
        return None;
    }
    ctrl.buffer_building.store(false, Ordering::Relaxed);

    // Binary-search on the IDR-only secondary index (~log n over just
    // the keyframes) rather than the old O(n) walk over every tag.
    let desired_input_ts = latest.saturating_sub(target_delay);
    let target = ctrl
        .ring
        .find_idr_near(desired_input_ts, idr_search_tolerance_ms(keyframe_interval))?;
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

    // Detailed cut trace - every cut writes one log line with the
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
    // input timeline. No backdating, no burst - the user model is
    // "save N seconds of buffer, when ready jump back N seconds, then
    // play at 1×" and that's exactly this.
    state.wall_anchor = Instant::now();
    state.wall_anchor_input_ts = cut.target.ts_ms;
    state.consumer_seq = cut.target.seq;
    state.last_sent_input_ts = cut.target.ts_ms;
    // The cut target is a horizontal-primary IDR; a vertical dest must
    // re-lead with its own canvas keyframe before resuming (see EgressState).
    state.awaiting_keyframe = true;
    // Update the per-dest atomic immediately so the ingest-side trim
    // sees the new (potentially backward) position right away and can't
    // evict tags we just rewound to.
    dest.consumer_seq
        .store(state.consumer_seq, Ordering::Relaxed);

    // Re-emit cached sequence headers on the new output timeline so
    // the destination decoder has fresh config before the first
    // post-cut frame. The previous code skipped this on the assumption
    // that platforms cache headers from the initial publish - which is
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
    // Sync the generation counter - the explicit resend above means
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
    // Drop the MutexGuard before the awaits. Clone is cheap - each
    // value is a tiny SPS/PPS blob and there are at most ~5 entries
    // (one per Enhanced-RTMP OneTrack track in a multi-track stream;
    // exactly one for the legacy / single-track case).
    let v_headers: Vec<(u8, Vec<u8>)> = ctrl
        .ring
        .video_seq_headers
        .lock()
        .iter()
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    let passthrough = dest.pass_through_multitrack_video.load(Ordering::Relaxed);
    if passthrough {
        // Twitch (EB): forward every cached track's seq header
        // bit-faithfully. Twitch's IVS pipeline binds each track's
        // SPS/PPS to its allocated transcoder slot - missing one
        // leaves that track with no decoder config, which Twitch
        // surfaces as resolution "x" in Inspector and the transcoder
        // pipeline as "no config bound to this session", killing the
        // stream at the TCP retransmit boundary ~60 s later.
        for (track_id, h) in &v_headers {
            crate::trace::log(
                "VIDEO_SEQ_HDR_SENT",
                &format!(
                    "ts=0x{:08x} track={} bytes={} hex={}",
                    ts,
                    track_id,
                    h.len(),
                    crate::trace::hex_prefix(h, 64)
                ),
            );
            sink.send_video(ts, h).await?;
        }
    } else if let Some(crate::h264::VideoEgress::Track(target)) = dest.video_egress() {
        // Non-Twitch destinations get the single-track-flattened form of
        // the canvas this destination wants: TrackId 0 for horizontal
        // (the default), or the vertical-canvas primary for a vertical
        // destination. Horizontal falls back to the only cached entry if
        // track 0 is missing (defensive - every real stream has a
        // track 0). Vertical requires an exact match: we must never
        // replay a landscape header to a vertical destination.
        //
        // A vertical destination whose canvas isn't resolved yet has
        // `video_egress() == None`, so this branch is skipped entirely
        // and no stale header is sent - the header arrives once Twitch
        // Dual Format is live. Audio replay below still runs.
        let pick = if target == 0 {
            v_headers
                .iter()
                .find(|(k, _)| *k == 0)
                .or_else(|| v_headers.first())
        } else {
            v_headers.iter().find(|(k, _)| *k == target)
        };
        if let Some((_, h)) = pick {
            let selected =
                crate::h264::select_video_bytes(h, crate::h264::VideoEgress::Track(target))
                    .unwrap_or(std::borrow::Cow::Borrowed(h.as_slice()));
            let bytes_out: &[u8] = &selected;
            crate::trace::log(
                "VIDEO_SEQ_HDR_SENT",
                &format!(
                    "ts=0x{:08x} track={} flattened bytes={} hex={}",
                    ts,
                    target,
                    bytes_out.len(),
                    crate::trace::hex_prefix(bytes_out, 64)
                ),
            );
            sink.send_video(ts, bytes_out).await?;
        }
    }
    // Audio seq-headers, same per-track shape as video, but keyed on the
    // AUDIO egress policy - Passthrough for every Twitch destination
    // regardless of EB session (Twitch's regular ingest accepts multi-track
    // audio / VOD audio track 1 without an EB allocation), a single flattened
    // track for everyone else. Run the cached header through the very same
    // `select_audio_bytes` the live path uses so the replayed config matches
    // the frames byte-for-byte (a non-Twitch dest gets its one track's
    // AudioSpecificConfig, flattened; the second-audio-track config is
    // dropped for platforms that can't decode it).
    let a_headers: Vec<(u8, Vec<u8>)> = ctrl
        .ring
        .audio_seq_headers
        .lock()
        .iter()
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    match dest.audio_egress() {
        crate::h264::AudioEgress::Passthrough => {
            for (track_id, h) in &a_headers {
                crate::trace::log(
                    "AUDIO_SEQ_HDR_SENT",
                    &format!(
                        "ts=0x{:08x} track={} bytes={} hex={}",
                        ts,
                        track_id,
                        h.len(),
                        crate::trace::hex_prefix(h, 32)
                    ),
                );
                sink.send_audio(ts, h).await?;
            }
        }
        egress @ crate::h264::AudioEgress::Track(target) => {
            // Prefer the exact track's cached config; fall back to the live
            // track 0 (then whatever is first) when the requested track isn't
            // cached, matching select_audio_bytes' live-track fallback so the
            // replayed config always agrees with the frames on the wire. A
            // legacy single-track config lands under key 0.
            let header = a_headers
                .iter()
                .find(|(k, _)| *k == target)
                .or_else(|| a_headers.iter().find(|(k, _)| *k == 0))
                .or_else(|| a_headers.first())
                .map(|(_, v)| v);
            if let Some(h) = header {
                if let Some(bytes) = crate::h264::select_audio_bytes(h, egress) {
                    crate::trace::log(
                        "AUDIO_SEQ_HDR_SENT",
                        &format!(
                            "ts=0x{:08x} track={} bytes={} hex={}",
                            ts,
                            target,
                            bytes.len(),
                            crate::trace::hex_prefix(&bytes, 32)
                        ),
                    );
                    sink.send_audio(ts, &bytes).await?;
                }
            }
        }
    }
    sink.flush().await
}

async fn wait_first_idr(ring: &Arc<DiskRing>) -> TagMeta {
    loop {
        // Register notification *before* checking - guarantees we don't
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
            // far enough back - otherwise find_idr_near may give us an
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
    // We reseed on a horizontal-primary IDR; a vertical dest must re-lead
    // with its own canvas's keyframe before streaming (see EgressState).
    state.awaiting_keyframe = true;
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
    // whatever the front happens to be - typically a P-frame - would
    // send frames that reference reference-frames that aren't in the
    // decoder's buffer → viewers see macroblocking until the next IDR.
    // Aligning to an IDR boundary loses a bit more content but keeps
    // the decode chain valid.
    if let Some(front) = ring.front_seq() {
        if seq < front {
            if let Some(m) = ring.oldest_idr_at_or_after(front) {
                return Some(m);
            }
            // No IDR in the ring at all (very early or pathological) -
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
        // Leading byte 0x17 (legacy AVC keyframe) lets the IDR survive
        // v0.1.3's primary-track gate in `Ring::append`; 0x27 marks the
        // inter-frames so they get classified the same way the real
        // wire pattern does. Bytes after the header are filler.
        let idr_payload: [u8; 50] = {
            let mut b = [0u8; 50];
            b[0] = 0x17;
            b
        };
        let p_payload: [u8; 50] = {
            let mut b = [0u8; 50];
            b[0] = 0x27;
            b
        };
        for s in 0..secs {
            for f in 0..fps {
                let ts = start_ms + s * 1000 + f * frame_ms;
                let is_idr = f == 0;
                let payload = if is_idr { &idr_payload } else { &p_payload };
                ctrl.on_tag(9, ts, payload, is_idr, false);
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

    // ── Shutdown signal (web Quit/Restart + tray Quit converge here) ──

    #[tokio::test]
    async fn shutdown_signal_reports_restart() {
        let h = harness(0);
        // notify_one stores a permit, so wait_shutdown resolves immediately
        // even though the request fires before we await - no lost wakeup.
        h.ctrl.request_restart();
        assert_eq!(h.ctrl.wait_shutdown().await, ShutdownKind::Restart);
    }

    #[tokio::test]
    async fn shutdown_signal_reports_quit() {
        let h = harness(0);
        h.ctrl.request_quit();
        assert_eq!(h.ctrl.wait_shutdown().await, ShutdownKind::Quit);
    }

    // ── Ingest key (optional publisher auth, off by default) ─────────

    #[tokio::test]
    async fn empty_ingest_key_accepts_any_publisher() {
        let h = harness(0);
        assert!(h
            .ctrl
            .begin_publish("literally-anything", "127.0.0.1")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn set_ingest_key_rejects_wrong_and_accepts_right() {
        let h = harness(0);
        h.ctrl.update_ingest_key("secret123".into());
        // Wrong key is rejected by the auth check, before the slot is taken,
        // so a following correct publish still succeeds.
        let err = h
            .ctrl
            .begin_publish("wrong", "127.0.0.1")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(h.ctrl.begin_publish("secret123", "127.0.0.1").await.is_ok());
    }

    #[tokio::test]
    async fn ingest_key_ignores_obs_multitrack_query_suffix() {
        // Under Enhanced Broadcasting, OBS publishes the stream key with a
        // `?clientConfigId=<id>` suffix appended (see create_service in OBS's
        // MultitrackVideoOutput.cpp). The exact ingest key must still be
        // accepted despite that suffix - the query is not part of the identity.
        let h = harness(0);
        h.ctrl.update_ingest_key("secret123".into());
        assert!(h
            .ctrl
            .begin_publish(
                "secret123?clientConfigId=instantclone-1723300000",
                "127.0.0.1"
            )
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn ingest_accepts_a_brokered_eb_key() {
        // Enhanced Broadcasting publishes with the Twitch session token, not the
        // ingest key. With an ingest key set, that token is rejected until the
        // /obs/multitrack-config proxy brokers it via remember_eb_key.
        let h = harness(0);
        h.ctrl.update_ingest_key("myingestkey".into());
        let err = h
            .ctrl
            .begin_publish("v1_eb_session_token", "127.0.0.1")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        h.ctrl.remember_eb_key("v1_eb_session_token".into());
        // OBS appends `?clientConfigId=<id>` to the token it publishes with;
        // begin_publish strips that query before matching the brokered token.
        assert!(h
            .ctrl
            .begin_publish("v1_eb_session_token?clientConfigId=abc123", "127.0.0.1")
            .await
            .is_ok());
    }

    // ── "Cut after this airs" (scheduled safe cut) ───────────────────

    #[test]
    fn safe_cut_requires_active_delay() {
        let h = harness(0);
        // Idle: nothing to schedule against.
        assert!(h.ctrl.schedule_safe_cut().is_err());
        // Armed-but-not-activated is still target=0 - output is at the
        // live edge, so a mark would fire (or hang) surprisingly.
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        assert!(h.ctrl.schedule_safe_cut().is_err());
        assert!(!h.ctrl.safe_cut_pending());
    }

    #[test]
    fn safe_cut_schedules_and_cancels() {
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        h.ctrl.activate_delay().expect("buffer is past armed");
        assert!(h.ctrl.schedule_safe_cut().is_ok());
        assert!(h.ctrl.safe_cut_pending());
        // No live consumer to measure against → remaining falls back to
        // the armed target, not 0 (0 would render a lying countdown).
        assert_eq!(h.ctrl.safe_cut_remaining_ms(), 2_000);
        h.ctrl.cancel_safe_cut();
        assert!(!h.ctrl.safe_cut_pending());
        assert_eq!(h.ctrl.safe_cut_remaining_ms(), 0);
        // Cancel must not have touched the active delay itself.
        assert_eq!(h.ctrl.target_delay_ms(), 2_000);
    }

    #[test]
    fn safe_cut_fires_only_after_slowest_consumer_passes_mark() {
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 4, 30);
        h.ctrl.activate_delay().expect("buffer is past armed");

        // A live destination whose consumer is still early in the ring.
        let st = h.ctrl.destination_state("d1");
        st.egress_alive.store(true, Ordering::Relaxed);
        st.consumer_seq.store(10, Ordering::Relaxed);

        h.ctrl.schedule_safe_cut().expect("delay is active");
        let before = h.ctrl.safe_cut_remaining_ms();
        assert!(before > 0, "mark is ahead of the consumer");

        // Consumer hasn't aired the mark yet → must NOT fire.
        h.ctrl.maybe_fire_safe_cut();
        assert!(h.ctrl.safe_cut_pending());
        assert_eq!(h.ctrl.target_delay_ms(), 2_000);

        // More stream arrives, and the consumer advances past the mark
        // (the newest tag's ts is beyond the mark by construction).
        feed_seconds(&h.ctrl, 4_000, 2, 30);
        let latest_seq = h.ctrl.ring.latest_seq().expect("ring has tags");
        st.consumer_seq.store(latest_seq, Ordering::Relaxed);

        h.ctrl.maybe_fire_safe_cut();
        assert!(!h.ctrl.safe_cut_pending(), "mark aired - must fire");
        assert_eq!(h.ctrl.target_delay_ms(), 0, "fire runs the normal cut");
        // The armed value survives, same as a manual Cut - the next
        // activate is instant.
        assert_eq!(h.ctrl.armed_delay_ms(), 2_000);
    }

    #[test]
    fn manual_cut_and_disarm_clear_scheduled_cut() {
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        h.ctrl.activate_delay().expect("buffer is past armed");
        h.ctrl.schedule_safe_cut().expect("delay is active");
        // Manual cut supersedes the mark.
        h.ctrl.stop_delay();
        assert!(!h.ctrl.safe_cut_pending());
        // Re-activate, schedule again, then disarm - mark dies with the delay.
        h.ctrl.activate_delay().expect("buffer still full");
        h.ctrl.schedule_safe_cut().expect("delay is active");
        h.ctrl.arm_delay(0);
        assert!(!h.ctrl.safe_cut_pending());
    }

    #[tokio::test]
    async fn new_publisher_session_clears_scheduled_cut() {
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        h.ctrl.activate_delay().expect("buffer is past armed");
        h.ctrl.schedule_safe_cut().expect("delay is active");
        // Fresh publisher: the mark's timestamp belongs to the OLD
        // session's timeline (the new one restarts near 0), so keeping
        // it would leave an unreachable mark pending forever.
        h.ctrl
            .begin_publish("key", "127.0.0.1")
            .await
            .expect("slot is free");
        assert!(!h.ctrl.safe_cut_pending());
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
        // Only 1 second of buffer - activate should refuse with remaining ETA
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
        // With buffer still full, phase is `ready` again - not `idle`.
        assert_eq!(h.ctrl.phase(), "ready");
    }

    // ── auto-activate-pending state machine ────────────────────────
    //
    // The v0.1.4 "Cut delay didn't stick" bug: auto-activate-when-ready
    // fired again immediately after the user hit Cut, because phase
    // reverted from "active" back to "ready" (buffer still full,
    // armed_delay_ms still set) and the edge detector treated that as
    // a fresh "*->ready" transition. Fix: Controller tracks an
    // auto_activate_pending slot - set on arm-from-non-active, cleared
    // on activate-success, cut, and begin_publish. Supervisor now
    // reads that slot instead of trying to infer state from phase
    // transitions. These tests pin the four edges of that machine.

    #[test]
    fn arm_from_disarmed_sets_auto_activate_pending() {
        let h = harness(0);
        assert!(
            !h.ctrl.auto_activate_pending(),
            "fresh controller must start clean"
        );
        h.ctrl.arm_delay(5_000);
        assert!(
            h.ctrl.auto_activate_pending(),
            "arm from target=0 must arm the pending slot"
        );
    }

    #[test]
    fn activate_consumes_auto_activate_pending() {
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        assert!(h.ctrl.auto_activate_pending());
        h.ctrl.activate_delay().expect("buffer is full");
        assert!(
            !h.ctrl.auto_activate_pending(),
            "successful activate must consume the pending slot"
        );
    }

    #[test]
    fn cut_consumes_auto_activate_pending_so_it_sticks() {
        // The bug-of-record: without this clear, the supervisor sees
        // phase revert to "ready" after cut and re-fires activate_delay.
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        h.ctrl.activate_delay().unwrap();
        // Slot was already cleared by activate. Re-arm to set it again,
        // then exercise the cut path explicitly. (arm_delay live-update
        // path - target > 0, so this DOESN'T set pending; we have to
        // simulate the post-cut state.)
        h.ctrl.stop_delay(); // cut: target -> 0, pending -> false
        assert!(
            !h.ctrl.auto_activate_pending(),
            "cut must keep pending false so the supervisor doesn't re-activate"
        );
        // Now re-arm (target=0 so this IS a fresh arm event) and verify
        // the slot refills - so the NEXT arm cycle auto-activates as
        // expected. This is the "auto-activate works after re-arming"
        // half of the user-requested semantics.
        h.ctrl.arm_delay(3_000);
        assert!(
            h.ctrl.auto_activate_pending(),
            "re-arm from cut-hold state must refill the pending slot"
        );
    }

    #[test]
    fn live_update_arm_does_not_set_auto_activate_pending() {
        // When the streamer is already active and adjusts the armed
        // value (slider drag, profile click during live), that's a
        // live-update, not a fresh arm. The pending slot must stay
        // false so a subsequent cut doesn't snap back to active via
        // auto-activate-when-ready.
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        h.ctrl.activate_delay().unwrap();
        assert!(!h.ctrl.auto_activate_pending(), "consumed by activate");

        // Live-update arm: target > 0, so this should NOT set pending.
        h.ctrl.arm_delay(2_500);
        assert!(
            !h.ctrl.auto_activate_pending(),
            "live-update arm during active must not arm the pending slot"
        );

        // Cut now → pending stays false → no auto-re-activate.
        h.ctrl.stop_delay();
        assert!(
            !h.ctrl.auto_activate_pending(),
            "cut after live-update arm must NOT magically refill pending"
        );
    }

    #[test]
    fn disarm_clears_auto_activate_pending() {
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        assert!(h.ctrl.auto_activate_pending());
        h.ctrl.arm_delay(0); // disarm
        assert!(
            !h.ctrl.auto_activate_pending(),
            "disarm clears pending - there's nothing to auto-activate into"
        );
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
        // While active, changing the armed amount must also re-target -
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
        // Two tags well below any wrap point - strictly increasing.
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

    // ── Enhanced Broadcasting stability ──────────────────────────────

    /// EB seq headers must survive across the `pace_and_send` resync
    /// path: the supervisor restarts egress without disturbing the
    /// publisher, the per-track BTreeMap stays populated, and the next
    /// `send_sequence_headers` for a Twitch destination re-emits every
    /// track from the cache. The bug we shipped + reverted was the
    /// pre-BTreeMap single-Option cache stomping all tracks down to
    /// just the last one received - verify via the ring's cache
    /// directly so we'd catch a regression to that storage shape.
    #[test]
    fn eb_seq_headers_survive_egress_restart() {
        let h = harness(0);
        // Simulate OBS sending OneTrack-format seq headers for tracks
        // 0..=3 (a typical Twitch EB four-rung ladder).
        for track in 0u8..=3 {
            let mut tag = vec![0x96, 0x00, 0x61, 0x76, 0x63, 0x31, track];
            tag.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
            h.ctrl
                .ring
                .append(9, 0, &tag, false, true)
                .expect("seq-header append");
        }
        // mark_ingest_dead clears codec state and EB overrides, but
        // the seq-header cache itself belongs to the ring and only
        // gets wiped by a fresh publisher session - verify it stays.
        let dest = h.ctrl.destination_state("d1");
        *dest.eb_override_url.lock() = Some("rtmps://stale".into());
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        h.ctrl.mark_ingest_dead();
        let cache = h.ctrl.ring.video_seq_headers.lock();
        assert_eq!(cache.len(), 4, "all 4 tracks must persist across cut");
        for track in 0u8..=3 {
            assert!(cache.contains_key(&track), "track {track} dropped");
        }
    }

    /// `mark_ingest_dead` must clear every destination's
    /// `eb_override_url`. A stale override would force the next
    /// (possibly non-EB) stream onto an IVS endpoint with no
    /// allocated session - exactly the silent 60-s-drop failure mode
    /// we were chasing before the override field landed.
    #[test]
    fn mark_ingest_dead_clears_all_eb_overrides() {
        let h = harness(0);
        for id in ["dest-a", "dest-b", "dest-c"] {
            let s = h.ctrl.destination_state(id);
            *s.eb_override_url.lock() = Some(format!("rtmps://ivs/{id}"));
        }
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        h.ctrl.mark_ingest_dead();
        for (id, s) in h.ctrl.all_destination_states() {
            assert!(
                s.eb_override_url.lock().is_none(),
                "override for {id} survived ingest cut"
            );
        }
    }

    /// `try_claim_vod_fetch` is single-flight: exactly one caller wins
    /// while a fetch is in flight. The supervisor fires every ~2 s; before
    /// this guard every tick spawned a fresh Twitch API call, each
    /// allocating a distinct IVS session and forcing an extra egress
    /// restart - the multi-session, wrong-broadcast-type symptom seen in
    /// Twitch Inspector. The latch is released by the fetch task on
    /// completion (modelled here by the explicit store), after which the
    /// next tick may claim again (e.g. to retry a failed fetch).
    #[test]
    fn try_claim_vod_fetch_admits_one_claimant() {
        let s = DestinationState::new("main".into());
        assert!(s.try_claim_vod_fetch(), "first caller must win the claim");
        for tick in 0..5 {
            assert!(
                !s.try_claim_vod_fetch(),
                "tick {tick} must see a fetch already in flight"
            );
        }
        // Fetch task finished (success or failure) - latch released.
        s.vod_fetch_pending.store(false, Ordering::Relaxed);
        assert!(
            s.try_claim_vod_fetch(),
            "after the fetch ends the next tick must be able to claim"
        );
    }

    /// The claim re-checks `eb_override_url` under its mutex before
    /// committing: a fetch that completed on an earlier tick (setting the
    /// override) must abort a redundant claim AND leave the latch clear,
    /// so the destination isn't left falsely "fetching" forever.
    #[test]
    fn try_claim_vod_fetch_skips_when_session_already_allocated() {
        let s = DestinationState::new("main".into());
        *s.eb_override_url.lock() = Some("rtmps://ivs/session".into());
        assert!(
            !s.try_claim_vod_fetch(),
            "must not claim when a session already exists"
        );
        assert!(
            !s.vod_fetch_pending.load(Ordering::Relaxed),
            "a skipped claim must release the latch, not leave it stuck"
        );
    }

    /// Regression guard for the lockout hole: the latch is decoupled from
    /// the override's lifecycle. The multitrack-config proxy (web.rs) and
    /// publisher disconnect both clear `eb_override_url` without touching
    /// the latch. After a successful fetch (override set, latch clear),
    /// clearing the override - as those paths do - must let the next tick
    /// re-claim and fetch a fresh session, not lock the destination into
    /// the legacy Source-Only ingest forever.
    #[test]
    fn try_claim_vod_fetch_reclaims_after_override_cleared() {
        let s = DestinationState::new("main".into());
        // Post-success state: session allocated, latch released by the task.
        *s.eb_override_url.lock() = Some("rtmps://ivs/session".into());
        s.vod_fetch_pending.store(false, Ordering::Relaxed);
        assert!(
            !s.try_claim_vod_fetch(),
            "a live session must block a re-fetch"
        );
        // Override cleared elsewhere (proxy cleanup / disconnect).
        *s.eb_override_url.lock() = None;
        assert!(
            s.try_claim_vod_fetch(),
            "cleared override must allow a fresh fetch - no permanent lockout"
        );
    }

    /// The single-flight guarantee under real thread contention: when a
    /// swarm of threads races to claim the same destination (the situation
    /// the atomic swap exists for), exactly one wins. A sequential test
    /// can't prove this - it's the concurrent claim that the supervisor's
    /// every-2s wake-up plus an in-flight fetch can produce. Deterministic
    /// (no sleeps): the atomic swap has exactly one false -> true edge, so
    /// the winner count is always 1 regardless of scheduling.
    #[test]
    fn try_claim_vod_fetch_admits_exactly_one_under_contention() {
        let s = DestinationState::new("main".into());
        let winners = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..32 {
                scope.spawn(|| {
                    if s.try_claim_vod_fetch() {
                        winners.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
        });
        assert_eq!(
            winners.load(Ordering::Relaxed),
            1,
            "exactly one racing claimant may hold the single-flight latch"
        );
    }

    /// A fetch that returns while still in its own session applies normally.
    #[test]
    fn apply_vod_session_if_current_writes_when_session_unchanged() {
        let s = DestinationState::new("main".into());
        let epoch = s.session_epoch();
        assert!(
            s.apply_vod_session_if_current("rtmps://ivs/live".into(), epoch),
            "same-session fetch must apply"
        );
        assert_eq!(*s.eb_override_url.lock(), Some("rtmps://ivs/live".into()));
    }

    /// Regression guard for the late-completion race: a VOD-session fetch
    /// spawned in one publisher session must NOT write its IVS URL if OBS
    /// disconnected (and bumped the epoch) while the request was in flight.
    /// Writing it would point the next stream at a dead session - the
    /// Source-Only failure this whole path exists to avoid.
    #[test]
    fn apply_vod_session_if_current_discards_after_disconnect() {
        let s = DestinationState::new("main".into());
        let epoch = s.session_epoch(); // captured when the fetch is spawned
        s.invalidate_session_override(); // OBS disconnects mid-fetch
        assert!(
            !s.apply_vod_session_if_current("rtmps://ivs/stale".into(), epoch),
            "a fetch outliving its session must be discarded"
        );
        assert!(
            s.eb_override_url.lock().is_none(),
            "the stale URL must not leak into the next session"
        );
    }

    /// `invalidate_session_override` clears the URL and advances the epoch
    /// together, so the disconnect both forgets the old session and trips
    /// any in-flight fetch's apply-time guard.
    #[test]
    fn invalidate_session_override_clears_url_and_bumps_epoch() {
        let s = DestinationState::new("main".into());
        *s.eb_override_url.lock() = Some("rtmps://ivs/old".into());
        let before = s.session_epoch();
        s.invalidate_session_override();
        assert!(s.eb_override_url.lock().is_none(), "url must clear");
        assert_eq!(s.session_epoch(), before + 1, "epoch must advance");
    }

    /// Across a full disconnect/reconnect, a fetch from the NEW session
    /// still applies: the epoch the supervisor captures after reconnect
    /// matches the current one, so only the pre-disconnect fetch is stale.
    #[test]
    fn apply_vod_session_if_current_applies_for_fresh_session_after_reconnect() {
        let s = DestinationState::new("main".into());
        s.invalidate_session_override(); // disconnect bumps epoch
        let fresh_epoch = s.session_epoch(); // supervisor re-captures post-reconnect
        assert!(
            s.apply_vod_session_if_current("rtmps://ivs/new".into(), fresh_epoch),
            "a fetch from the new session must apply"
        );
        assert_eq!(*s.eb_override_url.lock(), Some("rtmps://ivs/new".into()));
    }

    /// `complete_vod_fetch` on success applies the URL, reports Applied,
    /// and releases the latch so the override (now Some) is what blocks
    /// any re-fetch.
    #[test]
    fn complete_vod_fetch_applies_and_releases_on_success() {
        let s = DestinationState::new("main".into());
        assert!(s.try_claim_vod_fetch(), "latch held, as in production");
        let epoch = s.session_epoch();
        assert_eq!(
            s.complete_vod_fetch(Some("rtmps://ivs/live".into()), epoch),
            VodFetchOutcome::Applied
        );
        assert_eq!(*s.eb_override_url.lock(), Some("rtmps://ivs/live".into()));
        assert!(
            !s.vod_fetch_pending.load(Ordering::Relaxed),
            "latch must be released after a successful completion"
        );
    }

    /// A result that outlived its session reports DiscardedStale, writes
    /// nothing, and STILL releases the latch (the previously untested
    /// release-on-every-path invariant).
    #[test]
    fn complete_vod_fetch_discards_stale_and_releases() {
        let s = DestinationState::new("main".into());
        assert!(s.try_claim_vod_fetch());
        let epoch = s.session_epoch();
        s.invalidate_session_override(); // disconnect mid-fetch
        assert_eq!(
            s.complete_vod_fetch(Some("rtmps://ivs/stale".into()), epoch),
            VodFetchOutcome::DiscardedStale
        );
        assert!(s.eb_override_url.lock().is_none());
        assert!(
            !s.vod_fetch_pending.load(Ordering::Relaxed),
            "latch must be released even when the result is discarded"
        );
    }

    /// A failed fetch reports Failed, leaves no override, and releases the
    /// latch so the next supervisor tick can retry.
    #[test]
    fn complete_vod_fetch_releases_latch_on_failure() {
        let s = DestinationState::new("main".into());
        assert!(s.try_claim_vod_fetch());
        let epoch = s.session_epoch();
        assert_eq!(s.complete_vod_fetch(None, epoch), VodFetchOutcome::Failed);
        assert!(s.eb_override_url.lock().is_none());
        assert!(
            !s.vod_fetch_pending.load(Ordering::Relaxed),
            "latch must be released on failure"
        );
        assert!(
            s.try_claim_vod_fetch(),
            "a released latch lets the next tick retry"
        );
    }

    /// Stress the apply-vs-disconnect race: a late fetch's apply and the
    /// disconnect that should invalidate it run on two threads. Both take
    /// the override mutex for their whole body, so the two orderings are
    /// the only possibilities and both MUST end with no override - either
    /// the disconnect clears the just-written URL, or it bumps the epoch
    /// first so apply discards. A stale Some surviving here would be the
    /// Source-Only bug. A barrier collides the critical sections; the
    /// invariant holds every iteration regardless of who wins.
    #[test]
    fn apply_and_invalidate_never_leave_a_stale_override() {
        use std::sync::Barrier;
        for _ in 0..200 {
            let s = DestinationState::new("main".into());
            let epoch = s.session_epoch();
            let barrier = Barrier::new(2);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    barrier.wait();
                    s.apply_vod_session_if_current("rtmps://ivs/late".into(), epoch);
                });
                scope.spawn(|| {
                    barrier.wait();
                    s.invalidate_session_override();
                });
            });
            assert!(
                s.eb_override_url.lock().is_none(),
                "racing apply against the disconnect must never leave a stale override"
            );
        }
    }

    /// `note_multitrack_video` is sticky-with-decay: once set, the
    /// chip stays lit for a short window after the last multi-track
    /// tag arrived (so a momentary pause between IDRs doesn't drop
    /// the chip), and decays to false once stale. `reset_codec_state`
    /// must wipe it immediately so a fresh non-EB publisher session
    /// doesn't inherit the previous session's EB flag.
    #[test]
    fn eb_chip_clears_on_publisher_reset() {
        let h = harness(0);
        // `multitrack_video()` returns false when ingest is dead, so
        // simulate an active publisher session first. Also sleep
        // briefly so `process_now_ms()` advances past 0 - the chip
        // uses 0 as a sentinel for "never set" and would otherwise
        // race the process anchor on a freshly-started test binary.
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(2));
        h.ctrl.note_multitrack_video();
        assert!(h.ctrl.multitrack_video(), "chip must light on first tag");
        h.ctrl.reset_codec_state();
        assert!(
            !h.ctrl.multitrack_video(),
            "chip must clear when codec state resets"
        );
    }

    /// A fresh publisher session must clear out all cached audio and multi-track
    /// video sequence headers left over from a previous stream. While those
    /// headers are required to survive mid-stream egress supervisor restarts
    /// (tested via `eb_seq_headers_survive_egress_restart`), allowing them to
    /// leak into a subsequent session causes severe pipeline pollution. If a
    /// publisher reconnects without Enhanced Broadcasting, a failure to clear
    /// this state causes the egress engine to inject stale multi-track headers
    /// into Twitch, triggering an unrecoverable stream freeze.
    #[tokio::test]
    async fn begin_publish_purges_cached_sequence_headers_from_prior_sessions() {
        let h = harness(0);

        // 1. Populate the video sequence headers cache simulating an active
        //    Twitch EB multi-track ladder (Tracks 0..=3).
        for track_id in 0u8..=3 {
            let mut tag = vec![0x96, 0x00, 0x61, 0x76, 0x63, 0x31, track_id];
            tag.extend_from_slice(&[0xaa, 0xbb, 0xcc, track_id]);
            h.ctrl
                .ring
                .append(9, 0, &tag, false, true)
                .expect("failed to seed mock multi-track video sequence header");
        }

        // 2. Populate the audio sequence header cache simulating an active
        //    Twitch VOD-audio session (Tracks 0 and 1).
        for track_id in 0u8..=1 {
            // OneTrack multi-track audio seq-header per OBS's
            // flv_packet_audio_ex wire format:
            //   byte 0: 0x95 (SoundFormat=9 | PacketType=Multitrack)
            //   byte 1: MultiTrackType=0 | NestedPacketType=0 (Seq)
            //   bytes 2..6: FourCC = "mp4a"
            //   byte 6:    TrackId
            //   bytes 7..: AudioSpecificConfig
            let mut tag = vec![0x95, 0x00, 0x6d, 0x70, 0x34, 0x61, track_id];
            tag.extend_from_slice(&[0x12, 0x10, track_id, 0x00]);
            h.ctrl
                .ring
                .append(8, 0, &tag, false, true)
                .expect("failed to seed mock multi-track audio seq header");
        }

        // 3. Populate metadata cache simulating Publisher A's onMetaData.
        *h.ctrl.ring.metadata.lock() = Some(b"onMetaData-publisher-A".to_vec());

        // Validate baseline assumptions: caches must be fully loaded.
        {
            let video_cache = h.ctrl.ring.video_seq_headers.lock();
            assert_eq!(video_cache.len(), 4, "video cache must start with 4 tracks");
            let audio_cache = h.ctrl.ring.audio_seq_headers.lock();
            assert_eq!(audio_cache.len(), 2, "audio cache must start with 2 tracks");
            assert!(
                h.ctrl.ring.metadata.lock().is_some(),
                "metadata cache must be active"
            );
        }

        // 3. Simulate a clean stream teardown or crash. The supervisor invokes
        //    `mark_ingest_dead`, which updates atomic states but leaves headers
        //    intact by design to allow ongoing egress readers to recover.
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        h.ctrl.mark_ingest_dead();

        {
            let video_cache = h.ctrl.ring.video_seq_headers.lock();
            let audio_cache = h.ctrl.ring.audio_seq_headers.lock();
            assert_eq!(
                video_cache.len(),
                4,
                "regression: mark_ingest_dead cleared video headers early"
            );
            assert_eq!(
                audio_cache.len(),
                2,
                "regression: mark_ingest_dead cleared audio headers early"
            );
        }

        // 4. Critical transition: A brand new publisher hits the RTMP stack.
        //    `begin_publish` must perform atomic state purging of the ring caches.
        h.ctrl
            .begin_publish("fresh_incoming_stream_key", "127.0.0.1")
            .await
            .expect("begin_publish must accept the new session token assignment");

        // 5. Hard assertions to guarantee a zeroed cache allocation before streaming starts.
        let post_video_cache = h.ctrl.ring.video_seq_headers.lock();
        assert!(
            post_video_cache.is_empty(),
            "leak detected: begin_publish failed to purge video_seq_headers cache. \
             stale tracks remaining: {:?}",
            post_video_cache.keys()
        );

        let post_audio_cache = h.ctrl.ring.audio_seq_headers.lock();
        assert!(
            post_audio_cache.is_empty(),
            "leak detected: begin_publish failed to clear stale audio_seq_headers cache. \
             stale tracks remaining: {:?}",
            post_audio_cache.keys()
        );

        let post_metadata_cache = h.ctrl.ring.metadata.lock();
        assert!(
            post_metadata_cache.is_none(),
            "leak detected: begin_publish failed to clear stale onMetaData. \
             cached data: {:?}",
            post_metadata_cache
        );
    }

    /// User-visible regression: after a Stop Streaming / Start Streaming
    /// cycle in OBS (the proxy stays running), the delay bar froze at 0%
    /// and never filled even though tags were flowing. OBS's RTMP wire
    /// timestamps restart from ~0 on every fresh session, but the ring
    /// still held the prior session's tags at much higher ts_ms values.
    /// `oldest_ts()` returned the stale front and `latest_ts()` returned
    /// the fresh back, so `buffer_fill_ms = latest.saturating_sub(oldest)`
    /// saturated to 0 forever. `trim_older_than` could not rescue it
    /// either - its cutoff also saturated to 0 against the new session's
    /// low current_ts.
    ///
    /// Reported by the streamer on the v0.1.1 build: "stream no EB, stop,
    /// turn on EB, try to apply delay - bar doesn't fill". Not actually
    /// EB-specific: any stop-start cycle reproduces it.
    #[tokio::test]
    async fn buffer_fill_recovers_after_publisher_session_restart() {
        let h = harness(0);

        // Session 1: a few tags at "10 minutes into the stream" ts_ms,
        // standing in for a real prior stream. Three tags is enough to
        // populate the index front - the bug is purely about the ts
        // values at the front vs back, not tag count.
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        for offset in 0u64..3 {
            h.ctrl
                .ring
                .append(9, 600_000 + offset * 33, &[0xaa; 64], false, false)
                .expect("session 1 append");
        }
        assert!(
            h.ctrl.buffer_fill_ms() <= 100,
            "session 1 sanity: three same-timestamp-region tags = tiny span"
        );

        // OBS stops streaming. Publisher disconnects. The ring is
        // deliberately NOT cleared here so that a same-session blip
        // (network flap, brief reconnect) keeps its buffered tags -
        // only `begin_publish` of a fresh session wipes it.
        h.ctrl.mark_ingest_dead();

        // Fresh OBS Start Streaming. begin_publish must clear the
        // ring so the new session's ts_ms (starting near 0) is
        // measured against an empty index.
        h.ctrl
            .begin_publish("fresh-session-after-stop-start", "127.0.0.1")
            .await
            .expect("begin_publish must succeed on fresh session");

        // Session 2: simulate OBS sending tags with wire_ts starting
        // from 0, the standard RTMP behaviour on a new stream session.
        // After the fix, these tags populate an empty index and
        // buffer_fill_ms reflects their span. Before the fix, the
        // session 1 front sits at ts_ms=600_000 and latest_ts=66 makes
        // buffer_fill_ms saturate to 0.
        for offset in 0u64..3 {
            h.ctrl
                .ring
                .append(9, offset * 33, &[0xbb; 64], false, false)
                .expect("session 2 append");
        }

        let fill = h.ctrl.buffer_fill_ms();
        assert!(
            fill > 0,
            "buffer_fill_ms must reflect new session tags after \
             begin_publish, got {} (before the fix, the prior session's \
             high-ts front made latest - oldest saturate to 0)",
            fill,
        );
        assert!(
            fill <= 200,
            "session 2 fill must be the span of session 2 tags only \
             (~66 ms), not a phantom span that includes session 1; got {}",
            fill,
        );
    }

    /// Feed `count` IDR tags spaced `spacing_ms` apart, starting at
    /// `start_ms`. Leading byte 0x17 is the legacy AVC keyframe marker.
    fn feed_idrs(ctrl: &Controller, start_ms: u32, spacing_ms: u32, count: u32) {
        let mut payload = [0u8; 50];
        payload[0] = 0x17;
        for i in 0..count {
            ctrl.on_tag(9, start_ms + i * spacing_ms, &payload, true, false);
        }
    }

    #[test]
    fn keyframe_interval_is_zero_before_any_gap_is_measured() {
        let h = harness(0);
        assert_eq!(h.ctrl.keyframe_interval_ms(), 0);
        // One IDR opens the window but is not itself a gap.
        feed_idrs(&h.ctrl, 0, 2_000, 1);
        assert_eq!(h.ctrl.keyframe_interval_ms(), 0);
    }

    #[test]
    fn keyframe_interval_measures_mean_spacing() {
        let h = harness(0);
        feed_idrs(&h.ctrl, 0, 2_000, 4);
        assert_eq!(h.ctrl.keyframe_interval_ms(), 2_000);
    }

    #[test]
    fn keyframe_interval_measures_a_four_second_gop() {
        let h = harness(0);
        feed_idrs(&h.ctrl, 0, 4_000, 4);
        assert_eq!(h.ctrl.keyframe_interval_ms(), 4_000);
    }

    /// The measurement must FREEZE once the sample budget is spent.
    /// A value that keeps drifting is what makes a warning line flicker
    /// on and off mid-stream, which is the failure this design avoids.
    #[test]
    fn keyframe_interval_freezes_after_the_sample_budget() {
        let h = harness(0);
        feed_idrs(&h.ctrl, 0, 2_000, KEYFRAME_SAMPLE_GAPS + 1);
        let settled = h.ctrl.keyframe_interval_ms();
        assert_eq!(settled, 2_000);

        // A long stall afterwards (scene change, encoder hiccup) must not
        // move the reading.
        feed_idrs(&h.ctrl, 60_000, 10_000, 4);
        assert_eq!(
            h.ctrl.keyframe_interval_ms(),
            settled,
            "interval must not move after the sample budget is spent"
        );
    }

    /// A repeated or backwards timestamp would otherwise register as a
    /// zero-width gap and drag the mean toward 0, silencing a real warning.
    #[test]
    fn non_advancing_timestamps_do_not_pollute_the_mean() {
        let h = harness(0);
        let mut payload = [0u8; 50];
        payload[0] = 0x17;
        h.ctrl.on_tag(9, 0, &payload, true, false);
        h.ctrl.on_tag(9, 4_000, &payload, true, false);
        // Same timestamp three times over - a duplicated tag.
        for _ in 0..3 {
            h.ctrl.on_tag(9, 4_000, &payload, true, false);
        }
        assert_eq!(h.ctrl.keyframe_interval_ms(), 4_000);
    }

    /// Non-IDR video and audio tags must not be counted as keyframes.
    #[test]
    fn only_idr_video_tags_are_sampled() {
        let h = harness(0);
        let mut idr = [0u8; 50];
        idr[0] = 0x17;
        let mut p = [0u8; 50];
        p[0] = 0x27;
        h.ctrl.on_tag(9, 0, &idr, true, false);
        // P-frames and audio in between must be ignored entirely.
        for i in 1..10 {
            h.ctrl.on_tag(9, i * 100, &p, false, false);
            h.ctrl.on_tag(8, i * 100, &[0xaf, 0x01, 0x21], false, false);
        }
        h.ctrl.on_tag(9, 2_000, &idr, true, false);
        assert_eq!(h.ctrl.keyframe_interval_ms(), 2_000);
    }

    /// A reconnect may bring a completely different OBS profile, so the
    /// previous session's measurement must not leak into the new one.
    #[test]
    fn reset_codec_state_clears_the_measurement() {
        let h = harness(0);
        feed_idrs(&h.ctrl, 0, 4_000, 4);
        assert_eq!(h.ctrl.keyframe_interval_ms(), 4_000);
        h.ctrl.reset_codec_state();
        assert_eq!(h.ctrl.keyframe_interval_ms(), 0);
        assert_eq!(h.ctrl.stream_params().width, 0);
        assert_eq!(h.ctrl.stream_params().height, 0);
    }

    /// The packed dims atomic must round-trip a real resolution and never
    /// bleed width into height or vice-versa.
    #[test]
    fn stream_params_reports_packed_dimensions() {
        let h = harness(0);
        // 0x11 = AVC keyframe; sps_dimensions decodes 1920x1080 from a real
        // SPS, but the unit here only needs the packing path exercised via a
        // known resolution, so drive it through the public snapshot instead.
        h.ctrl
            .video_dims
            .store(((1920u64) << 32) | 1080, Ordering::Relaxed);
        let p = h.ctrl.stream_params();
        assert_eq!((p.width, p.height), (1920, 1080));
    }

    /// Dead band stays at the tuned 1500 ms for a 2 s GOP (and while
    /// unmeasured), and widens past half a GOP for long-keyframe encoders so
    /// the same-IDR re-cut bounce can't reappear at 3-4 s cadences.
    #[test]
    fn recut_dead_band_tracks_keyframe_interval() {
        assert_eq!(recut_dead_band_ms(0), 1_500, "unmeasured keeps the floor");
        assert_eq!(recut_dead_band_ms(2_000), 1_500, "2 s GOP is unchanged");
        assert!(recut_dead_band_ms(4_000) > 2_000, "4 s GOP widens the band");
        assert_eq!(recut_dead_band_ms(4_000), 2_500);
        // Must always clear half a GOP, or the bounce returns.
        for kf in [1_000u32, 2_000, 3_000, 4_000, 6_000] {
            assert!(
                recut_dead_band_ms(kf) > kf as u64 / 2,
                "dead band must exceed half a GOP for kf={kf}"
            );
        }
    }

    #[test]
    fn idr_search_tolerance_never_below_floor_and_scales_up() {
        assert_eq!(idr_search_tolerance_ms(0), 2_000);
        assert_eq!(idr_search_tolerance_ms(2_000), 2_000);
        assert_eq!(idr_search_tolerance_ms(4_000), 2_000);
        assert_eq!(
            idr_search_tolerance_ms(6_000),
            3_000,
            "6 s GOP needs 3 s reach"
        );
    }
}
