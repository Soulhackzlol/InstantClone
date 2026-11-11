//! HEVC residuals for the screens with soft detail: the 4 to 32 point
//! transforms (and the 4 point DST of small intra luma blocks),
//! quantisation, the decoder's exact reconstruction
//! (H.265 8.6), and residual_coding() written with CABAC (7.3.8.11).
//!
//! As on the H.264 side, the encoder rebuilds every block the way a
//! decoder will and judges it by that, so what it predicts from is always
//! what viewers have on screen.

use super::cabac::{CabacWriter, Context};

/// levelScale (8.6.3), by QP % 6.
const LEVEL_SCALE: [i64; 6] = [40, 45, 51, 57, 64, 72];
/// The forward multipliers that pair with `LEVEL_SCALE`.
const QUANT_SCALE: [i64; 6] = [26214, 23302, 20560, 18396, 16384, 14564];
/// The flat scaling factor m (scaling lists are off).
const FLAT_SCALE: i64 = 16;
/// QPc for a chroma qPi of 30 to 43 (Table 8-10).
const CHROMA_QP_FROM_30: [i32; 14] = [29, 30, 31, 32, 33, 33, 34, 34, 35, 35, 36, 36, 37, 37];

/// The chroma QP that goes with a luma QP (4:2:0, no offsets).
pub fn chroma_qp(luma_qp: i32) -> i32 {
    match luma_qp {
        qp if qp < 30 => qp,
        qp if qp > 43 => qp - 6,
        qp => CHROMA_QP_FROM_30[(qp - 30) as usize],
    }
}

/// The basis row of frequency `k` for an `size`-point transform.
fn basis(size: usize, k: usize, i: usize) -> i64 {
    i64::from(BASIS[k * (32 / size)][i])
}

/// The transform a block takes (8.6.4.2): the DCT, except a 4x4 block of
/// an intra unit's luma, which takes the DST. Both scale and shift alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kernel {
    Dct,
    Dst,
}

impl Kernel {
    /// The kernel of a `size` x `size` block, of intra luma or not.
    pub fn of(size: usize, is_intra_luma: bool) -> Self {
        if size == 4 && is_intra_luma {
            Kernel::Dst
        } else {
            Kernel::Dct
        }
    }

    /// Row `k` (a frequency), column `i` of the `size`-point matrix.
    fn entry(self, size: usize, k: usize, i: usize) -> i64 {
        match self {
            Kernel::Dct => basis(size, k, i),
            Kernel::Dst => i64::from(DST_4X4[k][i]),
        }
    }

    /// One dimension of the forward transform. The DST is only 4 points,
    /// so it multiplies by its matrix.
    fn forward_1d(self, input: &[i64], output: &mut [i64]) {
        match self {
            Kernel::Dct => forward_1d(input, output),
            Kernel::Dst => {
                for (k, value) in output.iter_mut().enumerate() {
                    *value = (0..4).map(|i| self.entry(4, k, i) * input[i]).sum();
                }
            }
        }
    }
}

/// The 4-point DST's matrix (transMatrix in 8.6.4.2), rows by frequency.
const DST_4X4: [[i8; 4]; 4] = [
    [29, 55, 74, 84],
    [74, 74, 0, -74],
    [84, -29, -74, 55],
    [55, -84, 74, -29],
];

/// The forward transform of a `size` x `size` residual (raster, row by
/// row): the reference encoder's two passes, whose shifts leave the
/// coefficients at the scale `QUANT_SCALE` expects. They come back raster
/// too: `[y * size + x]` has horizontal frequency x and vertical frequency
/// y. It doesn't depend on the QP, so a block tried at several QPs is
/// transformed once.
pub fn transform(residual: &[i32], size: usize, kernel: Kernel) -> Vec<i64> {
    debug_assert!(kernel == Kernel::Dct || size == 4, "the DST is 4x4 only");
    let log2 = size.trailing_zeros() as i64;
    let first_shift = log2 - 1;
    let second_shift = log2 + 6;
    let mut line = [0i64; 32];
    let mut sums = [0i64; 32];
    let mut rows = vec![0i64; size * size];
    for y in 0..size {
        for (i, value) in line[..size].iter_mut().enumerate() {
            *value = i64::from(residual[y * size + i]);
        }
        kernel.forward_1d(&line[..size], &mut sums[..size]);
        for k in 0..size {
            rows[y * size + k] = (sums[k] + (1 << (first_shift - 1))) >> first_shift;
        }
    }
    let mut coefficients = vec![0i64; size * size];
    for x in 0..size {
        for (j, value) in line[..size].iter_mut().enumerate() {
            *value = rows[j * size + x];
        }
        kernel.forward_1d(&line[..size], &mut sums[..size]);
        for k in 0..size {
            coefficients[k * size + x] = (sums[k] + (1 << (second_shift - 1))) >> second_shift;
        }
    }
    coefficients
}

