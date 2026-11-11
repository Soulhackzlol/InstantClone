//! H.264 residuals for the screens with soft detail (gradients, glows):
//! the 4x4 integer transform, quantisation, the decoder's exact
//! reconstruction (spec 8.5), and CAVLC residual blocks (9.2).
//!
//! The encoder never trusts its own rounding: after quantising a block it
//! rebuilds it the way a decoder will and looks at that, so what it
//! predicts from is always what viewers have on screen.

use super::bitstream::BitWriter;
use super::cavlc_tables::{
    CHROMA_DC_COEFF_TOKEN_BITS, CHROMA_DC_COEFF_TOKEN_LEN, CHROMA_DC_TOTAL_ZEROS_BITS,
    CHROMA_DC_TOTAL_ZEROS_LEN, COEFF_TOKEN_BITS, COEFF_TOKEN_LEN, RUN_BEFORE_BITS, RUN_BEFORE_LEN,
    TOTAL_ZEROS_BITS, TOTAL_ZEROS_LEN,
};

/// A 4x4 block in raster order: index `4 * row + column`.
pub type Block4 = [i32; 16];

/// Zig-zag scan of a 4x4 block (Table 8-13), as raster indices.
pub const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// Where luma4x4BlkIdx 0..16 sits in its macroblock, as (column, row) in
/// 4-sample units (Figure 6-10).
pub const LUMA_BLOCK_AT: [(usize, usize); 16] = [
    (0, 0),
    (1, 0),
    (0, 1),
    (1, 1),
    (2, 0),
    (3, 0),
    (2, 1),
    (3, 1),
    (0, 2),
    (1, 2),
    (0, 3),
    (1, 3),
    (2, 2),
    (3, 2),
    (2, 3),
    (3, 3),
];

/// normAdjust4x4 (8.5.9) by QP % 6, then by position class.
const NORM_ADJUST: [[i32; 3]; 6] = [
    [10, 16, 13],
    [11, 18, 14],
    [13, 20, 16],
    [14, 23, 18],
    [16, 25, 20],
    [18, 29, 23],
];

/// The forward multipliers that pair with `NORM_ADJUST`, so quantising and
/// rescaling a coefficient returns it to its own scale.
const QUANT_FACTOR: [[i32; 3]; 6] = [
    [13107, 5243, 8066],
    [11916, 4660, 7490],
    [10082, 4194, 6554],
    [9362, 3647, 5825],
    [8192, 3355, 5243],
    [7282, 2893, 4559],
];

/// QPc for a chroma qPI of 30 and up (Table 8-15); below 30 they are equal.
const CHROMA_QP_FROM_30: [i32; 22] = [
    29, 30, 31, 32, 32, 33, 34, 34, 35, 35, 36, 36, 37, 37, 37, 38, 38, 38, 39, 39, 39, 39,
];

/// The chroma QP that goes with a luma QP (chroma_qp_index_offset is 0).
pub fn chroma_qp(luma_qp: i32) -> i32 {
    match luma_qp {
        qp if qp < 30 => qp,
        qp => CHROMA_QP_FROM_30[(qp - 30) as usize],
    }
}

/// Which of the three scaling classes a coefficient belongs to: both
/// coordinates even, both odd, or mixed.
fn position_class(index: usize) -> usize {
    match ((index / 4) % 2, (index % 4) % 2) {
        (0, 0) => 0,
        (1, 1) => 1,
        _ => 2,
    }
}

/// LevelScale4x4 with the flat scaling list every stream here uses.
fn level_scale(qp: i32, index: usize) -> i32 {
    16 * NORM_ADJUST[(qp % 6) as usize][position_class(index)]
}

/// The forward core transform of a 4x4 residual block.
fn forward_core(block: &Block4) -> Block4 {
    let pass = |a: [i32; 4]| {
        let (s03, s12, d03, d12) = (a[0] + a[3], a[1] + a[2], a[0] - a[3], a[1] - a[2]);
        [s03 + s12, 2 * d03 + d12, s03 - s12, d03 - 2 * d12]
    };
    let mut rows = [0; 16];
    for row in 0..4 {
        let out = pass([
            block[4 * row],
            block[4 * row + 1],
            block[4 * row + 2],
            block[4 * row + 3],
        ]);
        rows[4 * row..4 * row + 4].copy_from_slice(&out);
    }
    let mut result = [0; 16];
    for column in 0..4 {
        let out = pass([
            rows[column],
            rows[4 + column],
            rows[8 + column],
            rows[12 + column],
        ]);
        for row in 0..4 {
            result[4 * row + column] = out[row];
        }
    }
    result
}

