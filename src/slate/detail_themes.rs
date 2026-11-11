//! Soft themes built from many small things instead of one smooth field:
//! halftone dots, a lit dot grid, rising sparks.
//!
//! Fine detail is what costs bits, so each keeps it cheap its own way.
//! Halftone moves in steps and only redraws dots whose size changed, so
//! most of its frames repeat the last and are skipped. Lattice keeps its
//! dots still and only changes how brightly they are lit. Embers draws a
//! few dozen sparks, so only the blocks they pass through change. Like the
//! other soft themes they keep the still pool round the text and loop
//! without a seam.

use super::font::Align;
use super::raster::{Canvas, Rgb};
use super::soft_themes::{centred_baseline, channels, hue_turned, paint_light, Pool};
use super::themes::{
    canvas_center, design_scale, draw_text_block, stepped, ScreenStyle, HEADLINE_COLOR,
};
use std::f32::consts::{PI, TAU};

/// Dot spacing at 1080 px: one dot per 16x16 coding block at full size.
const HALFTONE_PITCH_AT_1080: f32 = 16.0;
/// Dot sizes, from a speck to touching its neighbours.
const HALFTONE_SIZES: usize = 5;
/// How often per 8 s loop the smoke moves on: 5 times a second. Frames
/// between repeat the last and cost almost nothing.
const HALFTONE_STEPS: f32 = 40.0;
/// The ink before the accent tints it: a light grey, like newsprint
/// photographs reversed out of black.
const HALFTONE_INK: Rgb = Rgb::new(0xa4, 0xa8, 0xb0);

/// A smoky field printed as a halftone: dots on a fixed grid, each sized
/// by how dense the smoke is there, the smoke slowly curling.
pub fn draw_halftone(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    let scale = design_scale(canvas);
    // A whole number of 16x16 blocks, one dot each: a dot that straddles
    // blocks costs several times as much (in HEVC most of all).
    let pitch = (HALFTONE_PITCH_AT_1080 * scale / 16.0).round().max(1.0) as usize * 16;
    let baseline = centred_baseline(canvas);
    let pool = Pool::around_text(canvas, style, baseline);
    // Each size has its own shape and ink: thicker smoke prints bigger,
    // brighter dots, deeper in the accent, which gives the smoke depth.
    let sizes: Vec<(Vec<f32>, Rgb)> = (1..=HALFTONE_SIZES)
        .map(|size| {
            let thickness = size as f32 / HALFTONE_SIZES as f32;
            let shape = dot_coverage(pitch, 0.5 * pitch as f32 * thickness.sqrt());
            let ink = HALFTONE_INK.mix(style.accent, 0.15 + 0.35 * thickness);
            (shape, style.background.mix(ink, 0.55 + 0.45 * thickness))
        })
        .collect();
    // Phase 1.0 is the first step again, not one past the last.
    let turn = TAU * (stepped(phase, HALFTONE_STEPS) % 1.0);
    // The smoke is measured in short sides, so it has the same grain either
    // way up.
    let unit = canvas.width().min(canvas.height()) as f32;
    let (middle_x, middle_y) = canvas_center(canvas);
    for top in (0..canvas.height()).step_by(pitch) {
        for left in (0..canvas.width()).step_by(pitch) {
            let center_x = left as f32 + pitch as f32 / 2.0;
            let center_y = top as f32 + pitch as f32 / 2.0;
            let from_middle = (center_x - middle_x).hypot(center_y - middle_y) / unit;
            let thickness = smoke(center_x / unit, center_y / unit, turn)
                + SMOKE_EDGE_PULL * smoothstep(0.35, 1.0, from_middle);
            let density =
                smoothstep(SMOKE_CLEAR_BELOW, 0.9, thickness) * pool.reach(center_x, center_y);
            let size = (density * HALFTONE_SIZES as f32).round() as usize;
            if size > 0 {
                let (shape, ink) = &sizes[size - 1];
                stamp(canvas, shape, (left, top), pitch, *ink);
            }
        }
    }
    let (center_x, _) = canvas_center(canvas);
    draw_text_block(canvas, style, (center_x, baseline), Align::Center);
}

/// How thick the smoke is at (`x`, `y`), in short sides, at `turn`
/// (radians, once round per loop): about 0 to 1. Each pass bends the
/// coordinates by waves of the other one, which curls straight waves into
/// wisps; time only enters as whole turns, so the loop closes.
fn smoke(x: f32, y: f32, turn: f32) -> f32 {
    let (mut u, mut v) = (x * 3.6, y * 3.6);
    for pass in 1..=4 {
        let k = pass as f32;
        // Each pass sways back and forth by its own offset: the smoke
        // curls in place instead of streaming across the screen.
        let sway = SMOKE_SWAY * (turn + k * 1.9).sin();
        u += 0.7 / k * (k * 1.3 * v + sway + k * 1.7).sin();
        v += 0.7 / k * (k * 1.1 * u - sway + k * 0.9).cos();
    }
    0.5 + 0.3 * (u * 1.6).sin() * (v * 1.3).cos() + 0.2 * (u + v).sin()
}

