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
use crate::integrations::host::fmt_duration;
use crate::integrations::{Event, EventKind};
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

/// Rolling bitrate, lock-free: bytes are summed over windows of at least
/// `RATE_WINDOW_MS` and the rate is the last full window. A window that
/// hasn't rolled for `RATE_STALE_MS` means nothing has arrived since (the
/// stream stopped or froze), so the rate reads 0 rather than holding the
/// last second's value forever.
#[derive(Default)]
pub struct RateMeter {
    window_bytes: AtomicU64,
    /// 0 = no window yet.
    window_start_ms: AtomicU64,
    kbps: AtomicU32,
}

const RATE_WINDOW_MS: u64 = 1_000;
const RATE_STALE_MS: u64 = 2_500;

impl RateMeter {
    pub fn note(&self, bytes: usize) {
        self.note_at(bytes, process_now_ms());
    }

    fn note_at(&self, bytes: usize, now_ms: u64) {
        let now_ms = now_ms.max(1);
        let start = self.window_start_ms.load(Ordering::Relaxed);
        // First bytes, or the first after a gap: they mark the start of a
        // fresh window (so they count toward the time before it), instead
        // of the silence being averaged into the rate.
        if start == 0 || now_ms.saturating_sub(start) > RATE_STALE_MS {
            self.window_bytes.store(0, Ordering::Relaxed);
            self.window_start_ms.store(now_ms, Ordering::Relaxed);
            return;
        }
        let total = self.window_bytes.fetch_add(bytes as u64, Ordering::Relaxed) + bytes as u64;
        let elapsed = now_ms.saturating_sub(start);
        if elapsed >= RATE_WINDOW_MS {
            // bytes * 8 / ms = kbps
            self.kbps
                .store(((total * 8) / elapsed) as u32, Ordering::Relaxed);
            self.window_bytes.store(0, Ordering::Relaxed);
            self.window_start_ms.store(now_ms, Ordering::Relaxed);
        }
    }

    pub fn kbps(&self) -> u32 {
        self.kbps_at(process_now_ms())
    }

    fn kbps_at(&self, now_ms: u64) -> u32 {
        let start = self.window_start_ms.load(Ordering::Relaxed);
        if start == 0 || now_ms.saturating_sub(start) > RATE_STALE_MS {
            return 0;
        }
        self.kbps.load(Ordering::Relaxed)
    }
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
    /// Nothing is publishing, so there is no video to delay and the buffer
    /// cannot fill. Distinct from `BufferShort`, which is the same screen
    /// with an eta the user can wait out - this one waits on OBS.
    NoIngest,
    BufferShort {
        remaining_ms: u32,
    },
}

impl ActivateError {
    pub fn message(&self) -> String {
        match self {
            ActivateError::NotArmed => "no delay armed".to_string(),
            ActivateError::NoIngest => NO_INGEST.to_string(),
            ActivateError::BufferShort { remaining_ms } => {
                let secs = ((*remaining_ms + 500) / 1000).max(1);
                format!("buffer is still building - wait ~{}s", secs)
            }
        }
    }
}
/// What every surface says when a request would build delay state with
/// nothing publishing. The buffer holds no video and cannot fill, so arming
/// or going delayed has nothing to work with - and "preparing" on the
/// dashboard would be a progress bar that never moves.
/// How many log lines the in-memory ring keeps for the dashboard. Bounded
/// because a 24/7 relay logs for months: the oldest line is dropped rather
/// than letting the buffer track uptime.
const LOG_LINES_MAX: usize = 1_500;
/// How long OBS gets to send its first video frame after coming back from
/// a crash-protection hold before a missing picture counts as a freeze.
const FIRST_VIDEO_GRACE: Duration = Duration::from_secs(10);

pub const NO_INGEST: &str = "OBS isn't sending anything yet - start streaming first";

/// One hotkey / MIDI press, as the dashboard renders it: which action ran,
/// where it came from, whether it was refused, and a sequence number so a
/// repeat of the same action still reads as a new event.
#[derive(Debug, Clone)]
pub struct FiredAction {
    pub seq: u64,
    pub action: String,
    pub source: String,
    pub problem: Option<String>,
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
    pub rate_out: RateMeter,
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
    /// "not resolved yet" (OBS sends no 9:16 canvas, from its Additional
    /// canvas or Twitch Dual Format): a vertical destination then sends no
    /// video and the dashboard says what's missing, while every other
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

/// The Enhanced Broadcasting config last handed to OBS, and the Twitch
/// session it opened on one destination.
#[derive(Debug, Clone)]
pub struct EbSession {
    /// The rewritten GetClientConfiguration response OBS received.
    pub config: String,
    /// The session tokens OBS may publish with.
    pub auths: Vec<String>,
    pub dest_id: String,
    /// The IVS ingest URL the destination streams to for this session.
    pub ivs_url: String,
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
            rate_out: RateMeter::default(),
            shutdown_requested: AtomicBool::new(false),
            last_seq_header_gen: AtomicU32::new(0),
            eb_override_url: crate::sync::Mutex::new(None),
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
    /// destination has no resolved canvas yet (no 9:16 canvas on the wire):
    /// the caller drops all video and the dest waits, leaving every other
    /// destination untouched.
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

    pub(crate) fn note_outbound_bytes(&self, n: usize) {
        self.rate_out.note(n);
    }
}

/// `Controller::resumed_after_seq` when the ring held nothing.
const NO_SEQ: u64 = u64::MAX;
/// `Controller::eb_vertical_track` when the config has no vertical track.
const NO_TRACK: u8 = 0xFF;

/// `Controller::last_hold_end` values.
const HOLD_END_RESUMED: u8 = 1;
const HOLD_END_ENDED: u8 = 2;

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
    /// Set when the live publisher sends FCUnpublish or deleteStream, read
    /// (and cleared) when its connection closes: a disconnect after that
    /// goodbye is a deliberate stop, not a crash.
    unpublish_received: AtomicBool,
    /// When the last video tag arrived (process ms), 0 before the first
    /// one of a session or while crash protection is off. Used to spot a
    /// frozen OBS.
    last_video_tag_ms: AtomicU64,
    /// OBS froze (a freeze hold opened) and has sent no video since. The
    /// next video tag clears it and resumes; until then freeze detection
    /// stays quiet, so a freeze hold that ran out or was ended doesn't
    /// reopen itself. Kept apart from `hold` to stay lock-free per tag.
    ingest_frozen: AtomicBool,
    /// `hold` is Some. Lets the per-tag checks skip the lock.
    hold_open: AtomicBool,
    /// The newest ring seq when the last hold opened (`NO_SEQ`: none, or
    /// OBS is back): the delay tail ends there. A frozen OBS can keep
    /// sending audio, so the ring's own newest seq would keep moving.
    hold_tail_end_seq: AtomicU64,
    /// The last hold ran out while OBS was still gone: a destination still
    /// airing the delay tail finishes it, then ends (a delay as long as the
    /// hold time or longer airs in full). Not when the streamer ended it.
    tail_plays_out: AtomicBool,
    /// "The buffer holds only N s" was logged for the armed delay; cleared
    /// on the next arm so each new value is checked once.
    capacity_capped_logged: AtomicBool,
    /// Process ms of the last "rejected publisher" log line (see
    /// `log_rejected_publish`).
    last_rejection_log_ms: AtomicU64,
    /// The newest ring seq when the last hold ended because OBS came back
    /// (`NO_SEQ` when the ring was empty): the video before it is from
    /// before the hold, and a rejoin never starts there.
    resumed_after_seq: AtomicU64,
    /// How the last hold closed: 0 none yet, 1 resumed, 2 ended.
    last_hold_end: AtomicU8,
    /// Crash protection settings, mirrored from Settings by the supervisor.
    crash_protection: crate::sync::Mutex<crate::crash_protection::CrashProtection>,
    /// `crash_protection.enabled`, readable without a lock: the ingest path
    /// does its crash-protection bookkeeping only while this is set.
    crash_protection_on: AtomicBool,
    /// The open crash-protection hold: OBS dropped and destinations are
    /// being kept live on the reconnect screen. See `crate::crash_hold`.
    hold: crate::sync::Mutex<Option<crate::crash_hold::Hold>>,
    /// Encoded reconnect-screen loops, shared by every destination.
    pub slate_cache: Arc<crate::crash_hold::SlateCache>,
    /// The latest keyframe of each track the reconnect screen can't
    /// replace (HEVC, AV1), keyed like `video_seq_headers`: a hold re-sends
    /// it instead. See `crash_hold::is_held_keyframe`.
    held_keyframes: crate::sync::Mutex<std::collections::BTreeMap<u8, Arc<[u8]>>>,
    /// The Enhanced Broadcasting config last handed to OBS and the Twitch
    /// session it opened, so OBS coming back during a hold continues it.
    eb_session: crate::sync::Mutex<Option<EbSession>>,
    /// OBS asked for an Enhanced Broadcasting config (the kept one or a new
    /// one) since the hold opened: it is coming back on that session.
    /// Cleared when a hold opens.
    eb_session_claimed: AtomicBool,

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
    rate_in: RateMeter, // inbound (from OBS)

    // The integrations engine, attached once at startup. Events reach it
    // through `emit`, which never blocks and works from any thread.
    integrations: std::sync::OnceLock<Arc<crate::integrations::Handle>>,
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

    // Coordination
    publish_lock: Mutex<()>,

    // Detected codecs + enhanced-broadcasting state. Updated by the
    // ingest path on every tag (cheap atomic compare). The UI surfaces
    // these so the user can tell at a glance what OBS is sending and
    // whether Enhanced Broadcasting was caught + flattened.
    video_codec: AtomicU8,
    audio_codec: AtomicU8,
    /// The track the last Enhanced Broadcasting config OBS got puts the
    /// vertical canvas on, `NO_TRACK` when it has none. Finds the vertical
    /// track when its SPS can't be read (Twitch's HEVC for 2K channels).
    eb_vertical_track: AtomicU8,
    multitrack_video: AtomicBool,
    multitrack_audio: AtomicBool,
    /// OBS is sending a 2nd audio track (TrackId 1, the VOD / clean track)
    /// in this stream. A destination set to that track then gets only it
    /// (see `h264::select_audio_bytes`). Reset with the publisher.
    second_audio_track: AtomicBool,

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
    /// Whether `last_input_ts_u32` means anything yet. A session's first tag
    /// has nothing to be early or late relative to, so it is taken at face
    /// value; without this, a first tag in the upper half of the counter is
    /// indistinguishable from one stamped before the session started.
    input_ts_seen: AtomicBool,

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

    // The most recent action driven from outside the dashboard (a hotkey or
    // a MIDI pad), so the UI can point at the binding that just fired. The
    // sequence number is what makes a repeat of the same action visible:
    // pressing "cut" twice has to read as two events, not one.
    last_action: crate::sync::Mutex<Option<FiredAction>>,
    #[cfg_attr(not(windows), allow(dead_code))]
    last_action_seq: AtomicU64,

    // While the dashboard is capturing a new binding, global hotkeys have to
    // stand down: `RegisterHotKey` swallows the combo system-wide, so the
    // browser never sees the keypress and the action fires instead of being
    // recorded. Holds the deadline (process-relative ms) rather than a flag,
    // so a dashboard that goes away mid-capture cannot leave them off.
    #[cfg_attr(not(windows), allow(dead_code))]
    hotkey_capture_until_ms: AtomicU64,

    // Actions whose hotkey the OS refused to register, because another app
    // already owns the combo. Written by the tray on every (re)bind, read by
    // the dashboard so the row itself can say a binding is dead - the log
    // line alone is somewhere nobody setting a hotkey is looking.
    hotkey_conflicts: crate::sync::Mutex<Vec<String>>,