/// The decoder's inverse transform of scaled coefficients (8.5.12.2),
/// rounding included: the residual samples it adds to the prediction.
fn inverse_core(scaled: &Block4) -> Block4 {
    let pass = |d: [i32; 4]| {
        let (e0, e1) = (d[0] + d[2], d[0] - d[2]);
        let (e2, e3) = ((d[1] >> 1) - d[3], d[1] + (d[3] >> 1));
        [e0 + e3, e1 + e2, e1 - e2, e0 - e3]
    };
    let mut rows = [0; 16];
    for row in 0..4 {
        let out = pass([
            scaled[4 * row],
            scaled[4 * row + 1],
            scaled[4 * row + 2],
            scaled[4 * row + 3],
        ]);
        rows[4 * row..4 * row + 4].copy_from_slice(&out);
    }
    let mut result = [0; 16];
    for column in 0..4 {
        let out = pass([
            rows[column],
            rows[4 + column],
            rows[8 + column],
            rows[12 + column],
        ]);
        for row in 0..4 {
            result[4 * row + column] = (out[row] + 32) >> 6;
        }
    }
    result
}

/// The 4x4 Hadamard transform the luma DC coefficients go through. It is
/// its own inverse up to scale, so both directions use it.
fn hadamard4(block: &Block4) -> Block4 {
    let pass = |a: [i32; 4]| {
        let (s03, s12, d03, d12) = (a[0] + a[3], a[1] + a[2], a[0] - a[3], a[1] - a[2]);
        [s03 + s12, d03 + d12, s03 - s12, d03 - d12]
    };
    let mut rows = [0; 16];
    for row in 0..4 {
        let out = pass([
            block[4 * row],
            block[4 * row + 1],
            block[4 * row + 2],
            block[4 * row + 3],
        ]);
        rows[4 * row..4 * row + 4].copy_from_slice(&out);
    }
    let mut result = [0; 16];
    for column in 0..4 {
        let out = pass([
            rows[column],
            rows[4 + column],
            rows[8 + column],
            rows[12 + column],
        ]);
        for row in 0..4 {
            result[4 * row + column] = out[row];
        }
    }
    result
}

/// The 2x2 transform of the chroma DC coefficients, also its own inverse
/// up to scale.
fn hadamard2(c: [i32; 4]) -> [i32; 4] {
    [
        c[0] + c[1] + c[2] + c[3],
        c[0] - c[1] + c[2] - c[3],
        c[0] + c[1] - c[2] - c[3],
        c[0] - c[1] - c[2] + c[3],
    ]
}

/// `value * factor`, rounded to a third and shifted down: the usual intra
/// quantiser, which leans towards zero to save bits.
fn quantize(value: i32, factor: i32, shift: i32) -> i32 {
    let rounding = (1i64 << shift) / 3;
    let level = ((i64::from(value.abs()) * i64::from(factor) + rounding) >> shift) as i32;
    if value < 0 {
        -level
    } else {
        level
    }
}

/// An AC coefficient as the decoder rescales it (8.5.12.1).
fn rescale_ac(level: i32, qp: i32, index: usize) -> i32 {
    let scaled = level * level_scale(qp, index);
    if qp >= 24 {
        scaled << (qp / 6 - 4)
    } else {
        (scaled + (1 << (3 - qp / 6))) >> (4 - qp / 6)
    }
}

/// The quantised coefficients of one 16x16 luma block coded as Intra16x16.
pub struct LumaLevels {
    /// The 4x4 blocks' DC terms after their Hadamard transform, laid out
    /// like the blocks themselves (raster, `4 * row + column`).
    pub dc: Block4,
    /// Each 4x4 block's coefficients by raster block position; index 0
    /// (the DC term) stays 0.
    pub ac: [Block4; 16],
}

impl LumaLevels {
    pub fn has_ac(&self) -> bool {
        self.ac.iter().flatten().any(|level| *level != 0)
    }
}

