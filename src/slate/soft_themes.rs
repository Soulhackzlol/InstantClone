//! The soft themes: light that glows and drifts instead of flat shapes.
//!
//! Each paints a smooth field of light over the background, then the
//! headline and subline on top. The light never reaches the area behind
//! the text: a still pool there keeps the text blocks identical from frame
//! to frame, so they are sent once per keyframe and skipped after, and the
//! moving light only costs bits where it moves. Motion is a function of the
//! loop phase with whole periods per loop, so the loop has no seam.

use super::font::{self, Align, Weight};
use super::raster::{Canvas, Rgb};
use super::themes::{
    canvas_center, design_scale, draw_text_block, ScreenStyle, HEADLINE_PX, SUBLINE_DROP_PX,
    SUBLINE_PX,
};
use std::f32::consts::TAU;

/// The light is sampled every this many pixels and interpolated between.
const LIGHT_CELL_PX: usize = 8;
/// Room left round the text before light may start, at 1080 px.
const POOL_MARGIN_AT_1080: f32 = 36.0;
/// Extra room in plain pixels: a coding block that holds a letter reaches
/// up to 31 px past it, and all of that block must stay still.
const POOL_BLOCK_MARGIN_PX: f32 = 40.0;
/// How far, at 1080 px, the light takes to come up to full strength.
const POOL_FALLOFF_AT_1080: f32 = 260.0;

/// The still area round the text: the oval through the corners of the
/// text's rectangle (so the whole rectangle is inside it), and how softly
/// light rises past its edge. An oval reads as a vignette; a rectangle
/// read as a box.
struct Pool {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    falloff: f32,
}

impl Pool {
    /// The pool round centred text whose headline sits on `baseline_y`.
    fn around_text(canvas: &Canvas, style: &ScreenStyle, baseline_y: f32) -> Self {
        let scale = design_scale(canvas);
        let line = |size_px: f32, weight| font::Line {
            size_px: size_px * scale,
            weight,
            align: Align::Center,
        };
        let text_width = font::width(style.headline, line(HEADLINE_PX, Weight::SemiBold)).max(
            font::width(style.subline, line(SUBLINE_PX, Weight::Regular)),
        );
        let margin = POOL_MARGIN_AT_1080 * scale + POOL_BLOCK_MARGIN_PX;
        let (center_x, _) = canvas_center(canvas);
        Self {
            left: center_x - text_width / 2.0 - margin,
            right: center_x + text_width / 2.0 + margin,
            top: baseline_y - HEADLINE_PX * scale * 1.25 - margin,
            bottom: baseline_y + (SUBLINE_DROP_PX + SUBLINE_PX * 0.4) * scale + margin,
            falloff: POOL_FALLOFF_AT_1080 * scale,
        }
    }

