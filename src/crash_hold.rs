//! Crash protection at runtime: the hold window after OBS drops, and the
//! reconnect screen played into each destination while it lasts.
//!
//! When OBS disconnects without saying goodbye (FCUnpublish /
//! deleteStream), or stops sending video for `FREEZE_AFTER` while still
//! connected, and crash protection is on, the controller opens a hold.
//! Each destination's pump first plays out any buffered delay, then keeps
//! every video track it receives alive until OBS publishes again (the
//! pump's reconnect path takes over from there), the hold time runs out,
//! or the streamer ends it early:
//!
//! - H.264 tracks loop the reconnect screen at the track's own resolution,
//!   framed exactly like the stream's own tags (legacy, Enhanced RTMP, or
//!   an Enhanced Broadcasting multitrack tag with its track id).
//! - Other tracks (HEVC, AV1) hold their last keyframe, re-sent once a
//!   second, because the screen's encoder only speaks H.264.
//! - Every AAC track gets digital silence in its own format.
//!
//! Timestamps continue from the last real frame, so the platform sees one
//! unbroken stream.

use crate::controller::{Controller, DestinationState};
use crate::crash_protection::CrashProtection;
use crate::h264::{AudioEgress, VideoEgress};
use crate::rtmp::client::EgressSink;
use crate::slate::{SlateLoop, StreamShape};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Frame rate of the reconnect screen. It is a still-ish loop, so 30 is
/// plenty and halves the encode work of 60.
pub const SLATE_FPS: u32 = 30;

/// No video for this long from a connected OBS counts as a freeze.
pub const FREEZE_AFTER: Duration = Duration::from_secs(3);

/// A held keyframe is re-sent once a second (every this many screen
/// frames): a still picture needs no more, and keyframes are large.
const HELD_FRAME_EVERY: u64 = SLATE_FPS as u64;

/// OBS is back but its video hasn't rebuilt the armed delay yet: stop
/// waiting this long past the delay itself (a buffer too small to hold
/// it, or a delay raised meanwhile) and rejoin where the video is.
const REBUILD_GRACE: Duration = Duration::from_secs(5);

/// Used when a destination's resolution can't be read from its SPS.
const FALLBACK_SHAPE: (usize, usize) = (1280, 720);

// FLV and Enhanced RTMP tag header fields.
const FLV_VIDEO_AVC: u8 = 7;
const FLV_AUDIO_AAC: u8 = 10;
const FLV_AUDIO_EX: u8 = 9;
const AVC_NALU: u8 = 1;
const AAC_RAW: u8 = 1;
const EX_VIDEO: u8 = 0x80;
const EX_SEQUENCE_START: u8 = 0;
const EX_CODED_FRAMES: u8 = 1;
const EX_VIDEO_MULTITRACK: u8 = 6;
const EX_AUDIO_MULTITRACK: u8 = 5;
const ONE_TRACK: u8 = 0;
const FRAME_KEY: u8 = 1;
const FRAME_INTER: u8 = 2;
const FOURCC_AVC1: [u8; 4] = *b"avc1";
const FOURCC_MP4A: [u8; 4] = *b"mp4a";

/// Raw AAC-LC frames that decode to digital silence (verified with
/// ffmpeg: no decoder errors, every sample 0).
const SILENT_AAC_STEREO: &[u8] = &[0x21, 0x00, 0x49, 0x90, 0x02, 0x19, 0x00, 0x23, 0x80];
const SILENT_AAC_MONO: &[u8] = &[0x01, 0x40, 0x20, 0x07];
const AAC_OBJECT_LC: u8 = 2;
const AAC_FRAME_SAMPLES: f64 = 1024.0;
/// ISO 14496-3 sampling frequency index table.
const AAC_SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// A hold on air, as the dashboard, dock and tray show it.
#[derive(Debug, Clone, Copy)]
pub struct HoldStatus {
    pub reason: HoldReason,
    pub remaining: Duration,
    pub total: Duration,
}

/// An open hold: why, when it started, and when it gives up.
#[derive(Debug, Clone, Copy)]
pub struct Hold {
    pub reason: HoldReason,
    pub started: Instant,
    pub deadline: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldReason {
    /// OBS's connection dropped without a goodbye.
    Crash,
    /// OBS is still connected but stopped sending video.
    Freeze,
}

/// Where the hold stands, as the pumps see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldState {
    /// Destinations are on the reconnect screen.
    Holding,
    /// The last hold closed because OBS is sending again.
    Resumed,
    /// The last hold closed without OBS: it ran out or was ended.
    Ended,
}

