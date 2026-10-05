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
//! - H.264 and 8-bit 4:2:0 HEVC tracks loop the reconnect screen, encoded
//!   in the track's own codec at its own resolution and framed exactly
//!   like the stream's own tags (legacy, Enhanced RTMP, or an Enhanced
//!   Broadcasting multitrack tag with its track id).
//! - Other tracks (10-bit HEVC, AV1) hold their last keyframe, re-sent
//!   once a second, because the screen's encoders only speak 8-bit H.264
//!   and HEVC.
//! - Every AAC track gets digital silence in its own format.
//!
//! Timestamps continue from the last real frame, so the platform sees one
//! unbroken stream.

use crate::controller::{Controller, DestinationState};
use crate::crash_protection::CrashProtection;
use crate::h264::{AudioEgress, VideoEgress};
use crate::rtmp::client::EgressSink;
use crate::slate::{SlateCodec, SlateLoop, StreamShape};
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

/// Used when an H.264 track's resolution can't be read from its SPS.
const FALLBACK_SIZE: (u32, u32) = (1280, 720);

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
const FOURCC_HVC1: [u8; 4] = *b"hvc1";
const FOURCC_HEV1: [u8; 4] = *b"hev1";
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
    /// OBS stopped the stream on purpose, and crash protection holds every
    /// disconnect.
    Stopped,
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
    /// The reconnect screen, encoded like the track it replaces.
    Screen(ScreenTarget),
    /// The track's last keyframe as the destination receives it.
    HeldFrame(Arc<[u8]>),
}

/// How the reconnect screen stands in for one video track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenTarget {
    framing: Framing,
    /// The track's own FourCC, which Enhanced RTMP tags repeat.
    fourcc: [u8; 4],
    codec: SlateCodec,
    shape: StreamShape,
}

/// A `VideoSource` ready to play, as the tags the destination gets.
enum VideoTrack {
    /// The loop wrapped once in the track's framing, not once per frame.
    Screen {
        shape: StreamShape,
        codec: SlateCodec,
        /// The two IDR variants, then the P frames, in loop order.
        keyframes: [Vec<u8>; 2],
        deltas: Vec<Vec<u8>>,
    },
    HeldFrame(Arc<[u8]>),
}

