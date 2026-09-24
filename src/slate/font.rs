//! Text for the reconnect screen: a minimal TrueType reader and an
//! anti-aliased coverage rasterizer, enough to draw one line of Inter.
//!
//! The embedded fonts are built by `scripts/subset_slate_fonts.py`, which
//! guarantees what this reader relies on: a format 4 cmap, quadratic
//! outlines and no composite glyphs. No kerning; the screen text is a
//! short headline where it isn't missed.
//!
//! The rasterizer is the signed-area accumulation technique used by
//! font-rs (Raph Levien): each edge adds signed coverage to the cells it
//! crosses, and a running sum along each row turns that into per-pixel
//! coverage.

use super::raster::{Canvas, Rgb};
use std::sync::OnceLock;

static REGULAR_TTF: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular-slate.ttf");
static SEMIBOLD_TTF: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold-slate.ttf");
/// Vertical reach of a line relative to its size: accented capitals rise
/// to about 1.2 em above the baseline and descenders drop about 0.3 em.
const ABOVE_BASELINE_EM: f32 = 1.25;
const BELOW_BASELINE_EM: f32 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    SemiBold,
}

/// How one line of text is set.
#[derive(Debug, Clone, Copy)]
pub struct Line {
    pub size_px: f32,
    pub weight: Weight,
}

/// Draw `text` centred on `center_x` with its baseline at `baseline_y`.
pub fn draw_centered(
    canvas: &mut Canvas,
    text: &str,
    center_x: f32,
    baseline_y: f32,
    line: Line,
    color: Rgb,
) {
    let Some(font) = font_for(line.weight) else {
        return;
    };
    let scale = line.size_px / font.units_per_em;
    let glyphs: Vec<u16> = text.chars().map(|c| font.glyph_id(c)).collect();
    let width: f32 = glyphs.iter().map(|gid| font.advance(*gid) * scale).sum();

    let band_top = (baseline_y - line.size_px * ABOVE_BASELINE_EM)
        .floor()
        .max(0.0) as usize;
    let band_bottom = ((baseline_y + line.size_px * BELOW_BASELINE_EM)
        .ceil()
        .max(0.0) as usize)
        .min(canvas.height());
    if band_bottom <= band_top {
        return;
    }
    let mut coverage = Coverage::new(canvas.width(), band_top, band_bottom - band_top);
    let mut pen_x = center_x - width / 2.0;
    for gid in glyphs {
        for contour in font.contours(gid) {
            let to_pixels =
                |(x, y): (f32, f32)| (pen_x + x * scale, baseline_y - y * scale - band_top as f32);
            let points: Vec<(f32, f32)> = flatten_contour(&contour)
                .into_iter()
                .map(to_pixels)
                .collect();
            for pair in points.windows(2) {
                coverage.add_line(pair[0], pair[1]);
            }
        }
        pen_x += font.advance(gid) * scale;
    }
    coverage.blend_into(canvas, color);
}

fn font_for(weight: Weight) -> Option<&'static Font> {
    static REGULAR: OnceLock<Option<Font>> = OnceLock::new();
    static SEMIBOLD: OnceLock<Option<Font>> = OnceLock::new();
    match weight {
        Weight::Regular => REGULAR.get_or_init(|| Font::parse(REGULAR_TTF)).as_ref(),
        Weight::SemiBold => SEMIBOLD.get_or_init(|| Font::parse(SEMIBOLD_TTF)).as_ref(),
    }
}

/// One outline point in font units.
#[derive(Debug, Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
    on_curve: bool,
}

struct Font {
    data: &'static [u8],
    units_per_em: f32,
    long_loca: bool,
    num_glyphs: u16,
    num_h_metrics: u16,
    loca: usize,
    glyf: usize,
    hmtx: usize,
    cmap4: usize,
}

