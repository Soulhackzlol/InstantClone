//! The reconnect screen themes. Each draws one frame of the 2 s loop at
//! `phase` (0.0..1.0) onto a canvas already cleared to the background.
//!
//! Layout numbers are designed at 1080 px on the short side and scaled,
//! so the same theme works for every rendition and for vertical canvases.
//!
//! Every animation moves smoothly, frame by frame at the stream's rate,
//! and only over a small area: the rest of each frame repeats the last and
//! encodes as skipped blocks, which keeps the loop cheap for the
//! streamer's upload.

use super::font::{self, Align, Weight};
use super::pixel_font::{self, ADVANCE_COLUMNS, GLYPH_COLUMNS, GLYPH_ROWS};
use super::raster::{Canvas, Rgb};
use super::{detail_themes, soft_themes};
use crate::crash_protection::SlateTheme;
use std::f32::consts::{FRAC_PI_2, TAU};

/// Everything a theme needs from the streamer's settings.
pub struct ScreenStyle<'a> {
    pub theme: SlateTheme,
    pub accent: Rgb,
    pub background: Rgb,
    pub headline: &'a str,
    pub subline: &'a str,
}

pub fn draw(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    match style.theme {
        SlateTheme::Whisper => draw_whisper(canvas, style, phase),
        SlateTheme::Arcade => draw_arcade(canvas, style, phase),
        SlateTheme::Beacon => draw_beacon(canvas, style, phase),
        SlateTheme::Orbit => draw_orbit(canvas, style, phase),
        SlateTheme::Studio => draw_studio(canvas, style, phase),
        SlateTheme::Aurora => soft_themes::draw_aurora(canvas, style, phase),
        SlateTheme::Halo => soft_themes::draw_halo(canvas, style, phase),
        SlateTheme::Tide => soft_themes::draw_tide(canvas, style, phase),
        SlateTheme::Halftone => detail_themes::draw_halftone(canvas, style, phase),
        SlateTheme::Lattice => detail_themes::draw_lattice(canvas, style, phase),
        SlateTheme::Embers => detail_themes::draw_embers(canvas, style, phase),
    }
}

/// Short side over the 1080 px design size.
pub(super) fn design_scale(canvas: &Canvas) -> f32 {
    canvas.width().min(canvas.height()) as f32 / 1080.0
}

pub(super) fn canvas_center(canvas: &Canvas) -> (f32, f32) {
    (
        (canvas.width() as f32 / 2.0).round(),
        (canvas.height() as f32 / 2.0).round(),
    )
}

pub(super) const HEADLINE_COLOR: Rgb = Rgb::new(0xee, 0xf0, 0xf3);
const SUBLINE_COLOR: Rgb = Rgb::new(0x7d, 0x85, 0x91);
pub(super) const HEADLINE_PX: f32 = 46.0;
pub(super) const SUBLINE_PX: f32 = 25.0;
/// Headline baseline to subline baseline, at 1080 px.
pub(super) const SUBLINE_DROP_PX: f32 = 48.0;

/// Unlit dots and empty tracks: the background lifted a little toward the
/// text, so they read on any background the streamer picks.
fn idle_color(style: &ScreenStyle) -> Rgb {
    style.background.mix(HEADLINE_COLOR, 0.16)
}

/// Headline over subline, placed by `align` at `x`, with the headline's
/// baseline at `baseline_y`. The text-first themes all set it this way.
pub(super) fn draw_text_block(
    canvas: &mut Canvas,
    style: &ScreenStyle,
    (x, baseline_y): (f32, f32),
    align: Align,
) {
    let scale = design_scale(canvas);
    let headline = font::Line {
        size_px: HEADLINE_PX * scale,
        weight: Weight::SemiBold,
        align,
    };
    font::draw(
        canvas,
        style.headline,
        x,
        baseline_y,
        headline,
        HEADLINE_COLOR,
    );
    let subline = font::Line {
        size_px: SUBLINE_PX * scale,
        weight: Weight::Regular,
        align,
    };
    let subline_y = baseline_y + SUBLINE_DROP_PX * scale;
    font::draw(canvas, style.subline, x, subline_y, subline, SUBLINE_COLOR);
}

/// Minimal centred text over three dots whose glow chases left to right.
fn draw_whisper(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    let scale = design_scale(canvas);
    let (center_x, center_y) = canvas_center(canvas);
    draw_text_block(
        canvas,
        style,
        (center_x, center_y - 8.0 * scale),
        Align::Center,
    );

    let spacing = 30.0 * scale;
    let radius = (7.0 * scale).round().max(1.0);
    let dot_y = (center_y + 100.0 * scale).round();
    for index in 0..3 {
        let wave = phase - index as f32 / 6.0;
        let glow = 0.5 + 0.5 * (TAU * wave).sin();
        let dot_x = (center_x + (index as f32 - 1.0) * spacing).round();
        let color = idle_color(style).mix(style.accent, glow);
        canvas.fill_circle(dot_x, dot_y, radius, color);
    }
}

