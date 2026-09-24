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

/// One encoded loop, ready to be wrapped in RTMP video tags.
pub struct SlateLoop {
    /// AVCDecoderConfigurationRecord for the video sequence header.
    pub avc_config: Vec<u8>,
    /// The loop's first picture as an AVCC sample, encoded twice with
    /// idr_pic_id 0 and 1. Replays alternate them, because back-to-back
    /// IDRs must not share an id.
    pub keyframes: [Vec<u8>; 2],
    /// The rest of the loop (P pictures) as AVCC samples, in order.
    pub deltas: Vec<Vec<u8>>,
    pub shape: StreamShape,
}

impl SlateLoop {
    pub fn frame_count(&self) -> usize {
        1 + self.deltas.len()
    }
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

/// Still frame of the screen as a BMP, drawn by the same code as the
/// stream, so the dashboard preview is exactly what viewers get.
pub fn preview_bmp(settings: &CrashProtection, width: usize, height: usize, phase: f32) -> Vec<u8> {
    let style = screen_style(settings);
    let mut canvas = Canvas::new(width, height, style.background);
    themes::draw(&mut canvas, &style, phase);
    canvas.to_bmp()
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
        canvas.to_yuv420()
    };

    let first = render(0);
    let keyframes = [0, 1]
        .map(|idr_pic_id| bitstream::length_prefixed(&[&encoder::encode_idr(&first, idr_pic_id)]));
    let mut deltas = Vec::with_capacity(frame_count - 1);
    let mut previous = first;
    for index in 1..frame_count {
        let frame = render(index);
        let picture = encoder::encode_p(&frame, &previous, index as u32);
        deltas.push(bitstream::length_prefixed(&[&picture]));
        previous = frame;
    }

    let parameter_sets = encoder::parameter_sets(shape);
    Ok(SlateLoop {
        avc_config: bitstream::avc_config_record(&parameter_sets.sps, &parameter_sets.pps),
        keyframes,
        deltas,
        shape,
    })
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
        assert_eq!(slate.frame_count(), 60);
        assert_ne!(slate.keyframes[0], slate.keyframes[1], "idr_pic_id differs");
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
        for theme in [SlateTheme::Whisper, SlateTheme::Arcade] {
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
        for theme in [SlateTheme::Whisper, SlateTheme::Arcade] {
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

    /// Two loop replays as an Annex B elementary stream.
    fn annex_b(slate: &SlateLoop) -> Vec<u8> {
        let record = &slate.avc_config;
        let sps_len = u16::from_be_bytes([record[6], record[7]]) as usize;
        let sps = &record[8..8 + sps_len];
        let pps = &record[8 + sps_len + 3..];
        let mut out = Vec::new();
        for nal in [sps, pps] {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
        }
        for keyframe in &slate.keyframes {
            for sample in std::iter::once(keyframe).chain(&slate.deltas) {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(&sample[4..]);
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