impl Font {
    fn parse(data: &'static [u8]) -> Option<Self> {
        let table = |tag: &[u8; 4]| find_table(data, tag);
        let head = table(b"head")?;
        let cmap = table(b"cmap")?;
        Some(Self {
            data,
            units_per_em: read_u16(data, head + 18)? as f32,
            long_loca: read_u16(data, head + 50)? == 1,
            num_glyphs: read_u16(data, table(b"maxp")? + 4)?,
            num_h_metrics: read_u16(data, table(b"hhea")? + 34)?,
            loca: table(b"loca")?,
            glyf: table(b"glyf")?,
            hmtx: table(b"hmtx")?,
            cmap4: find_cmap4(data, cmap)?,
        })
    }

    /// Glyph for `c`, or 0 (.notdef) when the subset doesn't have it.
    fn glyph_id(&self, c: char) -> u16 {
        let code = c as u32;
        if code > 0xFFFF {
            return 0;
        }
        self.cmap4_lookup(code as u16).unwrap_or(0)
    }

    fn cmap4_lookup(&self, code: u16) -> Option<u16> {
        let d = self.data;
        let base = self.cmap4;
        let seg_count = (read_u16(d, base + 6)? / 2) as usize;
        let ends = base + 14;
        let starts = ends + seg_count * 2 + 2;
        let deltas = starts + seg_count * 2;
        let offsets = deltas + seg_count * 2;
        for seg in 0..seg_count {
            if read_u16(d, ends + seg * 2)? < code {
                continue;
            }
            let start = read_u16(d, starts + seg * 2)?;
            if start > code {
                return None;
            }
            let delta = read_u16(d, deltas + seg * 2)?;
            let range_offset = read_u16(d, offsets + seg * 2)? as usize;
            if range_offset == 0 {
                return Some(code.wrapping_add(delta));
            }
            let at = offsets + seg * 2 + range_offset + (code - start) as usize * 2;
            let gid = read_u16(d, at)?;
            return Some(if gid == 0 { 0 } else { gid.wrapping_add(delta) });
        }
        None
    }

    fn advance(&self, gid: u16) -> f32 {
        let index = gid.min(self.num_h_metrics.saturating_sub(1)) as usize;
        read_u16(self.data, self.hmtx + index * 4).unwrap_or(0) as f32
    }

    fn glyph_range(&self, gid: u16) -> Option<(usize, usize)> {
        if gid >= self.num_glyphs {
            return None;
        }
        let gid = gid as usize;
        let (start, end) = if self.long_loca {
            (
                read_u32(self.data, self.loca + gid * 4)?,
                read_u32(self.data, self.loca + gid * 4 + 4)?,
            )
        } else {
            let short = |i: usize| read_u16(self.data, self.loca + i * 2).map(|v| v as u32 * 2);
            (short(gid)?, short(gid + 1)?)
        };
        Some((self.glyf + start as usize, self.glyf + end as usize))
    }

    /// Outline contours of a simple glyph. Empty for blank glyphs
    /// (space) and for anything malformed.
    fn contours(&self, gid: u16) -> Vec<Vec<Point>> {
        self.glyph_range(gid)
            .filter(|(start, end)| end > start)
            .and_then(|(start, _)| parse_simple_glyph(self.data, start))
            .unwrap_or_default()
    }
}

fn parse_simple_glyph(d: &[u8], start: usize) -> Option<Vec<Vec<Point>>> {
    let contour_count = read_u16(d, start)? as i16;
    if contour_count <= 0 {
        return None;
    }
    let mut ends = Vec::with_capacity(contour_count as usize);
    for i in 0..contour_count as usize {
        ends.push(read_u16(d, start + 10 + i * 2)? as usize);
    }
    let point_count = *ends.last()? + 1;
    let instructions_len = read_u16(d, start + 10 + ends.len() * 2)? as usize;
    let mut at = start + 12 + ends.len() * 2 + instructions_len;

    let flags = read_flags(d, &mut at, point_count)?;
    let xs = read_coordinates(d, &mut at, &flags, 0x02, 0x10)?;
    let ys = read_coordinates(d, &mut at, &flags, 0x04, 0x20)?;

    let mut contours = Vec::with_capacity(ends.len());
    let mut first = 0;
    for end in ends {
        let contour = (first..=end.min(point_count - 1))
            .map(|i| Point {
                x: xs[i],
                y: ys[i],
                on_curve: flags[i] & 0x01 != 0,
            })
            .collect();
        contours.push(contour);
        first = end + 1;
    }
    Some(contours)
}