    // Raised when a hotkey or MIDI action moves the delay state. Those two
    // paths run outside the web layer, which is where every other state
    // change gets written to the config file, so the runtime watches this
    // and persists on their behalf.
    #[cfg_attr(not(windows), allow(dead_code))]
    state_dirty: Notify,
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
            unpublish_received: AtomicBool::new(false),
            last_video_tag_ms: AtomicU64::new(0),
            ingest_frozen: AtomicBool::new(false),
            hold_open: AtomicBool::new(false),
            hold_tail_end_seq: AtomicU64::new(NO_SEQ),
            tail_plays_out: AtomicBool::new(false),
            capacity_capped_logged: AtomicBool::new(false),
            last_rejection_log_ms: AtomicU64::new(0),
            resumed_after_seq: AtomicU64::new(NO_SEQ),
            last_hold_end: AtomicU8::new(0),
            crash_protection: crate::sync::Mutex::new(Default::default()),
            crash_protection_on: AtomicBool::new(false),
            hold: crate::sync::Mutex::new(None),
            slate_cache: Arc::default(),
            destinations: crate::sync::RwLock::new(HashMap::new()),
            ingest_disconnects: AtomicU32::new(0),
            rate_in: RateMeter::default(),
            integrations: std::sync::OnceLock::new(),
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
            held_keyframes: crate::sync::Mutex::new(std::collections::BTreeMap::new()),
            eb_session: crate::sync::Mutex::new(None),
            eb_session_claimed: AtomicBool::new(false),
            publish_lock: Mutex::new(()),
            video_codec: AtomicU8::new(0),
            audio_codec: AtomicU8::new(0),
            eb_vertical_track: AtomicU8::new(NO_TRACK),
            video_dims: AtomicU64::new(0),
            idr_window_open: AtomicBool::new(false),
            first_idr_ts_ms: AtomicU64::new(0),
            last_idr_ts_ms: AtomicU64::new(0),
            idr_gaps: AtomicU32::new(0),
            keyframe_interval_cached: AtomicU32::new(0),
            multitrack_video: AtomicBool::new(false),
            multitrack_audio: AtomicBool::new(false),
            second_audio_track: AtomicBool::new(false),
            seq_header_gen: AtomicU32::new(0),
            last_input_ts_u32: AtomicU32::new(0),
            input_ts_wrap_high: AtomicU32::new(0),
            input_ts_seen: AtomicBool::new(false),
            last_multitrack_video_ms: AtomicU64::new(0),
            backpressure_since_ms: AtomicU64::new(0),
            shutdown_notify: Notify::new(),
            shutdown_kind: AtomicU8::new(0),
            started: std::time::Instant::now(),
            logs: crate::sync::Mutex::new(std::collections::VecDeque::with_capacity(LOG_LINES_MAX)),
            midi: Arc::new(crate::midi::MidiState::new()),
            last_action: crate::sync::Mutex::new(None),
            last_action_seq: AtomicU64::new(0),
            hotkey_capture_until_ms: AtomicU64::new(0),
            hotkey_conflicts: crate::sync::Mutex::new(Vec::new()),
            state_dirty: Notify::new(),
        }
    }

    /// Shared MIDI state, for the listener thread and the web endpoints.
    pub fn midi(&self) -> &Arc<crate::midi::MidiState> {
        &self.midi
    }

    /// Actions whose hotkey another app owns, for the dashboard.
    pub fn hotkey_conflicts(&self) -> Vec<String> {
        self.hotkey_conflicts.lock().clone()
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
    /// Remember which track the config just handed to OBS puts the
    /// vertical canvas on.
    pub fn note_eb_config(&self, config: &str) {
        let track = crate::local_eb_config::vertical_track(config).unwrap_or(NO_TRACK);
        self.eb_vertical_track.store(track, Ordering::Relaxed);
    }
    /// The vertical track of the last config handed to OBS.
    pub fn eb_vertical_track(&self) -> Option<u8> {
        Some(self.eb_vertical_track.load(Ordering::Relaxed)).filter(|&t| t != NO_TRACK)
    }
    /// The portrait (9:16) track OBS is sending now. Only H.264 headers can
    /// be measured; for any other codec, the vertical track the Enhanced
    /// Broadcasting config named counts once OBS is actually sending it.
    pub fn vertical_track_on_wire(&self) -> Option<u8> {
        let headers = self.ring.video_seq_headers.lock();
        crate::h264::detect_vertical_primary_track(&headers).or_else(|| {
            self.eb_vertical_track()
                .filter(|track| headers.contains_key(track))
        })
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
            self.emit(Event::new(EventKind::EbDetected));
        }
    }
    pub fn note_multitrack_audio(&self, payload: &[u8]) {
        if !self.multitrack_audio.swap(true, Ordering::Relaxed) {
            self.log("ingest: multi-track audio detected (VOD audio track) - forwarding as-is.");
        }
        if !self.second_audio_track.load(Ordering::Relaxed)
            && crate::h264::multitrack_audio_has_track(payload, 1)
        {
            self.second_audio_track.store(true, Ordering::Relaxed);
        }
    }

    /// Whether the audio track `egress` asks for is in this stream (the live
    /// track always is). See `h264::select_audio_bytes`.
    pub fn audio_target_on_wire(&self, egress: crate::h264::AudioEgress) -> bool {
        match egress {
            crate::h264::AudioEgress::Passthrough | crate::h264::AudioEgress::Track(0) => true,
            crate::h264::AudioEgress::Track(1) => self.second_audio_track.load(Ordering::Relaxed),
            crate::h264::AudioEgress::Track(_) => false,
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
        self.second_audio_track.store(false, Ordering::Relaxed);
        self.last_multitrack_video_ms.store(0, Ordering::Relaxed);
        self.backpressure_since_ms.store(0, Ordering::Relaxed);
        self.last_input_ts_u32.store(0, Ordering::Relaxed);
        self.input_ts_wrap_high.store(0, Ordering::Relaxed);
        self.input_ts_seen.store(false, Ordering::Relaxed);
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
    /// Wrap detection rule: consecutive tags are never more than half the
    /// u32 space apart, so of the two possible distances between this
    /// timestamp and the last one, the shorter is the real one. That reading
    /// has to work in both directions:
    ///
    /// - *ahead* of the last tag but numerically below it: the counter
    ///   rolled over, so this tag belongs to the next epoch.
    /// - *behind* the last tag but numerically above it: this tag belongs to
    ///   the epoch we just left.
    ///
    /// The second case is what audio costs us. Audio and video interleave
    /// and cross each other by a few milliseconds constantly, so at the
    /// instant the counter rolls, one track is over the line and the other
    /// is not. Reading that straggler in the new epoch puts one tag 49.7
    /// days in the future, and `on_tag` hands exactly that value to
    /// `trim_older_than`, whose cutoff then sits past every frame in the
    /// ring: a single late audio tag evicts the entire delay and drops
    /// every viewer to live. Found by the 10,000-hour soak.
    ///
    /// Reading a straggler back into the previous epoch is self-correcting.
    /// The next in-epoch tag is once again below the stored `last` and rolls
    /// the counter forward again, so an alternating audio/video pattern
    /// across the boundary resolves each tag to its own correct epoch.
    ///
    /// Smaller backward jumps inside one epoch stay ordinary interleaving
    /// and change nothing here; pace_and_send drops those separately.
    fn expand_ts(&self, wire_ts: u32) -> u64 {
        let last = self.last_input_ts_u32.load(Ordering::Relaxed);
        let mut wrap_high = self.input_ts_wrap_high.load(Ordering::Relaxed);
        if !self.input_ts_seen.swap(true, Ordering::Relaxed) {
            // The first tag of a session defines where its timeline starts,
            // whatever it says. An encoder's clock does not have to begin
            // near zero - OBS reconnecting into a still-running session
            // carries on from wherever it had got to.
            self.last_input_ts_u32.store(wire_ts, Ordering::Relaxed);
            return wire_ts as u64;
        }
        if wire_ts.wrapping_sub(last) <= (1u32 << 31) {
            if wire_ts < last {
                wrap_high = wrap_high.wrapping_add(1);
            }
        } else if wire_ts > last {
            if wrap_high == 0 {
                // Behind the timeline with no cycle underneath to fall back
                // to: this tag is stamped before the session began, so it is
                // out of range rather than from a previous cycle. Reading it
                // literally puts it 49.7 days ahead, `on_tag` hands that to
                // the trim, and the cutoff lands past every frame in the
                // ring. Pin it to the moment we are already at, and leave
                // `last` alone - adopting a stamp from nowhere would make
                // the next ordinary tag look like a wrap.
                return last as u64;
            }
            wrap_high -= 1;
        }
        self.input_ts_wrap_high.store(wrap_high, Ordering::Relaxed);
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
                    d.rate_out.kbps(),
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
        self.capacity_capped_logged.store(false, Ordering::Relaxed);
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
        // No publisher means no video arriving: the buffer cannot grow, so
        // "still building" would be a countdown that never ends. Arming
        // stays allowed (that is pre-arm, and it is the point), but there is
        // nothing to switch on until OBS starts.
        if !self.ingest_alive() {
            return Err(ActivateError::NoIngest);
        }
        if !self.buffer_holds(armed) {
            let remaining_ms = armed.saturating_sub(self.buffer_fill_ms());
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

    /// Stand in for a connected publisher, for tests in other modules that
    /// push tags into the ring directly instead of going through
    /// `begin_publish` (see `feed_seconds` for the same idea in this one).
    #[cfg(test)]
    pub fn mark_ingest_alive_for_test(&self) {
        self.ingest_alive.store(true, Ordering::Relaxed);
    }

    /// The most recent hotkey / MIDI action, if there has been one this run.
    pub fn last_action(&self) -> Option<FiredAction> {
        self.last_action.lock().clone()
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
        if !self.buffer_holds(armed) {
            return "preparing";
        }
        "ready"
    }

    /// Whether the buffer holds a delay of `delay_ms`: it spans that much,
    /// or it is full and spans all it ever will at this bitrate (the delay
    /// then runs at what it holds, see `delayed_idr`).
    fn buffer_holds(&self, delay_ms: u32) -> bool {
        self.buffer_fill_ms() + 500 >= delay_ms || self.ring.is_saturated()
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
        self.rate_in.kbps()
    }

    // ---- Internal: ingest counters ----

    /// Called from the ingest path on every audio/video tag.
    pub fn note_inbound_bytes(&self, n: usize) {
        self.rate_in.note(n);
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
        if q.len() >= LOG_LINES_MAX {
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
                    self.log_rejected_publish("ingest: rejected publisher (rate limited)");
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
                    self.log_rejected_publish("ingest: rejected publisher (wrong stream key)");
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
            if !self.video_frozen_at(process_now_ms()) {
                self.log_rejected_publish("ingest: rejected second publisher (slot in use)");
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "another publisher is already active",
                ));
            }
            // The publisher holding the slot has sent no video for
            // FREEZE_AFTER: a hung OBS whose connection the OS keeps alive
            // would otherwise lock a restarted OBS out indefinitely. Hand the
            // slot over; the new token shuts the old connection out (see
            // `end_publish`). A freeze hold, if one is up, resumes below as
            // for any returning publisher.
            self.log("ingest: the connected publisher stopped sending video - handing over to the new one");
            self.ingest_alive.store(false, Ordering::Relaxed);
            self.note_ingest_disconnect();
            self.reset_codec_state();
        }

        // Wipe every per-session cache that survives mark_ingest_dead.
        // The seq-header caches deliberately persist across an in-session
        // egress restart (see `every_track_config_leads_each_twitch_connection`) so
        // we clear them only when a new publisher takes the slot -
        // otherwise stale multi-track SPS/PPS leak into a non-EB
        // session and freeze the destination's decoder.
        self.ring.video_seq_headers.lock().clear();
        self.ring.audio_seq_headers.lock().clear();
        self.held_keyframes.lock().clear();
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

        // A goodbye from an earlier connection must not mark this session's
        // end as a clean stop.
        self.unpublish_received.store(false, Ordering::Relaxed);
        self.last_video_tag_ms.store(0, Ordering::Relaxed);
        self.ingest_frozen.store(false, Ordering::Relaxed);
        // A hold past its deadline already ended for the pumps. Close it as
        // ended while ingest still reads as dead, so the sessions it kept
        // are forgotten rather than carried into this stream.
        self.expire_hold();
        let resuming = self.hold_active();
        if resuming {
            self.forget_unclaimed_eb_session();
        }
        // Bump token so any prior egress reader knows it's stale.
        let token = self.publisher_token.fetch_add(1, Ordering::SeqCst) + 1;
        self.ingest_alive.store(true, Ordering::Relaxed);
        self.resume_from_hold();
        if resuming {
            // Freeze detection only starts at a session's first video
            // frame, so an OBS that reconnects and never sends any would
            // keep the screen up with no deadline. Start its clock now,
            // with room for a slow encoder start before it counts.
            let grace = FIRST_VIDEO_GRACE.saturating_sub(crate::crash_hold::FREEZE_AFTER);
            self.last_video_tag_ms.store(
                process_now_ms() + grace.as_millis() as u64,
                Ordering::Relaxed,
            );
        }
        self.log("ingest: publisher connected");
        self.emit(Event::new(EventKind::ObsConnected));
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
        // Sample the encoder parameters the compatibility check compares
        // against the enabled destinations. Both are cheap: the dimension
        // decode runs once per sequence header, and the keyframe sampler
        // stops touching anything after its first few gaps.
        if kind == 9 {
            if is_seq {
                self.note_video_dimensions(payload);
            } else {
                if self.crash_protection_on.load(Ordering::Relaxed) {
                    self.note_video_for_crash_protection(payload);
                }
                if self.ingest_frozen.load(Ordering::Relaxed) {
                    self.ingest_frozen.store(false, Ordering::Relaxed);
                    // A hold past its deadline already ended for the pumps:
                    // report it as ended, not as OBS being back in time.
                    self.expire_hold();
                    self.resume_from_hold();
                }
                if is_idr {
                    self.sample_keyframe_interval(ts_ms);
                }
            }
        }
        let _ = self.ring.append(kind, ts_ms, payload, is_idr, is_seq);
        // Bump the generation on every seq header so each egress pump
        // knows to re-emit its cached config bytes on the next iteration.
        // After the append: a pump that sees the new generation must find
        // the new header in the cache, or it would resend the old one and
        // never learn of the change.
        if is_seq {
            self.seq_header_gen.fetch_add(1, Ordering::Relaxed);
        }
        // Cap the buffer to the user's armed delay (plus a small slack for
        // IDR alignment). This keeps the on-screen "Buffer N/N s" exactly
        // what the user asked for, instead of growing to the full disk cap.
        // The trim respects the slowest consumer across all destinations
        // so we never evict a tag any pump is still about to read.
        if !is_seq {
            let target = self.effective_target_buffer_ms();
            // Never trim the keyframe a delay of the whole buffer lands on
            // (see `delayed_idr`): with a GOP longer than the slack, it is
            // older than the cutoff, and without it the delay can't join.
            let delay_keyframe = self
                .ring
                .newest_idr_at_or_before(ts_ms.saturating_sub(u64::from(self.target_buffer_ms())))
                .map_or(u64::MAX, |idr| idr.seq);
            let keep_from = self.min_consumer_seq().min(delay_keyframe);
            self.ring.trim_older_than(target, ts_ms, keep_from);
            self.note_capacity_cap();
        }
    }

    /// Say once per armed value when the buffer is full before it spans the
    /// armed delay: the delay then runs at what the buffer holds.
    fn note_capacity_cap(&self) {
        let armed = self.armed_delay_ms();
        let fill = self.buffer_fill_ms();
        if armed == 0
            || self.capacity_capped_logged.load(Ordering::Relaxed)
            || fill + 500 >= armed
            || !self.ring.is_saturated()
        {
            return;
        }
        self.capacity_capped_logged.store(true, Ordering::Relaxed);
        self.log(format!(
            "delay: the buffer is full at {} s at this bitrate, short of the {} s armed, so the              delay runs at {} s. Raise the buffer size in System for the full delay.",
            fill / 1000,
            armed / 1000,
            fill / 1000
        ));
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

    /// Whether a delayed destination can join right now: no delay is on,
    /// or the buffer reaches back to it (see `delayed_idr`).
    pub fn delay_reachable(&self) -> bool {
        let target = u64::from(self.target_delay_ms());
        target == 0 || delayed_idr(self, target).is_some()
    }

    pub fn on_metadata(&self, payload: Vec<u8>) {
        *self.ring.metadata.lock() = Some(payload);
    }

    /// A publishing connection closed. Only the current publisher's closing
    /// counts: one that was handed over (`begin_publish`) has a stale token
    /// and must not end the session that replaced it.
    pub fn end_publish(&self, token: u64) {
        if token == self.publisher_token() {
            self.mark_ingest_dead();
        }
    }

    /// Log a refused publish at most once every 10 s: a peer reconnecting
    /// in a loop would otherwise push everything else out of the bounded
    /// log.
    fn log_rejected_publish(&self, line: &str) {
        // Stamped one ms late, so 0 only ever means "never" (the process
        // clock itself starts at 0).
        let stamp = process_now_ms() + 1;
        let last = self.last_rejection_log_ms.load(Ordering::Relaxed);
        if last != 0 && stamp.saturating_sub(last) < 10_000 {
            return;
        }
        self.last_rejection_log_ms.store(stamp, Ordering::Relaxed);
        self.log(line.to_string());
    }

    pub fn mark_ingest_dead(&self) {
        // Only the live publisher's own disconnect gets here (its
        // PublishGuard), once per session.
        if !self.ingest_alive() {
            return;
        }
        // Decide the hold before ingest reads as dead: a pump that sees
        // ingest gone with no hold open ends its destination.
        let stopped = self.unpublish_received.swap(false, Ordering::Relaxed);
        if !stopped || self.crash_protection.lock().every_disconnect {
            self.start_hold(crate::crash_hold::HoldReason::Crash);
        } else if self.close_hold(HOLD_END_ENDED).is_some() {
            // Stopped on purpose while a freeze hold was up: that ends it.
            self.log("crash protection: OBS stopped the stream - destinations ended");
        }
        // Only count when transitioning alive → dead, so a stray call
        // doesn't inflate the counter.
        if self.ingest_alive.swap(false, Ordering::Relaxed) {
            self.note_ingest_disconnect();
            self.reset_codec_state();
            if stopped {
                self.log("ingest: publisher stopped the stream");
            } else {
                self.log(
                    "ingest: publisher dropped without stopping (crash, kill or network loss)",
                );
            }
            // `protected`: a hold opened and has its own, more useful event.
            self.emit(
                Event::new(EventKind::ObsDisconnected)
                    .with("stopped", if stopped { "yes" } else { "no" })
                    .with("protected", if self.hold_active() { "yes" } else { "no" }),
            );
            // Clear any Enhanced Broadcasting URL overrides on the
            // way out - the next stream may or may not be EB, and a
            // stale override would force a non-EB stream onto an IVS
            // endpoint that has no allocated session. The
            // /obs/multitrack-config proxy sets a fresh override on
            // every new EB session anyway.
            //
            // A destination a crash-protection hold keeps live keeps its
            // session: OBS coming back continues it (see `held_eb_config`),
            // and the hold ending without OBS forgets it (`close_hold`).
            for (_id, state) in self.all_destination_states() {
                if !self.hold_keeps(&state) {
                    // Clear the override AND bump the session epoch
                    // atomically, so a VOD-session fetch still in flight
                    // discards its stale result instead of writing a
                    // dead-session IVS URL into the next stream (the
                    // late-completion race).
                    state.invalidate_session_override();
                }
                // Note: we deliberately do NOT touch `vod_fetch_pending`
                // here. It's owned solely by the fetch lifecycle (claim sets
                // it, the task clears it on completion within ~15 s). If a
                // fetch is in flight across a disconnect/reconnect, leaving
                // the latch set is what stops a second concurrent fetch from
                // being spawned for the same destination - clearing it here
                // would reintroduce the multi-session bug on a fast restart.
            }
        }
    }

    /// Whether the open hold keeps `dest` live on the reconnect screen.
    fn hold_keeps(&self, dest: &DestinationState) -> bool {
        self.hold_active()
            && dest.egress_alive.load(Ordering::Relaxed)
            && crate::crash_hold::covers(self, dest)
    }

    /// Remember the Enhanced Broadcasting config just handed to OBS and
    /// the Twitch session it opened.
    pub fn remember_eb_session(&self, session: EbSession) {
        *self.eb_session.lock() = Some(session);
        // OBS asked for this session, so it is Enhanced Broadcasting's own
        // even when a hold is open (one that had no session to hand back):
        // OBS publishing next must continue it, not drop it as stale.
        self.eb_session_claimed.store(true, Ordering::Relaxed);
    }

    /// The config to hand OBS again when it asks during a hold that is
    /// keeping its Twitch session live: the same tracks and session token,
    /// so its new publish continues the session instead of opening another
    /// one (which would restart the destination once OBS is back).
    pub fn held_eb_config(&self) -> Option<String> {
        if !self.hold_active() {
            return None;
        }
        let session = self.eb_session.lock().clone()?;
        let dest = self.destinations.read().get(&session.dest_id).cloned()?;
        if dest.eb_override_url.lock().as_deref() != Some(session.ivs_url.as_str()) {
            return None;
        }
        // The tokens were remembered when the session opened, possibly
        // longer ago than their TTL.
        for auth in &session.auths {
            self.remember_eb_key(auth.clone());
        }
        self.eb_session_claimed.store(true, Ordering::Relaxed);
        Some(session.config)
    }

    /// OBS is publishing again during a hold without having asked for an
    /// Enhanced Broadcasting config (held or fresh) since the hold opened:
    /// Enhanced Broadcasting is off now, and that multitrack session can't
    /// take a single-track stream, so the destination starts a fresh one.
    fn forget_unclaimed_eb_session(&self) {
        if self.eb_session_claimed.load(Ordering::Relaxed) {
            return;
        }
        let Some(session) = self.eb_session.lock().take() else {
            return;
        };
        if let Some(dest) = self.destinations.read().get(&session.dest_id) {
            if dest.eb_override_url.lock().as_deref() == Some(session.ivs_url.as_str()) {
                dest.invalidate_session_override();
                self.log(format!(
                    "[{}] OBS came back without Enhanced Broadcasting - starting a new \
                     Twitch session",
                    session.dest_id
                ));
            }
        }
    }

    /// The newest ring seq when OBS came back from the last hold, or None
    /// when the ring was empty (a new publisher starts it afresh).
    pub fn resumed_after_seq(&self) -> Option<u64> {
        Some(self.resumed_after_seq.load(Ordering::Relaxed)).filter(|seq| *seq != NO_SEQ)
    }

    /// The latest keyframe of a track the hold re-sends instead of the
    /// reconnect screen.
    pub fn held_keyframe(&self, track: u8) -> Option<Arc<[u8]>> {
        self.held_keyframes.lock().get(&track).cloned()
    }

    /// The live publisher sent FCUnpublish or deleteStream: it is ending
    /// the stream on purpose, so its disconnect is a stop, not a crash.
    pub fn note_unpublish(&self) {
        self.unpublish_received.store(true, Ordering::Relaxed);
    }

    /// Mirror the crash-protection settings (the supervisor calls this).
    pub fn update_crash_protection(&self, settings: crate::crash_protection::CrashProtection) {
        self.crash_protection_on
            .store(settings.enabled, Ordering::Relaxed);
        if !settings.enabled {
            // Off means no bookkeeping, and nothing stale left behind: an
            // old video stamp would read as a freeze and hold back egress.
            self.last_video_tag_ms.store(0, Ordering::Relaxed);
            self.held_keyframes.lock().clear();
            self.slate_cache.clear();
        }
        *self.crash_protection.lock() = settings;
    }

    /// Per video frame while crash protection is on: when video last
    /// arrived (for freeze detection), and the latest keyframe of any track
    /// the reconnect screen can't replace.
    fn note_video_for_crash_protection(&self, payload: &[u8]) {
        // Never 0: that means "no video yet this session".
        self.last_video_tag_ms
            .store(process_now_ms().max(1), Ordering::Relaxed);
        if crate::crash_hold::is_held_keyframe(payload) {
            let track = crate::h264::seq_header_track_id(payload);
            self.held_keyframes.lock().insert(track, Arc::from(payload));
        }
    }

    pub fn crash_protection(&self) -> crate::crash_protection::CrashProtection {
        self.crash_protection.lock().clone()
    }

    /// Whether destinations are being held on the reconnect screen now.
    pub fn hold_active(&self) -> bool {
        self.hold_open.load(Ordering::Relaxed)
            && self.hold_state() == crate::crash_hold::HoldState::Holding
    }

    /// Where the hold stands. A hold past its deadline already counts as
    /// ended, before `expire_hold` gets round to reporting it.
    pub fn hold_state(&self) -> crate::crash_hold::HoldState {
        use crate::crash_hold::HoldState;
        match *self.hold.lock() {
            Some(hold) if Instant::now() < hold.deadline => HoldState::Holding,
            Some(_) => HoldState::Ended,
            None if self.last_hold_end.load(Ordering::Relaxed) == HOLD_END_RESUMED => {
                HoldState::Resumed
            }
            None => HoldState::Ended,
        }
    }

    /// The hold while it is on air, for the dashboard, dock and tray.
    pub fn hold_status(&self) -> Option<crate::crash_hold::HoldStatus> {
        let hold = (*self.hold.lock())?;
        Some(crate::crash_hold::HoldStatus {
            reason: hold.reason,
            remaining: hold.deadline.checked_duration_since(Instant::now())?,
            total: hold.deadline - hold.started,
        })
    }

    /// Open a hold when crash protection is on and a destination is live
    /// to protect.
    fn start_hold(&self, reason: crate::crash_hold::HoldReason) {
        let settings = self.crash_protection();
        let any_live = self
            .all_destination_states()
            .iter()
            .any(|(_, state)| state.egress_alive.load(Ordering::Relaxed));
        if !settings.enabled || !any_live {
            return;
        }
        let hold_for = Duration::from_secs(u64::from(settings.hold_secs));
        let now = Instant::now();
        {
            let mut hold = self.hold.lock();
            if hold.is_some() {
                return;
            }
            *hold = Some(crate::crash_hold::Hold {
                reason,
                started: now,
                deadline: now + hold_for,
            });
        }
        self.hold_open.store(true, Ordering::Relaxed);
        self.hold_tail_end_seq
            .store(self.ring.latest_seq().unwrap_or(NO_SEQ), Ordering::Relaxed);
        self.tail_plays_out.store(false, Ordering::Relaxed);
        self.eb_session_claimed.store(false, Ordering::Relaxed);
        if reason == crate::crash_hold::HoldReason::Freeze {
            self.ingest_frozen.store(true, Ordering::Relaxed);
        }
        let window = crate::crash_hold::minutes_seconds(hold_for);
        let what = match reason {
            crate::crash_hold::HoldReason::Crash => "OBS dropped",
            crate::crash_hold::HoldReason::Freeze => "OBS stopped sending video",
        };
        self.log(format!(
            "crash protection: {what} - destinations stay live on the reconnect screen for up to {window}"
        ));
        self.emit(
            Event::new(EventKind::HoldOpened)
                .with(
                    "reason",
                    match reason {
                        crate::crash_hold::HoldReason::Crash => "crash",
                        crate::crash_hold::HoldReason::Freeze => "freeze",
                    },
                )
                .with("hold", fmt_duration(hold_for.as_millis() as u64)),
        );
    }

    /// Close the open hold, remembering how it ended for the pumps.
    fn close_hold(&self, end: u8) -> Option<crate::crash_hold::Hold> {
        // How it ended is stored before the lock is released, so
        // `hold_state` never reads "no hold" with the previous hold's end.
        let hold = {
            let mut open = self.hold.lock();
            let hold = open.take()?;
            self.last_hold_end.store(end, Ordering::Relaxed);
            hold
        };
        self.hold_open.store(false, Ordering::Relaxed);
        // OBS never came back: forget the sessions the hold kept for it
        // (see `mark_ingest_dead`); ending the hold ended them on Twitch. A
        // frozen OBS is still connected, so its session is still the
        // current one, and a config OBS asked for during the hold (the kept
        // session or a new one) is the one it is about to publish on.
        let obs_is_coming_back = self.eb_session_claimed.load(Ordering::Relaxed);
        if end == HOLD_END_ENDED && !self.ingest_alive() && !obs_is_coming_back {
            for (_id, state) in self.all_destination_states() {
                state.invalidate_session_override();
            }
        }
        Some(hold)
    }

    /// OBS is sending again (a new publisher, or video after a freeze).
    /// Runs before the tag that brought it back reaches the ring.
    fn resume_from_hold(&self) {
        let after = self.ring.latest_seq().unwrap_or(NO_SEQ);
        self.resumed_after_seq.store(after, Ordering::Relaxed);
        self.tail_plays_out.store(false, Ordering::Relaxed);
        self.hold_tail_end_seq.store(NO_SEQ, Ordering::Relaxed);
        let Some(hold) = self.close_hold(HOLD_END_RESUMED) else {
            return;
        };
        let lasted = crate::crash_hold::minutes_seconds(hold.started.elapsed());
        self.log(format!(
            "crash protection: OBS is back after {lasted} - resuming live"
        ));
        self.emit(Event::new(EventKind::ObsBack).with(
            "down_for",
            fmt_duration(hold.started.elapsed().as_millis() as u64),
        ));
    }

    /// Whether the publisher has sent no video for `FREEZE_AFTER` as of
    /// `now_ms` (process ms). Never true before its first video frame.
    fn video_frozen_at(&self, now_ms: u64) -> bool {
        let last = self.last_video_tag_ms.load(Ordering::Relaxed);
        last != 0
            && now_ms.saturating_sub(last) >= crate::crash_hold::FREEZE_AFTER.as_millis() as u64
    }

    /// A connected OBS that has sent no video for `FREEZE_AFTER` is frozen:
    /// open a hold for it (the supervisor calls this every tick).
    pub fn check_ingest_freeze(&self) {
        self.check_ingest_freeze_at(process_now_ms());
    }

    fn check_ingest_freeze_at(&self, now_ms: u64) {
        let latched = self.ingest_frozen.load(Ordering::Relaxed);
        if !latched
            && self.ingest_alive()
            && self.video_frozen_at(now_ms)
            && self.hold.lock().is_none()
        {
            self.start_hold(crate::crash_hold::HoldReason::Freeze);
        }
    }

    /// Whether OBS is connected and actually sending video, as opposed to
    /// connected but frozen. Egress waits for this before connecting.
    pub fn ingest_sending(&self) -> bool {
        self.ingest_alive() && !self.video_frozen_at(process_now_ms())
    }

    /// OBS is gone: disconnected, or frozen with a freeze hold opened for
    /// it and no video since. Nothing new reaches the buffer either way.
    pub fn obs_gone(&self) -> bool {
        !self.ingest_alive() || self.ingest_frozen.load(Ordering::Relaxed)
    }

    /// Whether a destination still airing the delay tail of an OBS that is
    /// gone finishes it before ending: yes once crash protection's time ran
    /// out (the tail airs in full), not when the streamer ended the hold.
    fn tail_plays_out(&self) -> bool {
        self.tail_plays_out.load(Ordering::Relaxed)
            || matches!(*self.hold.lock(), Some(open) if Instant::now() >= open.deadline)
    }

    /// The last tag of the delay tail: the newest when the hold opened, or
    /// the newest now when no hold marked it.
    fn delay_tail_end_seq(&self) -> Option<u64> {
        match self.hold_tail_end_seq.load(Ordering::Relaxed) {
            NO_SEQ => self.ring.latest_seq(),
            seq => Some(seq),
        }
    }

    /// Close a hold whose time ran out. The pumps end their sessions at the
    /// deadline on their own; this reports it once (the supervisor calls it
    /// every tick).
    pub fn expire_hold(&self) {
        let expired = matches!(*self.hold.lock(), Some(open) if Instant::now() >= open.deadline);
        let Some(hold) = expired.then(|| self.close_hold(HOLD_END_ENDED)).flatten() else {
            return;
        };
        self.tail_plays_out
            .store(self.obs_gone(), Ordering::Relaxed);
        let window = crate::crash_hold::minutes_seconds(hold.deadline - hold.started);
        self.log(format!(
            "crash protection: OBS didn't come back within {window} - destinations ended"
        ));
        self.emit(Event::new(EventKind::HoldExpired).with(
            "hold",
            fmt_duration((hold.deadline - hold.started).as_millis() as u64),
        ));
    }

    /// End the hold now (dashboard, dock, tray or hotkey). True when a hold
    /// was open.
    pub fn end_hold_now(&self) -> bool {
        let Some(hold) = self.close_hold(HOLD_END_ENDED) else {
            return false;
        };
        self.tail_plays_out.store(false, Ordering::Relaxed);
        let lasted = crate::crash_hold::minutes_seconds(hold.started.elapsed());
        self.log(format!(
            "crash protection: ended after {lasted} - destinations ended"
        ));
        self.emit(Event::new(EventKind::HoldEnded).with(
            "down_for",
            fmt_duration(hold.started.elapsed().as_millis() as u64),
        ));
        true
    }

    /// Attach the integrations engine. Called once at startup; events
    /// emitted before it is attached are dropped.
    pub fn attach_integrations(&self, handle: Arc<crate::integrations::Handle>) {
        let _ = self.integrations.set(handle);
    }

    pub fn integrations(&self) -> Option<&Arc<crate::integrations::Handle>> {
        self.integrations.get()
    }

    /// Tell the integrations something happened. Never blocks and needs no
    /// runtime, so it is safe from the tray, hotkey and MIDI threads.
    pub fn emit(&self, event: Event) {
        if let Some(handle) = self.integrations.get() {
            handle.emit(event);
        }
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
}

/// The platform a destination's ingest host belongs to, for the `{platform}`
/// variable of destination events.
fn platform_of_host(host: &str) -> &'static str {
    let host = host.to_ascii_lowercase();
    if host.contains("twitch") {
        "twitch"
    } else if host.contains("youtube") || host.contains("google") {
        "youtube"
    } else if host.contains("kick") || host.contains("live-video.net") {
        "kick"
    } else if host.contains("trovo") {
        "trovo"
    } else if host.contains("restream") {
        "restream"
    } else if host == "127.0.0.1" || host == "localhost" {
        "sink"
    } else {
        "custom"
    }
}

/// The named delay actions, and the hotkey-capture state that guards them.
///
/// Platform-neutral by construction: nothing in here makes a system call.
/// What is Windows-only is who *drives* it - the tray's `RegisterHotKey`
/// loop and the winmm MIDI listener - which is why other platforms compile
/// this and never reach it. Compiling it everywhere is the point: one
/// definition of what "toggle" means, and this suite's tests run on every
/// CI target instead of only on Windows, where a threading or ordering
/// mistake would go unseen until someone shipped a Linux driver.
///
/// The allow is scoped to the platforms that have no driver yet, so real
/// dead code is still an error on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
impl Controller {
    /// Stand global hotkeys down for `window_ms` while the dashboard records
    /// a new binding, or clear the suspension when `window_ms` is 0. The
    /// deadline is the safety net: a browser that dies mid-capture costs the
    /// user one window, not a session with no hotkeys.
    pub fn suspend_hotkeys(&self, window_ms: u32) {
        let until = if window_ms == 0 {
            0
        } else {
            process_now_ms() + window_ms as u64
        };
        self.hotkey_capture_until_ms.store(until, Ordering::Relaxed);
    }

    /// Whether global hotkeys are currently stood down.
    pub fn hotkeys_suspended(&self) -> bool {
        self.hotkeys_suspend_remaining_ms() > 0
    }

    /// How much of the suspension window is left, for the tray's resume
    /// timer. 0 when hotkeys are live.
    pub fn hotkeys_suspend_remaining_ms(&self) -> u32 {
        let until = self.hotkey_capture_until_ms.load(Ordering::Relaxed);
        until.saturating_sub(process_now_ms()) as u32
    }

    /// Replace the set of actions whose hotkey could not be registered.
    /// Empty on every platform without a global-key surface.
    pub fn set_hotkey_conflicts(&self, actions: Vec<String>) {
        *self.hotkey_conflicts.lock() = actions;
    }

    /// Run a named delay action. Shared by the keyboard hotkeys and MIDI
    /// bindings so both trigger identical behaviour. `default_ms` is the
    /// delay armed when starting from nothing; `source` tags the log line
    /// ("hotkey" / "midi"). Unknown action names are ignored. Every call
    /// here is atomic-only, so it is safe to invoke straight from an OS
    /// callback thread with no runtime.
    ///
    /// Returns a short problem message when the action could not do what was
    /// asked, and None when it did. The caller is expected to put that in
    /// front of the user: they are mid-game, they pressed a key, and the
    /// dashboard log they cannot see is the only other record of it.
    pub fn run_named_action(&self, action: &str, default_ms: u32, source: &str) -> Option<String> {
        let problem = self.dispatch_named_action(action, default_ms, source);
        // Publish what just happened before anything else: the dashboard
        // reads this on its next tick and lights up the row that fired,
        // which is the only feedback a user gets when the press landed while
        // they were looking at a game.
        self.record_fired_action(action, source, problem.clone());
        // Arm / activate / cut all move state the dashboard routes persist
        // on their way through. Nothing persists on this path, so ask the
        // runtime to save it - otherwise a delay armed by pad or key is
        // forgotten across a restart.
        self.state_dirty.notify_one();
        problem
    }

    /// Set the delay to `ms` (an integration's "set the delay"): arms it
    /// when nothing is armed, and changes it in place when a delay is armed
    /// or on air. Unlike the `arm` hotkey action, which toggles, this never
    /// disarms, so `!setdelay 30` twice leaves a 30 s delay.
    pub fn set_delay_to(&self, ms: u32, source: &str) {
        let ms = ms.clamp(1000, 600_000);
        self.arm_delay(ms);
        self.log(format!("[{source}] delay set to {} s", ms / 1000));
        self.record_fired_action("arm", source, None);
        self.state_dirty.notify_one();
    }

    fn dispatch_named_action(&self, action: &str, default_ms: u32, source: &str) -> Option<String> {
        match action {
            "toggle" => self.action_toggle(default_ms, source),
            "arm" => self.action_arm(default_ms, source),
            "activate" => self.action_activate(source),
            "cut" => {
                self.stop_delay();
                self.log(format!("[{source}] cut to live"));
                None
            }
            "cut_after" => self.action_cut_after(source),
            "end_hold" => {
                if self.end_hold_now() {
                    return None;
                }
                self.log(format!(
                    "[{source}] end crash protection ignored - nothing is on hold"
                ));
                Some("Crash protection isn't holding anything right now".to_string())
            }
            _ => None,
        }
    }

    /// Delay on/off. Cut to live when a delay is live (or a safe cut is
    /// pending), otherwise arm at the default and go delayed.
    fn action_toggle(&self, default_ms: u32, source: &str) -> Option<String> {
        if self.target_delay_ms() > 0 || self.safe_cut_pending() {
            self.stop_delay();
            self.log(format!("[{source}] delay off - cut to live"));
            return None;
        }
        if !self.ingest_alive() {
            self.log(format!("[{source}] delay on ignored - {NO_INGEST}"));
            return Some(NO_INGEST.to_string());
        }
        let ms = if self.armed_delay_ms() > 0 {
            self.armed_delay_ms()
        } else {
            default_ms
        };
        self.arm_delay(ms);
        match self.activate_delay() {
            Ok(d) => {
                self.log(format!("[{source}] delay on - {} s", d / 1000));
                None
            }
            Err(e) => {
                self.log(format!(
                    "[{source}] delay arming {} s - goes live once the buffer fills",
                    ms / 1000
                ));
                // Armed, but the stream is still going out live: the streamer
                // pressed "delay on" and is not protected yet. Only the buffer
                // case resolves on its own - with nothing publishing, "still
                // filling" would be a wait that never ends.
                Some(match e {
                    ActivateError::BufferShort { .. } => format!(
                        "Buffer still filling - the {} s delay starts as soon as it is ready.",
                        ms / 1000
                    ),
                    other => other.message(),
                })
            }
        }
    }

    /// Arm at the default delay, or free the buffer when it is already armed.
    /// Refused while a delay is on air, because disarming wipes the target
    /// too and one stray press would snap every viewer to live - that is what
    /// "cut" is for.
    fn action_arm(&self, default_ms: u32, source: &str) -> Option<String> {
        if self.target_delay_ms() > 0 {
            self.log(format!(
                "[{source}] arm ignored - delay is on air, cut to live first"
            ));
            return Some("The delay is on air. Cut to live first, then disarm the buffer.".into());
        }
        if self.armed_delay_ms() > 0 {
            self.arm_delay(0);
            self.log(format!("[{source}] disarmed - buffer freed"));
            return None;
        }
        if !self.ingest_alive() {
            self.log(format!("[{source}] arm ignored - {NO_INGEST}"));
            return Some(NO_INGEST.to_string());
        }
        self.arm_delay(default_ms);
        self.log(format!("[{source}] armed {} s", default_ms / 1000));
        None
    }

    fn action_activate(&self, source: &str) -> Option<String> {
        match self.activate_delay() {
            Ok(d) => {
                self.log(format!("[{source}] activated - {} s delay", d / 1000));
                None
            }
            Err(e) => {
                self.log(format!("[{source}] activate: {}", e.message()));
                Some(e.message())
            }
        }
    }

    /// First press schedules the safe cut, a second cancels the pending one,
    /// so a mistaken press stays reversible.
    fn action_cut_after(&self, source: &str) -> Option<String> {
        if self.safe_cut_pending() {
            self.cancel_safe_cut();
            self.log(format!("[{source}] cut after this airs - cancelled"));
            return None;
        }
        match self.schedule_safe_cut() {
            Ok(_) => {
                self.log(format!("[{source}] cut after this airs - scheduled"));
                None
            }
            Err(e) => {
                self.log(format!("[{source}] cut after: {e}"));
                Some(e.to_string())
            }
        }
    }

    /// Stamp the most recent externally-driven action for the dashboard.
    fn record_fired_action(&self, action: &str, source: &str, problem: Option<String>) {
        let seq = self.last_action_seq.fetch_add(1, Ordering::Relaxed) + 1;
        *self.last_action.lock() = Some(FiredAction {
            seq,
            action: action.to_string(),
            source: source.to_string(),
            problem,
        });
    }

    /// Woken whenever a hotkey or MIDI action changes the delay state, so
    /// the runtime can persist it the way the dashboard routes already do.
    pub fn state_dirty(&self) -> &tokio::sync::Notify {
        &self.state_dirty
    }
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
    let mut waiting_for_delay = false;

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
        // During a crash-protection hold a destination that lost its
        // platform connection reconnects and goes straight back onto the
        // reconnect screen - if the screen can cover it at all.
        let holding = ctrl.hold_active() && crate::crash_hold::covers(&ctrl, &dest);
        if !ctrl.ingest_sending() && !holding {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }
        // With a delay on, join only once the buffer reaches back to it (an
        // OBS restart empties it; a new destination may find it short).
        // Joining sooner would put viewers on live video, then jump back.
        if !holding && !ctrl.delay_reachable() {
            if !waiting_for_delay {
                waiting_for_delay = true;
                ctrl.log(format!(
                    "[{label}] waiting for the buffer to reach the delay before going live"
                ));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        waiting_for_delay = false;
        eprintln!(
            "[egress {}] connecting to {}:{}/{}",
            label, parsed.host, parsed.port, parsed.app
        );
        ctrl.log(format!(
            "[{}] connecting to {}:{}",
            label, parsed.host, parsed.port
        ));
        // Bounded: an edge that accepts TCP and then stalls in the TLS or
        // RTMP handshake would otherwise hold this destination on
        // "connecting" forever, never retrying.
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, EgressClient::connect(&parsed))
            .await
            .unwrap_or_else(|_| {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("no answer within {} s", CONNECT_TIMEOUT.as_secs()),
                ))
            });
        match connected {
            Ok(client) => {
                let connected_at = Instant::now();
                let was_alive = dest.egress_alive.swap(true, Ordering::Relaxed);
                if !was_alive {
                    ctrl.emit(
                        Event::new(EventKind::DestinationLive)
                            .with("destination", label.clone())
                            .with("platform", platform_of_host(&parsed.host)),
                    );
                }
                let sink = client.spawn_reader_drain();
                let pump_result = pump_dest(&ctrl, &dest, sink).await;
                dest.egress_alive.store(false, Ordering::Relaxed);
                // Not reading any more: its position must not hold back the
                // buffer's trim while it reconnects or waits.
                dest.consumer_seq.store(u64::MAX, Ordering::Relaxed);
                // Only a session that stayed up proves the endpoint works.
                // A platform that accepts the publish and drops it at once
                // (a bad key, a refusing edge) backs off like a failed
                // connect instead of being redialled every second forever.
                if connected_at.elapsed() >= STABLE_SESSION {
                    backoff = Duration::from_secs(1);
                }
                // Only a platform connection that failed counts as a
                // reconnect. The pump also ends cleanly when OBS goes away
                // or the destination is switched off; neither is one.
                let dropped = pump_result.is_err();
                if let Err(e) = pump_result {
                    // Twitch/etc. sometimes echo the stream key in error
                    // descriptions ("Authentication failed for live_…").
                    // Scrub before logging or webhooking - otherwise the
                    // key shows up in /logs (screen-shareable) and in
                    // the Discord webhook payload.
                    let safe = scrub_secret(&e.to_string(), &parsed.stream_key);
                    eprintln!("[egress {}] pump error: {}", label, safe);
                    ctrl.log(format!("[{}] disconnected ({})", label, safe));
                    ctrl.emit(
                        Event::new(EventKind::DestinationDropped)
                            .with("destination", label.clone())
                            .with("platform", platform_of_host(&parsed.host))
                            .with("reason", safe.clone()),
                    );
                    if ctrl.destination_alive_summary().0 == 0 {
                        ctrl.emit(Event::new(EventKind::AllDestinationsDown));
                    }
                }
                // Counted here (instead of at the loop tail) so a fresh
                // connect or a connect failure never bumps it: "Egress
                // reconnects: 1" must not be the resting state of a healthy
                // fresh stream.
                if dropped {
                    dest.reconnects.fetch_add(1, Ordering::Relaxed);
                }
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
fn scrub_secret(text: &str, secret: &str) -> String {
    // Characters, not bytes. A stream key is whatever the user pasted, and
    // this runs on the egress error path - so a byte-offset slice here aborts
    // the process (`panic = "abort"`) at the exact moment a destination is
    // already failing, and again on every reconnect, because the key is on
    // disk. The config redactors had the same bug in three places; this was
    // the fourth and it was missed when they were fixed.
    let chars: Vec<char> = secret.chars().collect();
    // An empty secret matches at every character boundary, so `replace`
    // would splice the marker through the whole message.
    if chars.is_empty() {
        return text.to_string();
    }
    // Too short to show a head and a tail without handing over most of it,
    // so it goes entirely. Platform-issued keys are never this short, but a
    // custom RTMP destination takes whatever the user pasted - and passing
    // that through unredacted put it straight into /logs and the Discord
    // payload, which is the one thing this function exists to prevent.
    if chars.len() < 6 {
        return text.replace(secret, "…");
    }
    let head: String = chars[..3].iter().collect();
    let tail: String = chars[chars.len() - 3..].iter().collect();
    text.replace(secret, &format!("{head}…{tail}"))
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

    // Connected to the platform in the middle of a crash-protection hold
    // (the platform connection itself dropped and came back, or this
    // destination was just switched on): go straight onto the reconnect
    // screen, whether OBS crashed or froze, then rejoin on its new video.
    let mut resume_after_ts: Option<u32> = None;
    let mut rejoin_from_seq: Option<u64> = None;
    if ctrl.hold_active() {
        send_sequence_headers(ctrl, dest, &mut sink, 0).await?;
        let outcome = crate::crash_hold::play(ctrl, dest, &mut sink, 0).await?;
        let crate::crash_hold::HoldExit::Resumed {
            rejoin_from_seq: from_seq,
        } = outcome.exit
        else {
            let _ = sink.send_delete_stream().await;
            return Ok(());
        };
        resume_after_ts = Some(outcome.last_ts);
        rejoin_from_seq = Some(from_seq);
        // The seed below is on the publisher OBS came back as.
        state.last_publisher_token = ctrl.publisher_token();
    }

    // Initial seed: if a delay is ALREADY active when this pump spawns
    // (multi-destination case - a second destination added mid-stream
    // while the first is on a 5 s delay), join at the right delayed
    // position. Otherwise we'd briefly emit live frames before
    // compute_delay_cut catches up, producing a visible ~5 s backward
    // jump for viewers of the new destination.
    // No seed means the publisher went away (or this destination was
    // disabled) before a keyframe arrived. Leave rather than hold an open
    // session to the platform with nothing to send down it.
    let seed = match rejoin_from_seq {
        Some(from_seq) => rejoin_idr(ctrl, dest, from_seq).await,
        None => seed_idr(ctrl, dest).await,
    };
    let Some(first_idr) = seed else {
        ctrl.log(format!(
            "[{}] nothing to send - the publisher went away before a keyframe",
            dest.id
        ));
        let _ = sink.send_delete_stream().await;
        return Ok(());
    };
    state.consumer_seq = first_idr.seq;
    state.input_ts_anchor = first_idr.ts_ms;
    state.output_ts_base = resume_after_ts.map_or(0, |ts| ts.wrapping_add(1));
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
        //
        // Crash protection changes that: during a hold the pump first plays
        // out whatever delay is still buffered, then loops the reconnect
        // screen until OBS returns (the reconnect branch below takes over)
        // or the hold ends.
        // A frozen OBS (connected, no video) gets the same treatment.
        //
        // With no hold covering OBS being gone (crash protection off, its
        // time ran out, or the streamer ended it) the session ends. When the
        // time ran out mid-tail, the tail airs in full first: the pump keeps
        // sending until it is out (a delay as long as the hold or longer).
        if ctrl.obs_gone() || ctrl.hold_active() {
            if !ctrl.hold_active() {
                if !ctrl.tail_plays_out() || delay_tail_sent(ctrl, &state) {
                    ctrl.log(format!(
                        "[{}] OBS is gone - closing destination session",
                        dest.id
                    ));
                    let _ = sink.send_delete_stream().await;
                    return Ok(());
                }
            } else if delay_tail_sent(ctrl, &state) {
                let outcome =
                    crate::crash_hold::play(ctrl, dest, &mut sink, last_output_ts(&state)).await?;
                // Continue the timeline from the last slate frame, so the
                // reconnect re-anchor stays strictly after it.
                state.output_ts_base = outcome.last_ts;
                state.input_ts_anchor = state.last_sent_input_ts;
                let crate::crash_hold::HoldExit::Resumed { rejoin_from_seq } = outcome.exit else {
                    let _ = sink.send_delete_stream().await;
                    return Ok(());
                };
                // OBS is back (a new publisher, or the same one after a
                // freeze): rejoin on its new video, a delay back when one is
                // armed. The destination's decoder holds the screen's
                // pictures, so it has to start at a keyframe.
                ctrl.log(format!("[{}] OBS is back - rejoining", dest.id));
                let Some(idr) = rejoin_idr(ctrl, dest, rejoin_from_seq).await else {
                    let _ = sink.send_delete_stream().await;
                    return Ok(());
                };
                state.last_publisher_token = ctrl.publisher_token();
                rejoin_at(ctrl, dest, &mut sink, &mut state, idr).await?;
                continue;
            }
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
            let Some(new_idr) = wait_for_idr(ctrl, dest, Some(watermark)).await else {
                ctrl.log(format!(
                    "[{}] publisher went away again before re-anchoring",
                    dest.id
                ));
                let _ = sink.send_delete_stream().await;
                return Ok(());
            };
            state.last_publisher_token = current_token;
            // OBS came back from a crash while the delay tail was still
            // airing: the rest of the tail went with the old session (a new
            // publisher starts the buffer afresh), and rejoining here would
            // put viewers on live. Cover the gap with the screen until the
            // delay has rebuilt from the new video, then rejoin delayed.
            if resumed_mid_delay(ctrl, dest) {
                let outcome =
                    crate::crash_hold::play(ctrl, dest, &mut sink, last_output_ts(&state)).await?;
                state.output_ts_base = outcome.last_ts;
                state.input_ts_anchor = state.last_sent_input_ts;
                let crate::crash_hold::HoldExit::Resumed { rejoin_from_seq } = outcome.exit else {
                    let _ = sink.send_delete_stream().await;
                    return Ok(());
                };
                let Some(idr) = rejoin_idr(ctrl, dest, rejoin_from_seq).await else {
                    let _ = sink.send_delete_stream().await;
                    return Ok(());
                };
                state.last_publisher_token = ctrl.publisher_token();
                rejoin_at(ctrl, dest, &mut sink, &mut state, idr).await?;
                continue;
            }
            rejoin_at(ctrl, dest, &mut sink, &mut state, new_idr).await?;
            continue;
        }

        // Mid-stream codec / resolution change: OBS pushed a fresh
        // SPS/PPS or AudioSpecificConfig and the cached one is now
        // stale. Send the new bytes on the current output_ts before the
        // next media tag so the decoder reconfigures cleanly.
        let cur_gen = ctrl.seq_header_gen.load(Ordering::Relaxed);
        if cur_gen != dest.last_seq_header_gen.load(Ordering::Relaxed) {
            ctrl.log(format!("[{}] sequence header changed - resending", dest.id));
            // On the last output timestamp, so the resent header lands
            // AFTER anything we've already sent (same trick as apply_cut).
            send_sequence_headers(ctrl, dest, &mut sink, last_output_ts(&state)).await?;
            dest.last_seq_header_gen.store(cur_gen, Ordering::Relaxed);
        }

        let next_real = next_or_wait(&ctrl.ring, state.consumer_seq, 500).await;
        // A new publisher or sequence header can arrive while we wait, and
        // the tag that woke us belongs to it: re-anchor or resend the header
        // above first, or that tag airs on the old timeline or config.
        let header_changed = ctrl.seq_header_gen.load(Ordering::Relaxed)
            != dest.last_seq_header_gen.load(Ordering::Relaxed);
        if header_changed || ctrl.publisher_token() != state.last_publisher_token {
            continue;
        }

        if let Some(meta) = next_real {
            // Fell off the back of the ring (eviction passed this pump), so
            // `next_or_wait` jumped ahead to the oldest keyframe. Take it as
            // a cut: the timeline re-anchors instead of pausing for the
            // skipped stretch, and a vertical destination waits for its
            // own keyframe.
            if meta.seq > state.consumer_seq {
                apply_cut(
                    ctrl,
                    dest,
                    &mut sink,
                    &mut state,
                    PendingCut { target: meta },
                )
                .await?;
                continue;
            }
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

/// Output timestamp of the last tag this pump sent.
fn last_output_ts(state: &EgressState) -> u32 {
    let delta = state
        .last_sent_input_ts
        .saturating_sub(state.input_ts_anchor) as u32;
    state.output_ts_base.wrapping_add(delta)
}

/// OBS just came back from a crash-protection hold, with a delay armed
/// that the screen can hold viewers behind while it rebuilds.
fn resumed_mid_delay(ctrl: &Controller, dest: &DestinationState) -> bool {
    ctrl.crash_protection_on.load(Ordering::Relaxed)
        && ctrl.target_delay_ms() > 0
        && ctrl.hold_state() == crate::crash_hold::HoldState::Resumed
        && crate::crash_hold::covers(ctrl, dest)
}

/// Re-anchor on `idr` and lead with the sequence headers: how every
/// rejoin starts (a new publisher, or OBS back after a hold).
async fn rejoin_at(
    ctrl: &Arc<Controller>,
    dest: &Arc<DestinationState>,
    sink: &mut EgressSink,
    state: &mut EgressState,
    idr: TagMeta,
) -> io::Result<()> {
    reseed_after_publisher_change(state, idr);
    send_sequence_headers(ctrl, dest, sink, state.output_ts_base).await?;
    dest.consumer_seq
        .store(state.consumer_seq, Ordering::Relaxed);
    dest.last_seq_header_gen.store(
        ctrl.seq_header_gen.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );
    Ok(())
}

/// The keyframe to rejoin at once OBS is back: the newest at least a delay
/// back from the live edge, like every delayed join (`delayed_idr`), and
/// never one from before `from_seq`, where OBS's new video starts. Without
/// a delay that is the newest keyframe.
async fn rejoin_idr(
    ctrl: &Arc<Controller>,
    dest: &Arc<DestinationState>,
    from_seq: u64,
) -> Option<TagMeta> {
    let delay = ctrl.target_delay_ms() as u64;
    let delayed = ctrl
        .ring
        .latest_ts()
        .and_then(|latest| {
            ctrl.ring
                .newest_idr_at_or_before(latest.saturating_sub(delay))
        })
        .filter(|idr| idr.seq >= from_seq);
    match delayed.or_else(|| ctrl.ring.oldest_idr_at_or_after(from_seq)) {
        Some(idr) => Some(idr),
        None => wait_for_idr(ctrl, dest, Some(from_seq.saturating_sub(1))).await,
    }
}

/// Whether every tag the ended publisher left in the buffer has been sent
/// (the delay tail), so the reconnect screen can take over.
fn delay_tail_sent(ctrl: &Controller, state: &EgressState) -> bool {
    ctrl.delay_tail_end_seq()
        .is_none_or(|end| state.consumer_seq > end)
}

/// Per-egress-session state. Lost on reconnect; re-anchored from scratch.
struct EgressState {
    consumer_seq: u64,
    input_ts_anchor: u64,      // original input ts of the most recent cut target
    output_ts_base: u32, // output ts assigned to the most recent cut target (RTMP wire is u32)
    wall_anchor: Instant, // wall clock at the most recent cut
    wall_anchor_input_ts: u64, // input ts that pairs with wall_anchor
    last_sent_input_ts: u64, // highest input ts we actually emitted (audio and
    // video interleave a few ms out of order) -
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
    // The wire send carries a u32. The delta is the time since the last
    // cut or reseed, which can run for days, so the truncation relies on
    // RTMP timestamps being modulo 2^32: wrapping_add against
    // output_ts_base gives the same wire value as the untruncated sum, and
    // players accept the wrap at the 49-day mark.
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
            // has `video_egress() == None`. Drop its AUDIO
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
            let egress = dest.audio_egress();
            let Some(selected) =
                crate::h264::select_audio_bytes(io_buf, egress, ctrl.audio_target_on_wire(egress))
            else {
                state.consumer_seq = meta.seq + 1;
                dest.consumer_seq
                    .store(state.consumer_seq, Ordering::Relaxed);
                return Ok(());
            };
            let bytes_out: &[u8] = &selected;
            let tags_so_far = dest.tags_sent.load(Ordering::Relaxed);
            if crate::trace::is_enabled() && (tags_so_far < 20 || tags_so_far.is_multiple_of(200)) {
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
            state.last_sent_input_ts = state.last_sent_input_ts.max(meta.ts_ms);
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
    state.last_sent_input_ts = state.last_sent_input_ts.max(meta.ts_ms);
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

/// The IDR a delay of `target_ms` lands on: the newest one at least that
/// old, so viewers are never closer to live than asked. None until the
/// buffer reaches back that far.
fn delayed_idr(ctrl: &Controller, target_ms: u64) -> Option<TagMeta> {
    let latest = ctrl.ring.latest_ts()?;
    let at_delay = latest
        .checked_sub(target_ms)
        .and_then(|desired| ctrl.ring.newest_idr_at_or_before(desired));
    // A full ring reaches back no further at this bitrate, so a longer
    // delay could never fill: it delays by as much as the ring holds
    // instead of waiting forever (`is_saturated`). Not from its very oldest
    // keyframe: each new tag overwrites the oldest, and a pump parked there
    // would be overwritten as it reads and keep falling off the back.
    at_delay.or_else(|| {
        if !ctrl.ring.is_saturated() {
            return None;
        }
        let oldest = ctrl.ring.oldest_ts()?;
        ctrl.ring
            .newest_idr_at_or_before(oldest + FULL_RING_MARGIN_MS)
            .or_else(|| ctrl.ring.oldest_idr_at_or_after(0))
    })
}

/// How far in from its oldest tag a delay capped by a full ring starts
/// (see `delayed_idr`): room for the reads to stay ahead of the overwrites.
const FULL_RING_MARGIN_MS: u64 = 3_000;

/// How long a connect to a platform (TCP, TLS and the RTMP handshake) may
/// take before it counts as failed and is retried.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a platform session has to stay up before a reconnect starts
/// from the shortest backoff again.
const STABLE_SESSION: Duration = Duration::from_secs(30);

/// A delay lands at or above its target (see `delayed_idr`); this much
/// below it still counts as on target, for send jitter.
const DELAY_JITTER_MS: u64 = 500;

/// Keyframe interval assumed until one is measured (OBS's default).
const DEFAULT_KEYFRAME_INTERVAL_MS: u64 = 2_000;

fn compute_delay_cut(ctrl: &Arc<Controller>, current: &TagMeta) -> Option<PendingCut> {
    let target_delay = ctrl.target_delay_ms.load(Ordering::Relaxed) as u64;
    let latest = ctrl.ring.latest_ts()?;
    let current_delay = latest.saturating_sub(current.ts_ms);
    if target_delay > 0 {
        return compute_delayed_cut(ctrl, current, current_delay, target_delay);
    }
    ctrl.buffer_building.store(false, Ordering::Relaxed);

    // Back to live: the keyframe nearest the live edge. Dead band scales
    // with the measured GOP (see recut_dead_band_ms), so a pump already
    // parked on the best keyframe doesn't re-cut to it every tick.
    let keyframe_interval = ctrl.keyframe_interval_ms();
    if current_delay < recut_dead_band_ms(keyframe_interval) {
        return None;
    }
    // Binary-search on the IDR-only secondary index (~log n over just
    // the keyframes) rather than the old O(n) walk over every tag.
    let target = ctrl
        .ring
        .find_idr_near(latest, idr_search_tolerance_ms(keyframe_interval))?;
    if target.seq == current.seq {
        return None;
    }
    Some(PendingCut { target })
}

/// The cut a delay of `target_delay` needs, if any. A delay lands on the
/// newest keyframe at least that old (see `delayed_idr`), so it sits
/// between the target and one keyframe interval above it. Re-cutting only
/// outside that window means a landed delay never re-trips the check, and
/// viewers are never closer to live than asked, even on long GOPs.
fn compute_delayed_cut(
    ctrl: &Controller,
    current: &TagMeta,
    current_delay: u64,
    target_delay: u64,
) -> Option<PendingCut> {
    let keyframe_interval = ctrl.keyframe_interval_ms();
    let gop = match keyframe_interval {
        0 => DEFAULT_KEYFRAME_INTERVAL_MS,
        measured => u64::from(measured),
    };
    let too_little = current_delay + DELAY_JITTER_MS < target_delay;
    let upper_band = recut_dead_band_ms(keyframe_interval).max(gop + DELAY_JITTER_MS);
    let too_much = current_delay > target_delay + upper_band;
    if !too_little && !too_much {
        ctrl.buffer_building.store(false, Ordering::Relaxed);
        return None;
    }
    // While OBS is gone (crashed, or frozen and held) the buffer can't
    // grow, so the delay only shrinks as the delay tail plays out. Jumping
    // back to restore it would replay the tail instead of finishing it.
    if too_little && (ctrl.hold_active() || ctrl.obs_gone()) {
        return None;
    }
    // "Build buffer first": until the buffer reaches back to the delay,
    // hold position; it fills at real time and the next check cuts.
    let Some(target) = delayed_idr(ctrl, target_delay) else {
        ctrl.buffer_building.store(true, Ordering::Relaxed);
        return None;
    };
    ctrl.buffer_building.store(false, Ordering::Relaxed);
    // A cut has to fix what it was called for. Too much delay moves
    // forward; with a keyframe gap longer than measured, the keyframe at
    // the delay can be the one this pump is already past, and cutting back
    // to it would replay the same stretch on every check. Too little delay
    // moves back, by more than the jitter: on a full ring the oldest
    // keyframe is barely behind this pump, and re-cutting to it would
    // replay a moment of video every check without adding any delay.
    let latest = current_delay + current.ts_ms;
    let worth_it = if too_much {
        target.seq > current.seq
    } else {
        target.seq < current.seq
            && latest.saturating_sub(target.ts_ms) > current_delay + DELAY_JITTER_MS
    };
    worth_it.then_some(PendingCut { target })
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
        // A header that can't be flattened for this destination (only
        // another track's has arrived so far) is not sent at all: a raw
        // multi-track header means nothing to a single-track platform. The
        // right one goes out when it arrives (new generation).
        let selected = pick.and_then(|(_, h)| {
            crate::h264::select_video_bytes(h, crate::h264::VideoEgress::Track(target))
        });
        if let Some(selected) = selected {
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
                if let Some(bytes) =
                    crate::h264::select_audio_bytes(h, egress, ctrl.audio_target_on_wire(egress))
                {
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

/// Wait for a usable IDR, or `None` when there is no longer any point in
/// waiting: the publisher has gone, or this destination was asked to stop.
///
/// Giving up matters more than it looks. By the time a pump calls this it has
/// already opened and authenticated an RTMP session to the platform, so a wait
/// that never ends is the "live but frozen" state the rest of this file exists
/// to avoid: the platform keeps the publish slot, viewers see a stalled
/// stream, and nothing notices, because the task is alive and simply parked.
/// The supervisor will not respawn a task that has not finished.
///
/// `min_seq` selects the two shapes this is used in: `None` for a fresh pump
/// seeding at the newest keyframe, `Some(watermark)` for a pump re-anchoring
/// after the publisher reconnected, which must not accept a keyframe from the
/// session that just ended.
///
/// The periodic wake is not a poll for tags - `on_append` covers those. It is
/// there because losing the publisher raises no append, so without it the
/// escape condition would never be looked at.
async fn wait_for_idr(
    ctrl: &Arc<Controller>,
    dest: &Arc<DestinationState>,
    min_seq: Option<u64>,
) -> Option<TagMeta> {
    loop {
        // Register notification *before* checking - guarantees we don't
        // miss an append that lands between the check and the await.
        let notified = ctrl.ring.on_append.notified();
        let found = match min_seq {
            Some(seq) => ctrl.ring.newest_idr_after(seq),
            None => ctrl.ring.newest_idr(),
        };
        if let Some(m) = found {
            return Some(m);
        }
        if !ctrl.ingest_alive() || dest.shutdown_requested.load(Ordering::Relaxed) {
            return None;
        }
        tokio::select! {
            _ = notified => {}
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    }
}

/// Pick the seed IDR for a freshly spawned egress pump. With a delay on,
/// join at the delayed position, waiting for the buffer to reach back that
/// far (`run_egress` normally waits before connecting); a live start would
/// air live video, then jump back. With no delay, the newest IDR.
async fn seed_idr(ctrl: &Arc<Controller>, dest: &Arc<DestinationState>) -> Option<TagMeta> {
    loop {
        let target = u64::from(ctrl.target_delay_ms());
        if target == 0 {
            return wait_for_idr(ctrl, dest, None).await;
        }
        let notified = ctrl.ring.on_append.notified();
        if let Some(idr) = delayed_idr(ctrl, target) {
            return Some(idr);
        }
        if !ctrl.ingest_alive() || dest.shutdown_requested.load(Ordering::Relaxed) {
            return None;
        }
        tokio::select! {
            _ = notified => {}
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    }
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
mod sim;

#[cfg(test)]
mod hold_sim;

#[cfg(test)]
mod delay_sim;

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 MB/s for three seconds reads 8000 kbps; two and a half seconds
    /// after the last byte it reads 0 (the stuck "17.40 Mbps" after OBS
    /// stopped); and the first bytes after the gap start a fresh window
    /// instead of averaging the silence in.
    #[test]
    fn a_bitrate_drops_to_zero_when_bytes_stop() {
        let meter = RateMeter::default();
        for ms in (0..=3_000).step_by(100) {
            meter.note_at(100_000, 10_000 + ms);
        }
        assert_eq!(meter.kbps_at(13_000), 8_000);
        assert_eq!(meter.kbps_at(15_400), 8_000, "a short pause keeps the rate");
        assert_eq!(meter.kbps_at(15_600), 0, "stale after 2.5 s");
        for ms in (0..=1_000).step_by(100) {
            meter.note_at(50_000, 30_000 + ms);
        }
        assert_eq!(meter.kbps_at(31_000), 4_000, "no silence averaged in");
    }
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
        // Tags only ever arrive from a connected publisher, but these tests
        // push them straight into the ring rather than going through
        // `begin_publish`. Say the publisher is live to match, or every
        // activate refuses with `NoIngest` - true of the harness, not of
        // what it is modelling. Tests that exercise the publish handshake
        // itself never call this, so their state stays untouched.
        ctrl.ingest_alive.store(true, Ordering::Relaxed);
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

    /// The mark fires only once every live destination has aired past it:
    /// the slowest one decides, a dead destination (whatever its cursor)
    /// has no say, a cursor sitting ON the mark has not aired it yet, and
    /// with nothing live there is nothing to measure against.
    #[test]
    fn safe_cut_fires_only_after_slowest_consumer_passes_mark() {
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 4, 30);
        h.ctrl.activate_delay().expect("buffer is past armed");
        let mark_seq = h.ctrl.ring.latest_seq().expect("ring has tags");
        h.ctrl.schedule_safe_cut().expect("delay is active");
        // More stream arrives, all of it after the mark.
        feed_seconds(&h.ctrl, 4_000, 2, 30);
        let still_pending = |why: &str| {
            h.ctrl.maybe_fire_safe_cut();
            assert!(h.ctrl.safe_cut_pending(), "{why}");
            assert_eq!(h.ctrl.target_delay_ms(), 2_000, "{why}");
        };
        let dest = |id: &str, alive: bool, seq: u64| {
            let st = h.ctrl.destination_state(id);
            st.egress_alive.store(alive, Ordering::Relaxed);
            st.consumer_seq.store(seq, Ordering::Relaxed);
        };

        dest("slow", false, 10);
        dest("fast", false, mark_seq + 5);
        dest("dead", false, 0);
        still_pending("no live destination to measure against");

        dest("slow", true, 10);
        dest("fast", true, mark_seq + 5);
        still_pending("the slower live destination has not aired the mark");

        dest("slow", true, mark_seq);
        still_pending("the mark's own frame has not gone out yet");

        dest("slow", true, mark_seq + 1);
        h.ctrl.maybe_fire_safe_cut();
        assert!(!h.ctrl.safe_cut_pending(), "aired everywhere live - fires");
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
        // The old publisher goes away first, the way a real disconnect
        // frees the slot - feeding tags above stands in for it having been
        // live.
        h.ctrl.mark_ingest_dead();
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
    fn cut_consumes_auto_activate_pending_so_it_sticks() {
        // The bug-of-record: without this clear, the supervisor sees
        // phase revert to "ready" after cut and re-fires activate_delay.
        // Cut while the buffer is still preparing: the slot is still set
        // (nothing has activated yet), and the streamer's "stay live" has
        // to win over the auto-activate the buffer filling would trigger.
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        assert!(h.ctrl.auto_activate_pending(), "armed, not yet activated");
        h.ctrl.stop_delay();
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

    // ── Named action (Hotkey & MIDI) test suite ──────────────────────

    #[test]
    fn named_action_arm_toggles_arming_and_disarming() {
        let h = harness(0);
        // Arming needs a publisher, so model one.
        h.ctrl.mark_ingest_alive_for_test();
        assert_eq!(h.ctrl.armed_delay_ms(), 0);

        // 1st press: arms the delay
        h.ctrl.run_named_action("arm", 5_000, "hotkey");
        assert_eq!(h.ctrl.armed_delay_ms(), 5_000);
        assert_eq!(h.ctrl.phase(), "preparing");

        // 2nd press: disarms and frees buffer target
        h.ctrl.run_named_action("arm", 5_000, "hotkey");
        assert_eq!(h.ctrl.armed_delay_ms(), 0);
        assert_eq!(h.ctrl.phase(), "idle");
    }

    #[test]
    fn setting_the_delay_twice_never_disarms_and_updates_a_live_delay() {
        let h = harness(0);
        h.ctrl.mark_ingest_alive_for_test();
        h.ctrl.set_delay_to(30_000, "integration");
        h.ctrl.set_delay_to(30_000, "integration");
        assert_eq!(h.ctrl.armed_delay_ms(), 30_000, "a repeat keeps it armed");

        let h = harness(0);
        h.ctrl.arm_delay(3_000);
        feed_seconds(&h.ctrl, 0, 5, 30);
        h.ctrl.activate_delay().unwrap();
        h.ctrl.set_delay_to(2_000, "integration");
        assert_eq!(
            h.ctrl.target_delay_ms(),
            2_000,
            "changed on air, not refused"
        );
        assert_eq!(h.ctrl.phase(), "active");
    }

    #[test]
    fn named_action_arm_never_disarms_a_delay_on_air() {
        // Disarming wipes the target as well, so an arm press while the
        // delay is live would cut every viewer to live. Misclick guard.
        let h = harness(0);
        h.ctrl.arm_delay(3_000);
        feed_seconds(&h.ctrl, 0, 5, 30);
        h.ctrl.activate_delay().unwrap();

        let problem = h.ctrl.run_named_action("arm", 5_000, "hotkey");

        assert!(
            problem.is_some(),
            "a refused action must hand the caller something to show"
        );
        assert_eq!(h.ctrl.target_delay_ms(), 3_000, "must stay delayed");
        assert_eq!(h.ctrl.armed_delay_ms(), 3_000);
        assert_eq!(h.ctrl.phase(), "active");
    }

    #[test]
    fn named_action_toggle_switches_between_live_and_delayed() {
        let h = harness(0);

        // From idle with buffer ready: 1st toggle arms and activates delay
        feed_seconds(&h.ctrl, 0, 5, 30);
        h.ctrl.run_named_action("toggle", 3_000, "hotkey");
        assert_eq!(h.ctrl.target_delay_ms(), 3_000);
        assert_eq!(h.ctrl.phase(), "active");

        // 2nd toggle cuts back to live (target = 0, but armed remains preserved)
        h.ctrl.run_named_action("toggle", 3_000, "hotkey");
        assert_eq!(h.ctrl.target_delay_ms(), 0);
        assert_eq!(h.ctrl.armed_delay_ms(), 3_000);
        assert_eq!(h.ctrl.phase(), "ready");

        // 3rd toggle re-activates using the preserved armed duration
        h.ctrl.run_named_action("toggle", 0, "hotkey");
        assert_eq!(h.ctrl.target_delay_ms(), 3_000);
        assert_eq!(h.ctrl.phase(), "active");
    }

    #[test]
    fn named_action_reports_only_what_it_could_not_do() {
        // The tray turns a returned message into a balloon, so anything that
        // worked has to stay quiet: a toast per keypress would land on a
        // display-captured scene mid-stream.
        let h = harness(0);
        // These presses model a streamer who is live.
        h.ctrl.mark_ingest_alive_for_test();
        assert_eq!(h.ctrl.run_named_action("arm", 5_000, "hotkey"), None);
        assert_eq!(h.ctrl.run_named_action("arm", 5_000, "hotkey"), None);
        assert_eq!(h.ctrl.run_named_action("cut", 0, "hotkey"), None);
        assert_eq!(h.ctrl.run_named_action("nonsense", 0, "hotkey"), None);

        // Activating with nothing armed cannot work, and says so.
        assert!(h.ctrl.run_named_action("activate", 0, "hotkey").is_some());

        // Neither can toggling on before the buffer holds the delay.
        let cold = harness(0);
        cold.ctrl.mark_ingest_alive_for_test();
        assert!(cold
            .ctrl
            .run_named_action("toggle", 5_000, "hotkey")
            .is_some());
        assert_eq!(cold.ctrl.armed_delay_ms(), 5_000, "it armed even so");
    }

    /// A delay cannot start with nothing arriving: the buffer holds no
    /// video and can never fill, so "still building" would be a countdown
    /// that never ends. Arming stays allowed - that is pre-arm.
    #[test]
    fn activate_refuses_while_nothing_is_publishing() {
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        assert!(
            matches!(h.ctrl.activate_delay(), Err(ActivateError::NoIngest)),
            "no publisher means no delay to switch on"
        );
        assert_eq!(h.ctrl.armed_delay_ms(), 2_000, "but it stays armed");

        // With a publisher and a filled buffer it works as before.
        feed_seconds(&h.ctrl, 0, 4, 30);
        assert_eq!(
            h.ctrl.activate_delay().expect("buffer is past armed"),
            2_000
        );

        // And the publisher going away does not retroactively cut it.
        h.ctrl.mark_ingest_dead();
        assert_eq!(h.ctrl.target_delay_ms(), 2_000);
    }

    /// Recording a combo that is already bound has to be possible: Windows
    /// hands a registered combo to us rather than to the browser, so the
    /// capture field would never see the key and the action would fire
    /// instead. The tray reads this to skip registration while it holds.
    #[test]
    fn hotkey_capture_suspends_and_expires_on_its_own() {
        let h = harness(0);
        assert!(!h.ctrl.hotkeys_suspended(), "live by default");

        h.ctrl.suspend_hotkeys(30_000);
        assert!(h.ctrl.hotkeys_suspended());
        let left = h.ctrl.hotkeys_suspend_remaining_ms();
        assert!(left > 25_000 && left <= 30_000, "got {left}");

        // The dashboard says it is done.
        h.ctrl.suspend_hotkeys(0);
        assert!(!h.ctrl.hotkeys_suspended(), "resumed on request");
        assert_eq!(h.ctrl.hotkeys_suspend_remaining_ms(), 0);

        // And a window that has already passed never holds them down: a
        // dashboard that dies mid-capture costs one window, not the session.
        h.ctrl.suspend_hotkeys(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(!h.ctrl.hotkeys_suspended(), "expired on its own");
    }

    /// Nothing publishing means the buffer cannot fill, so every action
    /// that would BUILD delay is refused - and every action that removes it
    /// still works, because OBS crashing mid-delay is exactly when someone
    /// needs to cut back to live.
    #[test]
    fn named_actions_refuse_to_build_delay_with_no_publisher() {
        let h = harness(0);
        assert!(!h.ctrl.ingest_alive(), "offline to start with");

        assert!(h.ctrl.run_named_action("arm", 5_000, "hotkey").is_some());
        assert_eq!(h.ctrl.armed_delay_ms(), 0, "arm must not arm");
        assert_eq!(h.ctrl.phase(), "idle", "and must not enter preparing");

        assert!(h.ctrl.run_named_action("toggle", 5_000, "hotkey").is_some());
        assert_eq!(h.ctrl.armed_delay_ms(), 0, "toggle must not arm either");
        assert_eq!(h.ctrl.target_delay_ms(), 0);

        assert!(h.ctrl.run_named_action("activate", 0, "hotkey").is_some());
        assert_eq!(h.ctrl.target_delay_ms(), 0);

        // Now the other direction: armed and on air when the publisher dies.
        feed_seconds(&h.ctrl, 0, 5, 30);
        h.ctrl.arm_delay(3_000);
        h.ctrl.activate_delay().expect("buffer is past armed");
        h.ctrl.mark_ingest_dead();

        assert_eq!(
            h.ctrl.run_named_action("cut", 0, "hotkey"),
            None,
            "cutting back to live must never be blocked"
        );
        assert_eq!(h.ctrl.target_delay_ms(), 0, "and must actually cut");
        assert_eq!(
            h.ctrl.run_named_action("arm", 5_000, "hotkey"),
            None,
            "disarming frees the buffer, which is never harmful"
        );
        assert_eq!(h.ctrl.armed_delay_ms(), 0);
    }

    #[test]
    fn named_action_records_what_fired_for_the_dashboard() {
        // The dashboard reads this to light up the row that fired, and the
        // sequence number is what makes a repeat of the same action a new
        // event rather than a no-op.
        let h = harness(0);
        h.ctrl.mark_ingest_alive_for_test();
        assert!(h.ctrl.last_action().is_none(), "nothing has fired yet");

        h.ctrl.run_named_action("arm", 5_000, "hotkey");
        let first = h.ctrl.last_action().expect("an action was recorded");
        assert_eq!(first.action, "arm");
        assert_eq!(first.source, "hotkey");
        assert!(first.problem.is_none());

        h.ctrl.run_named_action("arm", 5_000, "midi");
        let second = h.ctrl.last_action().expect("recorded");
        assert_eq!(second.source, "midi");
        assert!(second.seq > first.seq, "a repeat has to read as new");

        // A refusal carries its message, so the dashboard can show it.
        let cold = harness(0);
        cold.ctrl.run_named_action("activate", 0, "hotkey");
        let refused = cold.ctrl.last_action().expect("recorded");
        assert!(refused.problem.is_some(), "a refusal is worth surfacing");
    }

    #[test]
    fn named_action_activate_and_cut() {
        let h = harness(0);
        h.ctrl.arm_delay(2_000);
        feed_seconds(&h.ctrl, 0, 4, 30);

        // Activate action
        h.ctrl.run_named_action("activate", 0, "hotkey");
        assert_eq!(h.ctrl.target_delay_ms(), 2_000);
        assert_eq!(h.ctrl.phase(), "active");

        // Cut action (drops to live immediately)
        h.ctrl.run_named_action("cut", 0, "hotkey");
        assert_eq!(h.ctrl.target_delay_ms(), 0);
        assert_eq!(h.ctrl.phase(), "ready");
    }

    #[test]
    fn named_action_cut_after_schedules_and_cancels() {
        let h = harness(0);
        h.ctrl.arm_delay(3_000);
        feed_seconds(&h.ctrl, 0, 5, 30);
        h.ctrl.activate_delay().unwrap();
        assert!(!h.ctrl.safe_cut_pending());

        // 1st press: schedules safe-cut (cut-after)
        h.ctrl.run_named_action("cut_after", 0, "hotkey");
        assert!(
            h.ctrl.safe_cut_pending(),
            "1st press must schedule safe cut"
        );

        // 2nd press: cancels the scheduled safe-cut (reversible misclick)
        h.ctrl.run_named_action("cut_after", 0, "hotkey");
        assert!(!h.ctrl.safe_cut_pending(), "2nd press must cancel safe cut");
    }

    #[test]
    fn named_action_toggle_clears_pending_safe_cut() {
        let h = harness(0);
        h.ctrl.arm_delay(3_000);
        feed_seconds(&h.ctrl, 0, 5, 30);
        h.ctrl.activate_delay().unwrap();

        // Schedule a safe cut
        h.ctrl.run_named_action("cut_after", 0, "hotkey");
        assert!(h.ctrl.safe_cut_pending());

        // Toggle while safe cut is pending should immediately cut to live and cancel mark
        h.ctrl.run_named_action("toggle", 3_000, "hotkey");
        assert_eq!(h.ctrl.target_delay_ms(), 0);
        assert!(!h.ctrl.safe_cut_pending());
    }

    /// A wrap does not arrive as one tidy crossing. Audio buffering means
    /// several tags can still be in flight when video crosses, and every one
    /// of them has to read back into the epoch it was stamped in.
    #[test]
    fn several_late_tags_in_a_row_all_land_in_the_old_epoch() {
        let h = harness(0);
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        let v = [0x27u8; 64];

        h.ctrl.on_tag(9, 0xFFFF_FF00, &v, false, false);
        h.ctrl.on_tag(9, 0x0000_0020, &v, false, false); // video crosses
        assert_eq!(h.ctrl.ring.latest_ts(), Some((1u64 << 32) | 0x20));

        // Three stragglers, each later than the one before but all still on
        // the old side of the counter.
        for wire in [0xFFFF_FFA0u32, 0xFFFF_FFC0, 0xFFFF_FFE0] {
            h.ctrl.on_tag(8, wire, &v, false, false);
            assert_eq!(
                h.ctrl.ring.latest_ts(),
                Some(wire as u64),
                "straggler {wire:#x} left its epoch"
            );
        }

        // And the counter is still able to cross forward afterwards.
        h.ctrl.on_tag(9, 0x0000_0040, &v, false, false);
        assert_eq!(h.ctrl.ring.latest_ts(), Some((1u64 << 32) | 0x40));
    }

    /// Audio and video routinely carry the same timestamp. Equal is neither
    /// ahead nor behind, and must not be read as either.
    #[test]
    fn tags_sharing_a_timestamp_do_not_move_the_epoch() {
        let h = harness(0);
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        let v = [0x27u8; 64];

        h.ctrl.on_tag(9, 5_000, &v, false, false);
        h.ctrl.on_tag(8, 5_000, &v, false, false);
        h.ctrl.on_tag(9, 5_000, &v, false, false);
        assert_eq!(h.ctrl.ring.latest_ts(), Some(5_000));

        // Same, but sitting exactly on the wrap boundary.
        let h2 = harness(0);
        h2.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        h2.ctrl.on_tag(9, 0xFFFF_FFFF, &v, false, false);
        h2.ctrl.on_tag(8, 0xFFFF_FFFF, &v, false, false);
        assert_eq!(h2.ctrl.ring.latest_ts(), Some(0xFFFF_FFFF));
        h2.ctrl.on_tag(9, 0, &v, false, false);
        assert_eq!(
            h2.ctrl.ring.latest_ts(),
            Some(1u64 << 32),
            "the very next millisecond is the next epoch"
        );
    }

    /// Half the counter apart is the point where "ahead" and "behind" stop
    /// being distinguishable. Whichever way it is read, one tag must not
    /// move the timeline by 49.7 days.
    #[test]
    fn a_jump_of_exactly_half_the_counter_does_not_move_the_epoch() {
        let h = harness(0);
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        let v = [0x27u8; 64];

        h.ctrl.on_tag(9, 0, &v, false, false);
        h.ctrl.on_tag(9, 1u32 << 31, &v, false, false);
        assert_eq!(
            h.ctrl.ring.latest_ts(),
            Some(1u64 << 31),
            "exactly half the counter ahead is still this epoch"
        );
    }

    /// A publisher session starts at epoch 0, and there is no epoch below it.
    /// A tag stamped before the first one we saw cannot be read as "the
    /// previous cycle", and must not be read as 49.7 days into the future
    /// either - that value reaches `trim_older_than` and empties the ring.
    #[test]
    fn a_tag_from_before_the_first_one_cannot_jump_the_timeline() {
        let h = harness(0);
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        let v = [0x27u8; 64];

        // A short, ordinary session.
        h.ctrl.on_tag(9, 0, &v, true, false);
        h.ctrl.on_tag(9, 1_000, &v, false, false);
        h.ctrl.on_tag(9, 2_000, &v, false, false);
        let before = h.ctrl.ring.latest_ts().expect("a populated ring");

        // Now a tag stamped "60 ms before zero", which is what an audio
        // track lagging the video would carry if the session began at 0.
        h.ctrl.on_tag(8, 0u32.wrapping_sub(60), &v, false, false);
        let after = h.ctrl.ring.latest_ts().expect("a populated ring");

        assert!(
            after <= before + 60_000,
            "a stray early tag moved the timeline to {after} from {before}"
        );
    }

    /// A pump that reaches the IDR wait has already opened and authenticated
    /// a session to the platform. If it waits forever there, the platform
    /// keeps the publish slot and viewers see a frozen stream, while nothing
    /// in the app notices because the task is alive and merely parked - and
    /// the supervisor will not respawn a task that has not finished.
    #[tokio::test]
    async fn the_idr_wait_gives_up_when_there_is_nothing_left_to_wait_for() {
        let h = harness(0);
        let dest = h.ctrl.destination_state("d1");

        // No publisher and no keyframe: the wait must not hang.
        assert!(!h.ctrl.ingest_alive());
        let gave_up =
            tokio::time::timeout(std::time::Duration::from_secs(2), seed_idr(&h.ctrl, &dest))
                .await
                .expect("the wait has to return, not hang");
        assert!(gave_up.is_none(), "no publisher means no seed");

        // The other exit: a publisher is live, but this destination was
        // asked to stop before any keyframe arrived.
        h.ctrl.mark_ingest_alive_for_test();
        dest.shutdown_requested.store(true, Ordering::Relaxed);
        let stopped =
            tokio::time::timeout(std::time::Duration::from_secs(2), seed_idr(&h.ctrl, &dest))
                .await
                .expect("the wait has to return, not hang");
        assert!(stopped.is_none(), "a stopping destination stops waiting");

        // And when a keyframe does arrive it is still returned.
        dest.shutdown_requested.store(false, Ordering::Relaxed);
        feed_seconds(&h.ctrl, 0, 2, 30);
        let seeded =
            tokio::time::timeout(std::time::Duration::from_secs(2), seed_idr(&h.ctrl, &dest))
                .await
                .expect("returns")
                .expect("a keyframe is present");
        assert!(seeded.is_idr, "the seed has to be a keyframe");
    }

    /// The egress error path redacts the stream key before logging it. That
    /// redaction sliced bytes, so a key with a multi-byte character in it
    /// aborted the process at the exact moment a destination was already
    /// failing - and again on every reconnect, since the key is on disk.
    /// Three sibling copies of this were fixed a commit earlier; this one
    /// was missed, which is why it has its own test.
    #[test]
    fn scrubbing_a_key_from_an_error_never_panics() {
        for key in [
            "🎥abcde",      // 4-byte char straddling offset 3
            "abéde",        // 2-byte char at the tail boundary
            "日本語テスト", // every char multi-byte
            "live_señor_key",
            "ascii_key_here",
            "short", // under the length floor: elided whole
            "",
        ] {
            let msg = format!("connection refused while sending to {key}");
            let out = scrub_secret(&msg, key);
            if key.is_empty() {
                // Nothing to redact - and an empty needle must not splice the
                // marker in between every character of the message.
                assert_eq!(out, msg);
                continue;
            }
            // The failure text deliberately carries neither the key nor the
            // scrubbed line. A test about redaction that prints the value it
            // failed to redact would leak it into CI logs on the one run where
            // that matters, and code scanning flags the pattern for exactly
            // that reason. The fixture list above is short enough to find the
            // offending input without it.
            assert!(!out.contains(key), "a key survived redaction");
            assert!(out.contains('…'), "a key was not elided at all");
        }
        // ASCII behaviour is unchanged.
        assert_eq!(
            scrub_secret("failed for live_1234567890", "live_1234567890"),
            "failed for liv…890"
        );
        // Under the floor there is no safe head/tail, so the whole key goes
        // rather than passing through into the log line untouched.
        assert_eq!(scrub_secret("failed for abc12", "abc12"), "failed for …");
    }

    /// Audio and video cross each other by a few milliseconds all the time,
    /// so at the moment the u32 wire clock rolls over, one track is past it
    /// and the other is not. The straggler must be read in the epoch it was
    /// actually stamped in: promoting it puts one tag 49.7 days ahead, and
    /// `on_tag` feeds that to the trim, which then measures the whole ring
    /// against a cutoff past every frame in it and evicts the lot.
    #[test]
    fn a_late_tag_at_the_wrap_stays_in_the_epoch_it_came_from() {
        let h = harness(0);
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        let v = [0x27u8; 64];
        let a = [0xafu8; 64];

        // Video runs up to the edge of the wrap, audio interleaved behind it.
        h.ctrl.on_tag(9, 0xFFFF_FFC0, &v, false, false);
        h.ctrl.on_tag(8, 0xFFFF_FFD0, &a, false, false);
        h.ctrl.on_tag(9, 0xFFFF_FFE0, &v, false, false);

        // Video crosses first: one epoch up, 0x10 into the new counter.
        h.ctrl.on_tag(9, 0x0000_0010, &v, false, false);
        let crossed = h.ctrl.ring.latest_ts().expect("a populated ring");
        assert_eq!(crossed, (1u64 << 32) | 0x10, "video took the new epoch");

        // The audio tag that was still in flight is stamped 32 ms earlier,
        // and has to read as 32 ms earlier - not 49.7 days later.
        h.ctrl.on_tag(8, 0xFFFF_FFF0, &a, false, false);
        let straggler = h.ctrl.ring.latest_ts().expect("a populated ring");
        assert_eq!(
            straggler, 0xFFFF_FFF0,
            "the straggler belongs to the epoch it was stamped in"
        );
        assert_eq!(
            crossed - straggler,
            32,
            "and sits 32 ms before the tag that crossed, not 49.7 days after"
        );

        // The next video tag is back in the new epoch, so one late tag does
        // not leave the counter stuck an epoch behind.
        h.ctrl.on_tag(9, 0x0000_0030, &v, false, false);
        assert_eq!(
            h.ctrl.ring.latest_ts().expect("a populated ring"),
            (1u64 << 32) | 0x30,
            "the counter recovers on the next in-epoch tag"
        );
    }

    // -- Long-run soak ------------------------------------------------

    /// What a soak run observed, so each scenario can assert on its own
    /// terms instead of the driver guessing what matters.
    struct SoakStats {
        tags: u64,
        wraps: u64,
        reconnects: u64,
        secs: f64,
    }

    const WRAP_MS: u64 = 1u64 << 32; // u32 ms rolls over every 49.7 days
    const FRAME_MS: u64 = 33; // 30 fps, the dense rate
    const COARSE_MS: u64 = 1_000; // where nothing interesting happens
    const HOT_ZONE_MS: u64 = 90_000; // dense either side of a boundary
    const SOAK_ARMED_MS: u32 = 5_000;
    /// How far behind the video tag the interleaved audio tag is stamped.
    /// Deeper than one video frame on purpose, which is both what a real
    /// muxer's audio buffering looks like and what guarantees the case that
    /// matters: with a lag shorter than the frame step, whether any tag
    /// lands in the window between the two tracks crossing the wrap is down
    /// to the phase of the step, and a run can walk straight over the bug
    /// without ever touching it.
    const AUDIO_LAG_MS: u64 = 60;

    /// Send one tag and check it expanded to the timestamp it was stamped
    /// with. Equality is what catches an epoch read the wrong way round: a
    /// missed wrap loses 49.7 days, a straggler promoted into the new epoch
    /// gains them, and both still look monotonic.
    fn soak_emit(ctrl: &Controller, kind: u8, track_ms: u64, payload: &[u8], is_idr: bool) {
        ctrl.on_tag(kind, track_ms as u32, payload, is_idr, false);
        if let Some(ts) = ctrl.ring.latest_ts() {
            assert_eq!(
                ts,
                track_ms,
                "expanded timestamp is not where the tag was stamped, at {} h",
                track_ms / 3_600_000
            );
        }
    }

    /// Ring invariants that are too costly to check per tag.
    fn soak_check_ring(ctrl: &Controller, stream_ms: u64, prev_seq: &mut Option<u64>) {
        let oldest = ctrl.ring.oldest_ts().expect("a populated ring");
        let latest = ctrl.ring.latest_ts().expect("a populated ring");
        assert!(oldest <= latest, "ring front overtook its back");
        if let Some(seq) = ctrl.ring.latest_seq() {
            if let Some(prev) = *prev_seq {
                assert!(seq > prev, "sequence number went backwards");
            }
            *prev_seq = Some(seq);
        }
        assert!(
            latest - oldest <= (SOAK_ARMED_MS as u64) * 3,
            "trim stopped bounding the buffer at {} h: {} ms held for {SOAK_ARMED_MS} ms",
            stream_ms / 3_600_000,
            latest - oldest
        );
    }

    /// What a real restart does: the publisher goes, the session caches go
    /// with it, and OBS's wire clock starts again near 0.
    fn soak_restart(ctrl: &Controller) {
        ctrl.mark_ingest_dead();
        ctrl.ring.clear();
        ctrl.ingest_alive.store(true, Ordering::Relaxed);
        ctrl.arm_delay(SOAK_ARMED_MS);
    }

    /// Frames run dense either side of a wrap or a reconnect, where the
    /// arithmetic is interesting, and coarse in between so a run measured in
    /// years of stream time finishes in minutes.
    fn soak_step_ms(session_ms: u64, until_reconnect: u64) -> u64 {
        let into_epoch = session_ms % WRAP_MS;
        let near_wrap = into_epoch < HOT_ZONE_MS || WRAP_MS - into_epoch < HOT_ZONE_MS;
        if near_wrap || until_reconnect < HOT_ZONE_MS {
            FRAME_MS
        } else {
            COARSE_MS
        }
    }

    /// Drive `hours` of stream through the real ingest path, restarting the
    /// publisher every `reconnect_every_ms` (0 for one unbroken session).
    ///
    /// The timeline is never skipped: every millisecond of the requested
    /// duration passes through the same u32 truncation OBS puts on the wire.
    fn soak(hours: u64, reconnect_every_ms: u64, interleave_audio: bool) -> SoakStats {
        let h = harness(0);
        h.ctrl.ingest_alive.store(true, Ordering::Relaxed);
        h.ctrl.arm_delay(SOAK_ARMED_MS);

        // Small payloads: a run this long rewrites the ring's whole capacity
        // thousands of times over, and the tag count is what we are after.
        let mut idr = [0u8; 120];
        idr[0] = 0x17;
        let mut pframe = [0u8; 120];
        pframe[0] = 0x27;

        let total_ms = hours * 3_600_000;
        let mut stream_ms: u64 = 0; // the whole run
        let mut session_start: u64 = 0; // where the current session began
        let mut prev_seq: Option<u64> = None;
        let mut next_reconnect = if reconnect_every_ms == 0 {
            u64::MAX
        } else {
            reconnect_every_ms
        };
        let (mut tags, mut wraps, mut reconnects) = (0u64, 0u64, 0u64);
        let started = std::time::Instant::now();

        while stream_ms < total_ms {
            let session_ms = stream_ms - session_start;
            let is_idr = session_ms % 2_000 < FRAME_MS;
            let payload: &[u8] = if is_idr { &idr } else { &pframe };

            // Video, then the audio tag that was already in flight when the
            // video was sent.
            soak_emit(&h.ctrl, 9, session_ms, payload, is_idr);
            tags += 1;
            if interleave_audio && session_ms >= AUDIO_LAG_MS {
                soak_emit(&h.ctrl, 8, session_ms - AUDIO_LAG_MS, &pframe, false);
                tags += 1;
            }

            // No separate monotonicity check: soak_emit already pins every
            // tag to its own stamp, and the session clock only moves
            // forward, so ordering follows from it.
            if tags % 20_000 == 0 {
                soak_check_ring(&h.ctrl, stream_ms, &mut prev_seq);
            }

            // Keep the delay state machine moving, so this is not just an
            // append loop with a long clock.
            if session_ms > 60_000 && session_ms % 1_800_000 < COARSE_MS {
                let _ = h.ctrl.activate_delay();
                h.ctrl.arm_delay(SOAK_ARMED_MS);
            }

            if stream_ms >= next_reconnect {
                soak_restart(&h.ctrl);
                session_start = stream_ms;
                prev_seq = None;
                next_reconnect += reconnect_every_ms;
                reconnects += 1;
            }

            let step = soak_step_ms(session_ms, next_reconnect.saturating_sub(stream_ms));
            if (session_ms + step) % WRAP_MS < session_ms % WRAP_MS {
                wraps += 1;
            }
            stream_ms += step;
        }

        assert!(
            h.ctrl.logs.lock().len() <= LOG_LINES_MAX,
            "the log ring grew unbounded"
        );
        SoakStats {
            tags,
            wraps,
            reconnects,
            secs: started.elapsed().as_secs_f64(),
        }
    }

    fn soak_hours(default: u64) -> u64 {
        std::env::var("SOAK_HOURS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    /// One unbroken session, long enough to cross the RTMP timestamp wrap
    /// eight times. This is the unattended-relay case: nothing restarts OBS,
    /// so the wire clock runs until u32 milliseconds roll over at 49.7 days,
    /// and every expanded timestamp after that no longer fits in 32 bits.
    ///
    /// Debug build on purpose: overflow checks are on, so every counter that
    /// accumulates over the run is bounds-checked by the compiler rather
    /// than by an assertion someone remembered to write.
    ///
    ///   cargo test soak_continuous -- --ignored --nocapture
    ///   SOAK_HOURS=1500 cargo test soak_continuous -- --ignored --nocapture
    #[test]
    #[ignore = "long-running soak; run explicitly with --ignored"]
    fn soak_continuous_session_crosses_every_timestamp_wrap() {
        let hours = soak_hours(10_000);
        let s = soak(hours, 0, false);
        println!(
            "soak/continuous: {hours} h, {} tags, {} wraps, {:.1} s",
            s.tags, s.wraps, s.secs
        );
        if hours >= 1_200 {
            assert!(s.wraps >= 1, "a run this long has to cross a wrap");
        }
    }

    /// The same unbroken session, but with audio interleaved the way a real
    /// muxer sends it: each audio tag stamped a few milliseconds behind the
    /// video tag it followed. At every wrap that leaves one track over the
    /// line while the other is not, which is the case that put a tag 49.7
    /// days in the future and had the trim evict the entire delay buffer.
    #[test]
    #[ignore = "long-running soak; run explicitly with --ignored"]
    fn soak_interleaved_audio_across_every_wrap() {
        let hours = soak_hours(10_000);
        let s = soak(hours, 0, true);
        println!(
            "soak/interleaved: {hours} h, {} tags, {} wraps, {:.1} s",
            s.tags, s.wraps, s.secs
        );
        if hours >= 1_200 {
            assert!(s.wraps >= 1, "a run this long has to cross a wrap");
        }
    }

    /// The ordinary case: a streamer who goes live, stops, and goes live
    /// again, a few hours at a time. Each restart hands the ingest path a
    /// wire clock that jumps backwards to ~0 while the ring still holds the
    /// previous session, which is the shape of a wrap seen from the wrong
    /// side.
    #[test]
    #[ignore = "long-running soak; run explicitly with --ignored"]
    fn soak_many_sessions_reset_cleanly() {
        let hours = soak_hours(1_000);
        let s = soak(hours, 6 * 3_600_000, false);
        println!(
            "soak/sessions: {hours} h, {} tags, {} reconnects, {:.1} s",
            s.tags, s.reconnects, s.secs
        );
        if hours >= 100 {
            assert!(s.reconnects > 10, "the run has to restart the publisher");
        }
    }

    /// Harness with crash protection on and one destination streaming.
    fn protected_harness(every_disconnect: bool) -> Harness {
        let h = harness(0);
        h.ctrl
            .update_crash_protection(crate::crash_protection::CrashProtection {
                enabled: true,
                every_disconnect,
                ..Default::default()
            });
        h.ctrl
            .destination_state("live")
            .egress_alive
            .store(true, Ordering::Relaxed);
        h
    }

    /// OBS sends FCUnpublish + deleteStream from `RTMP_Close` on every
    /// deliberate stop; a crash closes the socket without them. Only a
    /// crash opens a hold, and a goodbye never leaks into the next session.
    #[tokio::test]
    async fn only_a_crash_opens_a_hold() {
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.note_unpublish();
        h.ctrl.mark_ingest_dead();
        assert!(!h.ctrl.hold_active(), "a deliberate stop ends the stream");

        h.ctrl.note_unpublish(); // stray goodbye between sessions
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.mark_ingest_dead();
        assert!(h.ctrl.hold_active(), "a drop without goodbye is a crash");

        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        assert_eq!(h.ctrl.hold_state(), crate::crash_hold::HoldState::Resumed);
    }

    #[tokio::test]
    async fn end_hold_hotkey_ends_a_hold_and_explains_when_there_is_none() {
        let h = protected_harness(false);
        assert!(h.ctrl.run_named_action("end_hold", 0, "hotkey").is_some());
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.mark_ingest_dead();
        assert!(h.ctrl.hold_active());
        assert_eq!(h.ctrl.run_named_action("end_hold", 0, "hotkey"), None);
        assert_eq!(h.ctrl.hold_state(), crate::crash_hold::HoldState::Ended);
        assert!(
            h.ctrl.run_named_action("end_hold", 0, "hotkey").is_some(),
            "nothing left to end"
        );
    }

    #[tokio::test]
    async fn no_hold_when_protection_is_off_or_nothing_is_live() {
        let h = harness(0);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.mark_ingest_dead();
        assert!(!h.ctrl.hold_active(), "protection is off by default");

        let h = protected_harness(false);
        h.ctrl
            .destination_state("live")
            .egress_alive
            .store(false, Ordering::Relaxed);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.mark_ingest_dead();
        assert!(
            !h.ctrl.hold_active(),
            "no live destination, nothing to protect"
        );
    }

    #[tokio::test]
    async fn a_frozen_publisher_opens_a_hold_that_its_next_frame_closes() {
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.on_tag(9, 0, &[0x27, 1, 0, 0, 0], false, false);
        assert!(h.ctrl.ingest_sending());
        let last = h.ctrl.last_video_tag_ms.load(Ordering::Relaxed);
        let freeze_ms = crate::crash_hold::FREEZE_AFTER.as_millis() as u64;
        assert!(!h.ctrl.video_frozen_at(last + freeze_ms - 1));
        assert!(
            h.ctrl.video_frozen_at(last + freeze_ms),
            "connected but frozen"
        );
        h.ctrl.check_ingest_freeze_at(last + 500);
        assert!(!h.ctrl.hold_active(), "a short gap is not a freeze");
        h.ctrl.check_ingest_freeze_at(last + freeze_ms);
        assert!(h.ctrl.hold_active());
        h.ctrl.on_tag(9, 40, &[0x27, 1, 0, 0, 0], false, false);
        assert_eq!(h.ctrl.hold_state(), crate::crash_hold::HoldState::Resumed);
        assert!(h.ctrl.ingest_sending());
    }

    /// OBS crashes with a 10 s delay on air: the buffer stops growing, so
    /// the delay only shrinks while the tail plays out. Live, that gap
    /// would re-cut back; during the hold it must not, or the tail loops.
    #[tokio::test]
    async fn a_crash_mid_delay_never_jumps_back_into_the_tail() {
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.arm_delay(10_000);
        feed_seconds(&h.ctrl, 0, 12, 30);
        h.ctrl.activate_delay().expect("the buffer holds the delay");
        // The pump is 4 s from the frozen live edge: 6 s short of the delay.
        let current = h.ctrl.ring.find_idr_near(8_000, 500).unwrap();
        assert!(
            compute_delay_cut(&h.ctrl, &current).is_some(),
            "live, it re-cuts"
        );
        h.ctrl.mark_ingest_dead();
        assert!(h.ctrl.hold_active());
        assert!(
            compute_delay_cut(&h.ctrl, &current).is_none(),
            "holding, the tail plays out once"
        );
    }

    /// The tray, global hotkeys and MIDI run on plain threads with no Tokio
    /// runtime. Ending a hold from them emits an integrations event, which
    /// must not need one: a panic there aborts the whole process.
    #[tokio::test]
    async fn ending_a_hold_off_the_runtime_does_not_panic() {
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.mark_ingest_dead();
        assert!(h.ctrl.hold_active());
        let ctrl = h.ctrl.clone();
        let hotkey = std::thread::spawn(move || ctrl.run_named_action("end_hold", 0, "hotkey"));
        assert_eq!(hotkey.join().expect("no panic off the runtime"), None);
        assert!(!h.ctrl.hold_active());
    }

    /// Opens a freeze hold for `h` and returns the last video stamp.
    fn freeze(h: &Harness) -> u64 {
        h.ctrl.on_tag(9, 0, &[0x27, 1, 0, 0, 0], false, false);
        let last = h.ctrl.last_video_tag_ms.load(Ordering::Relaxed);
        let freeze_ms = crate::crash_hold::FREEZE_AFTER.as_millis() as u64;
        h.ctrl.check_ingest_freeze_at(last + freeze_ms);
        assert!(h.ctrl.hold_active());
        last
    }

    #[tokio::test]
    async fn stopping_in_obs_ends_a_freeze_hold() {
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        freeze(&h);
        h.ctrl.note_unpublish();
        h.ctrl.mark_ingest_dead();
        assert!(!h.ctrl.hold_active(), "a deliberate stop ends it");
        assert_eq!(h.ctrl.hold_state(), crate::crash_hold::HoldState::Ended);
    }

    #[tokio::test]
    async fn a_freeze_hold_that_ended_does_not_reopen_until_video_returns() {
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        let last = freeze(&h);
        assert!(h.ctrl.end_hold_now());
        h.ctrl.check_ingest_freeze_at(last + 60_000);
        assert!(!h.ctrl.hold_active(), "still the same freeze");
        // Video comes back, then OBS freezes again: that one is protected.
        let last = freeze(&h);
        assert!(last > 0);
    }

    /// With a delay armed, the screen stays up after OBS is back until its
    /// new video spans the delay, and the pump rejoins from that video.
    #[tokio::test]
    async fn the_screen_waits_for_the_delay_to_rebuild_from_new_video() {
        let h = harness(0);
        h.ctrl.arm_delay(10_000);
        feed_seconds(&h.ctrl, 0, 2, 10);
        // OBS comes back (after a freeze: the old video stays in the ring).
        h.ctrl.resume_from_hold();
        let mut rebuild = None;
        let rebuilt =
            |rebuild: &mut Option<_>| crate::crash_hold::delay_rebuilt(&h.ctrl, rebuild, 3_000);
        assert_eq!(rebuilt(&mut rebuild), None, "no new video yet");
        let first_new = h.ctrl.ring.latest_seq().unwrap() + 1;
        feed_seconds(&h.ctrl, 10_000, 2, 10);
        assert_eq!(rebuilt(&mut rebuild), None, "1.9 s of new video");
        feed_seconds(&h.ctrl, 12_000, 2, 10);
        assert_eq!(rebuilt(&mut rebuild), Some(first_new), "3.9 s: rejoin");
    }

    const KEPT_IVS: &str = "rtmps://ivs/app/kept";
    const KEPT_TOKEN: &str = "kept-token";
    const FRESH_IVS: &str = "rtmps://ivs/app/fresh";
    const FRESH_TOKEN: &str = "fresh-token";

    /// How OBS left the stream.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum ObsLeaves {
        Crashes,
        /// Stopped on purpose, with "protect every disconnect" on.
        Stops,
        /// Still connected, but sending no video.
        Freezes,
    }

    /// When OBS asked for its Enhanced Broadcasting config again.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum ObsAsks {
        Never,
        DuringHold,
        AfterHold,
    }

    /// How the hold closed.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum HoldCloses {
        ObsReturns,
        /// `ticked`: the supervisor reported the deadline before OBS came back.
        Deadline {
            ticked: bool,
            obs_returns: bool,
        },
        EndNow {
            obs_returns: bool,
        },
    }

    #[derive(Debug)]
    struct HoldCase {
        had_eb_session: bool,
        leaves: ObsLeaves,
        asks: ObsAsks,
        closes: HoldCloses,
    }

    impl HoldCase {
        fn obs_returns(&self) -> bool {
            match self.closes {
                HoldCloses::ObsReturns => true,
                HoldCloses::Deadline { obs_returns, .. } | HoldCloses::EndNow { obs_returns } => {
                    obs_returns
                }
            }
        }

        fn is_possible(&self) -> bool {
            match self.asks {
                ObsAsks::Never => true,
                // A frozen OBS is still streaming: it asks for nothing.
                ObsAsks::DuringHold => self.leaves != ObsLeaves::Freezes,
                ObsAsks::AfterHold => {
                    self.leaves != ObsLeaves::Freezes
                        && self.obs_returns()
                        && self.closes != HoldCloses::ObsReturns
                }
            }
        }

        /// The Twitch session the destination must end on: the one OBS
        /// asked for and publishes on, or none for a single-track stream.
        fn expected_session(&self) -> Option<&'static str> {
            let kept = self.had_eb_session.then_some(KEPT_IVS);
            if self.leaves == ObsLeaves::Freezes {
                return kept;
            }
            match self.asks {
                ObsAsks::Never => None,
                ObsAsks::DuringHold => Some(kept.unwrap_or(FRESH_IVS)),
                ObsAsks::AfterHold => Some(FRESH_IVS),
            }
        }

        fn expected_state(&self) -> crate::crash_hold::HoldState {
            if self.closes == HoldCloses::ObsReturns {
                crate::crash_hold::HoldState::Resumed
            } else {
                crate::crash_hold::HoldState::Ended
            }
        }
    }

    fn all_hold_cases() -> Vec<HoldCase> {
        let mut closings = vec![HoldCloses::ObsReturns];
        for obs_returns in [true, false] {
            closings.push(HoldCloses::EndNow { obs_returns });
            for ticked in [true, false] {
                closings.push(HoldCloses::Deadline {
                    ticked,
                    obs_returns,
                });
            }
        }
        let mut cases = Vec::new();
        for had_eb_session in [true, false] {
            for leaves in [ObsLeaves::Crashes, ObsLeaves::Stops, ObsLeaves::Freezes] {
                for asks in [ObsAsks::Never, ObsAsks::DuringHold, ObsAsks::AfterHold] {
                    for &closes in &closings {
                        let case = HoldCase {
                            had_eb_session,
                            leaves,
                            asks,
                            closes,
                        };
                        if case.is_possible() {
                            cases.push(case);
                        }
                    }
                }
            }
        }
        cases
    }

    /// The fresh-session path of /obs/multitrack-config (web.rs): accept the
    /// token, point the Twitch destination at the session, keep it for a hold.
    fn open_eb_session(h: &Harness, ivs: &str, token: &str) {
        h.ctrl.remember_eb_key(token.into());
        *h.ctrl.destination_state("live").eb_override_url.lock() = Some(ivs.into());
        h.ctrl.remember_eb_session(EbSession {
            config: format!("{{{token}}}"),
            auths: vec![token.into()],
            dest_id: "live".into(),
            ivs_url: ivs.into(),
        });
    }

    /// OBS asks /obs/multitrack-config for its config, which hands back the
    /// session a hold kept or opens a new one. Returns OBS's stream key.
    fn obs_asks_for_eb_config(h: &Harness) -> &'static str {
        if h.ctrl.held_eb_config().is_some() {
            return KEPT_TOKEN;
        }
        open_eb_session(h, FRESH_IVS, FRESH_TOKEN);
        FRESH_TOKEN
    }

    /// Plays `case` out and returns what went wrong, if anything.
    async fn run_hold_case(case: &HoldCase) -> Vec<String> {
        let h = protected_harness(case.leaves == ObsLeaves::Stops);
        h.ctrl.update_ingest_key("k".into());
        let mut stream_key = "k";
        if case.had_eb_session {
            open_eb_session(&h, KEPT_IVS, KEPT_TOKEN);
            stream_key = KEPT_TOKEN;
        }
        h.ctrl.begin_publish(stream_key, "127.0.0.1").await.unwrap();
        h.ctrl.on_tag(9, 0, &[0x17, 0, 0, 0, 0, 1], false, true);
        match case.leaves {
            ObsLeaves::Crashes => h.ctrl.mark_ingest_dead(),
            ObsLeaves::Stops => {
                h.ctrl.note_unpublish();
                h.ctrl.mark_ingest_dead();
            }
            ObsLeaves::Freezes => {
                freeze(&h);
            }
        }
        assert!(h.ctrl.hold_active(), "{case:?}: no hold opened");

        stream_key = "k";
        if case.asks == ObsAsks::DuringHold {
            stream_key = obs_asks_for_eb_config(&h);
        }
        match case.closes {
            HoldCloses::ObsReturns => {}
            HoldCloses::Deadline { ticked, .. } => {
                if let Some(hold) = h.ctrl.hold.lock().as_mut() {
                    hold.deadline = Instant::now() - Duration::from_millis(1);
                }
                if ticked {
                    h.ctrl.expire_hold();
                }
            }
            HoldCloses::EndNow { .. } => {
                assert!(h.ctrl.end_hold_now(), "{case:?}: End now found no hold");
            }
        }
        if case.obs_returns() {
            if case.asks == ObsAsks::AfterHold {
                stream_key = obs_asks_for_eb_config(&h);
            }
            if case.leaves == ObsLeaves::Freezes {
                h.ctrl.on_tag(9, 40, &[0x27, 1, 0, 0, 0], false, false);
            } else if let Err(e) = h.ctrl.begin_publish(stream_key, "127.0.0.1").await {
                return vec![format!("{case:?}: OBS's publish was refused: {e}")];
            }
        }
        // The supervisor's next tick.
        h.ctrl.expire_hold();

        let mut failures = Vec::new();
        let session = h
            .ctrl
            .destination_state("live")
            .eb_override_url
            .lock()
            .clone();
        if session.as_deref() != case.expected_session() {
            failures.push(format!(
                "{case:?}: Twitch session {session:?}, expected {:?}",
                case.expected_session()
            ));
        }
        let state = h.ctrl.hold_state();
        if state != case.expected_state() {
            failures.push(format!(
                "{case:?}: hold {state:?}, expected {:?}",
                case.expected_state()
            ));
        }
        failures
    }

    /// OBS asking for its config again during a hold gets the kept session
    /// back, and that session's token is brokered again: it was accepted
    /// when the session opened, possibly longer ago than its TTL, and OBS's
    /// publish with it must still get past the ingest key. (The matrix below
    /// pre-brokers every token, so it can't see this.) Outside a hold there
    /// is nothing to hand back.
    #[tokio::test]
    async fn a_kept_session_is_handed_back_only_during_a_hold_and_rebrokers_its_token() {
        let h = protected_harness(false);
        h.ctrl.update_ingest_key("k".into());
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.on_tag(9, 0, &[0x17, 0, 0, 0, 0, 1], false, true);
        let live = h.ctrl.destination_state("live");
        *live.eb_override_url.lock() = Some(KEPT_IVS.into());
        // Remembered without brokering its token: as if the token expired.
        h.ctrl.remember_eb_session(EbSession {
            config: "{config}".into(),
            auths: vec![KEPT_TOKEN.into()],
            dest_id: "live".into(),
            ivs_url: KEPT_IVS.into(),
        });
        assert_eq!(h.ctrl.held_eb_config(), None, "only during a hold");

        h.ctrl.mark_ingest_dead();
        assert!(!h.ctrl.is_brokered_eb_key(KEPT_TOKEN));
        assert_eq!(h.ctrl.held_eb_config().as_deref(), Some("{config}"));
        h.ctrl
            .begin_publish(KEPT_TOKEN, "127.0.0.1")
            .await
            .expect("the token is let in again");
        assert_eq!(live.eb_override_url.lock().as_deref(), Some(KEPT_IVS));
        assert_eq!(h.ctrl.held_eb_config(), None, "the hold is over");
    }

    /// Every way a hold can play out with Enhanced Broadcasting in the mix.
    /// Twitch refuses a stream on the wrong session (the multitrack ladder on
    /// plain ingest, or one track on a multitrack session), so the
    /// destination must end on exactly the session OBS publishes on.
    #[tokio::test]
    async fn every_hold_path_leaves_twitch_on_the_session_obs_publishes_on() {
        let mut failures = Vec::new();
        for case in all_hold_cases() {
            failures.extend(run_hold_case(&case).await);
        }
        assert!(
            failures.is_empty(),
            "{} hold paths went wrong:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// OBS reconnects after a crash but never sends video: once the grace
    /// runs out that counts as a freeze, so the screen gets a deadline.
    #[tokio::test]
    async fn a_returning_obs_that_sends_no_video_is_a_freeze() {
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.on_tag(9, 0, &[0x17, 0, 0, 0, 0, 1], false, true);
        h.ctrl.mark_ingest_dead();
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        assert!(!h.ctrl.hold_active(), "resumed");
        let now = process_now_ms();
        let grace = FIRST_VIDEO_GRACE.as_millis() as u64;
        h.ctrl.check_ingest_freeze_at(now + grace - 1_000);
        assert!(!h.ctrl.hold_active(), "still within the grace");
        h.ctrl.check_ingest_freeze_at(now + grace + 1_000);
        assert!(h.ctrl.hold_active(), "no video after the grace is a freeze");
    }

    #[tokio::test]
    async fn protection_off_keeps_no_per_frame_state() {
        let h = harness(0);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        let hevc_keyframe = [0x91, b'h', b'v', b'c', b'1', 0, 0, 0, 7];
        h.ctrl.on_tag(9, 0, &hevc_keyframe, true, false);
        assert_eq!(h.ctrl.last_video_tag_ms.load(Ordering::Relaxed), 0);
        assert!(h.ctrl.held_keyframe(0).is_none());

        // Switched off mid-stream: nothing stale can later read as a freeze.
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.on_tag(9, 0, &hevc_keyframe, true, false);
        assert!(h.ctrl.held_keyframe(0).is_some());
        h.ctrl
            .update_crash_protection(crate::crash_protection::CrashProtection::default());
        assert_eq!(h.ctrl.last_video_tag_ms.load(Ordering::Relaxed), 0);
        assert!(h.ctrl.held_keyframe(0).is_none());
        assert!(h.ctrl.ingest_sending());
    }

    #[tokio::test]
    async fn an_hevc_stream_is_covered_by_holding_its_last_keyframe() {
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        let live = h.ctrl.destination_state("live");
        // Enhanced RTMP HEVC: SequenceStart, then a coded keyframe.
        h.ctrl
            .on_tag(9, 0, &[0x90, b'h', b'v', b'c', b'1', 1], false, true);
        assert!(
            !crate::crash_hold::covers(&h.ctrl, &live),
            "nothing to hold yet"
        );
        let keyframe = [0x91, b'h', b'v', b'c', b'1', 0, 0, 0, 7];
        h.ctrl.on_tag(9, 33, &keyframe, true, false);
        assert!(crate::crash_hold::covers(&h.ctrl, &live));
        assert_eq!(h.ctrl.held_keyframe(0).as_deref(), Some(&keyframe[..]));
    }

    #[tokio::test]
    async fn an_enhanced_broadcasting_destination_is_covered_track_by_track() {
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        let live = h.ctrl.destination_state("live");
        live.pass_through_multitrack_video
            .store(true, Ordering::Relaxed);
        // Track 0 legacy H.264, track 1 an H.264 rung, track 2 an HEVC rung.
        let one_track = |fourcc: &[u8; 4], track: u8, first: u8, packet: u8| {
            let mut tag = vec![first, packet];
            tag.extend_from_slice(fourcc);
            tag.extend_from_slice(&[track, 0, 0, 0, 7]);
            tag
        };
        h.ctrl.on_tag(9, 0, &[0x17, 0, 0, 0, 0, 1], false, true);
        h.ctrl
            .on_tag(9, 0, &one_track(b"avc1", 1, 0x96, 0x00), false, true);
        assert!(crate::crash_hold::covers(&h.ctrl, &live));
        h.ctrl
            .on_tag(9, 0, &one_track(b"hvc1", 2, 0x96, 0x00), false, true);
        assert!(
            !crate::crash_hold::covers(&h.ctrl, &live),
            "every track needs a picture"
        );
        h.ctrl
            .on_tag(9, 33, &one_track(b"hvc1", 2, 0x96, 0x01), false, false);
        assert!(crate::crash_hold::covers(&h.ctrl, &live));
        assert!(h.ctrl.held_keyframe(2).is_some());
        assert!(
            h.ctrl.held_keyframe(1).is_none(),
            "H.264 rungs get the screen"
        );
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
    /// A hold keeps a destination's Twitch session only when it keeps that
    /// destination on air: live, and coverable by the screen. One that is
    /// down, or a vertical one whose canvas isn't known yet (the screen
    /// can't cover it), ends with OBS, so its session goes with it, and a
    /// session fetch still in flight for it is discarded.
    #[tokio::test]
    async fn a_hold_keeps_the_session_only_of_destinations_it_keeps_on_air() {
        let h = protected_harness(false);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl.on_tag(9, 0, &[0x17, 0, 0, 0, 0, 1], false, true);
        let live = h.ctrl.destination_state("live");
        let down = h.ctrl.destination_state("down");
        let vertical = h.ctrl.destination_state("vertical");
        vertical.egress_alive.store(true, Ordering::Relaxed);
        vertical.egress_vertical.store(true, Ordering::Relaxed);
        assert!(crate::crash_hold::covers(&h.ctrl, &live));
        assert!(
            !crate::crash_hold::covers(&h.ctrl, &vertical),
            "no canvas to cover yet"
        );
        for dest in [&live, &down, &vertical] {
            *dest.eb_override_url.lock() = Some(format!("rtmps://ivs/app/{}", dest.id));
        }
        let epochs = [&down, &vertical].map(|dest| dest.session_epoch());

        h.ctrl.mark_ingest_dead();
        assert!(h.ctrl.hold_active());
        assert_eq!(
            live.eb_override_url.lock().as_deref(),
            Some("rtmps://ivs/app/live"),
            "kept on air, so its session is kept"
        );
        for (dest, epoch) in [&down, &vertical].into_iter().zip(epochs) {
            assert!(dest.eb_override_url.lock().is_none(), "{} ended", dest.id);
            assert!(dest.session_epoch() > epoch, "{}: fetch discarded", dest.id);
        }
    }

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
    /// (tested via `every_track_config_leads_each_twitch_connection`), allowing them to
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
    /// previous session's measurements must not leak into the new one.
    #[test]
    fn reset_codec_state_clears_the_measurement() {
        let h = harness(0);
        feed_idrs(&h.ctrl, 0, 4_000, 4);
        let header = crate::slate::test_sequence_header(64, 36);
        h.ctrl.on_tag(9, 12_000, &header, false, true);
        let before = h.ctrl.stream_params();
        assert_eq!(before.keyframe_interval_ms, 4_000);
        assert_eq!((before.width, before.height), (64, 36));
        h.ctrl.reset_codec_state();
        let after = h.ctrl.stream_params();
        assert_eq!(after.keyframe_interval_ms, 0);
        assert_eq!((after.width, after.height), (0, 0));
    }

    /// OBS's sequence header decodes to the resolution the compatibility
    /// check sees, width and height each in their own place.
    #[test]
    fn stream_params_reports_the_resolution_obs_sends() {
        let h = harness(0);
        assert_eq!(h.ctrl.stream_params().width, 0, "nothing sent yet");
        let header = crate::slate::test_sequence_header(64, 36);
        h.ctrl.on_tag(9, 0, &header, false, true);
        let p = h.ctrl.stream_params();
        assert_eq!((p.width, p.height), (64, 36));
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

    /// Going delayed from live has to jump back even when the delay is no
    /// longer than the re-cut dead band. Live sits a whole delay short of
    /// it, which the dead band (hysteresis for a delay already in place)
    /// used to read as close enough: 1 s against its 1.5 s floor, so the
    /// dashboard said the delay was on while viewers stayed on live.
    #[test]
    fn a_one_second_delay_jumps_back_from_live() {
        let h = harness(0);
        h.ctrl.arm_delay(1_000);
        feed_seconds(&h.ctrl, 0, 3, 30);
        h.ctrl.activate_delay().expect("the buffer holds the delay");
        let latest = h.ctrl.ring.latest_seq().expect("a populated ring");
        let (_, live_edge) = h.ctrl.ring.find_by_seq(latest).unwrap();
        assert!(
            compute_delay_cut(&h.ctrl, &live_edge).is_some(),
            "live is a whole second short of the delay"
        );
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

    /// OBS hung without its connection closing (the OS keeps ACKing), and
    /// the streamer restarts it: the new publisher takes the slot once the
    /// old one has sent no video for the freeze threshold, instead of being
    /// refused until the old socket dies. The old connection closing later
    /// must not end the new session.
    #[tokio::test]
    async fn a_new_publisher_takes_over_from_one_that_stopped_sending_video() {
        let h = harness(0);
        let first = h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl
            .on_tag(9, 0, &[0x17, 1, 0, 0, 0, 0, 0, 0, 1, 0x65], true, false);
        assert!(
            h.ctrl.begin_publish("k", "127.0.0.1").await.is_err(),
            "a publisher that is sending keeps the slot"
        );

        // The process clock starts at 0: let it pass the freeze threshold,
        // so "the last video frame" can sit that far back.
        let freeze_ms = crate::crash_hold::FREEZE_AFTER.as_millis() as u64;
        while process_now_ms() <= freeze_ms + 10 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        h.ctrl.last_video_tag_ms.store(1, Ordering::Relaxed);
        let second = h
            .ctrl
            .begin_publish("k", "127.0.0.1")
            .await
            .expect("a publisher that stopped sending video hands over");
        assert_ne!(first, second);

        h.ctrl.end_publish(first);
        assert!(
            h.ctrl.ingest_alive(),
            "the old connection closing is ignored"
        );
        h.ctrl.end_publish(second);
        assert!(!h.ctrl.ingest_alive(), "the current one closing ends it");
    }

    /// A peer retrying a refused publish in a loop logs one line per 10 s,
    /// not one per attempt, so the bounded log keeps everything else.
    #[tokio::test]
    async fn refused_publishes_are_logged_at_most_once_per_ten_seconds() {
        let h = harness(0);
        h.ctrl.begin_publish("k", "127.0.0.1").await.unwrap();
        h.ctrl
            .on_tag(9, 0, &[0x17, 1, 0, 0, 0, 0, 0, 0, 1, 0x65], true, false);
        for _ in 0..50 {
            let _ = h.ctrl.begin_publish("k", "127.0.0.1").await;
        }
        let refusals = h
            .ctrl
            .logs
            .lock()
            .iter()
            .filter(|line| line.contains("slot in use"))
            .count();
        assert_eq!(refusals, 1);
    }
}