/// Why playback of the reconnect screen stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldExit {
    /// OBS is sending again and its video since then covers the armed
    /// delay (at once without one): the pump rejoins at a keyframe at or
    /// after `rejoin_from_seq`, never on video from before the hold.
    Resumed { rejoin_from_seq: u64 },
    /// The hold ended (time ran out, the streamer ended it, or the
    /// destination can't be covered): end the platform session cleanly.
    Ended,
}

pub struct HoldOutcome {
    pub exit: HoldExit,
    /// Last output timestamp sent, so the pump's timeline stays monotonic.
    pub last_ts: u32,
}

/// How a destination already receives a track, so the hold's tags are
/// framed exactly like the stream's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// Legacy FLV: `0x17` / `0x27` AVC video, `0xAF`-style AAC audio.
    Legacy,
    /// Enhanced RTMP, one track: header byte, then the FourCC.
    Enhanced,
    /// Enhanced RTMP multitrack, OneTrack layout (Enhanced Broadcasting):
    /// header byte, layout byte, FourCC, then this track id.
    OneTrack(u8),
}

/// What one destination gets during a hold: a picture for every video
/// track it receives, and silence for every AAC track.
struct HoldPlan {
    video: Vec<VideoSource>,
    audio: Vec<SilentAudio>,
}

enum VideoSource {
    /// The reconnect screen, encoded at the track's own resolution.
    Screen {
        shape: StreamShape,
        framing: Framing,
    },
    /// The track's last keyframe as the destination receives it.
    HeldFrame(Arc<[u8]>),
}

/// A `VideoSource` ready to play, as the tags the destination gets.
enum VideoTrack {
    /// The loop wrapped once in the track's framing, not once per frame.
    Screen {
        shape: StreamShape,
        /// The two IDR variants, then the P frames, in loop order.
        keyframes: [Vec<u8>; 2],
        deltas: Vec<Vec<u8>>,
    },
    HeldFrame(Arc<[u8]>),
}

impl VideoTrack {
    fn screen(slate: &SlateLoop, framing: Framing) -> Self {
        VideoTrack::Screen {
            shape: slate.shape,
            keyframes: [0, 1].map(|replay| video_tag(framing, true, &slate.keyframes[replay])),
            deltas: slate
                .deltas
                .iter()
                .map(|sample| video_tag(framing, false, sample))
                .collect(),
        }
    }
}

/// Silent AAC matching one of the stream's own AudioSpecificConfigs, so
/// the destination's decoder needs no reconfiguration.
struct SilentAudio {
    framing: Framing,
    /// First byte of the stream's own legacy audio tags (format, rate,
    /// size, channels), reused so the silent tags look like the real ones.
    /// Unused for Enhanced RTMP framings.
    legacy_flags: u8,
    frame: &'static [u8],
    frame_ms: f64,
}

/// Whether this destination can be kept live on the reconnect screen.
/// When it can't, it ends like it did before crash protection.
pub fn covers(ctrl: &Controller, dest: &DestinationState) -> bool {
    plan_for(ctrl, dest).is_ok()
}

/// Keep a reconnect loop encoded for each of `dest`'s video tracks.
pub fn prebuild_for(ctrl: &Controller, dest: &DestinationState, settings: &CrashProtection) {
    let Some(egress) = dest.video_egress() else {
        return;
    };
    for (_, header) in video_headers_for(ctrl, egress) {
        if avc_framing(&header).is_some() {
            ctrl.slate_cache.prebuild(settings, screen_shape(&header));
        }
    }
}

/// Whether the hold plays this video tag's track by holding its last
/// keyframe: an Enhanced RTMP keyframe that isn't H.264 in a framing the
/// reconnect screen matches (HEVC, AV1, bundled multitrack layouts). The
/// controller keeps the latest one per track.
pub fn is_held_keyframe(tag: &[u8]) -> bool {
    let Some(&first) = tag.first() else {
        return false;
    };
    if first & EX_VIDEO == 0 || !crate::h264::classify_video_tag(tag).is_idr {
        return false;
    }
    let fourcc = match first & 0x0F {
        EX_VIDEO_MULTITRACK if tag.get(1).map(|layout| layout >> 4) == Some(ONE_TRACK) => {
            tag.get(2..6)
        }
        // A bundle of several tracks per tag: only the whole tag can be held.
        EX_VIDEO_MULTITRACK => None,
        _ => tag.get(1..5),
    };
    fourcc != Some(&FOURCC_AVC1[..])
}

/// The picture and silence for each track of `dest`, or why it can't be
/// covered.
fn plan_for(ctrl: &Controller, dest: &DestinationState) -> Result<HoldPlan, String> {
    let Some(egress) = dest.video_egress() else {
        return Err("its vertical canvas isn't known".into());
    };
    let headers = video_headers_for(ctrl, egress);
    if headers.is_empty() {
        return Err("no video has arrived yet".into());
    }
    let video = headers
        .iter()
        .map(|(track, header)| video_source(ctrl, egress, *track, header))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HoldPlan {
        video,
        audio: silent_audio_for(ctrl, dest.audio_egress()),
    })
}