/// Centred text under a dot that sends out two radar rings, half a loop
/// apart, each growing and fading into the background.
fn draw_beacon(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    let scale = design_scale(canvas);
    let (center_x, center_y) = canvas_center(canvas);
    let beacon_y = (center_y - 60.0 * scale).round();
    let thickness = (4.0 * scale).max(1.0);
    for offset in [0.0, 0.5] {
        let progress = (phase + offset).fract();
        let radius = (14.0 + 54.0 * progress) * scale;
        let color = style.background.mix(style.accent, 0.9 * (1.0 - progress));
        canvas.fill_ring(center_x, beacon_y, radius, radius - thickness, color);
    }
    canvas.fill_circle(center_x, beacon_y, (10.0 * scale).max(1.0), style.accent);
    draw_text_block(
        canvas,
        style,
        (center_x, center_y + 90.0 * scale),
        Align::Center,
    );
}

const ORBIT_DOTS: usize = 8;
/// How many dots the lit one's trail fades over.
const ORBIT_TRAIL_DOTS: f32 = 3.0;

/// Centred text under a ring of dots, the light chasing clockwise round
/// them with a fading trail: the classic loading spinner.
fn draw_orbit(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    let scale = design_scale(canvas);
    let (center_x, center_y) = canvas_center(canvas);
    let spinner_y = center_y - 50.0 * scale;
    let orbit_radius = 34.0 * scale;
    let dot_radius = (6.0 * scale).max(1.0);
    // Where the light is, in dots round the ring.
    let head = phase * ORBIT_DOTS as f32;
    for index in 0..ORBIT_DOTS {
        let glow = chase_glow(head, index, ORBIT_DOTS, ORBIT_TRAIL_DOTS);
        // Dot 0 sits at 12 o'clock; the index grows clockwise.
        let angle = TAU * index as f32 / ORBIT_DOTS as f32 - FRAC_PI_2;
        let dot_x = (center_x + orbit_radius * angle.cos()).round();
        let dot_y = (spinner_y + orbit_radius * angle.sin()).round();
        let color = idle_color(style).mix(style.accent, glow);
        canvas.fill_circle(dot_x, dot_y, dot_radius, color);
    }
    draw_text_block(
        canvas,
        style,
        (center_x, center_y + 50.0 * scale),
        Align::Center,
    );
}

/// How lit one of `count` lights in a row (or ring) is while a light
/// passes along it, `head` lights from the first: full where the light
/// is, fading over `trail` lights behind it, and rising over the one
/// ahead as it arrives. Moves smoothly as `head` does.
fn chase_glow(head: f32, index: usize, count: usize, trail: f32) -> f32 {
    let behind = (head - index as f32).rem_euclid(count as f32);
    if behind > count as f32 - 1.0 {
        // The light is on its way here.
        let arriving = behind - (count as f32 - 1.0);
        return arriving * arriving;
    }
    let fading = (1.0 - behind / trail).max(0.0);
    fading * fading
}

/// On a vertical canvas the text sits this far up from the bottom (of the
/// height): TikTok and Shorts cover the lowest part with comments and
/// buttons.
const STUDIO_VERTICAL_LIFT: f32 = 0.3;

/// A broadcast lower third: an accent bar beside left-aligned text, and a
/// short stripe sliding along a track under it.
fn draw_studio(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    let scale = design_scale(canvas);
    let px = |design: f32| (design * scale).round() as i32;
    let margin = px(96.0);
    let height = canvas.height() as f32;
    let lift = if height > canvas.width() as f32 {
        height * STUDIO_VERTICAL_LIFT
    } else {
        200.0 * scale
    };
    let baseline = (height - lift).round() as i32;

    let bar_width = px(8.0).max(1);
    canvas.fill_rect(
        margin,
        baseline - px(40.0),
        bar_width,
        px(100.0),
        style.accent,
    );
    let text_x = margin + bar_width + px(28.0);
    draw_text_block(canvas, style, (text_x as f32, baseline as f32), Align::Left);

    let track_y = baseline + px(SUBLINE_DROP_PX + 36.0);
    let track_width = px(360.0);
    let track_height = px(4.0).max(1);
    canvas.fill_rect(
        text_x,
        track_y,
        track_width,
        track_height,
        idle_color(style),
    );
    // The stripe enters from the left edge of the track and leaves at the
    // right, clipped to it, so it reads as continuous motion.
    let stripe_width = track_width / 4;
    let travel = (track_width + stripe_width) as f32;
    let stripe_x = text_x - stripe_width + (travel * phase).round() as i32;
    let left = stripe_x.max(text_x);
    let right = (stripe_x + stripe_width).min(text_x + track_width);
    canvas.fill_rect(left, track_y, right - left, track_height, style.accent);
}