/// How far, in radians of each wave, the smoke sways over a loop.
const SMOKE_SWAY: f32 = 0.4;
/// Smoke thinner than this shows no dots at all.
const SMOKE_CLEAR_BELOW: f32 = 0.56;
/// Smoke thickens towards the edges by up to this, so it frames the text
/// on every side instead of gathering wherever the waves happen to meet.
const SMOKE_EDGE_PULL: f32 = 0.12;

fn smoothstep(from: f32, to: f32, value: f32) -> f32 {
    let t = ((value - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How much of each pixel of a `pitch` x `pitch` cell a dot of `radius`
/// at its centre covers, 4x4 supersampled.
fn dot_coverage(pitch: usize, radius: f32) -> Vec<f32> {
    const SAMPLES: usize = 4;
    let center = pitch as f32 / 2.0;
    let mut coverage = Vec::with_capacity(pitch * pitch);
    for y in 0..pitch {
        for x in 0..pitch {
            let mut inside = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let dx = x as f32 + (sx as f32 + 0.5) / SAMPLES as f32 - center;
                    let dy = y as f32 + (sy as f32 + 0.5) / SAMPLES as f32 - center;
                    if dx * dx + dy * dy <= radius * radius {
                        inside += 1;
                    }
                }
            }
            coverage.push(inside as f32 / (SAMPLES * SAMPLES) as f32);
        }
    }
    coverage
}

/// Mix `color` into the `pitch` x `pitch` cell at (`left`, `top`) by
/// `coverage`.
fn stamp(
    canvas: &mut Canvas,
    coverage: &[f32],
    (left, top): (usize, usize),
    pitch: usize,
    color: Rgb,
) {
    for (index, amount) in coverage.iter().enumerate() {
        canvas.blend(left + index % pitch, top + index / pitch, color, *amount);
    }
}

/// Dot spacing and dot radius at 1080 px.
const LATTICE_PITCH_AT_1080: f32 = 32.0;
const LATTICE_DOT_AT_1080: f32 = 1.7;
/// Brightness steps a dot moves through as the light passes: a dot only
/// changes when it crosses one, not on every frame.
const LATTICE_LEVELS: f32 = 6.0;
/// The transform block side the dots are centred in, so each dot's
/// detail stays inside one block.
const TRANSFORM_BLOCK: usize = 4;

/// A fine grid of still dots, lit by two lights wandering under it: dots
/// in the light glow in the accent, the rest rest faint. Only the dots
/// show the light. A soft glow under them would be a gradient, and coded
/// within this theme's tolerance a gradient shows its blocks; flat
/// background stays clean (`slate::fidelity`).
pub fn draw_lattice(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    // Phase 1.0 is phase 0.0 exactly, so no dot's step rounds differently.
    let phase = phase.fract();
    let scale = design_scale(canvas);
    let baseline = centred_baseline(canvas);
    let pool = Pool::around_text(canvas, style, baseline);
    let (width, height) = (canvas.width() as f32, canvas.height() as f32);
    let lights = [((0.25, 0.35), 0.0), ((0.72, 0.62), 0.5)];
    let lit = |x: f32, y: f32| {
        lights
            .iter()
            .map(|((spot_x, spot_y), offset)| {
                let angle = TAU * (phase + offset);
                let dx = (x - spot_x - 0.14 * angle.cos()) * width / height;
                let dy = y - spot_y - 0.12 * (2.0 * angle).sin();
                (-(dx * dx + dy * dy) / 0.05).exp()
            })
            .sum::<f32>()
            .min(1.0)
    };

    // A whole number of transform blocks apart, every dot centred in one.
    let block = TRANSFORM_BLOCK;
    let pitch = ((LATTICE_PITCH_AT_1080 * scale / block as f32).round() as usize * block).max(8);
    let radius = (LATTICE_DOT_AT_1080 * scale).max(1.0);
    let cell = ((radius * 2.0).ceil() as usize + 2).next_multiple_of(2);
    let dot = dot_coverage(cell, radius);
    let idle = style.background.mix(HEADLINE_COLOR, 0.14);
    let bright = style.accent.mix(HEADLINE_COLOR, 0.35);
    // Centred, so the grid sits evenly between the edges.
    let columns = (canvas.width() / pitch).max(1);
    let rows = (canvas.height() / pitch).max(1);
    let first = |side: usize, count: usize| {
        (side.saturating_sub((count - 1) * pitch) / 2) / block * block + block / 2
    };
    let (first_x, first_y) = (first(canvas.width(), columns), first(canvas.height(), rows));
    for row in 0..rows {
        for column in 0..columns {
            let center_x = first_x + column * pitch;
            let center_y = first_y + row * pitch;
            let reach = pool.reach(center_x as f32, center_y as f32);
            if reach <= 0.0 {
                continue;
            }
            let light = lit(center_x as f32 / width, center_y as f32 / height);
            let light = (light * LATTICE_LEVELS).round() / LATTICE_LEVELS;
            let color = idle.mix(bright, light);
            let left = center_x.saturating_sub(cell / 2);
            let top = center_y.saturating_sub(cell / 2);
            for (index, amount) in dot.iter().enumerate() {
                canvas.blend(
                    left + index % cell,
                    top + index / cell,
                    color,
                    amount * reach,
                );
            }
        }
    }
    let (center_x, _) = canvas_center(canvas);
    draw_text_block(canvas, style, (center_x, baseline), Align::Center);
}

const EMBERS: u32 = 46;

/// Sparks drifting up from a low glow, swaying as they rise, fading in
/// and out. Each crosses the screen once or twice per loop.
pub fn draw_embers(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    let scale = design_scale(canvas);
    let baseline = centred_baseline(canvas);
    let pool = Pool::around_text(canvas, style, baseline);
    let (width, height) = (canvas.width() as f32, canvas.height() as f32);
    let accent = channels(style.accent);
    let warm = hue_turned(style.accent, 35.0);
    // The glow they rise from. It never changes, so it costs nothing
    // after the keyframe.
    paint_light(canvas, style, &pool, |_, y| {
        let rising = ((y - 0.5) / 0.5).clamp(0.0, 1.0).powi(2);
        accent.map(|channel| channel * 0.16 * rising)
    });
    for index in 0..EMBERS {
        let random = |salt: u32| unit_random(index * 8 + salt);
        let laps = 1.0 + (random(1) * 2.0).floor();
        let progress = (phase * laps + random(2)).fract();
        let sway_turns = 1.0 + (random(3) * 2.0).floor();
        let sway = (0.006 + 0.012 * random(4)) * (TAU * (phase * sway_turns + random(2))).sin();
        let spark = Spark {
            x: (random(0) + sway) * width,
            y: (1.08 - 1.16 * progress) * height,
            // Never under 2.5 px, so the sparks show on small previews too.
            size: ((5.0 + 7.0 * random(5)) * scale).max(2.5),
            brightness: (0.45 + 0.55 * random(6)) * (PI * progress).sin().powf(0.7),
            tint: [0, 1, 2].map(|c| accent[c] + (warm[c] - accent[c]) * random(7)),
        };
        draw_spark(canvas, &pool, &spark);
    }
    let (center_x, _) = canvas_center(canvas);
    draw_text_block(canvas, style, (center_x, baseline), Align::Center);
}

struct Spark {
    x: f32,
    y: f32,
    /// The soft halo's radius, in pixels; the bright core is 0.4 of it.
    size: f32,
    brightness: f32,
    tint: [f32; 3],
}

fn draw_spark(canvas: &mut Canvas, pool: &Pool, spark: &Spark) {
    let reach_px = 3.0 * spark.size;
    let x0 = (spark.x - reach_px).floor().max(0.0) as usize;
    let y0 = (spark.y - reach_px).floor().max(0.0) as usize;
    let x1 = ((spark.x + reach_px).ceil().max(0.0) as usize).min(canvas.width());
    let y1 = ((spark.y + reach_px).ceil().max(0.0) as usize).min(canvas.height());
    let halo = spark.size * spark.size;
    let core = halo * 0.16;
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = (x as f32 + 0.5 - spark.x, y as f32 + 0.5 - spark.y);
            let distance = dx * dx + dy * dy;
            let glow = 0.45 * (-distance / halo).exp() + 0.75 * (-distance / core).exp();
            let amount = spark.brightness * glow * pool.reach(x as f32, y as f32);
            if amount > 0.002 {
                canvas.add_light(x, y, spark.tint.map(|channel| channel * amount));
            }
        }
    }
}

