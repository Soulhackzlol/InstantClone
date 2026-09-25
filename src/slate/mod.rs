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
/// top to bottom in one BMP. Drawn by the same code as the stream, so the
/// dashboard preview is exactly what viewers get; one image for the whole
/// animation keeps the dashboard to a single request per change.
pub fn preview_bmp(
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
        .to_bmp()
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
        assert_eq!(types(&slate.keyframes[0]), vec![7, 8, 5], "SPS, PPS, IDR");
        assert_eq!(
            types(&slate.deltas[0]),
            vec![1],
            "P frames are just the slice"
        );
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

    #[test]
    fn loop_stays_small_at_1080p() {
        let shape = StreamShape {
            width: 1920,
            height: 1080,
            fps: 30,
        };
        for theme in SlateTheme::ALL {
            let slate = build_loop(&settings(theme), shape).unwrap();
            let loop_bytes: usize =
                slate.keyframes[0].len() + slate.deltas.iter().map(Vec::len).sum::<usize>();
            let kbps = loop_bytes * 8 / 2 / 1000;
            assert!(kbps < 1_000, "{theme:?} loop runs at {kbps} kbps");
        }
    }

    /// Decode two replays of the loop with ffmpeg and require every pixel
    /// to match what we rendered. Skips when ffmpeg isn't installed.
    #[test]
    fn ffmpeg_decodes_the_loop_losslessly() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            eprintln!("ffmpeg not found - skipping the decode round trip");
            return;
        }
        for theme in SlateTheme::ALL {
            let shape = StreamShape {
                width: 320,
                height: 180,
                fps: 10,
            };
            let slate = build_loop(&settings(theme), shape).unwrap();
            let decoded = decode_with_ffmpeg(&annex_b(&slate), shape.fps, theme);
            let expected = expected_frames(&settings(theme), shape);
            assert_eq!(decoded.len(), expected.len(), "{theme:?}: frame count");
            assert!(
                decoded == expected,
                "{theme:?}: decoded pixels differ from the render"
            );
        }
    }

    /// The NAL units of one AVCC sample (4-byte big-endian lengths).
    fn nal_units(sample: &[u8]) -> Vec<&[u8]> {
        let mut units = Vec::new();
        let mut at = 0;
        while at + 4 <= sample.len() {
            let len = u32::from_be_bytes(sample[at..at + 4].try_into().unwrap()) as usize;
            units.push(&sample[at + 4..at + 4 + len]);
            at += 4 + len;
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
