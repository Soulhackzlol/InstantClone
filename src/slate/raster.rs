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
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = (x.saturating_add(w)).clamp(0, self.width as i32) as usize;
        let y1 = (y.saturating_add(h)).clamp(0, self.height as i32) as usize;
        for row in y0..y1 {
            let start = row * self.width;
            self.pixels[start + x0..start + x1.max(x0)].fill(color);
        }
    }

    /// Circle with 4x4 supersampled edges.
    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Rgb) {
        const SAMPLES: usize = 4;
        let x0 = (cx - radius).floor().max(0.0) as usize;
        let y0 = (cy - radius).floor().max(0.0) as usize;
        let x1 = ((cx + radius).ceil() as usize).min(self.width);
        let y1 = ((cy + radius).ceil() as usize).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                let mut inside = 0;
                for sy in 0..SAMPLES {
                    for sx in 0..SAMPLES {
                        let px = x as f32 + (sx as f32 + 0.5) / SAMPLES as f32 - cx;
                        let py = y as f32 + (sy as f32 + 0.5) / SAMPLES as f32 - cy;
                        if px * px + py * py <= radius * radius {
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

    /// Convert to 4:2:0 planes padded to whole macroblocks. Padding
    /// repeats the edge pixels, which the SPS crops away again.
    pub fn to_yuv420(&self) -> YuvFrame {
        let coded_width = self.width.div_ceil(16) * 16;
        let coded_height = self.height.div_ceil(16) * 16;
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
    /// `pair`. Neighbouring pixels are usually the same colour, so each
    /// conversion is reused until the colour changes.
    fn convert_row_pair(&self, frame: &mut YuvFrame, pair: usize) {
        let coded_width = frame.width;
        let rows = [self.row(2 * pair), self.row(2 * pair + 1)];
        let at = |row: &[Rgb], x: usize| row[x.min(self.width - 1)];
        for (offset, row) in rows.iter().enumerate() {
            let start = (2 * pair + offset) * coded_width;
            let mut last = (row[0], luma(row[0]));
            for (x, sample) in frame.y[start..start + coded_width].iter_mut().enumerate() {
                let pixel = at(row, x);
                if pixel != last.0 {
                    last = (pixel, luma(pixel));
                }
                *sample = last.1;
            }
        }
        let chroma_width = coded_width / 2;
        let mut last: Option<([Rgb; 4], (u8, u8))> = None;
        for cx in 0..chroma_width {
            let quad = [
                at(rows[0], 2 * cx),
                at(rows[0], 2 * cx + 1),
                at(rows[1], 2 * cx),
                at(rows[1], 2 * cx + 1),
            ];
            let (u, v) = match last {
                Some((seen, samples)) if seen == quad => samples,
                _ => {
                    let samples = chroma_samples(quad);
                    last = Some((quad, samples));
                    samples
                }
            };
            frame.u[pair * chroma_width + cx] = u;
            frame.v[pair * chroma_width + cx] = v;
        }
    }

    /// Row `y`, repeating the bottom edge below the canvas.
    fn row(&self, y: usize) -> &[Rgb] {
        let start = y.min(self.height - 1) * self.width;
        &self.pixels[start..start + self.width]
    }

    /// 24-bit top-down BMP, for the dashboard preview. Uncompressed on
    /// purpose: it is a few hundred KB over localhost, and needs no
    /// deflate at runtime.
    pub fn to_bmp(&self) -> Vec<u8> {
        const HEADERS: u32 = 14 + 40;
        const PIXELS_PER_METRE: i32 = 2835; // 72 dpi
        let row_bytes = (self.width * 3).div_ceil(4) * 4;
        let image_bytes = (row_bytes * self.height) as u32;
        let mut out = Vec::with_capacity((HEADERS + image_bytes) as usize);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(HEADERS + image_bytes).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&HEADERS.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(self.width as i32).to_le_bytes());
        out.extend_from_slice(&(-(self.height as i32)).to_le_bytes()); // negative: top-down
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        out.extend_from_slice(&image_bytes.to_le_bytes());
        out.extend_from_slice(&PIXELS_PER_METRE.to_le_bytes());
        out.extend_from_slice(&PIXELS_PER_METRE.to_le_bytes());
        out.extend_from_slice(&[0; 8]); // palette counts
        for row in self.pixels.chunks(self.width) {
            let start = out.len();
            for pixel in row {
                out.extend_from_slice(&[pixel.b, pixel.g, pixel.r]);
            }
            out.resize(start + row_bytes, 0);
        }
        out
    }
}

/// Planar 4:2:0 frame at the coded size (multiples of 16).
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
const KR: f32 = 0.2126;
const KB: f32 = 0.0722;

fn luma_linear(color: Rgb) -> f32 {
    let (r, g, b) = (color.r as f32, color.g as f32, color.b as f32);
    KR * r + (1.0 - KR - KB) * g + KB * b
}

fn luma(color: Rgb) -> u8 {
    to_sample(16.0 + luma_linear(color) * 219.0 / 255.0)
}

/// U and V samples for a 2x2 block of pixels: their chroma, averaged.
fn chroma_samples(quad: [Rgb; 4]) -> (u8, u8) {
    let (mut u_sum, mut v_sum) = (0.0, 0.0);
    for pixel in quad {
        let (u, v) = chroma(pixel);
        u_sum += u;
        v_sum += v;
    }
    (
        to_sample(128.0 + u_sum / 4.0),
        to_sample(128.0 + v_sum / 4.0),
    )
}

/// Chroma offsets from 128, before averaging.
fn chroma(color: Rgb) -> (f32, f32) {
    let y = luma_linear(color);
    let u = (color.b as f32 - y) / (2.0 * (1.0 - KB)) * 224.0 / 255.0;
    let v = (color.r as f32 - y) / (2.0 * (1.0 - KR)) * 224.0 / 255.0;
    (u, v)
}

/// Round into 1..=254: 0 and 255 are never needed in limited range, and
/// keeping clear of 0 keeps I_PCM samples valid for strict decoders.
fn to_sample(value: f32) -> u8 {
    value.round().clamp(1.0, 254.0) as u8
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
    fn bmp_has_headers_padding_and_bgr_order() {
        let mut canvas = Canvas::new(3, 2, Rgb::new(0, 0, 0));
        canvas.fill_rect(0, 0, 1, 1, Rgb::new(10, 20, 30));
        let bmp = canvas.to_bmp();
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(bmp.len(), 54 + 12 * 2, "3 px rows pad from 9 to 12 bytes");
        assert_eq!(&bmp[54..57], &[30, 20, 10], "first pixel, blue first");
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