/// A repeatable pseudo-random number in 0..1 for `seed` (a 32-bit integer
/// hash): the sparks land in the same places every build.
fn unit_random(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x5bd1_e995;
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^= x >> 16;
    x as f32 / u32::MAX as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_dot_fills_its_cell_and_a_speck_barely_shows() {
        let full = dot_coverage(8, 4.0);
        let center = full[3 * 8 + 3];
        assert_eq!(center, 1.0);
        let speck: f32 = dot_coverage(8, 0.5).iter().sum();
        assert!(speck < 1.0, "a speck covers {speck} pixels");
    }

    #[test]
    fn random_numbers_repeat_and_spread() {
        assert_eq!(unit_random(7), unit_random(7));
        let values: Vec<f32> = (0..200).map(unit_random).collect();
        assert!(values.iter().all(|v| (0.0..=1.0).contains(v)));
        let low = values.iter().filter(|v| **v < 0.5).count();
        assert!((70..130).contains(&low), "{low} of 200 under a half");
    }

    #[test]
    fn the_smoke_has_clear_air_and_thick_patches() {
        let samples: Vec<f32> = (0..400)
            .map(|i| smoke((i % 20) as f32 / 10.0, (i / 20) as f32 / 20.0, 0.0))
            .collect();
        assert!(
            samples.iter().any(|s| *s < SMOKE_CLEAR_BELOW),
            "no clear air"
        );
        assert!(samples.iter().any(|s| *s > 0.9), "no thick smoke");
    }
}