/// The video sequence headers `egress` sends, keyed by ingest track id,
/// as the destination receives them (the same pick as the pump's
/// `send_sequence_headers`).
fn video_headers_for(ctrl: &Controller, egress: VideoEgress) -> Vec<(u8, Vec<u8>)> {
    let headers = ctrl.ring.video_seq_headers.lock();
    let target = match egress {
        VideoEgress::Passthrough => {
            return headers.iter().map(|(t, h)| (*t, h.clone())).collect();
        }
        VideoEgress::Track(target) => target,
    };
    let pick = headers
        .get_key_value(&target)
        .or_else(|| (target == 0).then(|| headers.iter().next()).flatten());
    pick.map(|(track, header)| {
        let sent = crate::h264::select_video_bytes(header, egress)
            .map(Cow::into_owned)
            .unwrap_or_else(|| header.clone());
        (*track, sent)
    })
    .into_iter()
    .collect()
}

/// The reconnect screen for an H.264 track, or its last keyframe for
/// anything else.
fn video_source(
    ctrl: &Controller,
    egress: VideoEgress,
    track: u8,
    header: &[u8],
) -> Result<VideoSource, String> {
    if let Some(framing) = avc_framing(header) {
        let shape = screen_shape(header);
        return Ok(VideoSource::Screen { shape, framing });
    }
    let codec = crate::h264::seq_header_codec(header).label();
    let Some(keyframe) = ctrl.held_keyframe(track) else {
        return Err(format!(
            "its {codec} video hasn't sent a keyframe to hold yet"
        ));
    };
    let sent: Arc<[u8]> = match crate::h264::select_video_bytes(&keyframe, egress) {
        Some(Cow::Borrowed(_)) => keyframe.clone(),
        Some(Cow::Owned(flattened)) => flattened.into(),
        None => return Err(format!("its {codec} keyframe can't be sent here")),
    };
    Ok(VideoSource::HeldFrame(sent))
}

/// The screen's shape for an H.264 track: its resolution, rounded down
/// to even sides, at the screen's frame rate.
fn screen_shape(header: &[u8]) -> StreamShape {
    let (width, height) = crate::h264::sps_dimensions(header)
        .map(|(w, h)| (w as usize & !1, h as usize & !1))
        .unwrap_or(FALLBACK_SHAPE);
    StreamShape {
        width,
        height,
        fps: SLATE_FPS,
    }
}

/// The framing of an H.264 sequence header the reconnect screen can
/// match, or `None` for other codecs and bundled multitrack layouts.
fn avc_framing(header: &[u8]) -> Option<Framing> {
    let &first = header.first()?;
    if first & EX_VIDEO == 0 {
        let is_avc_config = first & 0x0F == FLV_VIDEO_AVC && header.get(1) == Some(&0);
        return is_avc_config.then_some(Framing::Legacy);
    }
    let (framing, fourcc, _) = enhanced_sequence_start(header, EX_VIDEO_MULTITRACK)?;
    (fourcc == FOURCC_AVC1).then_some(framing)
}

/// Parse an Enhanced RTMP sequence-start tag (video or audio): its
/// framing, FourCC and codec config. `multitrack` is the packet type that
/// marks a multitrack tag for this media kind. Only the single-track and
/// OneTrack layouts carry one track's config.
fn enhanced_sequence_start(tag: &[u8], multitrack: u8) -> Option<(Framing, [u8; 4], &[u8])> {
    let packet_type = tag.first()? & 0x0F;
    if packet_type == EX_SEQUENCE_START {
        let fourcc = tag.get(1..5)?.try_into().ok()?;
        return Some((Framing::Enhanced, fourcc, tag.get(5..)?));
    }
    let layout = *tag.get(1)?;
    if packet_type != multitrack || layout >> 4 != ONE_TRACK || layout & 0x0F != EX_SEQUENCE_START {
        return None;
    }
    let fourcc = tag.get(2..6)?.try_into().ok()?;
    Some((Framing::OneTrack(*tag.get(6)?), fourcc, tag.get(7..)?))
}

/// Silence for each AAC track this destination already receives. Other
/// audio (Opus, HE-AAC, surround) gets none; video still plays.
fn silent_audio_for(ctrl: &Controller, egress: AudioEgress) -> Vec<SilentAudio> {
    let headers = ctrl.ring.audio_seq_headers.lock();
    let target = match egress {
        AudioEgress::Passthrough => {
            return headers
                .values()
                .filter_map(|h| silent_audio_from_header(h))
                .collect();
        }
        AudioEgress::Track(target) => target,
    };
    let Some(header) = headers
        .get(&target)
        .or_else(|| headers.get(&0))
        .or_else(|| headers.values().next())
    else {
        return Vec::new();
    };
    crate::h264::select_audio_bytes(header, egress)
        .and_then(|tag| silent_audio_from_header(&tag))
        .into_iter()
        .collect()
}