impl VideoTrack {
    fn screen(slate: &SlateLoop, target: ScreenTarget) -> Self {
        let tag = |keyframe: bool, sample: &[u8]| {
            video_tag(target.framing, target.fourcc, keyframe, sample)
        };
        VideoTrack::Screen {
            shape: slate.shape,
            codec: target.codec,
            keyframes: [0, 1].map(|replay| tag(true, &slate.keyframes[replay])),
            deltas: slate
                .deltas
                .iter()
                .map(|sample| tag(false, sample))
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
        if let Some(target) = screen_target(&header) {
            ctrl.slate_cache
                .prebuild(settings, target.shape, target.codec);
        }
    }
}

/// Whether the hold may play this video tag's track by holding its last
/// keyframe: an Enhanced RTMP keyframe that isn't H.264 in a framing the
/// reconnect screen matches (HEVC, AV1, bundled multitrack layouts). HEVC
/// usually gets the screen instead; its keyframe is kept for the HEVC the
/// screen can't encode (10-bit). The controller keeps the latest one per
/// track.
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

/// The reconnect screen for a track it can be encoded for, or its last
/// keyframe for anything else.
fn video_source(
    ctrl: &Controller,
    egress: VideoEgress,
    track: u8,
    header: &[u8],
) -> Result<VideoSource, String> {
    if let Some(target) = screen_target(header) {
        return Ok(VideoSource::Screen(target));
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

/// The screen's shape for a track of `width` x `height`: rounded down to
/// even sides, at the screen's frame rate.
fn screen_shape((width, height): (u32, u32)) -> StreamShape {
    StreamShape {
        width: width as usize & !1,
        height: height as usize & !1,
        fps: SLATE_FPS,
    }
}

/// How the reconnect screen replaces the track `header` describes, or
/// `None` when it can't: codecs other than H.264 and 8-bit 4:2:0 HEVC,
/// HEVC whose picture size can't be read, and bundled multitrack layouts.
fn screen_target(header: &[u8]) -> Option<ScreenTarget> {
    let &first = header.first()?;
    let avc_shape = || screen_shape(crate::h264::sps_dimensions(header).unwrap_or(FALLBACK_SIZE));
    if first & EX_VIDEO == 0 {
        let is_avc_config = first & 0x0F == FLV_VIDEO_AVC && header.get(1) == Some(&0);
        return is_avc_config.then(|| ScreenTarget {
            framing: Framing::Legacy,
            fourcc: FOURCC_AVC1,
            codec: SlateCodec::H264,
            shape: avc_shape(),
        });
    }
    let (framing, fourcc, config) = enhanced_sequence_start(header, EX_VIDEO_MULTITRACK)?;
    let (codec, shape) = match fourcc {
        FOURCC_AVC1 => (SlateCodec::H264, avc_shape()),
        FOURCC_HVC1 | FOURCC_HEV1 => (
            SlateCodec::Hevc,
            screen_shape(crate::h264::hevc_8bit_420_dimensions(config)?),
        ),
        _ => return None,
    };
    Some(ScreenTarget {
        framing,
        fourcc,
        codec,
        shape,
    })
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
    crate::h264::select_audio_bytes(header, egress, ctrl.audio_target_on_wire(egress))
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
            VideoSource::Screen(target) => {
                let built = ctrl
                    .slate_cache
                    .get(&settings, target.shape, target.codec)
                    .await;
                let Some(slate) = built else {
                    ctrl.log(format!(
                        "[{}] crash protection couldn't build the reconnect screen",
                        dest.id
                    ));
                    return Ok(ended);
                };
                VideoTrack::screen(&slate, target)
            }
            VideoSource::HeldFrame(tag) => VideoTrack::HeldFrame(tag),
        });
    }
    ctrl.log(format!(
        "[{}] crash protection: reconnect screen on air ({})",
        dest.id,
        describe(&video)
    ));
    // No sequence header: the loop's keyframes carry their own parameter
    // sets in-band under ids the stream doesn't use (see slate::encoder),
    // so the destination's decoder keeps the stream's config for the way
    // back.
    let base = after_ts.wrapping_add(1);
    stream_loop(ctrl, dest, sink, &video, &plan.audio, base).await
}