/// `output[k]` = the sum over `i` of basis row `k` times `input[i]`: one
/// dimension of the transform, by partial butterfly. The even rows of an
/// HEVC transform are symmetric and are the rows of the half-size one, the
/// odd rows antisymmetric, so each size is a half-size transform of the
/// ends' sums plus a quarter of the products for their differences. The
/// same integer sums as multiplying by the matrix, in a third of the work
/// at 32 points; the encoder spent most of a loop's time here.
fn forward_1d(input: &[i64], output: &mut [i64]) {
    let size = input.len();
    if size == 1 {
        output[0] = basis(1, 0, 0) * input[0];
        return;
    }
    let half = size / 2;
    let (mut sums, mut differences, mut even) = ([0i64; 16], [0i64; 16], [0i64; 16]);
    for i in 0..half {
        sums[i] = input[i] + input[size - 1 - i];
        differences[i] = input[i] - input[size - 1 - i];
    }
    forward_1d(&sums[..half], &mut even[..half]);
    let row_step = 32 / size;
    for m in 0..half {
        output[2 * m] = even[m];
        let row = &BASIS[(2 * m + 1) * row_step][..half];
        output[2 * m + 1] = row
            .iter()
            .zip(&differences[..half])
            .map(|(b, d)| i64::from(*b) * d)
            .sum();
    }
}

/// The levels of `coefficients` (from `transform`) at `qp`.
pub fn quantize(coefficients: &[i64], size: usize, qp: i32) -> Vec<i32> {
    let log2 = size.trailing_zeros() as i64;
    let qbits = 21 + i64::from(qp / 6) - log2;
    // Intra rounding: a third, leaning towards zero to save bits.
    let rounding = 171 << (qbits - 9);
    let factor = QUANT_SCALE[(qp % 6) as usize];
    coefficients
        .iter()
        .map(|coefficient| {
            let level = ((coefficient.abs() * factor + rounding) >> qbits) as i32;
            if *coefficient < 0 {
                -level
            } else {
                level
            }
        })
        .collect()
}

/// The residual a decoder rebuilds from `levels` (8.6.2 to 8.6.4).
pub fn rebuild(levels: &[i32], size: usize, qp: i32, kernel: Kernel) -> Vec<i32> {
    let log2 = size.trailing_zeros() as i64;
    let shift = 8 + log2 - 5;
    let scale = LEVEL_SCALE[(qp % 6) as usize] << (qp / 6);
    let clip16 = |value: i64| value.clamp(-32768, 32767);
    let scaled: Vec<i64> = levels
        .iter()
        .map(|level| clip16((i64::from(*level) * scale * FLAT_SCALE + (1 << (shift - 1))) >> shift))
        .collect();
    // Columns first (vertical frequencies), then rows. Most coefficients
    // of a smooth block are zero: each sum runs over the ones that aren't,
    // which gives the same result as the full sum, far faster.
    let nonzero_in_column =
        |x: usize| -> Vec<usize> { (0..size).filter(|k| scaled[k * size + x] != 0).collect() };
    let live_columns: Vec<(usize, Vec<usize>)> = (0..size)
        .map(|x| (x, nonzero_in_column(x)))
        .filter(|(_, rows)| !rows.is_empty())
        .collect();
    let mut columns = vec![0i64; size * size];
    for (x, frequencies) in &live_columns {
        for y in 0..size {
            let sum: i64 = frequencies
                .iter()
                .map(|k| kernel.entry(size, *k, y) * scaled[k * size + x])
                .sum();
            columns[y * size + x] = clip16((sum + 64) >> 7);
        }
    }
    let mut residual = vec![0; size * size];
    for y in 0..size {
        for x in 0..size {
            let sum: i64 = live_columns
                .iter()
                .map(|(k, _)| kernel.entry(size, *k, x) * columns[y * size + k])
                .sum();
            residual[y * size + x] = ((sum + 2048) >> 12) as i32;
        }
    }
    residual
}