/// Parse an AAC sequence-header tag (legacy `[flags, 0x00, ASC...]`, or
/// Enhanced RTMP `mp4a`) into matching silence.
fn silent_audio_from_header(header: &[u8]) -> Option<SilentAudio> {
    let &first = header.first()?;
    let (framing, legacy_flags, asc) = match first >> 4 {
        FLV_AUDIO_AAC if header.get(1) == Some(&0) => (Framing::Legacy, first, header.get(2..)?),
        FLV_AUDIO_EX => {
            let (framing, fourcc, asc) = enhanced_sequence_start(header, EX_AUDIO_MULTITRACK)?;
            if fourcc != FOURCC_MP4A {
                return None;
            }
            (framing, 0, asc)
        }
        _ => return None,
    };
    let asc = asc.get(..2)?;
    let object_type = asc[0] >> 3;
    let rate_index = ((asc[0] & 0x07) << 1) | (asc[1] >> 7);
    let channels = (asc[1] >> 3) & 0x0F;
    if object_type != AAC_OBJECT_LC {
        return None;
    }
    let sample_rate = *AAC_SAMPLE_RATES.get(rate_index as usize)?;
    let frame = match channels {
        1 => SILENT_AAC_MONO,
        2 => SILENT_AAC_STEREO,
        _ => return None,
    };
    Some(SilentAudio {
        framing,
        legacy_flags,
        frame,
        frame_ms: AAC_FRAME_SAMPLES * 1000.0 / sample_rate as f64,
    })
}

/// Play the reconnect screen into `dest` until the hold ends or OBS comes
/// back. `after_ts` is the last output timestamp already sent.
pub async fn play(
    ctrl: &Arc<Controller>,
    dest: &Arc<DestinationState>,
    sink: &mut EgressSink,
    after_ts: u32,
) -> io::Result<HoldOutcome> {
    let ended = HoldOutcome {
        exit: HoldExit::Ended,
        last_ts: after_ts,
    };
    let plan = match plan_for(ctrl, dest) {
        Ok(plan) => plan,
        Err(why) => {
            ctrl.log(format!(
                "[{}] crash protection can't cover this destination: {why}",
                dest.id
            ));
            return Ok(ended);
        }
    };
    let settings = ctrl.crash_protection();
    let mut video = Vec::with_capacity(plan.video.len());
    for source in plan.video {
        video.push(match source {
            VideoSource::Screen { shape, framing } => {
                let Some(slate) = ctrl.slate_cache.get(&settings, shape).await else {
                    ctrl.log(format!(
                        "[{}] crash protection couldn't build the reconnect screen",
                        dest.id
                    ));
                    return Ok(ended);
                };
                VideoTrack::screen(&slate, framing)
            }
            VideoSource::HeldFrame(tag) => VideoTrack::HeldFrame(tag),
        });
    }
    ctrl.log(format!(
        "[{}] crash protection: reconnect screen on air ({})",
        dest.id,
        describe(&video)
    ));
    // No sequence header: the loop's keyframes carry their own SPS/PPS
    // in-band under ids the stream doesn't use (see slate::encoder), so the
    // destination's decoder keeps the stream's config for the way back.
    let base = after_ts.wrapping_add(1);
    stream_loop(ctrl, dest, sink, &video, &plan.audio, base).await
}

