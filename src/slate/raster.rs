//! Tiny RGB rasterizer for the reconnect screen, plus the BT.709
//! limited-range conversion to the 4:2:0 planes the encoder consumes.
//!
//! Shapes are flat fills. Only edges get partial coverage, so the
//! interior of every shape stays one exact colour and encodes as a
//! predicted (near-free) macroblock.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `#rrggbb` (either case). Anything else is None.
    pub fn from_hex(text: &str) -> Option<Self> {
        let hex = text.trim().strip_prefix('#')?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        Some(Self::new(
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Linear mix towards `other`: 0.0 is self, 1.0 is other.
    pub fn mix(self, other: Rgb, amount: f32) -> Rgb {
        let amount = amount.clamp(0.0, 1.0);
        let channel = |from: u8, to: u8| -> u8 {
            (from as f32 + (to as f32 - from as f32) * amount).round() as u8
        };
        Rgb::new(
            channel(self.r, other.r),
            channel(self.g, other.g),
            channel(self.b, other.b),
        )
    }
}

/// RGB drawing surface at the visible (uncropped) resolution.
pub struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<Rgb>,
}

impl Canvas {
    pub fn new(width: usize, height: usize, background: Rgb) -> Self {
        Self {
            width,
            height,
            pixels: vec![background; width * height],
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Opaque rectangle, clipped to the canvas.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgb) {
        let x0 = (x.max(0) as usize).min(self.width);
        let y0 = (y.max(0) as usize).min(self.height);
        let x1 = (x.saturating_add(w)).clamp(0, self.width as i32) as usize;
        let y1 = (y.saturating_add(h)).clamp(0, self.height as i32) as usize;
        for row in y0..y1 {
            let start = row * self.width;
            self.pixels[start + x0..start + x1.max(x0)].fill(color);
        }
    }

    /// Paint the whole canvas from `colour`, a smooth function of position
    /// (0.0..1.0 across, then down) giving red, green and blue in 0.0..255.0.
    /// It is sampled on a grid every `cell` pixels and interpolated between:
    /// exact enough for anything smooth, at a fraction of a call per pixel.
    pub fn paint_smooth(&mut self, cell: usize, colour: impl Fn(f32, f32) -> [f32; 3]) {
        let (columns, rows) = (self.width / cell + 2, self.height / cell + 2);
        let mut grid = Vec::with_capacity(columns * rows);
        for row in 0..rows {
            for column in 0..columns {
                let x = (column * cell) as f32 / self.width as f32;
                let y = (row * cell) as f32 / self.height as f32;
                grid.push(colour(x, y));
            }
        }
        let step = 1.0 / cell as f32;
        // Each pixel row first blends the two grid rows around it, once per
        // grid column; the pixels then only blend across. Doing both per
        // pixel was most of the time a frame of light took to paint.
        let mut line = vec![[0.0f32; 3]; columns];
        for y in 0..self.height {
            let (row, down) = (y / cell, (y % cell) as f32 * step);
            let (above, below) = (&grid[row * columns..], &grid[(row + 1) * columns..]);
            for (column, blended) in line.iter_mut().enumerate() {
                *blended = std::array::from_fn(|channel| {
                    let top = above[column][channel];
                    top + (below[column][channel] - top) * down
                });
            }
            let pixels = &mut self.pixels[y * self.width..(y + 1) * self.width];
            for (column, span) in pixels.chunks_mut(cell).enumerate() {
                let (left, right) = (line[column], line[column + 1]);
                for (offset, pixel) in span.iter_mut().enumerate() {
                    let across = offset as f32 * step;
                    // Clamped first, so adding a half and truncating rounds
                    // like `round` does, without its library call.
                    let channel = |index: usize| {
                        let value = left[index] + (right[index] - left[index]) * across;
                        (value.clamp(0.0, 255.0) + 0.5) as u8
                    };
                    *pixel = Rgb::new(channel(0), channel(1), channel(2));
                }
            }
        }
    }

    /// The pixel at (`x`, `y`).
    #[cfg(test)]
    pub fn pixel(&self, x: usize, y: usize) -> Rgb {
        self.pixels[y * self.width + x]
    }

    /// Circle with 4x4 supersampled edges.
    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Rgb) {
        self.fill_ring(cx, cy, radius, 0.0, color);
    }

    /// The band between `inner` and `outer` radius, 4x4 supersampled.
    pub fn fill_ring(&mut self, cx: f32, cy: f32, outer: f32, inner: f32, color: Rgb) {
        const SAMPLES: usize = 4;
        let x0 = (cx - outer).floor().max(0.0) as usize;
        let y0 = (cy - outer).floor().max(0.0) as usize;
        let x1 = ((cx + outer).ceil().max(0.0) as usize).min(self.width);
        let y1 = ((cy + outer).ceil().max(0.0) as usize).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                let mut inside = 0;
                for sy in 0..SAMPLES {
                    for sx in 0..SAMPLES {
                        let px = x as f32 + (sx as f32 + 0.5) / SAMPLES as f32 - cx;
                        let py = y as f32 + (sy as f32 + 0.5) / SAMPLES as f32 - cy;
                        let distance_squared = px * px + py * py;
                        if distance_squared <= outer * outer && distance_squared >= inner * inner {
                            inside += 1;
                        }
                    }
                }
                self.blend(x, y, color, inside as f32 / (SAMPLES * SAMPLES) as f32);
            }
        }
    }

    /// Mix `color` into one pixel by `coverage` (0.0..=1.0).
    pub fn blend(&mut self, x: usize, y: usize, color: Rgb, coverage: f32) {
        if x >= self.width || y >= self.height || coverage <= 0.0 {
            return;
        }
        let pixel = &mut self.pixels[y * self.width + x];
        *pixel = if coverage >= 1.0 {
            color
        } else {
            pixel.mix(color, coverage)
        };
    }

    /// Add `light` (red, green and blue on the 0..255 scale) to one pixel,
    /// as light adds up: glows that overlap get brighter.
    pub fn add_light(&mut self, x: usize, y: usize, light: [f32; 3]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let pixel = &mut self.pixels[y * self.width + x];
        let lit =
            |channel: u8, added: f32| (f32::from(channel) + added).round().clamp(0.0, 255.0) as u8;
        *pixel = Rgb::new(
            lit(pixel.r, light[0]),
            lit(pixel.g, light[1]),
            lit(pixel.b, light[2]),
        );
    }

    /// Convert to 4:2:0 planes padded to whole H.264 macroblocks.
    #[cfg(test)]
    pub fn to_yuv420(&self) -> YuvFrame {
        self.to_yuv420_padded(16)
    }

    /// Convert to 4:2:0 planes padded to whole `block` x `block` squares
    /// (16 for H.264 macroblocks, 32 for HEVC CTBs). Padding repeats the
    /// edge pixels, which the SPS crops away again.
    pub fn to_yuv420_padded(&self, block: usize) -> YuvFrame {
        let coded_width = self.width.div_ceil(block) * block;
        let coded_height = self.height.div_ceil(block) * block;
        let mut frame = YuvFrame::blank(coded_width, coded_height);
        for pair in 0..coded_height / 2 {
            self.convert_row_pair(&mut frame, pair);
        }
        frame
    }

    /// `to_yuv420`, reusing `before_frame` (the conversion of `before`)
    /// for every pair of rows the two canvases share. A loop's frames
    /// differ in a few rows, and converting the rest again was 90% of the
    /// time it takes to build one.
    pub fn to_yuv420_after(&self, before: &Canvas, before_frame: &YuvFrame) -> YuvFrame {
        let mut frame = before_frame.clone();
        for pair in 0..frame.height / 2 {
            let changed = [2 * pair, 2 * pair + 1]
                .iter()
                .any(|&y| self.row(y) != before.row(y));
            if changed {
                self.convert_row_pair(&mut frame, pair);
            }
        }
        frame
    }

    /// Coded luma rows `2 * pair` and `2 * pair + 1`, and chroma row
    /// `pair`. Columns past the canvas repeat its right edge.
    fn convert_row_pair(&self, frame: &mut YuvFrame, pair: usize) {
        let tables = &*BT709;
        let coded_width = frame.width;
        let rows = [self.row(2 * pair), self.row(2 * pair + 1)];
        for (offset, row) in rows.iter().enumerate() {
            let start = (2 * pair + offset) * coded_width;
            let line = &mut frame.y[start..start + coded_width];
            for (sample, pixel) in line.iter_mut().zip(row.iter()) {
                *sample = luma(tables, *pixel);
            }
            let edge = line[self.width - 1];
            line[self.width..].fill(edge);
        }
        let chroma_width = coded_width / 2;
        let at = |row: &[Rgb], x: usize| row[x.min(self.width - 1)];
        let start = pair * chroma_width;
        for cx in 0..chroma_width {
            let quad = [
                at(rows[0], 2 * cx),
                at(rows[0], 2 * cx + 1),
                at(rows[1], 2 * cx),
                at(rows[1], 2 * cx + 1),
            ];
            (frame.u[start + cx], frame.v[start + cx]) = chroma_samples(tables, quad);
        }
    }

    /// Row `y`, repeating the bottom edge below the canvas.
    fn row(&self, y: usize) -> &[Rgb] {
        let start = y.min(self.height - 1) * self.width;
        &self.pixels[start..start + self.width]
    }

    /// PNG of the canvas, for the dashboard's thumbnails.
    pub fn to_png(&self) -> Vec<u8> {
        let rgb: Vec<u8> = self.pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
        super::png::encode(self.width, self.height, &rgb)
    }
}