/// A rough count of the bits `levels` cost, to weigh them against raw
/// samples before coding anything.
pub fn estimated_bits(levels: &[i32]) -> usize {
    levels
        .iter()
        .filter(|level| **level != 0)
        .map(|level| 3 + 2 * (32 - level.unsigned_abs().leading_zeros()) as usize)
        .sum::<usize>()
        + 16
}

/// The context variables residual_coding() reads, by ctxInc. initValues
/// are from H.265 Tables 9-26 to 9-31: initType 0 for I slices, 1 for P.
pub struct ResidualContexts {
    last_x: [Context; 18],
    last_y: [Context; 18],
    coded_sub_block: [Context; 4],
    significant: [Context; 42],
    greater1: [Context; 24],
    greater2: [Context; 6],
}

#[rustfmt::skip]
const LAST_PREFIX_INIT: [[u8; 18]; 2] = [
    [110, 110, 124, 125, 140, 153, 125, 127, 140, 109, 111, 143, 127, 111, 79, 108, 123, 63],
    [125, 110, 94, 110, 95, 79, 125, 111, 110, 78, 110, 111, 111, 95, 94, 108, 123, 108],
];
const CODED_SUB_BLOCK_INIT: [[u8; 4]; 2] = [[91, 171, 134, 141], [121, 140, 61, 154]];
#[rustfmt::skip]
const SIGNIFICANT_INIT: [[u8; 42]; 2] = [
    [
        111, 111, 125, 110, 110, 94, 124, 108, 124, 107, 125, 141, 179, 153,
        125, 107, 125, 141, 179, 153, 125, 107, 125, 141, 179, 153, 125, 140,
        139, 182, 182, 152, 136, 152, 136, 153, 136, 139, 111, 136, 139, 111,
    ],
    [
        155, 154, 139, 153, 139, 123, 123, 63, 153, 166, 183, 140, 136, 153,
        154, 166, 183, 140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 170,
        153, 123, 123, 107, 121, 107, 121, 167, 151, 183, 140, 151, 183, 140,
    ],
];
#[rustfmt::skip]
const GREATER1_INIT: [[u8; 24]; 2] = [
    [
        140, 92, 137, 138, 140, 152, 138, 139, 153, 74, 149, 92, 139, 107,
        122, 152, 140, 179, 166, 182, 140, 227, 122, 197,
    ],
    [
        154, 196, 196, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121,
        136, 137, 169, 194, 166, 167, 154, 167, 137, 182,
    ],
];
const GREATER2_INIT: [[u8; 6]; 2] = [
    [138, 153, 136, 167, 152, 152],
    [107, 167, 91, 122, 107, 167],
];

impl ResidualContexts {
    pub fn new(is_p_slice: bool, slice_qp: i32) -> Self {
        let set = usize::from(is_p_slice);
        Self {
            last_x: contexts(&LAST_PREFIX_INIT[set], slice_qp),
            last_y: contexts(&LAST_PREFIX_INIT[set], slice_qp),
            coded_sub_block: contexts(&CODED_SUB_BLOCK_INIT[set], slice_qp),
            significant: contexts(&SIGNIFICANT_INIT[set], slice_qp),
            greater1: contexts(&GREATER1_INIT[set], slice_qp),
            greater2: contexts(&GREATER2_INIT[set], slice_qp),
        }
    }
}

fn contexts<const N: usize>(init_values: &[u8; N], slice_qp: i32) -> [Context; N] {
    init_values.map(|value| Context::new(value, slice_qp))
}

/// The up-right diagonal scan of a `size` x `size` grid (6.5.3), as
/// (x, y) pairs.
fn diagonal_scan(size: usize) -> Vec<(usize, usize)> {
    let mut scan = Vec::with_capacity(size * size);
    let (mut x, mut y) = (0usize, 0isize);
    while scan.len() < size * size {
        while y >= 0 {
            if x < size && (y as usize) < size {
                scan.push((x, y as usize));
            }
            y -= 1;
            x += 1;
        }
        y = x as isize;
        x = 0;
    }
    scan
}