/// Transform and quantise a 16x16 residual (raster, 256 samples) at `qp`.
pub fn quantize_luma(residual: &[i32; 256], qp: i32) -> LumaLevels {
    let shift = 15 + qp / 6;
    let factors = &QUANT_FACTOR[(qp % 6) as usize];
    let mut dc_terms = [0; 16];
    let mut ac = [[0; 16]; 16];
    for block in 0..16 {
        let (block_row, block_column) = (block / 4, block % 4);
        let mut samples = [0; 16];
        for row in 0..4 {
            let start = (4 * block_row + row) * 16 + 4 * block_column;
            samples[4 * row..4 * row + 4].copy_from_slice(&residual[start..start + 4]);
        }
        let coefficients = forward_core(&samples);
        dc_terms[block] = coefficients[0];
        for index in 1..16 {
            ac[block][index] = quantize(coefficients[index], factors[position_class(index)], shift);
        }
    }
    // The 4x4 Hadamard transform multiplies a flat block's DC by 16, and the
    // decoder's rescaling of it undoes a quarter of that less than an AC
    // coefficient's does.
    let dc = hadamard4(&dc_terms).map(|term| quantize(term, factors[0], shift + 2));
    LumaLevels { dc, ac }
}

/// The 16x16 residual a decoder rebuilds from `levels` (8.5.2, 8.5.10).
pub fn rebuild_luma(levels: &LumaLevels, qp: i32) -> [i32; 256] {
    let scale = level_scale(qp, 0);
    let dc = hadamard4(&levels.dc).map(|term| {
        if qp >= 36 {
            (term * scale) << (qp / 6 - 6)
        } else {
            (term * scale + (1 << (5 - qp / 6))) >> (6 - qp / 6)
        }
    });
    let mut residual = [0; 256];
    for block in 0..16 {
        let (block_row, block_column) = (block / 4, block % 4);
        let mut scaled = [0; 16];
        scaled[0] = dc[block];
        for index in 1..16 {
            scaled[index] = rescale_ac(levels.ac[block][index], qp, index);
        }
        let samples = inverse_core(&scaled);
        for row in 0..4 {
            let start = (4 * block_row + row) * 16 + 4 * block_column;
            residual[start..start + 4].copy_from_slice(&samples[4 * row..4 * row + 4]);
        }
    }
    residual
}

/// The quantised coefficients of one 8x8 chroma block.
pub struct ChromaLevels {
    /// The four 4x4 blocks' DC terms after their 2x2 transform, raster.
    pub dc: [i32; 4],
    /// Each 4x4 block's coefficients, raster by block; index 0 stays 0.
    pub ac: [Block4; 4],
}

impl ChromaLevels {
    pub fn has_dc(&self) -> bool {
        self.dc.iter().any(|level| *level != 0)
    }

    pub fn has_ac(&self) -> bool {
        self.ac.iter().flatten().any(|level| *level != 0)
    }
}

/// Transform and quantise an 8x8 chroma residual (raster, 64 samples) at
/// the chroma QP `qp`.
pub fn quantize_chroma(residual: &[i32; 64], qp: i32) -> ChromaLevels {
    let shift = 15 + qp / 6;
    let factors = &QUANT_FACTOR[(qp % 6) as usize];
    let mut dc_terms = [0; 4];
    let mut ac = [[0; 16]; 4];
    for block in 0..4 {
        let (block_row, block_column) = (block / 2, block % 2);
        let mut samples = [0; 16];
        for row in 0..4 {
            let start = (4 * block_row + row) * 8 + 4 * block_column;
            samples[4 * row..4 * row + 4].copy_from_slice(&residual[start..start + 4]);
        }
        let coefficients = forward_core(&samples);
        dc_terms[block] = coefficients[0];
        for index in 1..16 {
            ac[block][index] = quantize(coefficients[index], factors[position_class(index)], shift);
        }
    }
    // The 2x2 transform multiplies a flat block's DC by 4: half the luma
    // case, so one bit less.
    let dc = hadamard2(dc_terms).map(|term| quantize(term, factors[0], shift + 1));
    ChromaLevels { dc, ac }
}