fn read_flags(d: &[u8], at: &mut usize, count: usize) -> Option<Vec<u8>> {
    let mut flags = Vec::with_capacity(count);
    while flags.len() < count {
        let flag = *d.get(*at)?;
        *at += 1;
        flags.push(flag);
        if flag & 0x08 != 0 {
            let repeats = *d.get(*at)?;
            *at += 1;
            flags.extend(std::iter::repeat_n(flag, repeats as usize));
        }
    }
    flags.truncate(count);
    Some(flags)
}

/// Decode one delta-encoded coordinate array. `short_bit` marks a 1-byte
/// delta whose sign is `same_bit`; without `short_bit`, `same_bit` means
/// "unchanged" and otherwise a signed 2-byte delta follows.
fn read_coordinates(
    d: &[u8],
    at: &mut usize,
    flags: &[u8],
    short_bit: u8,
    same_bit: u8,
) -> Option<Vec<f32>> {
    let mut value: i32 = 0;
    let mut out = Vec::with_capacity(flags.len());
    for flag in flags {
        if flag & short_bit != 0 {
            let delta = *d.get(*at)? as i32;
            *at += 1;
            value += if flag & same_bit != 0 { delta } else { -delta };
        } else if flag & same_bit == 0 {
            value += read_u16(d, *at)? as i16 as i32;
            *at += 2;
        }
        out.push(value as f32);
    }
    Some(out)
}

/// Turn a contour of on/off-curve points into a closed polyline in font
/// units. Two off-curve points in a row imply an on-curve midpoint.
fn flatten_contour(points: &[Point]) -> Vec<(f32, f32)> {
    let Some(start_index) = points.iter().position(|p| p.on_curve) else {
        return Vec::new();
    };
    let start = points[start_index];
    let mut out = vec![(start.x, start.y)];
    let mut control: Option<Point> = None;
    let rotated = points[start_index + 1..]
        .iter()
        .chain(&points[..=start_index]);
    for point in rotated {
        let current = *out.last().unwrap_or(&(start.x, start.y));
        match (control, point.on_curve) {
            (None, true) => out.push((point.x, point.y)),
            (None, false) => control = Some(*point),
            (Some(c), true) => {
                push_quadratic(&mut out, current, (c.x, c.y), (point.x, point.y));
                control = None;
            }
            (Some(c), false) => {
                let mid = ((c.x + point.x) / 2.0, (c.y + point.y) / 2.0);
                push_quadratic(&mut out, current, (c.x, c.y), mid);
                control = Some(*point);
            }
        }
    }
    out
}

/// Append a quadratic Bezier as line segments. Segment count follows
/// how far the control point bends the curve (font-rs's heuristic,
/// in font units so it's generous at small sizes).
fn push_quadratic(out: &mut Vec<(f32, f32)>, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32)) {
    let bend_x = p0.0 - 2.0 * p1.0 + p2.0;
    let bend_y = p0.1 - 2.0 * p1.1 + p2.1;
    let bend = (bend_x * bend_x + bend_y * bend_y).sqrt();
    let steps = (bend.sqrt() / 2.0).ceil().clamp(1.0, 24.0) as usize;
    for step in 1..=steps {
        let t = step as f32 / steps as f32;
        let u = 1.0 - t;
        out.push((
            u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
            u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
        ));
    }
}

/// Signed-area accumulation buffer for a band of canvas rows starting
/// at `top`. Edge coordinates are relative to the band.
struct Coverage {
    width: usize,
    top: usize,
    height: usize,
    cells: Vec<f32>,
}

impl Coverage {
    fn new(width: usize, top: usize, height: usize) -> Self {
        // Two spare columns so edges at the right border never index out.
        Self {
            width,
            top,
            height,
            cells: vec![0.0; (width + 2) * height],
        }
    }

