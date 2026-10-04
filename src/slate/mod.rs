//! The crash-protection reconnect screen.
//!
//! Draws the streamer's chosen theme and encodes it as a 2 s H.264 loop:
//! one keyframe plus P frames, built once per stream shape and replayed
//! with fresh timestamps for as long as the hold lasts. Keeping the loop
//! exactly one keyframe interval long means every replay starts on an
//! IDR, so viewers who join mid-hold get a picture within 2 s.

mod bitstream;
mod encoder;
mod font;
mod pixel_font;
mod png;
mod raster;
mod themes;

pub use encoder::StreamShape;
pub use raster::Rgb;

use crate::crash_protection::CrashProtection;
use raster::Canvas;
use std::fmt;
use themes::ScreenStyle;

pub const LOOP_MS: usize = 2000;
const MIN_SIDE: usize = 16;
const MAX_SIDE: usize = 8192;
const MAX_FPS: u32 = 240;

/// One encoded loop, ready to be wrapped in RTMP video tags. It needs no
/// sequence header of its own: every keyframe carries the loop's SPS and
/// PPS in-band, under ids the stream it interrupts doesn't use.
pub struct SlateLoop {
    /// The loop's first picture as an AVCC sample (SPS, PPS, IDR), encoded
    /// twice with idr_pic_id 0 and 1. Replays alternate them, because
    /// back-to-back IDRs must not share an id.
    pub keyframes: [Vec<u8>; 2],
    /// The rest of the loop (P pictures) as AVCC samples, in order.
    pub deltas: Vec<Vec<u8>>,
    pub shape: StreamShape,
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
) -> Result<SlateLoop, UnsupportedShape> {
    let side_ok = |side: usize| (MIN_SIDE..=MAX_SIDE).contains(&side) && side.is_multiple_of(2);
    if !side_ok(shape.width) || !side_ok(shape.height) || !(1..=MAX_FPS).contains(&shape.fps) {
        return Err(UnsupportedShape(shape));
    }
    let style = screen_style(settings);
    let frame_count = frames_per_loop(shape.fps);
    let render = |index: usize| {
        let mut canvas = Canvas::new(shape.width, shape.height, style.background);
        themes::draw(&mut canvas, &style, index as f32 / frame_count as f32);
        canvas
    };

    let parameter_sets = encoder::parameter_sets(shape);
    let mut previous_canvas = render(0);
    let mut previous = previous_canvas.to_yuv420();
    let keyframes = [0, 1].map(|idr_pic_id| {
        let idr = encoder::encode_idr(&previous, idr_pic_id);
        bitstream::length_prefixed(&[&parameter_sets.sps, &parameter_sets.pps, &idr])
    });
    let mut deltas = Vec::with_capacity(frame_count - 1);
    for index in 1..frame_count {
        let canvas = render(index);
        // Only the animated rows change between frames.
        let frame = canvas.to_yuv420_after(&previous_canvas, &previous);
        let picture = encoder::encode_p(&frame, &previous, index as u32);
        deltas.push(bitstream::length_prefixed(&[&picture]));
        (previous_canvas, previous) = (canvas, frame);
    }

    Ok(SlateLoop {
        keyframes,
        deltas,
        shape,
    })
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
        eprintln!("NOTE: ffmpeg not found, so {check} did not run");
    }
    found
}