/// Planar 4:2:0 frame at the coded size (whole macroblocks or CTBs).
#[derive(Clone)]
pub struct YuvFrame {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl YuvFrame {
    fn blank(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            y: vec![16; width * height],
            u: vec![128; width * height / 4],
            v: vec![128; width * height / 4],
        }
    }
}

// BT.709 coefficients, limited range: the colour space OBS streams in by
// default, and what the SPS VUI advertises.
const KR: f64 = 0.2126;
const KB: f64 = 0.0722;
/// Fixed point: the conversion runs on every pixel of every frame, and
/// table lookups in integers take a fraction of the float arithmetic.
const FRACTION_BITS: u32 = 16;

/// Each channel's share of Y, U and V by its value, in 16.16 fixed point:
/// the conversion is linear, so a pixel's samples are sums of three.
struct Bt709 {
    /// [channel][value], channels red, green, blue.
    y: [[i32; 256]; 3],
    u: [[i32; 256]; 3],
    v: [[i32; 256]; 3],
}

static BT709: std::sync::LazyLock<Bt709> = std::sync::LazyLock::new(|| {
    let kg = 1.0 - KR - KB;
    // Y' = Kr R + Kg G + Kb B, U = (B - Y') / 2(1 - Kb), V = (R - Y') /
    // 2(1 - Kr); then scaled to the 219 and 224 levels of limited range.
    let y_weights = [KR, kg, KB].map(|k| k * 219.0 / 255.0);
    let u_scale = 224.0 / 255.0 / (2.0 * (1.0 - KB));
    let v_scale = 224.0 / 255.0 / (2.0 * (1.0 - KR));
    let u_weights = [-KR * u_scale, -kg * u_scale, (1.0 - KB) * u_scale];
    let v_weights = [(1.0 - KR) * v_scale, -kg * v_scale, -KB * v_scale];
    let table = |weights: [f64; 3]| {
        weights.map(|weight| {
            std::array::from_fn(|value| {
                (weight * value as f64 * f64::from(1 << FRACTION_BITS)).round() as i32
            })
        })
    };
    Bt709 {
        y: table(y_weights),
        u: table(u_weights),
        v: table(v_weights),
    }
});

