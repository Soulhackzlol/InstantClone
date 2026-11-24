//! When a coded block is close enough to what was drawn.
//!
//! Exact for the flat screens (tolerance 0). With a tolerance, four
//! limits hold. No sample may be further off than the tolerance, and the
//! block as a whole may not sit more than half a level off on average: the
//! eye hardly sees a few scattered samples off by two on soft light, but it
//! does see a whole square a level lighter than the one beside it (with
//! deblocking off, those squares are what a gradient shows as a grid).
//! Nor may any one row or column sit far off on average (`line_tolerance`):
//! a block predicted straight down from the edge of a letter carries that
//! edge's pattern with it, and block after block it lines up into streaks
//! running down the screen. And where the drawing is flat, nothing masks
//! an error at all: a spark that passed would leave a faint trail of its
//! blocks behind, and a streak shows on the plain background around the
//! text. So a block of one level is coded exactly, and one of two levels
//! may be off by one at most.

/// Spread (largest sample less smallest) up to which a block counts as
/// nearly flat.
const FLAT_SPREAD: u8 = 1;
/// The most a sample of a nearly flat block may be off.
const FLAT_TOLERANCE: i32 = 1;

/// `shown` (what a decoder will have) against `source` (what was drawn),
/// sample for sample. Both are one square block, row by row.
pub fn is_faithful(source: &[u8], shown: &[u8], tolerance: i32) -> bool {
    if tolerance == 0 {
        return source == shown;
    }
    let tolerance = sample_tolerance(source, tolerance);
    let line_limit = line_tolerance(tolerance);
    let side = source.len().isqrt();
    debug_assert_eq!(side * side, source.len(), "a square block");
    // Running error of the whole block, each row and each column.
    let mut total_error = 0i32;
    let mut column_errors = [0i32; MAX_SIDE];
    for (row, (drawn_row, shown_row)) in source.chunks(side).zip(shown.chunks(side)).enumerate() {
        let mut row_error = 0i32;
        for (column, (drawn, decoded)) in drawn_row.iter().zip(shown_row).enumerate() {
            let error = i32::from(*decoded) - i32::from(*drawn);
            if error.abs() > tolerance {
                return false;
            }
            row_error += error;
            column_errors[column] += error;
        }
        if !is_line_within(row_error, side, line_limit) {
            return false;
        }
        total_error += row_error;
        // Once the last row is in, the columns are whole.
        if row + 1 == side
            && !column_errors[..side]
                .iter()
                .all(|e| is_line_within(*e, side, line_limit))
        {
            return false;
        }
    }
    is_within_half_a_level(total_error, source.len())
}

/// Largest block side checked: HEVC's 32x32 units.
const MAX_SIDE: usize = 32;

/// Whether `count` samples that are `total_error` off in all sit within
/// half a level on average.
fn is_within_half_a_level(total_error: i32, count: usize) -> bool {
    total_error.unsigned_abs() as usize * 2 <= count
}

/// How far one row or column may sit off on average: a level, or a
/// quarter of the tolerance where hard edges hide more (Halftone's dots
/// take two). Half a level, as for the whole block, costs several times
/// the bits on soft light; at a level the streaks were gone from the
/// frames looked at, and on Halftone two cost a third of what one does,
/// with only faint colour left by the text (measured,
/// `slate::tests::measure_loops`).
fn line_tolerance(sample_tolerance: i32) -> i32 {
    (sample_tolerance / 4).max(1)
}

/// Whether a line of `count` samples that are `total_error` off in all
/// sits within `limit` levels on average.
fn is_line_within(total_error: i32, count: usize, limit: i32) -> bool {
    total_error.unsigned_abs() as usize <= count * limit.unsigned_abs() as usize
}

/// The most any one sample of `source` may be off: the tolerance, none
/// where `source` is flat, one level where it is nearly flat. An encoder
/// coding a block piece by piece can give up on it as soon as one piece is
/// further off.
pub fn sample_tolerance(source: &[u8], tolerance: i32) -> i32 {
    let (lowest, highest) = source.iter().fold((u8::MAX, u8::MIN), |(low, high), s| {
        (low.min(*s), high.max(*s))
    });
    if highest == lowest {
        0
    } else if highest.saturating_sub(lowest) <= FLAT_SPREAD {
        tolerance.min(FLAT_TOLERANCE)
    } else {
        tolerance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scattered_error_passes_but_a_shifted_block_does_not() {
        let drawn: Vec<u8> = (0..16).map(|i| 96 + i / 2).collect();
        let mut speckled = drawn.clone();
        speckled[3] += 2;
        speckled[9] -= 2;
        assert!(is_faithful(&drawn, &speckled, 2));
        let shifted: Vec<u8> = drawn.iter().map(|s| s + 1).collect();
        assert!(
            !is_faithful(&drawn, &shifted, 2),
            "a whole block a level off"
        );
        let half: Vec<u8> = drawn
            .iter()
            .enumerate()
            .map(|(i, s)| s + (i % 2) as u8)
            .collect();
        assert!(is_faithful(&drawn, &half, 2), "half a level on average");
        let past: Vec<u8> = drawn.iter().map(|s| s + 3).collect();
        assert!(!is_faithful(&drawn, &past, 2), "past the tolerance");
        assert!(!is_faithful(&drawn, &speckled, 0), "tolerance 0 is exact");
    }

    /// A column or row more than a level off is a streak in the making,
    /// even when the block as a whole averages out.
    #[test]
    fn a_line_of_error_does_not_pass() {
        let drawn: Vec<u8> = (0..16).map(|i| 96 + (i % 4) * 2).collect();
        let mut column = drawn.clone();
        let mut row = drawn.clone();
        for i in 0..4 {
            column[i * 4 + 1] += 2;
            column[i * 4 + 2] -= 2;
            row[4 + i] += 2;
            row[8 + i] -= 2;
        }
        assert!(!is_faithful(&drawn, &column, 2), "two opposite columns");
        assert!(!is_faithful(&drawn, &row, 2), "two opposite rows");
        let mut checker = drawn.clone();
        for (i, sample) in checker.iter_mut().enumerate() {
            if (i / 4 + i % 4) % 2 == 0 {
                *sample += 2;
            } else {
                *sample -= 2;
            }
        }
        assert!(
            is_faithful(&drawn, &checker, 2),
            "error that doesn't line up"
        );
    }

    /// Flat background shows any error as a pattern: none on a flat block,
    /// one level at most on a nearly flat one.
    #[test]
    fn flat_blocks_are_held_closer_whatever_the_tolerance() {
        let flat = [40u8; 16];
        let mut trail = flat;
        trail[5] = 41;
        assert!(!is_faithful(&flat, &trail, 8), "a level off on flat");
        let mut nearly_flat = flat;
        nearly_flat[0] = 41;
        trail = nearly_flat;
        trail[5] = 42;
        assert!(!is_faithful(&nearly_flat, &trail, 8), "two levels off");
        trail[5] = 41;
        assert!(is_faithful(&nearly_flat, &trail, 8), "one level is fine");
    }
}