const ARCADE_SUBLINE: Rgb = Rgb::new(0xb9, 0xb2, 0xd6);
const ARCADE_SQUARES: usize = 8;
/// Headline cell size at 1080 px: exactly one macroblock, so every
/// block of the pixel art is a single flat colour.
const ARCADE_CELL_AT_1080: f32 = 16.0;
const ARCADE_CURSOR_COLUMNS: i32 = 4;
/// How many squares the lit one's trail fades over.
const ARCADE_TRAIL_SQUARES: f32 = 2.0;

/// Pixel art snapped to the cell grid.
fn draw_arcade(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    let width = canvas.width() as f32;
    let height = canvas.height() as f32;
    let scale = design_scale(canvas);
    let headline = pixel_font::to_pixel_text(style.headline);
    let subline = pixel_font::to_pixel_text(style.subline);

    // Headline plus a cursor of 4 columns and a 1 column gap.
    let head_columns = (headline.len() * ADVANCE_COLUMNS + GLYPH_COLUMNS) as f32;
    let cell = (ARCADE_CELL_AT_1080 * scale)
        .round()
        .min((width * 0.88 / head_columns).floor())
        .max(2.0) as i32;
    let sub_columns = (subline.len() * ADVANCE_COLUMNS).max(1) as f32;
    let sub_cell = ((cell as f32 * 0.45).round())
        .min((width * 0.9 / sub_columns).floor())
        .max(1.0) as i32;
    let square = cell * 2;
    let rows = GLYPH_ROWS as i32;
    let total_height = 11 * cell + rows * sub_cell + 5 * cell + square;

    let head_x = snap((width - head_columns * cell as f32) / 2.0, cell);
    let head_y = snap(height / 2.0 - total_height as f32 / 2.0, cell);
    draw_pixel_text(canvas, &headline, head_x, head_y, cell, style.accent);
    if phase < 0.5 {
        let cursor_x = head_x + (headline.len() * ADVANCE_COLUMNS) as i32 * cell;
        canvas.fill_rect(
            cursor_x,
            head_y,
            ARCADE_CURSOR_COLUMNS * cell,
            rows * cell,
            style.accent,
        );
    }

    let sub_y = head_y + 11 * cell;
    let sub_x = snap((width - sub_columns * sub_cell as f32) / 2.0, sub_cell);
    draw_pixel_text(canvas, &subline, sub_x, sub_y, sub_cell, ARCADE_SUBLINE);

    let row_width = ARCADE_SQUARES as i32 * square + (ARCADE_SQUARES as i32 - 1) * cell;
    let row_x = snap((width - row_width as f32) / 2.0, cell);
    let row_y = snap((sub_y + rows * sub_cell + 5 * cell) as f32, cell);
    let head = phase * ARCADE_SQUARES as f32;
    let idle = idle_color(style);
    for index in 0..ARCADE_SQUARES {
        let glow = chase_glow(head, index, ARCADE_SQUARES, ARCADE_TRAIL_SQUARES);
        let color = idle.mix(style.accent, glow);
        canvas.fill_rect(
            row_x + index as i32 * (square + cell),
            row_y,
            square,
            square,
            color,
        );
    }
}

fn draw_pixel_text(canvas: &mut Canvas, text: &[char], x: i32, y: i32, cell: i32, color: Rgb) {
    for (index, c) in text.iter().enumerate() {
        let Some(rows) = pixel_font::glyph(*c) else {
            continue;
        };
        let glyph_x = x + (index * ADVANCE_COLUMNS) as i32 * cell;
        for (row, bits) in rows.iter().enumerate() {
            for column in (0..GLYPH_COLUMNS).filter(|column| pixel_font::is_lit(*bits, *column)) {
                canvas.fill_rect(
                    glyph_x + column as i32 * cell,
                    y + row as i32 * cell,
                    cell,
                    cell,
                    color,
                );
            }
        }
    }
}

/// Round `value` to the nearest multiple of `step`.
fn snap(value: f32, step: i32) -> i32 {
    (value / step as f32).round() as i32 * step
}

#[cfg(test)]
mod tests {
    use super::super::raster::YuvFrame;
    use super::*;

