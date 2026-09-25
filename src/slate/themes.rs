//! The reconnect screen themes. Each draws one frame of the 2 s loop at
//! `phase` (0.0..1.0) onto a canvas already cleared to the background.
//!
//! Layout numbers are designed at 1080 px on the short side and scaled,
//! so the same theme works for every rendition and for vertical canvases.

use super::font::{self, Weight};
use super::pixel_font::{self, ADVANCE_COLUMNS, GLYPH_COLUMNS, GLYPH_ROWS};
use super::raster::{Canvas, Rgb};
use crate::crash_protection::SlateTheme;

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
    }
}

/// Short side over the 1080 px design size.
fn design_scale(canvas: &Canvas) -> f32 {
    canvas.width().min(canvas.height()) as f32 / 1080.0
}

const WHISPER_HEADLINE: Rgb = Rgb::new(0xee, 0xf0, 0xf3);
const WHISPER_SUBLINE: Rgb = Rgb::new(0x7d, 0x85, 0x91);
const WHISPER_DOT_IDLE: Rgb = Rgb::new(0x2a, 0x2f, 0x38);
/// Glow levels per dot. Stepping the pulse means most frames repeat the
/// previous one exactly and encode as skipped blocks.
const WHISPER_GLOW_STEPS: f32 = 8.0;

fn draw_whisper(canvas: &mut Canvas, style: &ScreenStyle, phase: f32) {
    let scale = design_scale(canvas);
    let center_x = (canvas.width() as f32 / 2.0).round();
    let center_y = (canvas.height() as f32 / 2.0).round();

    let headline_line = font::Line {
        size_px: 46.0 * scale,
        weight: Weight::SemiBold,
    };
    font::draw_centered(
        canvas,
        style.headline,
        center_x,
        center_y - 8.0 * scale,
        headline_line,
        WHISPER_HEADLINE,
    );
    let subline_line = font::Line {
        size_px: 25.0 * scale,
        weight: Weight::Regular,
    };
    font::draw_centered(
        canvas,
        style.subline,
        center_x,
        center_y + 40.0 * scale,
        subline_line,
        WHISPER_SUBLINE,
    );

    // Three dots whose glow chases left to right once per loop.
    let spacing = 30.0 * scale;
    let radius = (7.0 * scale).round().max(1.0);
    for index in 0..3 {
        let wave = phase - index as f32 / 6.0;
        let glow = 0.5 + 0.5 * (std::f32::consts::TAU * wave).sin();
        let glow = (glow * WHISPER_GLOW_STEPS).round() / WHISPER_GLOW_STEPS;
        let dot_x = (center_x + (index as f32 - 1.0) * spacing).round();
        let dot_y = (center_y + 100.0 * scale).round();
        canvas.fill_circle(
            dot_x,
            dot_y,
            radius,
            WHISPER_DOT_IDLE.mix(style.accent, glow),
        );
    }
}

const ARCADE_SUBLINE: Rgb = Rgb::new(0xb9, 0xb2, 0xd6);
const ARCADE_SQUARE_IDLE: Rgb = Rgb::new(0x2a, 0x23, 0x44);
const ARCADE_SQUARES: usize = 8;
/// Headline cell size at 1080 px: exactly one macroblock, so every
/// block of the pixel art is a single flat colour.
const ARCADE_CELL_AT_1080: f32 = 16.0;
const ARCADE_CURSOR_COLUMNS: i32 = 4;

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
    let lit = ((phase * ARCADE_SQUARES as f32) as usize).min(ARCADE_SQUARES - 1);
    let trail = (lit + ARCADE_SQUARES - 1) % ARCADE_SQUARES;
    for index in 0..ARCADE_SQUARES {
        let color = match index {
            i if i == lit => style.accent,
            i if i == trail => ARCADE_SQUARE_IDLE.mix(style.accent, 0.4),
            _ => ARCADE_SQUARE_IDLE,
        };
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

    fn arcade_style() -> ScreenStyle<'static> {
        ScreenStyle {
            theme: SlateTheme::Arcade,
            accent: Rgb::new(0x5a, 0xc8, 0xfa),
            background: SlateTheme::Arcade.default_background(),
            headline: "Reconnecting",
            subline: "Back in a moment",
        }
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
        let style = arcade_style();
        let mut canvas = Canvas::new(1920, 1080, style.background);
        draw(&mut canvas, &style, 0.75);
        let frame = canvas.to_yuv420();
        // Headline cells are 16 px and snapped to multiples of 16, so the
        // block rows the headline occupies are all single-colour.
        let blocks_wide = 1920 / 16;
        let flat = (0..blocks_wide)
            .filter(|bx| is_flat_block(&frame, *bx, 24))
            .count();
        assert_eq!(flat, blocks_wide, "every headline-row block should be flat");
    }

    #[test]
    fn arcade_cursor_blinks_with_the_loop() {
        let style = arcade_style();
        let mut first_half = Canvas::new(640, 360, style.background);
        let mut second_half = Canvas::new(640, 360, style.background);
        draw(&mut first_half, &style, 0.1);
        draw(&mut second_half, &style, 0.6);
        assert!(first_half.to_yuv420().y != second_half.to_yuv420().y);
    }

    #[test]
    fn themes_fit_vertical_and_tiny_canvases() {
        for theme in [SlateTheme::Whisper, SlateTheme::Arcade] {
            let style = ScreenStyle {
                theme,
                ..arcade_style()
            };
            for (width, height) in [(1080, 1920), (284, 160), (32, 32)] {
                let mut canvas = Canvas::new(width, height, style.background);
                draw(&mut canvas, &style, 0.3);
            }
        }
    }
}