/// The last position's prefix (Table 9-39 binarisation) and its suffix
/// with the suffix's length.
fn last_position_parts(position: usize) -> (u32, u32, u8) {
    if position < 4 {
        return (position as u32, 0, 0);
    }
    let group = usize::BITS - 1 - position.leading_zeros();
    let upper_half = position >= 3 << (group - 1);
    let prefix = 2 * group + u32::from(upper_half);
    let base = (1usize << (group - 1)) * (2 + usize::from(upper_half));
    (prefix, (position - base) as u32, (group - 1) as u8)
}

/// residual_coding() for one transform block whose `levels` are not all
/// zero, in the diagonal scan. Small intra blocks would take another scan
/// in the vertical and horizontal modes, so the encoder never splits an
/// intra unit coded in those (see `hevc::IntraMode::splits`).
pub fn write(
    cabac: &mut CabacWriter,
    contexts: &mut ResidualContexts,
    levels: &[i32],
    size: usize,
    is_chroma: bool,
) {
    let log2 = size.trailing_zeros() as usize;
    debug_assert!(
        (2..=5).contains(&log2) && !(is_chroma && log2 == 5),
        "luma blocks are 4 to 32, chroma 4 to 16"
    );
    let groups = size / 4;
    let group_scan = diagonal_scan(groups);
    let within = diagonal_scan(4);
    let level_at = |group: usize, n: usize| {
        let (gx, gy) = group_scan[group];
        let (x, y) = within[n];
        levels[(4 * gy + y) * size + 4 * gx + x]
    };
    let last = (0..groups * groups * 16)
        .rev()
        .find(|at| level_at(at / 16, at % 16) != 0)
        .expect("residual_coding needs a level that is not zero");
    let (last_group, last_n) = (last / 16, last % 16);

    // last_sig_coeff_x/y_prefix, then their suffixes.
    let (ctx_offset, ctx_shift) = if is_chroma {
        (15, log2 - 2)
    } else {
        (3 * (log2 - 2) + ((log2 - 1) >> 2), (log2 + 1) >> 2)
    };
    let (gx, gy) = group_scan[last_group];
    let (x, y) = within[last_n];
    let parts = [
        last_position_parts(4 * gx + x),
        last_position_parts(4 * gy + y),
    ];
    let max_prefix = (2 * log2 - 1) as u32;
    for (axis, (prefix, _, _)) in parts.iter().enumerate() {
        let axis_contexts = if axis == 0 {
            &mut contexts.last_x
        } else {
            &mut contexts.last_y
        };
        for bin in 0..max_prefix.min(*prefix + 1) {
            let ctx = &mut axis_contexts[ctx_offset + (bin as usize >> ctx_shift)];
            cabac.decision(ctx, bin < *prefix);
        }
    }
    for (prefix, suffix, length) in parts {
        if prefix > 3 {
            cabac.bypass_bits(suffix, length);
        }
    }

    let mut coded = vec![false; groups * groups];
    let mut greater1_ctx = 1;
    for group in (0..=last_group).rev() {
        let (gx, gy) = group_scan[group];
        let right = gx + 1 < groups && coded[gy * groups + gx + 1];
        let below = gy + 1 < groups && coded[(gy + 1) * groups + gx];
        let has_levels = (0..16).any(|n| level_at(group, n) != 0);
        let mut infer_dc = false;
        if group < last_group && group > 0 {
            let ctx = usize::from(right || below) + if is_chroma { 2 } else { 0 };
            cabac.decision(&mut contexts.coded_sub_block[ctx], has_levels);
            coded[gy * groups + gx] = has_levels;
            infer_dc = true;
        } else {
            coded[gy * groups + gx] = true;
        }
        if !coded[gy * groups + gx] {
            continue;
        }

        // significant_coeff_flag, highest scan position first. A 4x4
        // block takes each position's context from a table instead.
        let previous = usize::from(right) + 2 * usize::from(below);
        let plane_offset = if is_chroma { 27 } else { 0 };
        let offset = plane_offset
            + match (is_chroma, log2) {
                (false, 3) => 9 + if group > 0 { 3 } else { 0 },
                (false, _) => 21 + if group > 0 { 3 } else { 0 },
                (true, 3) => 9,
                (true, _) => 12,
            };
        let significance_ctx = |x: usize, y: usize| {
            if log2 == 2 {
                plane_offset + SIGNIFICANCE_4X4[4 * y + x]
            } else {
                offset + significance_increment(previous, x, y)
            }
        };
        let mut significant: Vec<usize> = Vec::with_capacity(16);
        let start = if group == last_group {
            significant.push(last_n);
            last_n as isize - 1
        } else {
            15
        };
        for n in (1..=start.max(0) as usize).rev() {
            let (x, y) = within[n];
            let is_set = level_at(group, n) != 0;
            let ctx = significance_ctx(x, y);
            cabac.decision(&mut contexts.significant[ctx], is_set);
            if is_set {
                significant.push(n);
                infer_dc = false;
            }
        }
        if start >= 0 {
            if infer_dc {
                significant.push(0);
            } else {
                let ctx = match (group, is_chroma) {
                    (0, false) => 0,
                    (0, true) => 27,
                    _ => offset + 2,
                };
                let is_set = level_at(group, 0) != 0;
                cabac.decision(&mut contexts.significant[ctx], is_set);
                if is_set {
                    significant.push(0);
                }
            }
        }
        write_levels(cabac, contexts, &significant, |n| level_at(group, n), {
            let mut set = if group > 0 && !is_chroma { 2 } else { 0 };
            if group != last_group && greater1_ctx == 0 {
                set += 1;
            }
            (set, is_chroma, &mut greater1_ctx)
        });
    }
}

