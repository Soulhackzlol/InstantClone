//! The crash-protection reconnect screen.
//!
//! Draws the streamer's chosen theme and encodes it as an H.264 or HEVC
//! loop, built once per stream shape and codec and replayed with fresh
//! timestamps for as long as the hold lasts. A loop is one or more 2 s
//! keyframe intervals, each a keyframe plus P frames, so viewers who join
//! mid-hold get a picture within 2 s. Flat themes loop in one interval and
//! encode losslessly; soft ones (gradients, glows) loop over several and
//! encode within a small tolerance. The intervals of a loop are encoded
//! side by side, one thread each, as each starts from its own keyframe.

mod bitstream;
mod cabac;
mod cavlc_tables;
mod encoder;
mod fidelity;
mod font;
mod hevc;
mod hevc_residual;
mod mp4;
mod pixel_font;
mod png;
mod raster;
mod residual;
mod soft_themes;
mod themes;

pub use encoder::StreamShape;
pub use raster::Rgb;

use crate::crash_protection::CrashProtection;
use raster::Canvas;
use std::fmt;
use themes::ScreenStyle;

/// Every keyframe interval of a loop is this long.
pub const KEYFRAME_INTERVAL_MS: usize = 2000;
const MIN_SIDE: usize = 16;
const MAX_SIDE: usize = 8192;
const MAX_FPS: u32 = 240;

/// The codec a loop is encoded in: the one of the track it stands in for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlateCodec {
    H264,
    Hevc,
}

impl SlateCodec {
    pub fn label(self) -> &'static str {
        match self {
            SlateCodec::H264 => "H.264",
            SlateCodec::Hevc => "HEVC",
        }
    }

    /// The square the encoder codes in, which frames are padded to.
    fn block_size(self) -> usize {
        match self {
            SlateCodec::H264 => 16,
            SlateCodec::Hevc => hevc::CTB,
        }
    }
}

/// One encoded loop, ready to be wrapped in RTMP video tags. It needs no
/// sequence header of its own: every keyframe carries the loop's
/// parameter sets in-band, under ids the stream it interrupts doesn't use.
pub struct SlateLoop {
    /// The keyframe intervals, in play order. All are the same length.
    pub intervals: Vec<Interval>,
    pub shape: StreamShape,
}

/// One keyframe interval of a loop, as length-prefixed samples (or, once
/// framed for a destination, as its tags).
pub struct Interval {
    /// The interval's first picture (H.264: SPS, PPS, IDR; HEVC: VPS, SPS,
    /// PPS, IDR), twice. Replays alternate them: back-to-back H.264 IDRs
    /// must not share an idr_pic_id, so those two differ; HEVC IDRs carry
    /// no id, so those two are the same.
    pub keyframes: [Vec<u8>; 2],
    /// The rest of the interval: P pictures.
    pub deltas: Vec<Vec<u8>>,
}

/// The encoder only handles 4:2:0 frames with even sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedShape(pub StreamShape);

impl fmt::Display for UnsupportedShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shape = self.0;
        write!(
            f,
            "can't build a reconnect screen at {}x{} {} fps (sides must be even, \
             {MIN_SIDE} to {MAX_SIDE} px, and 1 to {MAX_FPS} fps)",
            shape.width, shape.height, shape.fps
        )
    }
}

impl std::error::Error for UnsupportedShape {}

/// The screen at each of `phases` (moments of the loop, 0..1), stacked
/// top to bottom in one PNG. Drawn by the same code as the stream, so the
/// dashboard preview is exactly what viewers get; one image for the whole
/// animation keeps the dashboard to a single request per change.
pub fn preview_png(
    settings: &CrashProtection,
    width: usize,
    height: usize,
    phases: &[f32],
) -> Vec<u8> {
    let style = screen_style(settings);
    let frames = phases
        .iter()
        .map(|phase| {
            let mut canvas = Canvas::new(width, height, style.background);
            themes::draw(&mut canvas, &style, *phase);
            canvas
        })
        .collect();
    Canvas::stacked(frames)
        .unwrap_or_else(|| Canvas::new(width, height, style.background))
        .to_png()
}

/// Frame rate of the dashboard's video preview of a soft screen: the
/// stream's own, so it moves exactly as viewers see it.
const PREVIEW_FPS: u32 = 30;

/// The screen as a looping H.264 video, for the dashboard's preview of
/// soft themes: smooth motion a strip of stills can't show, from the
/// encoder the stream uses, so the preview is what viewers get. `None`
/// for a size the encoder can't take.
pub fn preview_mp4(settings: &CrashProtection, width: usize, height: usize) -> Option<Vec<u8>> {
    let shape = StreamShape {
        width,
        height,
        fps: PREVIEW_FPS,
    };
    let slate = build_loop(settings, shape, SlateCodec::H264).ok()?;
    let sets = encoder::parameter_sets(shape);
    Some(mp4::h264_loop(&slate, &sets, PREVIEW_FPS))
}

fn screen_style(settings: &CrashProtection) -> ScreenStyle<'_> {
    ScreenStyle {
        theme: settings.theme,
        accent: settings.accent,
        background: settings.resolved_background(),
        headline: &settings.headline,
        subline: &settings.subline,
    }
}