/// "1920x1080, 1280x720, last keyframe held" for the log.
fn describe(video: &[VideoTrack]) -> String {
    video
        .iter()
        .map(|track| match track {
            VideoTrack::Screen { shape, .. } => format!("{}x{}", shape.width, shape.height),
            VideoTrack::HeldFrame(_) => "last keyframe held".to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Send screen frames and silent audio, interleaved by timestamp and paced
/// in real time, until OBS returns or the hold ends.
async fn stream_loop(
    ctrl: &Arc<Controller>,
    dest: &Arc<DestinationState>,
    sink: &mut EgressSink,
    video: &[VideoTrack],
    audio: &[SilentAudio],
    base: u32,
) -> io::Result<HoldOutcome> {
    let frame_ms = 1000.0 / SLATE_FPS as f64;
    let start = tokio::time::Instant::now();
    let mut video_index = 0u64;
    let mut audio_index = vec![0u64; audio.len()];
    let mut last_ts = base;
    let mut rebuild: Option<Rebuild> = None;
    loop {
        let exit = match ctrl.hold_state() {
            _ if dest.shutdown_requested.load(Ordering::Relaxed) => Some(HoldExit::Ended),
            HoldState::Holding => {
                // OBS may drop again while the delay rebuilds: start over.
                rebuild = None;
                None
            }
            // Back, then gone again without a new hold (a deliberate stop).
            HoldState::Resumed if !ctrl.ingest_alive() => Some(HoldExit::Ended),
            // With a delay armed the screen stays up until OBS's new video
            // covers it, so viewers never see live.
            HoldState::Resumed => {
                let delay = u64::from(ctrl.target_delay_ms());
                delay_rebuilt(ctrl, &mut rebuild, delay)
                    .map(|rejoin_from_seq| HoldExit::Resumed { rejoin_from_seq })
            }
            HoldState::Ended => Some(HoldExit::Ended),
        };
        if let Some(exit) = exit {
            return Ok(HoldOutcome { exit, last_ts });
        }
        sink.drain_pings().await?;

        let video_at = video_index as f64 * frame_ms;
        // The audio track due soonest, when it is due before the next
        // video frame.
        let audio_due = audio
            .iter()
            .zip(&audio_index)
            .map(|(track, sent)| *sent as f64 * track.frame_ms)
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|(_, at)| *at < video_at);
        let at = audio_due.map_or(video_at, |(_, at)| at);
        tokio::time::sleep_until(start + Duration::from_micros((at * 1000.0) as u64)).await;
        let ts = base.wrapping_add(at.round() as u32);

        if let Some((track, _)) = audio_due {
            sink.send_audio(ts, &audio_tag(&audio[track])).await?;
            audio_index[track] += 1;
        } else {
            for track in video {
                let Some(tag) = video_frame(track, video_index) else {
                    continue;
                };
                sink.send_video(ts, tag).await?;
                dest.tags_sent.fetch_add(1, Ordering::Relaxed);
                dest.bytes_sent
                    .fetch_add(tag.len() as u64, Ordering::Relaxed);
                dest.note_outbound_bytes(tag.len());
            }
            video_index += 1;
        }
        last_ts = ts;
    }
}

/// Where OBS's video since it came back starts, tracked while the screen
/// waits for it to rebuild the armed delay.
pub(crate) struct Rebuild {
    /// The newest ring seq when OBS came back (None: the ring was empty,
    /// as it is for a new publisher). Its new video starts after it.
    after_seq: Option<u64>,
    since: Instant,
}

/// The ring seq OBS's new video starts at, once that video spans
/// `delay_ms` (at its first tag for no delay; or once the wait has run
/// `REBUILD_GRACE` past the delay).
pub(crate) fn delay_rebuilt(
    ctrl: &Controller,
    rebuild: &mut Option<Rebuild>,
    delay_ms: u64,
) -> Option<u64> {
    let rebuild = rebuild.get_or_insert_with(|| Rebuild {
        after_seq: ctrl.resumed_after_seq(),
        since: Instant::now(),
    });
    let first_seq = match rebuild.after_seq {
        Some(seq) => seq + 1,
        None => ctrl.ring.front_seq()?,
    };
    if rebuild.since.elapsed() >= Duration::from_millis(delay_ms) + REBUILD_GRACE {
        return Some(first_seq);
    }
    let (_, first) = ctrl.ring.find_by_seq(first_seq)?;
    let spanned = ctrl.ring.latest_ts()?.saturating_sub(first.ts_ms);
    (spanned >= delay_ms).then_some(first_seq)
}

/// The tag `track` sends for screen frame `index`, if any: every frame of
/// the loop, or the held keyframe once a second.
fn video_frame(track: &VideoTrack, index: u64) -> Option<&[u8]> {
    match track {
        VideoTrack::Screen {
            keyframes, deltas, ..
        } => {
            let frames = 1 + deltas.len() as u64;
            let position = (index % frames) as usize;
            if position == 0 {
                // Alternate the two IDR variants: back-to-back IDRs must not
                // share an idr_pic_id.
                let replay = (index / frames) as usize % 2;
                Some(&keyframes[replay])
            } else {
                Some(&deltas[position - 1])
            }
        }
        VideoTrack::HeldFrame(tag) => index.is_multiple_of(HELD_FRAME_EVERY).then_some(&tag[..]),
    }
}

/// An H.264 coded-frame tag in `framing`, zero composition time.
fn video_tag(framing: Framing, keyframe: bool, sample: &[u8]) -> Vec<u8> {
    let frame_type = if keyframe { FRAME_KEY } else { FRAME_INTER };
    let mut tag = Vec::with_capacity(sample.len() + 12);
    match framing {
        Framing::Legacy => tag.extend_from_slice(&[frame_type << 4 | FLV_VIDEO_AVC, AVC_NALU]),
        Framing::Enhanced => {
            tag.push(EX_VIDEO | frame_type << 4 | EX_CODED_FRAMES);
            tag.extend_from_slice(&FOURCC_AVC1);
        }
        Framing::OneTrack(track) => {
            tag.push(EX_VIDEO | frame_type << 4 | EX_VIDEO_MULTITRACK);
            tag.push(ONE_TRACK << 4 | EX_CODED_FRAMES);
            tag.extend_from_slice(&FOURCC_AVC1);
            tag.push(track);
        }
    }
    tag.extend_from_slice(&[0, 0, 0]);
    tag.extend_from_slice(sample);
    tag
}

/// One silent AAC frame in the track's own framing.
fn audio_tag(audio: &SilentAudio) -> Vec<u8> {
    let mut tag = Vec::with_capacity(audio.frame.len() + 7);
    match audio.framing {
        Framing::Legacy => tag.extend_from_slice(&[audio.legacy_flags, AAC_RAW]),
        Framing::Enhanced => {
            tag.push(FLV_AUDIO_EX << 4 | EX_CODED_FRAMES);
            tag.extend_from_slice(&FOURCC_MP4A);
        }
        Framing::OneTrack(track) => {
            tag.push(FLV_AUDIO_EX << 4 | EX_AUDIO_MULTITRACK);
            tag.push(ONE_TRACK << 4 | EX_CODED_FRAMES);
            tag.extend_from_slice(&FOURCC_MP4A);
            tag.push(track);
        }
    }
    tag.extend_from_slice(audio.frame);
    tag
}

/// Encoded reconnect-screen loops, one per destination shape, rebuilt when
/// the screen's look changes. Encoding a 1080p loop takes a few hundred
/// ms of one core, so loops are built ahead of time while OBS is live and
/// a hold usually starts with one ready.
#[derive(Default)]
pub struct SlateCache {
    /// Per (width, height): the loop and the settings it was built with.
    ready: crate::sync::Mutex<HashMap<SlateKey, (CrashProtection, Arc<SlateLoop>)>>,
    building: crate::sync::Mutex<HashSet<SlateKey>>,
}

/// A slate loop's (width, height).
type SlateKey = (usize, usize);

impl SlateCache {
    fn cached(&self, settings: &CrashProtection, shape: StreamShape) -> Option<Arc<SlateLoop>> {
        self.ready
            .lock()
            .get(&(shape.width, shape.height))
            .filter(|(built_with, _)| built_with.same_screen(settings))
            .map(|(_, slate)| slate.clone())
    }

    /// The loop for `shape`, building it now if it isn't ready. When a
    /// build of it is already running (a prebuild, or another destination
    /// at the same resolution) this waits for that one rather than encode
    /// the same loop twice at the moment OBS drops.
    pub async fn get(
        self: &Arc<Self>,
        settings: &CrashProtection,
        shape: StreamShape,
    ) -> Option<Arc<SlateLoop>> {
        const BUILD_POLL: Duration = Duration::from_millis(50);
        let key = (shape.width, shape.height);
        loop {
            if let Some(slate) = self.cached(settings, shape) {
                return Some(slate);
            }
            if self.building.lock().insert(key) {
                break;
            }
            tokio::time::sleep(BUILD_POLL).await;
        }
        // The build runs as its own task, so it always finishes and clears
        // `building`: a caller cancelled mid-build (its pump aborted by the
        // supervisor) must not leave every later caller waiting forever.
        let cache = self.clone();
        let settings = settings.clone();
        let task = tokio::spawn(async move {
            let slate = build(settings.clone(), shape).await;
            if let Some(slate) = &slate {
                cache.ready.lock().insert(key, (settings, slate.clone()));
            }
            cache.building.lock().remove(&key);
            slate
        });
        task.await.ok().flatten()
    }

    /// Start building the loop for `shape` in the background, unless it is
    /// ready or already being built.
    pub fn prebuild(self: &Arc<Self>, settings: &CrashProtection, shape: StreamShape) {
        let key = (shape.width, shape.height);
        if self.cached(settings, shape).is_some() || self.building.lock().contains(&key) {
            return;
        }
        let cache = self.clone();
        let settings = settings.clone();
        tokio::spawn(async move {
            cache.get(&settings, shape).await;
        });
    }

    /// Drop every loop (crash protection was switched off).
    pub fn clear(&self) {
        self.ready.lock().clear();
    }
}

/// Encode a loop off the async runtime.
async fn build(settings: CrashProtection, shape: StreamShape) -> Option<Arc<SlateLoop>> {
    tokio::task::spawn_blocking(move || {
        crate::slate::build_loop(&settings, shape)
            .ok()
            .map(Arc::new)
    })
    .await
    .ok()
    .flatten()
}

/// "1:05" for a duration, for logs and webhooks.
pub fn minutes_seconds(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::select_video_bytes;

    fn aac_header(flags: u8, asc: [u8; 2]) -> Vec<u8> {
        vec![flags, 0, asc[0], asc[1]]
    }

    /// An Enhanced Broadcasting (OneTrack) sequence-start tag.
    fn one_track_header(first: u8, fourcc: &[u8; 4], track: u8, config: &[u8]) -> Vec<u8> {
        let mut tag = vec![first, ONE_TRACK << 4 | EX_SEQUENCE_START];
        tag.extend_from_slice(fourcc);
        tag.push(track);
        tag.extend_from_slice(config);
        tag
    }

    #[test]
    fn silence_matches_the_streams_own_aac_config() {
        // AAC-LC, 48 kHz (index 3), stereo: 00010 0011 0010 000.
        let stereo = silent_audio_from_header(&aac_header(0xAF, [0x11, 0x90])).unwrap();
        assert_eq!(stereo.frame, SILENT_AAC_STEREO);
        assert_eq!(stereo.framing, Framing::Legacy);
        assert_eq!(audio_tag(&stereo)[..2], [0xAF, AAC_RAW]);
        assert!((stereo.frame_ms - 21.333).abs() < 0.01);
        // AAC-LC, 44.1 kHz (index 4), mono.
        let mono = silent_audio_from_header(&aac_header(0xAE, [0x12, 0x08])).unwrap();
        assert_eq!(mono.frame, SILENT_AAC_MONO);
        assert!((mono.frame_ms - 23.22).abs() < 0.01);
    }

    #[test]
    fn a_vod_audio_track_gets_silence_under_its_own_track_id() {
        let header = one_track_header(0x95, b"mp4a", 1, &[0x11, 0x90]);
        let silence = silent_audio_from_header(&header).unwrap();
        assert_eq!(silence.framing, Framing::OneTrack(1));
        let tag = audio_tag(&silence);
        assert_eq!(tag[..7], [0x95, 0x01, b'm', b'p', b'4', b'a', 1]);
        // It flattens back to the legacy silent frame any ingest accepts.
        let flat = crate::h264::flatten_multitrack_audio(&tag, 1).unwrap();
        assert_eq!(flat[..2], [0xAF, AAC_RAW]);
        assert_eq!(&flat[2..], SILENT_AAC_STEREO);
    }

    #[test]
    fn unsupported_audio_gets_no_silence() {
        // HE-AAC (object type 5), 5.1 channels, a non-AAC tag, Opus.
        assert!(silent_audio_from_header(&aac_header(0xAF, [0x29, 0x90])).is_none());
        assert!(silent_audio_from_header(&aac_header(0xAF, [0x11, 0xB0])).is_none());
        assert!(silent_audio_from_header(&[0x2F, 0x00, 0x11, 0x90]).is_none());
        assert!(silent_audio_from_header(&aac_header(0xAF, [0x11, 0x90])[..2]).is_none());
        assert!(silent_audio_from_header(&one_track_header(0x95, b"Opus", 1, &[0; 8])).is_none());
    }

    #[test]
    fn the_screen_matches_each_h264_framing_and_nothing_else() {
        assert_eq!(avc_framing(&[0x17, 0, 0, 0, 0, 1]), Some(Framing::Legacy));
        let mut enhanced = vec![EX_VIDEO | FRAME_KEY << 4 | EX_SEQUENCE_START];
        enhanced.extend_from_slice(b"avc1");
        assert_eq!(avc_framing(&enhanced), Some(Framing::Enhanced));
        let rung = one_track_header(0x96, b"avc1", 2, &[1]);
        assert_eq!(avc_framing(&rung), Some(Framing::OneTrack(2)));
        assert_eq!(avc_framing(&one_track_header(0x96, b"hvc1", 2, &[1])), None);
        // ManyTracks bundles several tracks' configs in one tag.
        let mut bundle = one_track_header(0x96, b"avc1", 0, &[1]);
        bundle[1] = 1 << 4;
        assert_eq!(avc_framing(&bundle), None);
    }

    #[test]
    fn screen_frames_are_framed_like_the_track_they_replace() {
        assert_eq!(
            video_tag(Framing::Legacy, true, &[9, 9]),
            vec![0x17, 1, 0, 0, 0, 9, 9]
        );
        assert_eq!(video_tag(Framing::Legacy, false, &[9])[0], 0x27);
        let rung = video_tag(Framing::OneTrack(3), true, &[9, 9]);
        assert_eq!(rung[..7], [0x96, 0x01, b'a', b'v', b'c', b'1', 3]);
        // A vertical destination fed track 3 sees the plain legacy frame.
        let flat = select_video_bytes(&rung, VideoEgress::Track(3)).unwrap();
        assert_eq!(&flat[..], &[0x17, 1, 0, 0, 0, 9, 9]);
        let enhanced = video_tag(Framing::Enhanced, false, &[9]);
        assert_eq!(enhanced[..5], [0xA1, b'a', b'v', b'c', b'1']);
    }

    #[test]
    fn only_keyframes_the_screen_cant_replace_are_held() {
        let key = EX_VIDEO | FRAME_KEY << 4 | EX_VIDEO_MULTITRACK;
        let coded = ONE_TRACK << 4 | EX_CODED_FRAMES;
        let hevc = [key, coded, b'h', b'v', b'c', b'1', 1, 0, 0, 0, 7];
        assert!(is_held_keyframe(&hevc));
        let mut avc = hevc;
        avc[2..6].copy_from_slice(b"avc1");
        assert!(!is_held_keyframe(&avc));
        let mut inter = hevc;
        inter[0] = EX_VIDEO | FRAME_INTER << 4 | EX_VIDEO_MULTITRACK;
        assert!(!is_held_keyframe(&inter));
        let mut bundle = avc;
        bundle[1] = 1 << 4 | EX_CODED_FRAMES;
        assert!(is_held_keyframe(&bundle));
        let av1 = [
            EX_VIDEO | FRAME_KEY << 4 | EX_CODED_FRAMES,
            b'a',
            b'v',
            b'0',
            b'1',
            7,
        ];
        assert!(is_held_keyframe(&av1));
        assert!(!is_held_keyframe(&[0x17, 1, 0, 0, 0, 0, 0, 0, 1, 0x65]));
    }

    #[test]
    fn a_held_keyframe_is_resent_once_a_second() {
        let held = VideoTrack::HeldFrame(Arc::from(&[1u8, 2, 3][..]));
        assert!(video_frame(&held, 0).is_some());
        assert!(video_frame(&held, 1).is_none());
        assert!(video_frame(&held, HELD_FRAME_EVERY - 1).is_none());
        assert_eq!(video_frame(&held, HELD_FRAME_EVERY).unwrap()[..], [1, 2, 3]);
    }

    /// A caller cancelled while its loop is being built (its pump aborted)
    /// must not leave the resolution marked as building for everyone else.
    #[tokio::test]
    async fn a_cancelled_build_does_not_wedge_the_cache() {
        let cache = Arc::new(SlateCache::default());
        let settings = CrashProtection::default();
        let shape = StreamShape {
            width: 320,
            height: 180,
            fps: SLATE_FPS,
        };
        let first = {
            let cache = cache.clone();
            let settings = settings.clone();
            tokio::spawn(async move { cache.get(&settings, shape).await })
        };
        tokio::task::yield_now().await;
        first.abort();
        let later = tokio::time::timeout(Duration::from_secs(10), cache.get(&settings, shape));
        assert!(later.await.expect("not wedged").is_some());
    }

    /// OBS is back, but its new video comes in too slowly to span the
    /// delay (a struggling encoder, or a delay raised meanwhile). The screen
    /// can't wait forever: after the grace it rejoins where OBS's new video
    /// starts, never on the video from before the hold.
    #[test]
    fn the_screen_stops_waiting_for_the_delay_after_the_grace() {
        const DELAY_MS: u64 = 10_000;
        let path =
            std::env::temp_dir().join(format!("ic-rebuild-grace-{}.buf", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let ring = Arc::new(crate::buffer::DiskRing::create(&path, 1 << 20).unwrap());
        let ctrl = Controller::new(ring, 0);
        let keyframe = [0x17, 1, 0, 0, 0];
        ctrl.ring.append(9, 0, &keyframe, true, false).unwrap();
        let first_new = ctrl.ring.append(9, 5_000, &keyframe, true, false).unwrap();
        ctrl.ring.append(9, 6_000, &keyframe, true, false).unwrap();
        let mut rebuild = Some(Rebuild {
            after_seq: Some(0),
            since: Instant::now(),
        });
        assert_eq!(
            delay_rebuilt(&ctrl, &mut rebuild, DELAY_MS),
            None,
            "1 s of 10"
        );

        let waited = Duration::from_millis(DELAY_MS) + REBUILD_GRACE;
        rebuild.as_mut().unwrap().since = Instant::now() - waited;
        assert_eq!(delay_rebuilt(&ctrl, &mut rebuild, DELAY_MS), first_new);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn durations_print_as_minutes_and_seconds() {
        assert_eq!(minutes_seconds(Duration::from_secs(65)), "1:05");
        assert_eq!(minutes_seconds(Duration::from_secs(120)), "2:00");
    }
}