    /// Add one edge. Straight port of font-rs `draw_line`, with x clamped
    /// to the canvas: anything left of 0 or right of the width only shifts
    /// where the row's coverage starts, never what visible pixels get.
    fn add_line(&mut self, from: (f32, f32), to: (f32, f32)) {
        if (from.1 - to.1).abs() <= f32::EPSILON {
            return;
        }
        let (direction, top, bottom) = if from.1 < to.1 {
            (1.0, from, to)
        } else {
            (-1.0, to, from)
        };
        let dx_dy = (bottom.0 - top.0) / (bottom.1 - top.1);
        let mut x = top.0;
        if top.1 < 0.0 {
            x -= top.1 * dx_dy;
        }
        let first_row = top.1.max(0.0) as usize;
        let last_row = (bottom.1.ceil().max(0.0) as usize).min(self.height);
        for row in first_row..last_row {
            let dy = (row as f32 + 1.0).min(bottom.1) - (row as f32).max(top.1);
            let x_next = x + dx_dy * dy;
            self.add_row_piece(row, x, x_next, dy * direction);
            x = x_next;
        }
    }

    /// Distribute one row's piece of an edge, from `x` to `x_next`, whose
    /// signed height is `delta`.
    fn add_row_piece(&mut self, row: usize, x: f32, x_next: f32, delta: f32) {
        let max_x = self.width as f32;
        let (x, x_next) = (x.clamp(0.0, max_x), x_next.clamp(0.0, max_x));
        let (x0, x1) = if x < x_next { (x, x_next) } else { (x_next, x) };
        let stride = self.width + 2;
        let cells = &mut self.cells[row * stride..(row + 1) * stride];
        let x0_floor = x0.floor();
        let x0i = x0_floor as usize;
        let x1_ceil = x1.ceil();
        let x1i = x1_ceil as usize;
        if x1i <= x0i + 1 {
            let middle = 0.5 * (x + x_next) - x0_floor;
            cells[x0i] += delta - delta * middle;
            cells[x0i + 1] += delta * middle;
            return;
        }
        let inverse_span = (x1 - x0).recip();
        let x0_fraction = x0 - x0_floor;
        let first_area = 0.5 * inverse_span * (1.0 - x0_fraction) * (1.0 - x0_fraction);
        let x1_fraction = x1 - x1_ceil + 1.0;
        let last_area = 0.5 * inverse_span * x1_fraction * x1_fraction;
        cells[x0i] += delta * first_area;
        if x1i == x0i + 2 {
            cells[x0i + 1] += delta * (1.0 - first_area - last_area);
        } else {
            let second_area = inverse_span * (1.5 - x0_fraction);
            cells[x0i + 1] += delta * (second_area - first_area);
            for cell in &mut cells[x0i + 2..x1i - 1] {
                *cell += delta * inverse_span;
            }
            let before_last = second_area + (x1i - x0i - 3) as f32 * inverse_span;
            cells[x1i - 1] += delta * (1.0 - before_last - last_area);
        }
        cells[x1i] += delta * last_area;
    }

    fn blend_into(&self, canvas: &mut Canvas, color: Rgb) {
        let stride = self.width + 2;
        for row in 0..self.height {
            let mut running = 0.0;
            for column in 0..self.width {
                running += self.cells[row * stride + column];
                canvas.blend(column, self.top + row, color, running.abs().min(1.0));
            }
        }
    }
}

fn find_table(data: &[u8], tag: &[u8; 4]) -> Option<usize> {
    let count = read_u16(data, 4)? as usize;
    (0..count).find_map(|i| {
        let record = 12 + i * 16;
        (data.get(record..record + 4)? == tag)
            .then(|| read_u32(data, record + 8).map(|v| v as usize))?
    })
}