/// Render and encode the screen described by `settings` at `shape`.
pub fn build_loop(
    settings: &CrashProtection,
    shape: StreamShape,
    codec: SlateCodec,
) -> Result<SlateLoop, UnsupportedShape> {
    let side_ok = |side: usize| (MIN_SIDE..=MAX_SIDE).contains(&side) && side.is_multiple_of(2);
    if !side_ok(shape.width) || !side_ok(shape.height) || !(1..=MAX_FPS).contains(&shape.fps) {
        return Err(UnsupportedShape(shape));
    }
    let style = screen_style(settings);
    let frames = frames_per_interval(shape.fps);
    let count = settings.theme.loop_intervals();
    let plan = |index: usize| IntervalPlan {
        first_frame: index * frames,
        frames,
        loop_frames: count * frames,
        tolerance: settings.theme.tolerance(),
    };
    let intervals = std::thread::scope(|scope| {
        let style = &style;
        let workers: Vec<_> = (0..count)
            .map(|index| scope.spawn(move || encode_interval(style, shape, codec, plan(index))))
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("encoding a screen interval panicked"))
            .collect()
    });
    Ok(SlateLoop { intervals, shape })
}

/// Which frames of the loop one interval holds, and how closely to code
/// them.
#[derive(Debug, Clone, Copy)]
struct IntervalPlan {
    first_frame: usize,
    frames: usize,
    loop_frames: usize,
    tolerance: u8,
}

/// Render and encode one keyframe interval. Each picture is coded against
/// what a decoder shows for the one before, so with a tolerance the small
/// differences never pile up.
fn encode_interval(
    style: &ScreenStyle,
    shape: StreamShape,
    codec: SlateCodec,
    plan: IntervalPlan,
) -> Interval {
    let render = |index: usize| {
        let mut canvas = Canvas::new(shape.width, shape.height, style.background);
        let phase = (plan.first_frame + index) as f32 / plan.loop_frames as f32;
        themes::draw(&mut canvas, style, phase);
        canvas
    };
    let mut source_canvas = render(0);
    let mut source = source_canvas.to_yuv420_padded(codec.block_size());
    let (keyframes, mut shown) = match codec {
        SlateCodec::H264 => {
            let sets = encoder::parameter_sets(shape);
            let first = encoder::encode_idr_within(&source, 0, plan.tolerance);
            let second = encoder::encode_idr_within(&source, 1, plan.tolerance);
            let sample = |idr: &[u8]| bitstream::length_prefixed(&[&sets.sps, &sets.pps, idr]);
            ([sample(&first.nal), sample(&second.nal)], first.shown)
        }
        SlateCodec::Hevc => {
            let [vps, sps, pps] = hevc::parameter_sets(shape);
            let idr = hevc::encode_idr_within(&source, plan.tolerance);
            let keyframe = bitstream::length_prefixed(&[&vps, &sps, &pps, &idr.nal]);
            ([keyframe.clone(), keyframe], idr.shown)
        }
    };
    let mut deltas = Vec::with_capacity(plan.frames - 1);
    for index in 1..plan.frames {
        let canvas = render(index);
        // Only the animated rows change between frames.
        let frame = canvas.to_yuv420_after(&source_canvas, &source);
        let frame_num = index as u32;
        let coded = match codec {
            SlateCodec::H264 => encoder::encode_p_within(&frame, &shown, frame_num, plan.tolerance),
            SlateCodec::Hevc => hevc::encode_p_within(&frame, &shown, frame_num, plan.tolerance),
        };
        deltas.push(bitstream::length_prefixed(&[&coded.nal]));
        shown = coded.shown;
        (source_canvas, source) = (canvas, frame);
    }
    Interval { keyframes, deltas }
}

/// A legacy FLV H.264 sequence-header tag describing a `width` x `height`
/// stream, for tests that need video the reconnect screen can cover.
#[cfg(test)]
pub fn test_sequence_header(width: usize, height: usize) -> Vec<u8> {
    let sets = encoder::parameter_sets(StreamShape {
        width,
        height,
        fps: 30,
    });
    let (sps, pps) = (&sets.sps, &sets.pps);
    // AVCDecoderConfigurationRecord: version 1, profile/compat/level from
    // the SPS, 4-byte NAL lengths, one SPS, one PPS.
    let mut tag = vec![0x17, 0, 0, 0, 0, 1, sps[1], sps[2], sps[3], 0xFF, 0xE1];
    tag.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    tag.extend_from_slice(sps);
    tag.push(1);
    tag.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    tag.extend_from_slice(pps);
    tag
}