/// sigCtx of each position of a 4x4 block, raster (ctxIdxMap, 9.3.4.2.5).
/// The last position is never coded: a block's last level is signalled.
const SIGNIFICANCE_4X4: [usize; 16] = [0, 1, 4, 5, 2, 3, 4, 5, 6, 6, 8, 8, 7, 7, 8, 8];

/// sigCtx inside a sub-block (9.3.4.2.5) for a position other than its
/// first, from which neighbouring sub-blocks have levels.
fn significance_increment(previous: usize, x: usize, y: usize) -> usize {
    match previous {
        0 if x + y < 3 => 1,
        0 => 0,
        1 => 2 - y.min(2),
        2 => 2 - x.min(2),
        _ => 2,
    }
}

/// The levels of one sub-block's significant positions (`significant`,
/// highest scan position first): greater-than-one and greater-than-two
/// flags, the signs, then what is left of each magnitude.
fn write_levels(
    cabac: &mut CabacWriter,
    contexts: &mut ResidualContexts,
    significant: &[usize],
    level_at: impl Fn(usize) -> i32,
    (set, is_chroma, greater1_ctx): (usize, bool, &mut usize),
) {
    if significant.is_empty() {
        return;
    }
    let magnitudes: Vec<u32> = significant
        .iter()
        .map(|n| level_at(*n).unsigned_abs())
        .collect();
    *greater1_ctx = 1;
    let mut first_greater1 = None;
    for (m, magnitude) in magnitudes.iter().take(8).enumerate() {
        let is_greater = *magnitude > 1;
        let ctx = 4 * set + *greater1_ctx + if is_chroma { 16 } else { 0 };
        cabac.decision(&mut contexts.greater1[ctx], is_greater);
        if is_greater {
            *greater1_ctx = 0;
            first_greater1.get_or_insert(m);
        } else if (1..3).contains(greater1_ctx) {
            *greater1_ctx += 1;
        }
    }
    if let Some(m) = first_greater1 {
        let ctx = set + if is_chroma { 4 } else { 0 };
        cabac.decision(&mut contexts.greater2[ctx], magnitudes[m] > 2);
    }
    for n in significant {
        cabac.bypass_bits(u32::from(level_at(*n) < 0), 1);
    }
    let mut rice = 0u8;
    for (m, magnitude) in magnitudes.iter().enumerate() {
        let (base, threshold) = if m < 8 {
            let greater1 = u32::from(*magnitude > 1);
            let greater2 = u32::from(first_greater1 == Some(m) && *magnitude > 2);
            let threshold = if first_greater1 == Some(m) { 3 } else { 2 };
            (1 + greater1 + greater2, threshold)
        } else {
            (1, 1)
        };
        if base != threshold {
            continue;
        }
        write_remaining(cabac, magnitude - base, rice);
        if *magnitude > 3 << rice {
            rice = (rice + 1).min(4);
        }
    }
}