/// "2560x1440 HEVC, 1280x720 H.264, last keyframe held" for the log.
fn describe(video: &[VideoTrack]) -> String {
    video
        .iter()
        .map(|track| match track {
            VideoTrack::Screen { shape, codec, .. } => {
                format!("{}x{} {}", shape.width, shape.height, codec.label())
            }
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
/// `delay_ms` from its first keyframe (so at that keyframe for no delay;
/// or once the wait has run `REBUILD_GRACE` past the delay). Measured from
/// the keyframe, not the first tag: a frozen encoder often resumes on a
/// P-frame, and counting from it let the rejoin land short of the delay, or
/// wait on air with nothing to send until the keyframe came.
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
    let first_keyframe = ctrl.ring.oldest_idr_at_or_after(first_seq)?;
    let spanned = ctrl.ring.latest_ts()?.saturating_sub(first_keyframe.ts_ms);
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

/// A coded-frame tag in `framing`, zero composition time. Enhanced RTMP
/// framings name the codec by `fourcc`; legacy framing is H.264 only.
fn video_tag(framing: Framing, fourcc: [u8; 4], keyframe: bool, sample: &[u8]) -> Vec<u8> {
    let frame_type = if keyframe { FRAME_KEY } else { FRAME_INTER };
    let mut tag = Vec::with_capacity(sample.len() + 12);
    match framing {
        Framing::Legacy => tag.extend_from_slice(&[frame_type << 4 | FLV_VIDEO_AVC, AVC_NALU]),
        Framing::Enhanced => {
            tag.push(EX_VIDEO | frame_type << 4 | EX_CODED_FRAMES);
            tag.extend_from_slice(&fourcc);
        }
        Framing::OneTrack(track) => {
            tag.push(EX_VIDEO | frame_type << 4 | EX_VIDEO_MULTITRACK);
            tag.push(ONE_TRACK << 4 | EX_CODED_FRAMES);
            tag.extend_from_slice(&fourcc);
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

/// Encoded reconnect-screen loops, one per destination shape and codec,
/// rebuilt when the screen's look changes. Encoding a 1080p loop takes a
/// few hundred ms of one core, so loops are built ahead of time while OBS
/// is live and a hold usually starts with one ready.
#[derive(Default)]
pub struct SlateCache {
    /// Per (width, height, codec): the loop and the settings it was built
    /// with.
    ready: crate::sync::Mutex<HashMap<SlateKey, (CrashProtection, Arc<SlateLoop>)>>,
    building: crate::sync::Mutex<HashSet<SlateKey>>,
}

/// A slate loop's (width, height, codec).
type SlateKey = (usize, usize, SlateCodec);

fn slate_key(shape: StreamShape, codec: SlateCodec) -> SlateKey {
    (shape.width, shape.height, codec)
}

impl SlateCache {
    fn cached(&self, settings: &CrashProtection, key: SlateKey) -> Option<Arc<SlateLoop>> {
        self.ready
            .lock()
            .get(&key)
            .filter(|(built_with, _)| built_with.same_screen(settings))
            .map(|(_, slate)| slate.clone())
    }

    /// The loop for `shape` in `codec`, building it now if it isn't
    /// ready. When a build of it is already running (a prebuild, or
    /// another destination at the same resolution) this waits for that one
    /// rather than encode the same loop twice at the moment OBS drops.
    pub async fn get(
        self: &Arc<Self>,
        settings: &CrashProtection,
        shape: StreamShape,
        codec: SlateCodec,
    ) -> Option<Arc<SlateLoop>> {
        const BUILD_POLL: Duration = Duration::from_millis(50);
        let key = slate_key(shape, codec);
        loop {
            if let Some(slate) = self.cached(settings, key) {
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
            let slate = build(settings.clone(), shape, codec).await;
            if let Some(slate) = &slate {
                cache.ready.lock().insert(key, (settings, slate.clone()));
            }
            cache.building.lock().remove(&key);
            slate
        });
        task.await.ok().flatten()
    }

    /// Start building the loop for `shape` in `codec` in the background,
    /// unless it is ready or already being built.
    pub fn prebuild(
        self: &Arc<Self>,
        settings: &CrashProtection,
        shape: StreamShape,
        codec: SlateCodec,
    ) {
        let key = slate_key(shape, codec);
        if self.cached(settings, key).is_some() || self.building.lock().contains(&key) {
            return;
        }
        let cache = self.clone();
        let settings = settings.clone();
        tokio::spawn(async move {
            cache.get(&settings, shape, codec).await;
        });
    }

    /// Drop every loop (crash protection was switched off).
    pub fn clear(&self) {
        self.ready.lock().clear();
    }
}

/// Encode a loop off the async runtime.
async fn build(
    settings: CrashProtection,
    shape: StreamShape,
    codec: SlateCodec,
) -> Option<Arc<SlateLoop>> {
    tokio::task::spawn_blocking(move || {
        crate::slate::build_loop(&settings, shape, codec)
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

    fn framing_and_codec(header: &[u8]) -> Option<(Framing, SlateCodec)> {
        screen_target(header).map(|target| (target.framing, target.codec))
    }

    #[test]
    fn the_screen_matches_each_h264_framing() {
        let legacy = Some((Framing::Legacy, SlateCodec::H264));
        assert_eq!(framing_and_codec(&[0x17, 0, 0, 0, 0, 1]), legacy);
        let mut enhanced = vec![EX_VIDEO | FRAME_KEY << 4 | EX_SEQUENCE_START];
        enhanced.extend_from_slice(b"avc1");
        let enhanced_h264 = Some((Framing::Enhanced, SlateCodec::H264));
        assert_eq!(framing_and_codec(&enhanced), enhanced_h264);
        let rung = one_track_header(0x96, b"avc1", 2, &[1]);
        let rung_h264 = Some((Framing::OneTrack(2), SlateCodec::H264));
        assert_eq!(framing_and_codec(&rung), rung_h264);
        // ManyTracks bundles several tracks' configs in one tag.
        let mut bundle = one_track_header(0x96, b"avc1", 0, &[1]);
        bundle[1] = 1 << 4;
        assert_eq!(framing_and_codec(&bundle), None);
        assert_eq!(
            framing_and_codec(&one_track_header(0x96, b"av01", 2, &[1])),
            None
        );
    }

    /// Twitch 2K channels send Enhanced Broadcasting tracks as 8-bit HEVC:
    /// they get the screen in HEVC at their own size, under their own
    /// FourCC and track id. 10-bit HEVC, or a config whose size can't be
    /// read, falls back to the held keyframe.
    #[test]
    fn eight_bit_hevc_tracks_get_the_screen_in_hevc() {
        let config = crate::slate::test_hevc_config(2560, 1440);
        let rung = one_track_header(0x96, b"hvc1", 1, &config);
        let target = screen_target(&rung).unwrap();
        assert_eq!(target.framing, Framing::OneTrack(1));
        assert_eq!(target.codec, SlateCodec::Hevc);
        assert_eq!(target.fourcc, *b"hvc1");
        assert_eq!((target.shape.width, target.shape.height), (2560, 1440));

        let mut ten_bit = config.clone();
        ten_bit[17] = 0xF8 | 2; // bitDepthLumaMinus8
        assert_eq!(
            screen_target(&one_track_header(0x96, b"hvc1", 1, &ten_bit)),
            None
        );
        assert_eq!(
            screen_target(&one_track_header(0x96, b"hvc1", 1, &[1])),
            None
        );
    }

    #[test]
    fn screen_frames_are_framed_like_the_track_they_replace() {
        assert_eq!(
            video_tag(Framing::Legacy, FOURCC_AVC1, true, &[9, 9]),
            vec![0x17, 1, 0, 0, 0, 9, 9]
        );
        assert_eq!(
            video_tag(Framing::Legacy, FOURCC_AVC1, false, &[9])[0],
            0x27
        );
        let rung = video_tag(Framing::OneTrack(3), FOURCC_AVC1, true, &[9, 9]);
        assert_eq!(rung[..7], [0x96, 0x01, b'a', b'v', b'c', b'1', 3]);
        // A vertical destination fed track 3 sees the plain legacy frame.
        let flat = select_video_bytes(&rung, VideoEgress::Track(3)).unwrap();
        assert_eq!(&flat[..], &[0x17, 1, 0, 0, 0, 9, 9]);
        let enhanced = video_tag(Framing::Enhanced, FOURCC_AVC1, false, &[9]);
        assert_eq!(enhanced[..5], [0xA1, b'a', b'v', b'c', b'1']);
        let hevc = video_tag(Framing::OneTrack(1), FOURCC_HVC1, true, &[9]);
        assert_eq!(hevc[..10], [0x96, 0x01, b'h', b'v', b'c', b'1', 1, 0, 0, 0]);
        // An HEVC keyframe is one the controller recognises as a keyframe.
        assert!(crate::h264::classify_video_tag(&hevc).is_idr);
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
            tokio::spawn(async move { cache.get(&settings, shape, SlateCodec::H264).await })
        };
        tokio::task::yield_now().await;
        first.abort();
        let later = cache.get(&settings, shape, SlateCodec::H264);
        let later = tokio::time::timeout(Duration::from_secs(10), later);
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