/// An HEVCDecoderConfigurationRecord (hvcC) for a `width` x `height`
/// 8-bit 4:2:0 stream with 4-byte NAL lengths, for tests that need HEVC
/// video the reconnect screen can cover.
#[cfg(test)]
pub fn test_hevc_config(width: usize, height: usize) -> Vec<u8> {
    let sets = hevc::parameter_sets(StreamShape {
        width,
        height,
        fps: 30,
    });
    // Version 1, Main profile, Main + Main 10 compatible, progressive and
    // frame-only, level 4.
    let mut record = vec![1, 0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 120];
    // No segmentation or parallelism, 4:2:0, 8-bit luma and chroma, no
    // frame rate, one temporal layer, 4-byte NAL lengths, three arrays.
    record.extend_from_slice(&[0xF0, 0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0, 0, 0x0F, 3]);
    for nal in &sets {
        // array_completeness and the NAL type, then one NAL unit.
        record.push(0x80 | (nal[0] >> 1));
        record.extend_from_slice(&1u16.to_be_bytes());
        record.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        record.extend_from_slice(nal);
    }
    record
}

/// Tests run ffmpeg one at a time: several at once load the machine
/// enough to fail the wall-clock simulations (`controller::user_sim`)
/// running beside them. Hold the guard for the whole ffmpeg run.
#[cfg(test)]
fn ffmpeg_turn() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A test that panicked holding the turn leaves nothing to protect.
    TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether ffmpeg runs here, for the tests that decode with it. Missing
/// on a dev machine skips them with a note; missing on CI (`CI` set)
/// fails, so a runner without ffmpeg can't turn them into silent passes.
#[cfg(test)]
fn ffmpeg_is_available(check: &str) -> bool {
    let found = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok();
    if !found {
        // The Linux CI job installs ffmpeg, so there a missing one is a
        // broken job, not a machine without it; the Windows job skips.
        assert!(
            !(cfg!(target_os = "linux") && std::env::var_os("CI").is_some()),
            "ffmpeg is required on Linux CI for {check}"
        );
        log_line!("NOTE: ffmpeg not found, so {check} did not run");
    }
    found
}