/// The 8x8 chroma residual a decoder rebuilds from `levels` (8.5.11).
pub fn rebuild_chroma(levels: &ChromaLevels, qp: i32) -> [i32; 64] {
    let scale = level_scale(qp, 0);
    let dc = hadamard2(levels.dc).map(|term| ((term * scale) << (qp / 6)) >> 5);
    let mut residual = [0; 64];
    for block in 0..4 {
        let (block_row, block_column) = (block / 2, block % 2);
        let mut scaled = [0; 16];
        scaled[0] = dc[block];
        for index in 1..16 {
            scaled[index] = rescale_ac(levels.ac[block][index], qp, index);
        }
        let samples = inverse_core(&scaled);
        for row in 0..4 {
            let start = (4 * block_row + row) * 8 + 4 * block_column;
            residual[start..start + 4].copy_from_slice(&samples[4 * row..4 * row + 4]);
        }
    }
    residual
}

/// Which coeff_token table a block uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenTable {
    /// By nC, the mean coefficient count of the blocks left and above.
    Neighbours(u32),
    /// The chroma DC block's own table (nC = -1).
    ChromaDc,
}

/// A level too large for the 12-bit escape Baseline profile allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelTooLarge;

/// residual_block_cavlc (7.3.5.3.2) for `levels` in scan order, lowest
/// frequency first. Returns TotalCoeff, which the blocks to the right and
/// below need for their own coeff_token.
pub fn write_block(
    w: &mut BitWriter,
    levels: &[i32],
    table: TokenTable,
) -> Result<u32, LevelTooLarge> {
    // Highest frequency first, as the syntax codes them.
    let coded: Vec<(usize, i32)> = levels
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, level)| *level != 0)
        .rev()
        .collect();
    let total = coded.len();
    let trailing_ones = coded
        .iter()
        .take(3)
        .take_while(|(_, level)| level.abs() == 1)
        .count();
    let token = 4 * total + trailing_ones;
    match table {
        TokenTable::ChromaDc => w.bits(
            u32::from(CHROMA_DC_COEFF_TOKEN_BITS[token]),
            CHROMA_DC_COEFF_TOKEN_LEN[token],
        ),
        TokenTable::Neighbours(n_c) => {
            let which = match n_c {
                0..=1 => 0,
                2..=3 => 1,
                4..=7 => 2,
                _ => 3,
            };
            w.bits(
                u32::from(COEFF_TOKEN_BITS[which][token]),
                COEFF_TOKEN_LEN[which][token],
            );
        }
    }
    if total == 0 {
        return Ok(0);
    }

    let mut suffix_length = u8::from(total > 10 && trailing_ones < 3);
    for (order, (_, level)) in coded.iter().enumerate() {
        if order < trailing_ones {
            w.bit(*level < 0); // trailing_ones_sign_flag
            continue;
        }
        let magnitude = level.unsigned_abs();
        let mut code = 2 * magnitude - 2 + u32::from(*level < 0);
        // The first level after fewer than three trailing ones can't be
        // +-1, so its code starts one step in.
        if order == trailing_ones && trailing_ones < 3 {
            code -= 2;
        }
        write_level(w, code, suffix_length)?;
        if suffix_length == 0 {
            suffix_length = 1;
        }
        if magnitude > (3 << (suffix_length - 1)) && suffix_length < 6 {
            suffix_length += 1;
        }
    }

    let last_position = coded[0].0;
    let mut zeros_left = last_position + 1 - total;
    if total < levels.len() {
        match table {
            TokenTable::ChromaDc => w.bits(
                u32::from(CHROMA_DC_TOTAL_ZEROS_BITS[total - 1][zeros_left]),
                CHROMA_DC_TOTAL_ZEROS_LEN[total - 1][zeros_left],
            ),
            TokenTable::Neighbours(_) => w.bits(
                u32::from(TOTAL_ZEROS_BITS[total - 1][zeros_left]),
                TOTAL_ZEROS_LEN[total - 1][zeros_left],
            ),
        }
    }
    // run_before for every coefficient but the lowest, while zeros remain.
    for pair in coded.windows(2) {
        if zeros_left == 0 {
            break;
        }
        let run = pair[0].0 - pair[1].0 - 1;
        let which = zeros_left.min(7) - 1;
        w.bits(
            u32::from(RUN_BEFORE_BITS[which][run]),
            RUN_BEFORE_LEN[which][run],
        );
        zeros_left -= run;
    }
    Ok(total as u32)
}