    /// How much of the light reaches the pixel at (`x`, `y`): none in the
    /// pool, all of it a falloff past its edge, eased between.
    fn reach(&self, x: f32, y: f32) -> f32 {
        let radius_x = (self.right - self.left) / 2.0 * std::f32::consts::SQRT_2;
        let radius_y = (self.bottom - self.top) / 2.0 * std::f32::consts::SQRT_2;
        let dx = (x - (self.left + self.right) / 2.0) / radius_x;
        let dy = (y - (self.top + self.bottom) / 2.0) / radius_y;
        let outside = (dx * dx + dy * dy).sqrt() - 1.0;
        if outside <= 0.0 {
            return 0.0;
        }
        // Pixels past the edge, measured along the oval's short axis.
        let t = (outside * radius_x.min(radius_y) / self.falloff).min(1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

/// Where the headline of a centred soft theme sits: the two lines centred
/// on the canvas as a pair.
fn centred_baseline(canvas: &Canvas) -> f32 {
    let (_, center_y) = canvas_center(canvas);
    center_y + 4.0 * design_scale(canvas)
}

/// Paint the background plus `light` (RGB to add, in 0..255, from the
/// position as fractions of the width and height), held back by the pool.
fn paint_light(
    canvas: &mut Canvas,
    style: &ScreenStyle,
    pool: &Pool,
    light: impl Fn(f32, f32) -> [f32; 3],
) {
    let base = channels(style.background);
    let (width, height) = (canvas.width() as f32, canvas.height() as f32);
    canvas.paint_smooth(LIGHT_CELL_PX, |x, y| {
        let reach = pool.reach(x * width, y * height);
        if reach <= 0.0 {
            return base;
        }
        let added = light(x, y);
        [0, 1, 2].map(|channel| base[channel] + reach * added[channel])
    });
}

fn channels(color: Rgb) -> [f32; 3] {
    [f32::from(color.r), f32::from(color.g), f32::from(color.b)]
}

/// `color` with its hue turned by `degrees`, keeping its brightness (the
/// hue-rotate matrix of the CSS filter effects spec).
fn hue_turned(color: Rgb, degrees: f32) -> [f32; 3] {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let [r, g, b] = channels(color);
    let row = |lr: f32,
               lg: f32,
               lb: f32,
               (cr, cg, cb): (f32, f32, f32),
               (sr, sg, sb): (f32, f32, f32)| {
        (r * (lr + cos * cr + sin * sr)
            + g * (lg + cos * cg + sin * sg)
            + b * (lb + cos * cb + sin * sb))
            .clamp(0.0, 255.0)
    };
    [
        row(
            0.213,
            0.715,
            0.072,
            (0.787, -0.715, -0.072),
            (-0.213, -0.715, 0.928),
        ),
        row(
            0.213,
            0.715,
            0.072,
            (-0.213, 0.285, -0.072),
            (0.143, 0.140, -0.283),
        ),
        row(
            0.213,
            0.715,
            0.072,
            (-0.213, -0.715, 0.928),
            (-0.787, 0.715, 0.072),
        ),
    ]
}

/// Width over height, so shapes measured in heights stay round.
fn aspect(canvas: &Canvas) -> f32 {
    canvas.width() as f32 / canvas.height() as f32
}

/// Three glows, in the accent and the hues either side of it, each
/// circling its own spot once per loop and breathing as it goes.
pub fn draw_aurora(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    const ORBIT: f32 = 0.07;
    const SPREAD: f32 = 0.10;
    const STRENGTH: f32 = 0.7;
    let glows = [
        (channels(style.accent), (0.20, 0.28), 0.0),
        (hue_turned(style.accent, 55.0), (0.80, 0.32), 1.0 / 3.0),
        (hue_turned(style.accent, -55.0), (0.50, 0.82), 2.0 / 3.0),
    ];
    let baseline = centred_baseline(canvas);
    let pool = Pool::around_text(canvas, style, baseline);
    let aspect = aspect(canvas);
    paint_light(canvas, style, &pool, |x, y| {
        let mut light = [0.0; 3];
        for (tint, (spot_x, spot_y), offset) in glows {
            let angle = TAU * (phase + offset);
            let dx = (x - spot_x - ORBIT * angle.cos()) * aspect;
            let dy = y - spot_y - ORBIT * angle.sin();
            let breath = 0.8 + 0.2 * (angle * 2.0).sin();
            let glow = STRENGTH * breath * (-(dx * dx + dy * dy) / SPREAD).exp();
            for channel in 0..3 {
                light[channel] += tint[channel] * glow;
            }
        }
        light
    });
    let (center_x, _) = canvas_center(canvas);
    draw_text_block(canvas, style, (center_x, baseline), Align::Center);
}

/// A soft elliptical ring of light round the text, breathing slightly,
/// with a brighter arc travelling round it once per loop.
pub fn draw_halo(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    const RADIUS_X: f32 = 0.34;
    const RADIUS_Y: f32 = 0.36;
    const THICKNESS: f32 = 0.11;
    const STRENGTH: f32 = 0.42;
    const ARC_BOOST: f32 = 1.1;
    let accent = channels(style.accent);
    let baseline = centred_baseline(canvas);
    let pool = Pool::around_text(canvas, style, baseline);
    let breath = 0.025 * (TAU * phase).sin();
    paint_light(canvas, style, &pool, |x, y| {
        // In units of the ring's own radii: an ellipse shaped like the
        // canvas, round in these units.
        let rx = (x - 0.5) / RADIUS_X;
        let ry = (y - 0.5) / RADIUS_Y;
        let radius = (rx * rx + ry * ry).sqrt();
        let off_ring = (radius - 1.0 - breath) / THICKNESS;
        let ring = (-off_ring * off_ring).exp();
        let toward_arc = 0.5 + 0.5 * (ry.atan2(rx) - TAU * phase).cos();
        let arc = toward_arc.powi(6);
        let inner_wash = 0.08 * (-radius * radius * 2.0).exp();
        let glow = STRENGTH * ring * (0.45 + ARC_BOOST * arc) + inner_wash;
        accent.map(|channel| channel * glow)
    });
    let (center_x, _) = canvas_center(canvas);
    draw_text_block(canvas, style, (center_x, baseline), Align::Center);
}

/// Two families of soft diagonal swells crossing each other, one in the
/// accent rolling up, one in a neighbouring hue rolling down, brighter
/// towards the bottom of the screen.
pub fn draw_tide(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    let layers = [
        (channels(style.accent), 0.5_f32, 1.1, 1.0, 0.46),
        (hue_turned(style.accent, 40.0), -0.35, 0.8, -1.0, 0.34),
    ];
    let baseline = centred_baseline(canvas);
    let pool = Pool::around_text(canvas, style, baseline);
    let aspect = aspect(canvas);
    paint_light(canvas, style, &pool, |x, y| {
        let mut light = [0.0; 3];
        let rising = 0.3 + 0.7 * y;
        for (tint, slant, waves, direction, strength) in layers {
            let along = x * aspect * slant.sin() + y * slant.cos();
            let swell = 0.5 + 0.5 * (TAU * (along * waves - direction * phase)).cos();
            let glow = strength * rising * swell.powi(3);
            for channel in 0..3 {
                light[channel] += tint[channel] * glow;
            }
        }
        light
    });
    let (center_x, _) = canvas_center(canvas);
    draw_text_block(canvas, style, (center_x, baseline), Align::Center);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crash_protection::SlateTheme;

    fn style(theme: SlateTheme) -> ScreenStyle<'static> {
        ScreenStyle {
            theme,
            accent: Rgb::new(0x5a, 0xc8, 0xfa),
            background: Rgb::new(0x0a, 0x0c, 0x12),
            headline: "Reconnecting",
            subline: "The stream will be back in a moment",
        }
    }

    /// The text never moves and no light reaches it, so every pixel of
    /// the pool is the same in every frame: that is what lets the encoder
    /// skip the text blocks between keyframes.
    #[test]
    fn the_pool_round_the_text_stays_still() {
        for theme in [SlateTheme::Aurora, SlateTheme::Halo, SlateTheme::Tide] {
            let style = style(theme);
            let frame = |phase: f32| {
                let mut canvas = Canvas::new(640, 360, style.background);
                super::super::themes::draw(&mut canvas, &style, phase);
                canvas
            };
            let pool = Pool::around_text(&frame(0.0), &style, centred_baseline(&frame(0.0)));
            let (first, later) = (frame(0.0), frame(0.37));
            let mut moved_outside = false;
            for y in 0..360 {
                for x in 0..640 {
                    let inside = (x as f32) >= pool.left
                        && (x as f32) <= pool.right
                        && (y as f32) >= pool.top
                        && (y as f32) <= pool.bottom;
                    let same = first.pixel(x, y) == later.pixel(x, y);
                    if inside {
                        assert!(same, "{theme:?}: pixel {x},{y} in the pool moved");
                    } else {
                        moved_outside |= !same;
                    }
                }
            }
            assert!(moved_outside, "{theme:?}: nothing moves at all");
        }
    }

    /// A loop has no seam: the last moment leads back into the first.
    #[test]
    fn the_light_comes_round_to_where_it_started() {
        for theme in [SlateTheme::Aurora, SlateTheme::Halo, SlateTheme::Tide] {
            let style = style(theme);
            let frame = |phase: f32| {
                let mut canvas = Canvas::new(320, 180, style.background);
                super::super::themes::draw(&mut canvas, &style, phase);
                canvas.to_png()
            };
            assert!(frame(0.0) == frame(1.0), "{theme:?}");
        }
    }

    #[test]
    fn turning_the_hue_keeps_grey_grey_and_comes_full_circle() {
        let grey = Rgb::new(100, 100, 100);
        for channel in hue_turned(grey, 77.0) {
            assert!((channel - 100.0).abs() < 0.5, "{channel}");
        }
        let accent = Rgb::new(0x5a, 0xc8, 0xfa);
        let round = hue_turned(accent, 360.0);
        for (turned, original) in round.iter().zip(channels(accent)) {
            assert!((turned - original).abs() < 0.5);
        }
    }
}