fn frames_per_interval(fps: u32) -> usize {
    (fps as usize * KEYFRAME_INTERVAL_MS / 1000).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crash_protection::SlateTheme;
    use std::process::Command;

    const CODECS: [SlateCodec; 2] = [SlateCodec::H264, SlateCodec::Hevc];

    fn settings(theme: SlateTheme) -> CrashProtection {
        CrashProtection {
            theme,
            ..CrashProtection::default()
        }
    }

    /// A flat theme loops in one 2 s keyframe interval, a soft one in four;
    /// every interval is 2 s of frames, and the two H.264 keyframes of each
    /// differ (their idr_pic_id).
    #[test]
    fn loops_are_whole_keyframe_intervals() {
        let shape = StreamShape {
            width: 320,
            height: 180,
            fps: 30,
        };
        for (theme, intervals) in [(SlateTheme::Arcade, 1), (SlateTheme::Aurora, 4)] {
            for codec in CODECS {
                let slate = build_loop(&settings(theme), shape, codec).unwrap();
                assert_eq!(slate.intervals.len(), intervals, "{theme:?} {codec:?}");
                for interval in &slate.intervals {
                    assert_eq!(1 + interval.deltas.len(), 60, "{theme:?} {codec:?}");
                    if codec == SlateCodec::H264 {
                        assert_ne!(interval.keyframes[0], interval.keyframes[1]);
                    }
                }
            }
        }
    }

    #[test]
    fn keyframes_carry_their_own_parameter_sets_in_band() {
        let shape = StreamShape {
            width: 320,
            height: 180,
            fps: 10,
        };
        for (codec, keyframe_types) in [
            // SPS, PPS, IDR.
            (SlateCodec::H264, vec![7, 8, 5]),
            // VPS, SPS, PPS, IDR_N_LP.
            (SlateCodec::Hevc, vec![32, 33, 34, 20]),
        ] {
            let slate = build_loop(&settings(SlateTheme::Tide), shape, codec).unwrap();
            let types = |sample: &[u8]| {
                nal_units(sample)
                    .iter()
                    .map(|nal| nal_type(codec, nal))
                    .collect::<Vec<_>>()
            };
            for keyframe in slate.intervals.iter().flat_map(|i| &i.keyframes) {
                assert_eq!(types(keyframe), keyframe_types, "{codec:?} keyframe");
            }
            for delta in slate.intervals.iter().flat_map(|i| &i.deltas) {
                // A non-IDR slice in H.264, TRAIL_R in HEVC: both type 1.
                assert_eq!(
                    types(delta),
                    vec![1],
                    "{codec:?} P frames are just the slice"
                );
            }
        }
    }

    /// The SPS must describe the visible size, cropping included, or the
    /// screen comes out padded or rescaled against the stream it covers.
    /// Read back with the parser that sizes real streams.
    #[test]
    fn sequence_header_describes_the_visible_size() {
        for (width, height) in [
            (1920, 1080),
            (1080, 1920),
            (1280, 720),
            (854, 480),
            (2560, 1440),
            (200, 360),
            (16, 16),
        ] {
            let header = test_sequence_header(width, height);
            assert_eq!(
                crate::h264::sps_dimensions(&header),
                Some((width as u32, height as u32)),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn odd_or_absurd_shapes_are_rejected() {
        let base = settings(SlateTheme::Whisper);
        for (width, height, fps) in [(321, 180, 30), (320, 8, 30), (320, 180, 0), (9000, 180, 30)] {
            let shape = StreamShape { width, height, fps };
            assert_eq!(
                build_loop(&base, shape, SlateCodec::H264).err(),
                Some(UnsupportedShape(shape))
            );
        }
    }

    /// Every theme's loop at `width` x `height`, in both codecs, built at
    /// the rate a hold really uses, stays under `max_kbps`, and a soft
    /// theme under twice that: light that moves everywhere costs more than
    /// a few moving shapes, and the screen replaces a stream that carries
    /// several times as much.
    fn assert_every_theme_fits(width: usize, height: usize, max_kbps: usize) {
        let shape = StreamShape {
            width,
            height,
            fps: crate::crash_hold::SLATE_FPS,
        };
        let every_pair = SlateTheme::ALL
            .into_iter()
            .flat_map(|theme| CODECS.map(|codec| (theme, codec)));
        for (theme, codec) in every_pair {
            let slate = build_loop(&settings(theme), shape, codec).unwrap();
            let loop_bytes: usize = slate
                .intervals
                .iter()
                .map(|i| i.keyframes[0].len() + i.deltas.iter().map(Vec::len).sum::<usize>())
                .sum();
            let loop_ms = slate.intervals.len() * KEYFRAME_INTERVAL_MS;
            // Bits per millisecond are kbps.
            let kbps = loop_bytes * 8 / loop_ms;
            let budget = if theme.is_soft() {
                2 * max_kbps
            } else {
                max_kbps
            };
            assert!(
                kbps < budget,
                "{theme:?} {codec:?} loop at {width}x{height} runs at {kbps} kbps"
            );
        }
    }

    /// The screen goes to every destination for as long as a hold lasts,
    /// on the streamer's upload: under 1 Mbps at 1080p.
    #[test]
    fn loop_stays_small_at_1080p() {
        assert_every_theme_fits(1920, 1080, 1_000);
    }

    /// Destinations set to Vertical get the screen at 1080x1920: the same
    /// pixel count and, as themes scale by the short side, the same layout
    /// scale as 1080p, so the same 1 Mbps.
    #[test]
    fn loop_stays_small_on_a_vertical_canvas() {
        assert_every_theme_fits(1080, 1920, 1_000);
    }

    /// 2K channels stream 1440p. Themes scale their layout by the short
    /// side, 1440 / 1080, and the costly blocks are the edges of what they
    /// draw, so the budget scales the same way: 1333 kbps.
    #[test]
    fn loop_stays_small_at_1440p() {
        assert_every_theme_fits(2560, 1440, 1_333);
    }

    /// Decode two replays of the loop with ffmpeg and require every pixel
    /// to match what we rendered (soft themes: within their tolerance), in
    /// both codecs. Every theme at 320x180
    /// (180 isn't whole macroblocks or CTBs), then one portrait 200x360 so
    /// a real decoder checks the cropping of the width too, then one
    /// 640x360 so larger text meets every block coding. Sizes stay small:
    /// at 720p the raw video written to the temp dir slows the wall-clock
    /// simulations (`controller::user_sim`) running beside this test
    /// enough to fail them. Each ffmpeg run costs about half a second.
    #[test]
    fn ffmpeg_decodes_the_loop_as_rendered() {
        if !ffmpeg_is_available("the reconnect screen decode round trip") {
            return;
        }
        let landscape = SlateTheme::ALL.map(|theme| (theme, 320, 180));
        let others = [
            (SlateTheme::Studio, 200, 360),
            (SlateTheme::Arcade, 640, 360),
        ];
        for codec in CODECS {
            for (theme, width, height) in landscape.into_iter().chain(others) {
                let shape = StreamShape {
                    width,
                    height,
                    fps: 10,
                };
                let slate = build_loop(&settings(theme), shape, codec).unwrap();
                let decoded = decode_with_ffmpeg(&annex_b(&slate), shape.fps, codec, theme);
                let expected = expected_frames(&settings(theme), shape);
                let what = format!("{theme:?} {codec:?} at {width}x{height}");
                assert_eq!(decoded.len(), expected.len(), "{what}: frame count");
                let worst = decoded
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap_or(0);
                assert!(
                    worst <= theme.tolerance(),
                    "{what}: decoded pixels are up to {worst} off the render"
                );
            }
        }
    }

    fn nal_type(codec: SlateCodec, nal: &[u8]) -> u8 {
        match codec {
            SlateCodec::H264 => nal[0] & 0x1F,
            SlateCodec::Hevc => (nal[0] >> 1) & 0x3F,
        }
    }

    /// The NAL units of one AVCC sample (4-byte big-endian lengths). Fails
    /// unless the lengths tile the sample exactly and every unit is well
    /// formed: a stray byte, or a start code inside a unit, breaks every
    /// decoder downstream.
    fn nal_units(sample: &[u8]) -> Vec<&[u8]> {
        let mut units = Vec::new();
        let mut rest = sample;
        while !rest.is_empty() {
            assert!(rest.len() >= 4, "truncated length prefix");
            let (prefix, tail) = rest.split_at(4);
            let len = u32::from_be_bytes(prefix.try_into().unwrap()) as usize;
            assert!(
                (1..=tail.len()).contains(&len),
                "NAL length {len} doesn't fit the sample"
            );
            let (unit, tail) = tail.split_at(len);
            assert_eq!(unit[0] & 0x80, 0, "forbidden_zero_bit is set");
            assert!(
                !unit.windows(3).any(|w| w[0] == 0 && w[1] == 0 && w[2] <= 2),
                "start code pattern inside a NAL unit"
            );
            units.push(unit);
            rest = tail;
        }
        units
    }

    /// What a destination's decoder sees through a hold: the stream, whose
    /// parameter sets (id 0) came once at its start; one loop of the screen
    /// with its own in-band; then the stream again from a keyframe that
    /// carries none. Every frame must decode without an error, the screen
    /// exactly as rendered, and the stream after it exactly as it decodes
    /// alone: the screen must not disturb the stream's parameter sets.
    /// x264 and x265 stand in for OBS's encoder.
    #[test]
    fn a_stream_cut_to_the_screen_and_back_decodes_cleanly() {
        if !ffmpeg_is_available("the cut to the reconnect screen and back") {
            return;
        }
        let shape = StreamShape {
            width: 320,
            height: 180,
            fps: 10,
        };
        let theme = SlateTheme::Beacon;
        let frame_bytes = shape.width * shape.height * 3 / 2;
        for codec in CODECS {
            let Some(stream) = encode_stand_in_stream(codec, shape) else {
                assert!(
                    !(cfg!(target_os = "linux") && std::env::var_os("CI").is_some()),
                    "the {codec:?} encoder is required on Linux CI"
                );
                continue;
            };
            let cut = second_keyframe_offset(&stream, codec);
            let before = &stream[..cut];
            // OBS's keyframes reach a destination without parameter sets:
            // drop any the encoder repeated.
            let after = without_parameter_sets(&stream[cut..], codec);
            let slate = build_loop(&settings(theme), shape, codec).unwrap();
            let interval = &slate.intervals[0];
            let screen =
                annex_b_samples(std::iter::once(&interval.keyframes[0]).chain(&interval.deltas));
            let spliced = [before, &screen[..], &after[..]].concat();

            let continuous = [before, &after[..]].concat();
            let alone = decode_with_ffmpeg(&continuous, shape.fps, codec, theme);
            let decoded = decode_with_ffmpeg(&spliced, shape.fps, codec, theme);
            let loop_bytes = frames_per_interval(shape.fps) * frame_bytes;
            let before_bytes = STAND_IN_GOP * frame_bytes;
            assert_eq!(
                decoded.len(),
                alone.len() + loop_bytes,
                "{codec:?}: frame count"
            );
            let (stream_before, rest) = decoded.split_at(before_bytes);
            let (screen_frames, stream_after) = rest.split_at(loop_bytes);
            assert!(
                stream_before == &alone[..before_bytes],
                "{codec:?}: stream before the cut"
            );
            let expected_screen = expected_frames(&settings(theme), shape);
            assert!(
                screen_frames == &expected_screen[..loop_bytes],
                "{codec:?}: the screen"
            );
            assert!(
                stream_after == &alone[before_bytes..],
                "{codec:?}: stream after the screen"
            );
        }
    }

    /// Keyframe interval of the stand-in stream, in frames.
    const STAND_IN_GOP: usize = 10;

    /// Two keyframe intervals (IDR, no B-frames) of a test pattern from
    /// x264 or x265, tagged BT.709 limited range as OBS tags its streams by
    /// default and the screen tags itself. Matching tags matter to the
    /// check, not to viewers: when they differ, ffmpeg converts frames
    /// after the screen and they stop matching the stream decoded alone.
    /// `None`, with a note, when this ffmpeg can't run the encoder.
    fn encode_stand_in_stream(codec: SlateCodec, shape: StreamShape) -> Option<Vec<u8>> {
        let gop = STAND_IN_GOP.to_string();
        let (encoder, format, options): (&str, &str, Vec<String>) = match codec {
            SlateCodec::H264 => (
                "libx264",
                "h264",
                [
                    "-g",
                    &gop,
                    "-keyint_min",
                    &gop,
                    "-sc_threshold",
                    "0",
                    "-bf",
                    "0",
                ]
                .map(String::from)
                .to_vec(),
            ),
            SlateCodec::Hevc => (
                "libx265",
                "hevc",
                vec![
                    "-x265-params".into(),
                    format!(
                        "keyint={gop}:min-keyint={gop}:scenecut=0:bframes=0:\
                         open-gop=0:pools=1:frame-threads=1:log-level=error"
                    ),
                ],
            ),
        };
        let source = format!(
            "testsrc2=size={}x{}:rate={}",
            shape.width, shape.height, shape.fps
        );
        let _turn = ffmpeg_turn();
        let output = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", &source, "-frames:v"])
            .arg((2 * STAND_IN_GOP).to_string())
            .args(["-pix_fmt", "yuv420p", "-c:v", encoder])
            .args(&options)
            .args(["-color_primaries", "bt709", "-color_trc", "bt709"])
            .args(["-colorspace", "bt709", "-color_range", "tv"])
            .args(["-f", format, "-"])
            .output()
            .unwrap();
        if !output.status.success() || output.stdout.is_empty() {
            let why = String::from_utf8_lossy(&output.stderr);
            log_line!(
                "NOTE: ffmpeg couldn't run {encoder} ({}), so the {codec:?} cut check didn't run",
                why.trim()
            );
            return None;
        }
        Some(output.stdout)
    }

    /// Byte offset of each Annex B NAL unit's start code, with its type.
    fn nal_starts(stream: &[u8], codec: SlateCodec) -> Vec<(usize, u8)> {
        let mut starts = Vec::new();
        for at in 0..stream.len().saturating_sub(3) {
            if stream[at..at + 3] != [0, 0, 1] {
                continue;
            }
            let start = if at > 0 && stream[at - 1] == 0 {
                at - 1
            } else {
                at
            };
            starts.push((start, nal_type(codec, &stream[at + 3..])));
        }
        starts
    }

    /// `stream` with its VPS, SPS and PPS NAL units left out.
    fn without_parameter_sets(stream: &[u8], codec: SlateCodec) -> Vec<u8> {
        let starts = nal_starts(stream, codec);
        let ends = starts
            .iter()
            .skip(1)
            .map(|(start, _)| *start)
            .chain([stream.len()]);
        let is_parameter_set = |nal_type: u8| match codec {
            SlateCodec::H264 => matches!(nal_type, 7 | 8),
            SlateCodec::Hevc => matches!(nal_type, 32..=34),
        };
        starts
            .iter()
            .zip(ends)
            .filter(|((_, nal_type), _)| !is_parameter_set(*nal_type))
            .flat_map(|((start, _), end)| &stream[*start..end])
            .copied()
            .collect()
    }

    /// Where the access unit of the stream's second keyframe begins: its
    /// first slice, or the SEI or delimiter NAL units just before it.
    fn second_keyframe_offset(stream: &[u8], codec: SlateCodec) -> usize {
        type NalTest = fn(u8) -> bool;
        let (is_keyframe, is_prefix): (NalTest, NalTest) = match codec {
            SlateCodec::H264 => (|t| t == 5, |t| matches!(t, 6 | 9)),
            SlateCodec::Hevc => (|t| (16..=21).contains(&t), |t| matches!(t, 35 | 39)),
        };
        let nals = nal_starts(stream, codec);
        let (mut first, _) = nals
            .iter()
            .enumerate()
            .filter(|(_, (_, nal_type))| is_keyframe(*nal_type))
            .nth(1)
            .expect("the stand-in stream has a second keyframe");
        while first > 0 && is_prefix(nals[first - 1].1) {
            first -= 1;
        }
        nals[first].0
    }

    /// Samples as an Annex B elementary stream.
    fn annex_b_samples<'a>(samples: impl Iterator<Item = &'a Vec<u8>>) -> Vec<u8> {
        let mut out = Vec::new();
        for sample in samples {
            for nal in nal_units(sample) {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(nal);
            }
        }
        out
    }

    /// Two loop replays as an Annex B elementary stream, keyframes
    /// alternating as a hold sends them.
    fn annex_b(slate: &SlateLoop) -> Vec<u8> {
        let count = slate.intervals.len();
        let samples = (0..2 * count).flat_map(|sent| {
            let interval = &slate.intervals[sent % count];
            std::iter::once(&interval.keyframes[sent % 2]).chain(&interval.deltas)
        });
        annex_b_samples(samples)
    }

    /// Raw H.264 and HEVC carry no timestamps, so the input rate is given
    /// explicitly, and `-fps_mode passthrough` stops ffmpeg dropping
    /// identical frames. One decoder thread keeps the round trip light on
    /// a machine that runs the whole suite in parallel.
    fn decode_with_ffmpeg(
        stream: &[u8],
        fps: u32,
        codec: SlateCodec,
        theme: SlateTheme,
    ) -> Vec<u8> {
        let format = match codec {
            SlateCodec::H264 => "h264",
            SlateCodec::Hevc => "hevc",
        };
        // Tests decode in parallel, sometimes the same theme: number each
        // run so no two share a temp file.
        static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir();
        let tag = format!(
            "instantclone-slate-{}-{run}-{}",
            std::process::id(),
            theme.id()
        );
        let input = dir.join(format!("{tag}.{format}"));
        let output = dir.join(format!("{tag}.yuv"));
        std::fs::write(&input, stream).unwrap();
        let _turn = ffmpeg_turn();
        let result = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-threads",
                "1",
                "-r",
                &fps.to_string(),
                "-f",
                format,
                "-i",
            ])
            .arg(&input)
            .args([
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&output)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr).to_string();
        let decoded = std::fs::read(&output).unwrap_or_default();
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
        assert!(
            result.status.success() && stderr.trim().is_empty(),
            "ffmpeg ({}): {stderr}",
            result.status
        );
        decoded
    }

    /// A moving test picture with everything a soft screen has: a drifting
    /// two-way gradient, a soft blob, a hard-edged bar, and a patch of
    /// noise no prediction or transform can afford (it must go out raw).
    fn soft_test_frame(width: usize, height: usize, time: usize) -> raster::YuvFrame {
        let mut frame = Canvas::new(width, height, Rgb::new(0, 0, 0)).to_yuv420_padded(32);
        let (width, height) = (frame.width, frame.height);
        let t = time as f32;
        let blob = |x: f32, y: f32| {
            let (dx, dy) = (x - width as f32 * 0.6 - 3.0 * t, y - height as f32 * 0.4);
            90.0 * (-(dx * dx + dy * dy) / 900.0).exp()
        };
        let mut noise = 0x2545_F491u32;
        for y in 0..height {
            for x in 0..width {
                let (fx, fy) = (x as f32, y as f32);
                let mut value = 40.0 + 0.35 * fx + 0.2 * fy + 1.5 * t + blob(fx, fy);
                if (20..28).contains(&y) && x > 40 {
                    value = 235.0;
                }
                if x < 24 && y >= height - 24 {
                    noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    value = (noise >> 24) as f32;
                }
                frame.y[y * width + x] = value.clamp(16.0, 235.0) as u8;
            }
        }
        for y in 0..height / 2 {
            for x in 0..width / 2 {
                let at = y * (width / 2) + x;
                frame.u[at] = (128.0 + 0.25 * x as f32 - 0.5 * t).clamp(16.0, 240.0) as u8;
                frame.v[at] = (110.0 + 0.3 * y as f32 + blob(2.0 * x as f32, 2.0 * y as f32) / 3.0)
                    .clamp(16.0, 240.0) as u8;
            }
        }
        frame
    }

    fn frame_bytes(frame: &raster::YuvFrame) -> Vec<u8> {
        [&frame.y[..], &frame.u[..], &frame.v[..]].concat()
    }

    fn worst_difference(a: &raster::YuvFrame, b: &raster::YuvFrame) -> u8 {
        frame_bytes(a)
            .iter()
            .zip(frame_bytes(b))
            .map(|(a, b)| a.abs_diff(b))
            .max()
            .unwrap_or(0)
    }

    /// Soft pictures are coded with residual, so the encoder has to rebuild
    /// each block exactly as a decoder will: every later prediction and
    /// skip leans on it. Decode with ffmpeg and require its pixels to be
    /// the encoder's own, and those to stay within the tolerance, in both
    /// codecs.
    #[test]
    fn ffmpeg_shows_exactly_what_the_encoder_thinks_it_shows() {
        if !ffmpeg_is_available("the soft screen decode round trip") {
            return;
        }
        let sizes = [(64, 64, 2), (320, 192, 1), (320, 192, 3)];
        for (codec, (width, height, tolerance)) in CODECS
            .into_iter()
            .flat_map(|codec| sizes.map(|size| (codec, size)))
        {
            let shape = StreamShape {
                width,
                height,
                fps: 10,
            };
            let what = format!("{codec:?} {width}x{height} within {tolerance}");
            let first = soft_test_frame(width, height, 0);
            let (keyframe, mut shown) = match codec {
                SlateCodec::H264 => {
                    let sets = encoder::parameter_sets(shape);
                    let idr = encoder::encode_idr_within(&first, 0, tolerance);
                    let sample = bitstream::length_prefixed(&[&sets.sps, &sets.pps, &idr.nal]);
                    (sample, idr.shown)
                }
                SlateCodec::Hevc => {
                    let [vps, sps, pps] = hevc::parameter_sets(shape);
                    let idr = hevc::encode_idr_within(&first, tolerance);
                    let sample = bitstream::length_prefixed(&[&vps, &sps, &pps, &idr.nal]);
                    (sample, idr.shown)
                }
            };
            assert!(
                worst_difference(&first, &shown) <= tolerance,
                "{what}: keyframe"
            );
            let mut samples = vec![keyframe];
            let mut expected = frame_bytes(&shown);
            for time in 1..8 {
                let frame = soft_test_frame(width, height, time);
                let coded = match codec {
                    SlateCodec::H264 => {
                        encoder::encode_p_within(&frame, &shown, time as u32, tolerance)
                    }
                    SlateCodec::Hevc => {
                        hevc::encode_p_within(&frame, &shown, time as u32, tolerance)
                    }
                };
                assert!(
                    worst_difference(&frame, &coded.shown) <= tolerance,
                    "{what}: picture {time}"
                );
                samples.push(bitstream::length_prefixed(&[&coded.nal]));
                expected.extend(frame_bytes(&coded.shown));
                shown = coded.shown;
            }
            let stream = annex_b_samples(samples.iter());
            let decoded = decode_with_ffmpeg(&stream, shape.fps, codec, SlateTheme::Whisper);
            assert_eq!(decoded.len(), expected.len(), "{what}: frame count");
            let first_wrong = decoded.iter().zip(&expected).position(|(a, b)| a != b);
            assert_eq!(first_wrong, None, "{what}: first differing sample");
        }
    }

    /// Not a check: prints what a drifting aurora costs at 1080p, by
    /// tolerance and by how fast it moves. Run with
    /// `cargo test --release measure_soft_loop -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn measure_soft_loop() {
        let (width, height, fps) = (1920, 1080, 30);
        let seconds = 4;
        let aurora = |canvas: &mut Canvas, phase: f32, travel: f32| {
            let blobs: [([f32; 3], f32, f32); 3] = [
                ([90.0, 200.0, 250.0], 0.0, 0.25),
                ([150.0, 90.0, 240.0], 0.33, 0.6),
                ([40.0, 220.0, 170.0], 0.66, 0.9),
            ];
            canvas.paint_smooth(8, |x, y| {
                let mut colour = [11.0, 13.0, 18.0];
                for (tint, offset, across) in blobs {
                    let angle = std::f32::consts::TAU * (phase + offset);
                    let cx = across + travel * angle.cos();
                    let cy = 0.5 + travel * angle.sin();
                    let (dx, dy) = ((x - cx) * 16.0 / 9.0, y - cy);
                    let glow = 0.42 * (-(dx * dx + dy * dy) / 0.12).exp();
                    for channel in 0..3 {
                        colour[channel] += tint[channel] * glow;
                    }
                }
                colour
            });
        };
        let runs = CODECS
            .into_iter()
            .flat_map(|codec| [0.05, 0.15].map(|travel| (codec, travel)))
            .flat_map(|(codec, travel)| [1, 2, 3].map(|tolerance| (codec, travel, tolerance)));
        for (codec, travel, tolerance) in runs {
            let started = std::time::Instant::now();
            let frame_count = fps * seconds;
            let mut bytes = 0;
            let mut keyframe = 0;
            let mut shown: Option<raster::YuvFrame> = None;
            for index in 0..frame_count {
                let mut canvas = Canvas::new(width, height, Rgb::new(11, 13, 18));
                // One trip round in 16 s.
                aurora(&mut canvas, index as f32 / (16 * fps) as f32, travel);
                let frame = canvas.to_yuv420_padded(codec.block_size());
                let at = index % (2 * fps);
                let coded = match (&shown, codec) {
                    (Some(before), SlateCodec::H264) if at != 0 => {
                        encoder::encode_p_within(&frame, before, at as u32, tolerance)
                    }
                    (Some(before), SlateCodec::Hevc) if at != 0 => {
                        hevc::encode_p_within(&frame, before, at as u32, tolerance)
                    }
                    (_, SlateCodec::H264) => encoder::encode_idr_within(&frame, 0, tolerance),
                    (_, SlateCodec::Hevc) => hevc::encode_idr_within(&frame, tolerance),
                };
                if at == 0 {
                    keyframe = coded.nal.len();
                }
                bytes += coded.nal.len();
                shown = Some(coded.shown);
            }
            log_line!(
                "{codec:?} travel {travel} tolerance {tolerance}: {} kbps, keyframe {} KB, built in {:?}",
                bytes * 8 / (seconds * 1000),
                keyframe / 1000,
                started.elapsed()
            );
        }
    }

    /// The dashboard's video preview must play in a real decoder: ffmpeg
    /// reads it as an MP4 of the whole loop, frame for frame.
    #[test]
    fn the_video_preview_is_a_playable_mp4_of_one_loop() {
        if !ffmpeg_is_available("the video preview check") {
            return;
        }
        let theme = SlateTheme::Aurora;
        let video = preview_mp4(&settings(theme), 192, 108).expect("an even size");
        let path =
            std::env::temp_dir().join(format!("instantclone-preview-{}.mp4", std::process::id()));
        std::fs::write(&path, &video).unwrap();
        let _turn = ffmpeg_turn();
        let probe = Command::new("ffprobe")
            .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
            .args([
                "-show_entries",
                "stream=codec_name,width,height,nb_read_frames",
            ])
            .args(["-of", "csv=p=0"])
            .arg(&path)
            .output();
        let _ = std::fs::remove_file(&path);
        let Ok(probe) = probe else {
            log_line!("NOTE: ffprobe not found, so the video preview check did not run");
            return;
        };
        let frames = frames_per_interval(PREVIEW_FPS) * theme.loop_intervals();
        assert_eq!(
            String::from_utf8_lossy(&probe.stdout).trim(),
            format!("h264,192,108,{frames}"),
            "ffprobe: {}",
            String::from_utf8_lossy(&probe.stderr)
        );
        assert!(
            probe.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&probe.stderr)
        );
    }

    /// The rendered frames, cropped to the visible size, twice over.
    fn expected_frames(settings: &CrashProtection, shape: StreamShape) -> Vec<u8> {
        let style = screen_style(settings);
        let frame_count = frames_per_interval(shape.fps) * settings.theme.loop_intervals();
        let mut one_loop = Vec::new();
        for index in 0..frame_count {
            let mut canvas = Canvas::new(shape.width, shape.height, style.background);
            themes::draw(&mut canvas, &style, index as f32 / frame_count as f32);
            let frame = canvas.to_yuv420();
            let (chroma_stride, chroma_width, chroma_height) =
                (frame.width / 2, shape.width / 2, shape.height / 2);
            let planes = [
                (&frame.y, frame.width, shape.width, shape.height),
                (&frame.u, chroma_stride, chroma_width, chroma_height),
                (&frame.v, chroma_stride, chroma_width, chroma_height),
            ];
            for (plane, stride, width, height) in planes {
                for row in 0..height {
                    one_loop.extend_from_slice(&plane[row * stride..row * stride + width]);
                }
            }
        }
        [one_loop.clone(), one_loop].concat()
    }
}