/// coeff_abs_level_remaining (9.3.3.11): a Rice code, with an Exp-Golomb
/// escape past three times the Rice step.
fn write_remaining(cabac: &mut CabacWriter, value: u32, rice: u8) {
    if value < 3 << rice {
        let prefix = value >> rice;
        cabac.bypass_bits((1 << (prefix + 1)) - 2, prefix as u8 + 1);
        cabac.bypass_bits(value & ((1 << rice) - 1), rice);
        return;
    }
    let mut extra = 0u8;
    while value >= ((1 << (extra + 1)) + 2) << rice {
        extra += 1;
    }
    let base = ((1u32 << extra) + 2) << rice;
    let ones = 3 + u32::from(extra);
    // `ones` ones and the zero that ends them.
    for _ in 0..ones {
        cabac.bypass_bits(1, 1);
    }
    cabac.bypass_bits(0, 1);
    cabac.bypass_bits(value - base, extra + rice);
}

/// The HEVC inverse transform basis (H.265 8.6.4.2): row k is the k-th
/// frequency of the 32-point transform; the 16-point one uses every other
/// row, the 8-point one every fourth.
#[rustfmt::skip]
pub const BASIS: [[i8; 32]; 32] = [
    [64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64],
    [90, 90, 88, 85, 82, 78, 73, 67, 61, 54, 46, 38, 31, 22, 13, 4, -4, -13, -22, -31, -38, -46, -54, -61, -67, -73, -78, -82, -85, -88, -90, -90],
    [90, 87, 80, 70, 57, 43, 25, 9, -9, -25, -43, -57, -70, -80, -87, -90, -90, -87, -80, -70, -57, -43, -25, -9, 9, 25, 43, 57, 70, 80, 87, 90],
    [90, 82, 67, 46, 22, -4, -31, -54, -73, -85, -90, -88, -78, -61, -38, -13, 13, 38, 61, 78, 88, 90, 85, 73, 54, 31, 4, -22, -46, -67, -82, -90],
    [89, 75, 50, 18, -18, -50, -75, -89, -89, -75, -50, -18, 18, 50, 75, 89, 89, 75, 50, 18, -18, -50, -75, -89, -89, -75, -50, -18, 18, 50, 75, 89],
    [88, 67, 31, -13, -54, -82, -90, -78, -46, -4, 38, 73, 90, 85, 61, 22, -22, -61, -85, -90, -73, -38, 4, 46, 78, 90, 82, 54, 13, -31, -67, -88],
    [87, 57, 9, -43, -80, -90, -70, -25, 25, 70, 90, 80, 43, -9, -57, -87, -87, -57, -9, 43, 80, 90, 70, 25, -25, -70, -90, -80, -43, 9, 57, 87],
    [85, 46, -13, -67, -90, -73, -22, 38, 82, 88, 54, -4, -61, -90, -78, -31, 31, 78, 90, 61, 4, -54, -88, -82, -38, 22, 73, 90, 67, 13, -46, -85],
    [83, 36, -36, -83, -83, -36, 36, 83, 83, 36, -36, -83, -83, -36, 36, 83, 83, 36, -36, -83, -83, -36, 36, 83, 83, 36, -36, -83, -83, -36, 36, 83],
    [82, 22, -54, -90, -61, 13, 78, 85, 31, -46, -90, -67, 4, 73, 88, 38, -38, -88, -73, -4, 67, 90, 46, -31, -85, -78, -13, 61, 90, 54, -22, -82],
    [80, 9, -70, -87, -25, 57, 90, 43, -43, -90, -57, 25, 87, 70, -9, -80, -80, -9, 70, 87, 25, -57, -90, -43, 43, 90, 57, -25, -87, -70, 9, 80],
    [78, -4, -82, -73, 13, 85, 67, -22, -88, -61, 31, 90, 54, -38, -90, -46, 46, 90, 38, -54, -90, -31, 61, 88, 22, -67, -85, -13, 73, 82, 4, -78],
    [75, -18, -89, -50, 50, 89, 18, -75, -75, 18, 89, 50, -50, -89, -18, 75, 75, -18, -89, -50, 50, 89, 18, -75, -75, 18, 89, 50, -50, -89, -18, 75],
    [73, -31, -90, -22, 78, 67, -38, -90, -13, 82, 61, -46, -88, -4, 85, 54, -54, -85, 4, 88, 46, -61, -82, 13, 90, 38, -67, -78, 22, 90, 31, -73],
    [70, -43, -87, 9, 90, 25, -80, -57, 57, 80, -25, -90, -9, 87, 43, -70, -70, 43, 87, -9, -90, -25, 80, 57, -57, -80, 25, 90, 9, -87, -43, 70],
    [67, -54, -78, 38, 85, -22, -90, 4, 90, 13, -88, -31, 82, 46, -73, -61, 61, 73, -46, -82, 31, 88, -13, -90, -4, 90, 22, -85, -38, 78, 54, -67],
    [64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64],
    [61, -73, -46, 82, 31, -88, -13, 90, -4, -90, 22, 85, -38, -78, 54, 67, -67, -54, 78, 38, -85, -22, 90, 4, -90, 13, 88, -31, -82, 46, 73, -61],
    [57, -80, -25, 90, -9, -87, 43, 70, -70, -43, 87, 9, -90, 25, 80, -57, -57, 80, 25, -90, 9, 87, -43, -70, 70, 43, -87, -9, 90, -25, -80, 57],
    [54, -85, -4, 88, -46, -61, 82, 13, -90, 38, 67, -78, -22, 90, -31, -73, 73, 31, -90, 22, 78, -67, -38, 90, -13, -82, 61, 46, -88, 4, 85, -54],
    [50, -89, 18, 75, -75, -18, 89, -50, -50, 89, -18, -75, 75, 18, -89, 50, 50, -89, 18, 75, -75, -18, 89, -50, -50, 89, -18, -75, 75, 18, -89, 50],
    [46, -90, 38, 54, -90, 31, 61, -88, 22, 67, -85, 13, 73, -82, 4, 78, -78, -4, 82, -73, -13, 85, -67, -22, 88, -61, -31, 90, -54, -38, 90, -46],
    [43, -90, 57, 25, -87, 70, 9, -80, 80, -9, -70, 87, -25, -57, 90, -43, -43, 90, -57, -25, 87, -70, -9, 80, -80, 9, 70, -87, 25, 57, -90, 43],
    [38, -88, 73, -4, -67, 90, -46, -31, 85, -78, 13, 61, -90, 54, 22, -82, 82, -22, -54, 90, -61, -13, 78, -85, 31, 46, -90, 67, 4, -73, 88, -38],
    [36, -83, 83, -36, -36, 83, -83, 36, 36, -83, 83, -36, -36, 83, -83, 36, 36, -83, 83, -36, -36, 83, -83, 36, 36, -83, 83, -36, -36, 83, -83, 36],
    [31, -78, 90, -61, 4, 54, -88, 82, -38, -22, 73, -90, 67, -13, -46, 85, -85, 46, 13, -67, 90, -73, 22, 38, -82, 88, -54, -4, 61, -90, 78, -31],
    [25, -70, 90, -80, 43, 9, -57, 87, -87, 57, -9, -43, 80, -90, 70, -25, -25, 70, -90, 80, -43, -9, 57, -87, 87, -57, 9, 43, -80, 90, -70, 25],
    [22, -61, 85, -90, 73, -38, -4, 46, -78, 90, -82, 54, -13, -31, 67, -88, 88, -67, 31, 13, -54, 82, -90, 78, -46, 4, 38, -73, 90, -85, 61, -22],
    [18, -50, 75, -89, 89, -75, 50, -18, -18, 50, -75, 89, -89, 75, -50, 18, 18, -50, 75, -89, 89, -75, 50, -18, -18, 50, -75, 89, -89, 75, -50, 18],
    [13, -38, 61, -78, 88, -90, 85, -73, 54, -31, 4, 22, -46, 67, -82, 90, -90, 82, -67, 46, -22, -4, 31, -54, 73, -85, 90, -88, 78, -61, 38, -13],
    [9, -25, 43, -57, 70, -80, 87, -90, 90, -87, 80, -70, 57, -43, 25, -9, -9, 25, -43, 57, -70, 80, -87, 90, -90, 87, -80, 70, -57, 43, -25, 9],
    [4, -13, 22, -31, 38, -46, 54, -61, 67, -73, 78, -82, 85, -88, 90, -90, 90, -90, 88, -85, 82, -78, 73, -67, 61, -54, 46, -38, 31, -22, 13, -4],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diagonal_scan_runs_up_and_right() {
        let scan = diagonal_scan(4);
        let xs: Vec<usize> = scan.iter().map(|(x, _)| *x).collect();
        let ys: Vec<usize> = scan.iter().map(|(_, y)| *y).collect();
        assert_eq!(xs, [0, 0, 1, 0, 1, 2, 0, 1, 2, 3, 1, 2, 3, 2, 3, 3]);
        assert_eq!(ys, [0, 1, 0, 2, 1, 0, 3, 2, 1, 0, 3, 2, 1, 3, 2, 3]);
        assert_eq!(diagonal_scan(8).len(), 64);
    }

    #[test]
    fn last_positions_split_as_the_table_says() {
        assert_eq!(last_position_parts(3), (3, 0, 0));
        assert_eq!(last_position_parts(4), (4, 0, 1));
        assert_eq!(last_position_parts(5), (4, 1, 1));
        assert_eq!(last_position_parts(6), (5, 0, 1));
        assert_eq!(last_position_parts(8), (6, 0, 2));
        assert_eq!(last_position_parts(12), (7, 0, 2));
        assert_eq!(last_position_parts(31), (9, 7, 3));
    }

    /// The forward transform as the matrix product it stands for.
    fn transform_by_matrix(residual: &[i32], size: usize) -> Vec<i64> {
        let log2 = size.trailing_zeros() as i64;
        let (first_shift, second_shift) = (log2 - 1, log2 + 6);
        let mut rows = vec![0i64; size * size];
        for y in 0..size {
            for k in 0..size {
                let sum: i64 = (0..size)
                    .map(|i| basis(size, k, i) * i64::from(residual[y * size + i]))
                    .sum();
                rows[y * size + k] = (sum + (1 << (first_shift - 1))) >> first_shift;
            }
        }
        let mut coefficients = vec![0i64; size * size];
        for x in 0..size {
            for k in 0..size {
                let sum: i64 = (0..size)
                    .map(|j| basis(size, k, j) * rows[j * size + x])
                    .sum();
                coefficients[k * size + x] = (sum + (1 << (second_shift - 1))) >> second_shift;
            }
        }
        coefficients
    }

    /// The butterfly gives exactly what multiplying by the matrix does, for
    /// any residual at every size.
    #[test]
    fn the_fast_transform_is_the_matrix_product() {
        let mut seed = 12_345u32;
        let mut sample = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 16) as i32 % 511 - 255
        };
        for size in [4, 8, 16, 32] {
            for _ in 0..25 {
                let residual: Vec<i32> = (0..size * size).map(|_| sample()).collect();
                assert_eq!(
                    transform(&residual, size, Kernel::Dct),
                    transform_by_matrix(&residual, size),
                    "{size}x{size}"
                );
            }
        }
    }

    /// Quantising and rebuilding a ramp at a fine QP returns it to within a
    /// level, at every block size: the forward pass and the decoder's
    /// scaling agree.
    #[test]
    fn a_ramp_survives_a_fine_quantiser() {
        let kernels = [4, 8, 16, 32]
            .map(|size| (size, Kernel::Dct))
            .into_iter()
            .chain([(4, Kernel::Dst)]);
        for (size, kernel) in kernels {
            let residual: Vec<i32> = (0..size * size)
                .map(|at| (at % size) as i32 * 2 - (at / size) as i32 + 7)
                .collect();
            let levels = quantize(&transform(&residual, size, kernel), size, 4);
            let rebuilt = rebuild(&levels, size, 4, kernel);
            let worst = residual
                .iter()
                .zip(&rebuilt)
                .map(|(a, b)| (a - b).abs())
                .max()
                .unwrap();
            assert!(worst <= 1, "{kernel:?} {size}x{size}: worst error {worst}");
        }
    }
}