/// Offset of the Unicode BMP format 4 subtable.
fn find_cmap4(data: &[u8], cmap: usize) -> Option<usize> {
    let count = read_u16(data, cmap + 2)? as usize;
    (0..count).find_map(|i| {
        let record = cmap + 4 + i * 8;
        let platform = read_u16(data, record)?;
        let encoding = read_u16(data, record + 2)?;
        let offset = cmap + read_u32(data, record + 4)? as usize;
        let is_unicode = platform == 0 || (platform == 3 && encoding == 1);
        (is_unicode && read_u16(data, offset)? == 4).then_some(offset)
    })
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_parse_and_map_latin() {
        for weight in [Weight::Regular, Weight::SemiBold] {
            let font = font_for(weight).expect("embedded font must parse");
            assert_eq!(font.units_per_em, 2048.0);
            assert_ne!(font.glyph_id('R'), 0);
            assert_ne!(font.glyph_id('é'), 0, "Latin-1 is in the subset");
            assert_eq!(
                font.glyph_id('€'),
                0,
                "outside the subset falls back to .notdef"
            );
            assert!(font.advance(font.glyph_id('M')) > font.advance(font.glyph_id('i')));
        }
    }

    #[test]
    fn letters_have_outlines_and_space_does_not() {
        let font = font_for(Weight::Regular).unwrap();
        assert!(!font.contours(font.glyph_id('o')).is_empty());
        assert_eq!(
            font.contours(font.glyph_id('o')).len(),
            2,
            "o has an inner and outer contour"
        );
        assert!(font.contours(font.glyph_id(' ')).is_empty());
    }

    #[test]
    fn filled_square_covers_its_inside_exactly() {
        let mut coverage = Coverage::new(10, 0, 10);
        let square = [(2.0, 2.0), (8.0, 2.0), (8.0, 8.0), (2.0, 8.0), (2.0, 2.0)];
        for pair in square.windows(2) {
            coverage.add_line(pair[0], pair[1]);
        }
        let mut canvas = Canvas::new(10, 10, Rgb::new(0, 0, 0));
        coverage.blend_into(&mut canvas, Rgb::new(255, 255, 255));
        let frame = canvas.to_yuv420();
        assert_eq!(frame.y[5 * 16 + 5], 235, "inside is fully covered");
        assert_eq!(frame.y[5 * 16 + 1], 16, "outside is untouched");
        assert_eq!(frame.y[5 * 16 + 8], 16, "right edge is exclusive");
    }

    #[test]
    fn slanted_edge_gives_partial_coverage() {
        let mut coverage = Coverage::new(10, 0, 4);
        let triangle = [(0.0, 0.0), (8.0, 4.0), (0.0, 4.0), (0.0, 0.0)];
        for pair in triangle.windows(2) {
            coverage.add_line(pair[0], pair[1]);
        }
        let mut canvas = Canvas::new(10, 4, Rgb::new(0, 0, 0));
        coverage.blend_into(&mut canvas, Rgb::new(255, 255, 255));
        let frame = canvas.to_yuv420();
        let row = 2;
        let samples: Vec<u8> = (0..10).map(|x| frame.y[row * 16 + x]).collect();
        assert_eq!(samples[0], 235);
        assert!(
            samples[4] > 16 && samples[4] < 235,
            "edge pixel is blended: {samples:?}"
        );
        assert_eq!(samples[9], 16);
    }

    #[test]
    fn centred_text_draws_symmetrically() {
        let mut canvas = Canvas::new(200, 60, Rgb::new(0, 0, 0));
        let line = Line {
            size_px: 40.0,
            weight: Weight::SemiBold,
        };
        draw_centered(
            &mut canvas,
            "HOH",
            100.0,
            45.0,
            line,
            Rgb::new(255, 255, 255),
        );
        let frame = canvas.to_yuv420();
        let lit: Vec<usize> = (0..200)
            .filter(|x| frame.y[30 * frame.width + x] > 128)
            .collect();
        let (first, last) = (*lit.first().unwrap(), *lit.last().unwrap());
        assert!(
            (first as i32 + last as i32 - 199).abs() <= 2,
            "not centred: {first}..{last}"
        );
    }
}
