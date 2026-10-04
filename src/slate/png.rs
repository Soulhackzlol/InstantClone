//! Minimal PNG writer for the dashboard preview of the reconnect screen.
//!
//! The screens are flat colour with a little anti-aliased text, so rows
//! filtered against their neighbours ("Sub" or "Up", whichever leaves
//! smaller values) are almost all zeros. Deflating those needs no real
//! LZ77 search: repeats of the previous byte (distance 1 back-references)
//! in one fixed-Huffman block squeeze a 24-frame preview strip to a few
//! hundred KB, where the uncompressed bitmap was 9 MB.
//!
//! Format references: PNG (RFC 2083), zlib (RFC 1950), deflate (RFC 1951).

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const BYTES_PER_PIXEL: usize = 3;
const FILTER_SUB: u8 = 1;
const FILTER_UP: u8 = 2;
/// Deflate's longest match and shortest one worth sending.
const MAX_MATCH: usize = 258;
const MIN_MATCH: usize = 3;
/// zlib header for deflate with a 32 KB window and no preset dictionary;
/// the pair is a multiple of 31, as zlib requires.
const ZLIB_HEADER: [u8; 2] = [0x78, 0x01];

/// Encode `rgb` (rows of `width` pixels, 3 bytes each, top to bottom) as
/// an 8-bit RGB PNG.
pub fn encode(width: usize, height: usize, rgb: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgb.len(),
        width * height * BYTES_PER_PIXEL,
        "pixel data size"
    );
    let mut out = SIGNATURE.to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    // 8 bits per channel, colour type 2 (RGB), default compression and
    // filter methods, no interlacing.
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    write_chunk(&mut out, b"IHDR", &header);
    write_chunk(&mut out, b"IDAT", &zlib(&filtered_rows(width, rgb)));
    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// Each row prefixed by its filter type, filtered with whichever of Sub