fn frames_per_loop(fps: u32) -> usize {
    (fps as usize * LOOP_MS / 1000).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crash_protection::SlateTheme;
    use std::process::Command;

    fn settings(theme: SlateTheme) -> CrashProtection {
        CrashProtection {
            theme,
            ..CrashProtection::default()
        }
    }

    #[test]
    fn loop_is_one_keyframe_interval_long() {
        let shape = StreamShape {
            width: 320,
            height: 180,
            fps: 30,
        };
        let slate = build_loop(&settings(SlateTheme::Arcade), shape).unwrap();
        assert_eq!(1 + slate.deltas.len(), 60);
        assert_ne!(slate.keyframes[0], slate.keyframes[1], "idr_pic_id differs");
    }

    #[test]
    fn keyframes_carry_their_own_parameter_sets_in_band() {
        let shape = StreamShape {
            width: 320,
            height: 180,
            fps: 10,
        };
        let slate = build_loop(&settings(SlateTheme::Whisper), shape).unwrap();
        let types = |sample: &[u8]| {
            nal_units(sample)
                .iter()
                .map(|nal| nal[0] & 0x1F)
                .collect::<Vec<_>>()
        };
        for keyframe in &slate.keyframes {
            assert_eq!(types(keyframe), vec![7, 8, 5], "SPS, PPS, IDR");
        }
        for delta in &slate.deltas {
            assert_eq!(types(delta), vec![1], "P frames are just the slice");
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
                build_loop(&base, shape).err(),
                Some(UnsupportedShape(shape))
            );
        }
    }

    /// Every theme's loop at `width` x `height`, built at the rate a hold
    /// really uses, stays under `max_kbps`.
    fn assert_every_theme_fits(width: usize, height: usize, max_kbps: usize) {
        let shape = StreamShape {
            width,
            height,
            fps: crate::crash_hold::SLATE_FPS,
        };
        for theme in SlateTheme::ALL {
            let slate = build_loop(&settings(theme), shape).unwrap();
            let loop_bytes: usize =
                slate.keyframes[0].len() + slate.deltas.iter().map(Vec::len).sum::<usize>();
            // Bits per millisecond are kbps.
            let kbps = loop_bytes * 8 / LOOP_MS;
            assert!(
                kbps < max_kbps,
                "{theme:?} loop at {width}x{height} runs at {kbps} kbps"
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
    /// to match what we rendered. Every theme at 320x180 (180 isn't whole
    /// macroblocks), then one portrait 200x360 so a real decoder checks the
    /// cropping of the width too; each ffmpeg run costs about half a second.
    #[test]
    fn ffmpeg_decodes_the_loop_losslessly() {
        if !ffmpeg_is_available("the reconnect screen decode round trip") {
            return;
        }
        let landscape = SlateTheme::ALL.map(|theme| (theme, 320, 180));
        let portrait = (SlateTheme::Studio, 200, 360);
        for (theme, width, height) in landscape.into_iter().chain([portrait]) {
            let shape = StreamShape {
                width,
                height,
                fps: 10,
            };
            let slate = build_loop(&settings(theme), shape).unwrap();
            let decoded = decode_with_ffmpeg(&annex_b(&slate), shape.fps, theme);
            let expected = expected_frames(&settings(theme), shape);
            assert_eq!(
                decoded.len(),
                expected.len(),
                "{theme:?} at {width}x{height}: frame count"
            );
            assert!(
                decoded == expected,
                "{theme:?} at {width}x{height}: decoded pixels differ from the render"
            );
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

    /// Two loop replays as an Annex B elementary stream.
    fn annex_b(slate: &SlateLoop) -> Vec<u8> {
        let mut out = Vec::new();
        for keyframe in &slate.keyframes {
            for sample in std::iter::once(keyframe).chain(&slate.deltas) {
                for nal in nal_units(sample) {
                    out.extend_from_slice(&[0, 0, 0, 1]);
                    out.extend_from_slice(nal);
                }
            }
        }
        out
    }

    /// Raw H.264 carries no timestamps, so the input rate is given
    /// explicitly and `-vsync 0` stops ffmpeg dropping identical frames.
    fn decode_with_ffmpeg(stream: &[u8], fps: u32, theme: SlateTheme) -> Vec<u8> {
        let dir = std::env::temp_dir();
        let tag = format!("instantclone-slate-{}-{}", std::process::id(), theme.id());
        let input = dir.join(format!("{tag}.h264"));
        let output = dir.join(format!("{tag}.yuv"));
        std::fs::write(&input, stream).unwrap();
        let result = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-r",
                &fps.to_string(),
                "-f",
                "h264",
                "-i",
            ])
            .arg(&input)
            .args(["-vsync", "0", "-f", "rawvideo", "-pix_fmt", "yuv420p"])
            .arg(&output)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr).to_string();
        let decoded = std::fs::read(&output).unwrap_or_default();
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
        assert!(
            result.status.success() && stderr.trim().is_empty(),
            "ffmpeg: {stderr}"
        );
        decoded
    }

    /// The rendered frames, cropped to the visible size, twice over.
    fn expected_frames(settings: &CrashProtection, shape: StreamShape) -> Vec<u8> {
        let style = screen_style(settings);
        let frame_count = frames_per_loop(shape.fps);
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