/// level_prefix and level_suffix for one levelCode (9.2.2.1).
fn write_level(w: &mut BitWriter, code: u32, suffix_length: u8) -> Result<(), LevelTooLarge> {
    const ESCAPE_BITS: u8 = 12;
    let (prefix, suffix, suffix_bits) = if suffix_length == 0 {
        match code {
            0..=13 => (code, 0, 0),
            14..=29 => (14, code - 14, 4),
            _ => (15, code - 30, ESCAPE_BITS),
        }
    } else if (code >> suffix_length) < 15 {
        (
            code >> suffix_length,
            code & ((1 << suffix_length) - 1),
            suffix_length,
        )
    } else {
        (15, code - (15 << suffix_length), ESCAPE_BITS)
    };
    if suffix >= 1 << ESCAPE_BITS {
        return Err(LevelTooLarge);
    }
    for _ in 0..prefix {
        w.bit(false);
    }
    w.bit(true);
    w.bits(suffix, suffix_bits);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_of(levels: &[i32], table: TokenTable) -> String {
        let mut w = BitWriter::new();
        write_block(&mut w, levels, table).unwrap();
        let length = w.bit_len();
        let bytes = w.into_aligned_bytes();
        (0..length)
            .map(|at| {
                if (bytes[at / 8] >> (7 - at % 8)) & 1 == 1 {
                    '1'
                } else {
                    '0'
                }
            })
            .collect()
    }

    /// The worked example CAVLC texts use (Richardson, "H.264 and MPEG-4
    /// Video Compression", 6.4.12).
    #[test]
    fn a_block_codes_as_the_textbook_example_does() {
        // Scan order: 0, 3, 0, 1, -1, -1, 0, 1, then zeros.
        let mut levels = [0; 16];
        levels[..8].copy_from_slice(&[0, 3, 0, 1, -1, -1, 0, 1]);
        // coeff_token 0000100 (TotalCoeff 5, TrailingOnes 3), signs 0 1 1,
        // level +1 as 1, level +3 as 0010, total_zeros 3 as 111,
        // run_before 1, 0, 0, 1 as 10, 1, 1, 01.
        assert_eq!(
            bits_of(&levels, TokenTable::Neighbours(0)),
            "000010001110010111101101"
        );
    }

    #[test]
    fn an_empty_block_is_just_its_token() {
        assert_eq!(bits_of(&[0; 16], TokenTable::Neighbours(0)), "1");
        assert_eq!(bits_of(&[0; 16], TokenTable::Neighbours(2)), "11");
        assert_eq!(bits_of(&[0; 16], TokenTable::Neighbours(5)), "1111");
        assert_eq!(bits_of(&[0; 16], TokenTable::Neighbours(9)), "000011");
        assert_eq!(bits_of(&[0; 4], TokenTable::ChromaDc), "01");
    }

    /// Quantising and rebuilding a smooth ramp at a fine QP returns it to
    /// within a level: the transform pair and the scaling tables agree.
    #[test]
    fn a_ramp_survives_a_fine_quantiser() {
        let mut residual = [0; 256];
        for (index, sample) in residual.iter_mut().enumerate() {
            *sample = (index % 16) as i32 * 2 + (index / 16) as i32 - 20;
        }
        let rebuilt = rebuild_luma(&quantize_luma(&residual, 12), 12);
        let worst = residual
            .iter()
            .zip(&rebuilt)
            .map(|(a, b)| (a - b).abs())
            .max()
            .unwrap();
        assert!(worst <= 1, "worst error {worst}");

        let mut chroma = [0; 64];
        for (index, sample) in chroma.iter_mut().enumerate() {
            *sample = (index % 8) as i32 * 3 - (index / 8) as i32;
        }
        let rebuilt = rebuild_chroma(&quantize_chroma(&chroma, 12), 12);
        let worst = chroma
            .iter()
            .zip(&rebuilt)
            .map(|(a, b)| (a - b).abs())
            .max()
            .unwrap();
        assert!(worst <= 1, "worst chroma error {worst}");
    }

    #[test]
    fn a_level_past_the_escape_is_refused() {
        let mut w = BitWriter::new();
        let huge = [5000, 0, 0, 0];
        assert_eq!(
            write_block(&mut w, &huge, TokenTable::ChromaDc),
            Err(LevelTooLarge)
        );
    }
}