/// (difference from the pixel to the left) and Up (from the pixel above)
/// leaves the smaller values: the usual PNG heuristic.
fn filtered_rows(width: usize, rgb: &[u8]) -> Vec<u8> {
    let stride = width * BYTES_PER_PIXEL;
    let mut out = Vec::with_capacity(rgb.len() + rgb.len() / stride.max(1));
    let mut sub = vec![0u8; stride];
    let mut up = vec![0u8; stride];
    for (index, row) in rgb.chunks(stride).enumerate() {
        for x in 0..stride {
            let left = if x >= BYTES_PER_PIXEL {
                row[x - BYTES_PER_PIXEL]
            } else {
                0
            };
            let above = if index > 0 {
                rgb[(index - 1) * stride + x]
            } else {
                0
            };
            sub[x] = row[x].wrapping_sub(left);
            up[x] = row[x].wrapping_sub(above);
        }
        let cost =
            |bytes: &[u8]| -> u64 { bytes.iter().map(|b| (*b as i8).unsigned_abs() as u64).sum() };
        let (filter, bytes) = if cost(&up) < cost(&sub) {
            (FILTER_UP, &up)
        } else {
            (FILTER_SUB, &sub)
        };
        out.push(filter);
        out.extend_from_slice(bytes);
    }
    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// zlib stream holding one fixed-Huffman deflate block.
fn zlib(data: &[u8]) -> Vec<u8> {
    let mut bits = BitWriter::default();
    bits.write(1, 1); // BFINAL: this is the last block
    bits.write(1, 2); // BTYPE 01: fixed Huffman codes
    let mut i = 0;
    while i < data.len() {
        let byte = data[i];
        bits.literal(byte);
        i += 1;
        // Repeats of this byte go out as distance-1 back-references.
        let mut run = data[i..].iter().take_while(|b| **b == byte).count();
        while run >= MIN_MATCH {
            let length = run.min(MAX_MATCH);
            bits.length(length);
            bits.huffman(0, 5); // distance code 0: distance 1, no extra bits
            i += length;
            run -= length;
        }
    }
    bits.huffman(0, 7); // end-of-block (code 256)
    let mut out = ZLIB_HEADER.to_vec();
    out.extend_from_slice(&bits.finish());
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Deflate's bit order: values fill each byte from the least significant
/// bit, while Huffman codes go most significant bit first.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    buffer: u64,
    count: u32,
}

impl BitWriter {
    fn write(&mut self, value: u32, width: u32) {
        self.buffer |= u64::from(value) << self.count;
        self.count += width;
        while self.count >= 8 {
            self.bytes.push(self.buffer as u8);
            self.buffer >>= 8;
            self.count -= 8;
        }
    }

    fn huffman(&mut self, code: u32, width: u32) {
        self.write(code.reverse_bits() >> (32 - width), width);
    }

    /// A literal byte in the fixed code (RFC 1951 3.2.6).
    fn literal(&mut self, byte: u8) {
        match byte {
            0..=143 => self.huffman(0x30 + u32::from(byte), 8),
            _ => self.huffman(0x190 + u32::from(byte) - 144, 9),
        }
    }

    /// A match length (3..=258): its length code, then its extra bits.
    fn length(&mut self, length: usize) {
        const BASE: [usize; 29] = [
            3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99,
            115, 131, 163, 195, 227, 258,
        ];
        const EXTRA: [u32; 29] = [
            0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
        ];
        let index = BASE.iter().rposition(|base| *base <= length).unwrap_or(0);
        let code = 257 + index as u32;
        // Codes 257..279 are 7 bits (0000000 + n), 280..287 are 8 bits.
        if code <= 279 {
            self.huffman(code - 256, 7);
        } else {
            self.huffman(0xc0 + code - 280, 8);
        }
        self.write((length - BASE[index]) as u32, EXTRA[index]);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push(self.buffer as u8);
        }
        self.bytes
    }
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 bytes is the most that can be summed before b can overflow.
    for chunk in data.chunks(5552) {
        for byte in chunk {
            a += u32::from(*byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_their_reference_values() {
        // Standard check values for the ASCII string "123456789".
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"123456789"), 0x091e_01de);
    }

    #[test]
    fn a_flat_frame_compresses_to_almost_nothing() {
        let (width, height) = (480, 270);
        let rgb: Vec<u8> = [0x0e, 0x0f, 0x12].repeat(width * height);
        let png = encode(width, height, &rgb);
        assert_eq!(&png[..8], &SIGNATURE);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 480);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 270);
        // Each row costs about 13 bytes: its filter byte breaks the run of
        // zeros, which then takes a literal and a few 258-byte matches.
        assert!(
            png.len() < 4_000,
            "{} bytes for a flat 480x270 frame",
            png.len()
        );
    }

    /// Decode with ffmpeg and require every pixel back. Needs ffmpeg the
    /// same way the reconnect screen's own decode test does.
    #[test]
    fn ffmpeg_decodes_the_png_losslessly() {
        use std::process::{Command, Stdio};
        if !super::super::ffmpeg_is_available("the PNG decode check") {
            return;
        }
        let (width, height) = (37, 11);
        // Noise, runs and repeated rows, so every filter and match path runs.
        let rgb: Vec<u8> = (0..width * height * 3)
            .map(|i| match (i / 3) % width {
                0..=9 => 200,
                10..=19 => ((i * 7919) % 251) as u8,
                _ => (i / (width * 3)) as u8 * 20,
            })
            .collect();
        let path = std::env::temp_dir().join(format!("ic-png-test-{}.png", std::process::id()));
        std::fs::write(&path, encode(width, height, &rgb)).unwrap();
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .stderr(Stdio::inherit())
            .output()
            .unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(decoded.status.success(), "ffmpeg rejected the PNG");
        assert!(decoded.stdout == rgb, "decoded pixels differ");
    }
}