/// A pixel's share in one plane's `table`.
fn share(table: &[[i32; 256]; 3], color: Rgb) -> i32 {
    table[0][usize::from(color.r)] + table[1][usize::from(color.g)] + table[2][usize::from(color.b)]
}

fn luma(tables: &Bt709, color: Rgb) -> u8 {
    to_sample(16, share(&tables.y, color), 0)
}

/// U and V samples for a 2x2 block of pixels: their chroma, averaged.
fn chroma_samples(tables: &Bt709, quad: [Rgb; 4]) -> (u8, u8) {
    let sum = |table| quad.iter().map(|pixel| share(table, *pixel)).sum::<i32>();
    (
        to_sample(128, sum(&tables.u), 2),
        to_sample(128, sum(&tables.v), 2),
    )
}

/// `offset` plus a fixed-point `sum` of 2^`average_bits` shares averaged,
/// rounded into 1..=254: 0 and 255 are never needed in limited range, and
/// keeping clear of 0 keeps I_PCM samples valid for strict decoders.
fn to_sample(offset: i32, sum: i32, average_bits: u32) -> u8 {
    let shift = FRACTION_BITS + average_bits;
    let value = ((offset << shift) + sum + (1 << (shift - 1))) >> shift;
    value.clamp(1, 254) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_rejects_junk() {
        assert_eq!(Rgb::from_hex("#5AC8FA"), Some(Rgb::new(0x5a, 0xc8, 0xfa)));
        assert_eq!(Rgb::new(1, 2, 3).to_hex(), "#010203");
        assert_eq!(Rgb::from_hex("5ac8fa"), None);
        assert_eq!(Rgb::from_hex("#5ac8f"), None);
        assert_eq!(Rgb::from_hex("#5ac8fg"), None);
    }

    #[test]
    fn fill_rect_clips_to_canvas() {
        let mut canvas = Canvas::new(8, 8, Rgb::new(0, 0, 0));
        canvas.fill_rect(-4, 6, 100, 100, Rgb::new(255, 0, 0));
        assert_eq!(canvas.row(5)[0], Rgb::new(0, 0, 0));
        assert_eq!(canvas.row(7)[7], Rgb::new(255, 0, 0));
    }

    #[test]
    fn yuv_pads_to_macroblocks_and_maps_black_and_white() {
        let mut canvas = Canvas::new(20, 18, Rgb::new(0, 0, 0));
        canvas.fill_rect(0, 0, 10, 18, Rgb::new(255, 255, 255));
        let frame = canvas.to_yuv420();
        assert_eq!((frame.width, frame.height), (32, 32));
        assert_eq!(frame.y[0], 235, "white is limited-range 235");
        assert_eq!(frame.y[19], 16, "black is limited-range 16");
        assert_eq!(frame.y[31], 16, "padding repeats the right edge");
        assert_eq!(frame.u[0], 128);
    }

    #[test]
    fn converting_only_changed_rows_matches_a_full_conversion() {
        let mut before = Canvas::new(20, 18, Rgb::new(10, 20, 30));
        before.fill_rect(2, 2, 6, 3, Rgb::new(200, 40, 90));
        let mut after = Canvas::new(20, 18, Rgb::new(10, 20, 30));
        after.fill_rect(2, 2, 6, 3, Rgb::new(200, 40, 90));
        // A middle row and the bottom edge, which the padding repeats.
        after.fill_rect(5, 9, 3, 1, Rgb::new(90, 250, 12));
        after.fill_rect(0, 17, 20, 1, Rgb::new(255, 255, 255));
        let reused = after.to_yuv420_after(&before, &before.to_yuv420());
        let full = after.to_yuv420();
        assert!(reused.y == full.y && reused.u == full.u && reused.v == full.v);
    }
}