    fn style(theme: SlateTheme) -> ScreenStyle<'static> {
        ScreenStyle {
            theme,
            accent: Rgb::new(0x5a, 0xc8, 0xfa),
            background: theme.default_background(),
            headline: "Reconnecting",
            subline: "Back in a moment",
        }
    }

    fn render(style: &ScreenStyle, width: usize, height: usize, phase: f32) -> YuvFrame {
        let mut canvas = Canvas::new(width, height, style.background);
        draw(&mut canvas, style, phase);
        canvas.to_yuv420()
    }

    fn is_flat_block(frame: &YuvFrame, block_x: usize, block_y: usize) -> bool {
        let first = frame.y[block_y * 16 * frame.width + block_x * 16];
        (0..16).all(|dy| {
            (0..16)
                .all(|dx| frame.y[(block_y * 16 + dy) * frame.width + block_x * 16 + dx] == first)
        })
    }

    #[test]
    fn arcade_headline_lands_on_the_macroblock_grid_at_1080p() {
        let frame = render(&style(SlateTheme::Arcade), 1920, 1080, 0.75);
        // Headline cells are 16 px and snapped to multiples of 16, so the
        // block rows the headline occupies are all single-colour.
        let blocks_wide = 1920 / 16;
        let flat = (0..blocks_wide)
            .filter(|bx| is_flat_block(&frame, *bx, 24))
            .count();
        assert_eq!(flat, blocks_wide, "every headline-row block should be flat");
        // An empty row is flat too: the headline must really be on it.
        let background = frame.y[0];
        let row = &frame.y[24 * 16 * frame.width..][..frame.width];
        assert!(
            row.iter().any(|luma| *luma != background),
            "the headline moved off block row 24"
        );
    }

    #[test]
    fn every_theme_animates_across_the_loop() {
        for theme in SlateTheme::ALL {
            let style = style(theme);
            let early = render(&style, 640, 360, 0.1);
            // A quarter loop apart: Beacon repeats every half loop, by design.
            let late = render(&style, 640, 360, 0.35);
            assert!(early.y != late.y, "{theme:?} is a still image");
        }
    }

    #[test]
    fn every_theme_draws_its_own_picture() {
        let frames: Vec<Vec<u8>> = SlateTheme::ALL
            .iter()
            .map(|theme| render(&style(*theme), 640, 360, 0.3).y)
            .collect();
        for (index, frame) in frames.iter().enumerate() {
            let twins = frames.iter().filter(|other| *other == frame).count();
            assert_eq!(
                twins,
                1,
                "{:?} looks like another theme",
                SlateTheme::ALL[index]
            );
        }
    }

    /// Full-size vertical canvases are built in full by the budget tests in
    /// `slate`; this covers the tiny ones, either way up.
    #[test]
    fn themes_render_on_tiny_canvases_without_panicking() {
        for theme in SlateTheme::ALL {
            for (width, height) in [(284, 160), (160, 284), (32, 32)] {
                render(&style(theme), width, height, 0.3);
            }
        }
    }

    /// Across the whole loop the track row is painted on exactly the
    /// track's columns, including while the stripe enters and leaves, and
    /// the stripe (a second colour on the track) shows up at some point.
    #[test]
    fn studio_stripe_stays_on_its_track() {
        let style = style(SlateTheme::Studio);
        let track_left = 96 + 8 + 28;
        let track_columns: Vec<usize> = (track_left..track_left + 360).collect();
        let track_row = (1080.0 - 200.0 + SUBLINE_DROP_PX + 36.0) as usize;
        let mut stripe_seen = false;
        let frames = stream_frames(SlateTheme::Studio);
        for frame in 0..frames {
            let phase = frame as f32 / frames as f32;
            let frame = render(&style, 1920, 1080, phase);
            let row = &frame.y[track_row * frame.width..][..1920];
            let background_luma = row[0];
            let painted: Vec<usize> = (0..1920).filter(|x| row[*x] != background_luma).collect();
            assert_eq!(
                painted, track_columns,
                "phase {phase}: the track row is painted off the track's columns"
            );
            stripe_seen |= painted.iter().any(|x| row[*x] != row[painted[0]]);
        }
        assert!(stripe_seen, "the stripe never showed on the track");
    }

    /// Frames in one loop of `theme` at the stream's frame rate.
    fn stream_frames(theme: SlateTheme) -> usize {
        let per_interval =
            crate::crash_hold::SLATE_FPS as usize * super::super::KEYFRAME_INTERVAL_MS / 1000;
        per_interval * theme.loop_intervals()
    }

    /// Smooth motion: over the first second of every theme's loop, each
    /// frame the stream sends moves on from the one before. An animation
    /// held for a few frames at a time is what reads as choppy.
    #[test]
    fn every_theme_moves_on_every_frame() {
        for theme in SlateTheme::ALL {
            let style = style(theme);
            let frames = stream_frames(theme);
            let at = |frame: usize| render(&style, 480, 270, frame as f32 / frames as f32);
            let mut before = at(0);
            for frame in 1..crate::crash_hold::SLATE_FPS as usize {
                let now = at(frame);
                assert!(
                    now.y != before.y || now.u != before.u,
                    "{theme:?} holds frame {frame}"
                );
                before = now;
            }
        }
    }
}
