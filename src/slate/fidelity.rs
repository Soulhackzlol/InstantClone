//! When a coded block is close enough to what was drawn.
//!
//! Exact for the flat screens (tolerance 0). With a tolerance, three
//! limits hold. No sample may be further off than the tolerance, and the
//! block as a whole may not sit more than half a level off on average: the
//! eye hardly sees a few scattered samples off by two on soft light, but it
//! does see a whole square a level lighter than the one beside it (with
//! deblocking off, those squares are what a gradient shows as a grid).
//! And where the drawing is flat, nothing masks an error at all: a spark
//! that passed would leave a faint trail of its blocks behind, so a flat
//! block may be off by one level at most.

/// Spread (largest sample less smallest) up to which a block counts as
/// flat.
const FLAT_SPREAD: u8 = 1;
/// The most a sample of a flat block may be off.
const FLAT_TOLERANCE: i32 = 1;

/// `shown` (what a decoder will have) against `source` (what was drawn),
/// sample for sample.
pub fn is_faithful(source: &[u8], shown: &[u8], tolerance: i32) -> bool {
    if tolerance == 0 {
        return source == shown;
    }
    let tolerance = sample_tolerance(source, tolerance);
    let mut total_error = 0i32;
    for (drawn, decoded) in source.iter().zip(shown) {
        let error = i32::from(*decoded) - i32::from(*drawn);
        if error.abs() > tolerance {
            return false;
        }
        total_error += error;
    }
    // Mean error within half a level.
    total_error.unsigned_abs() as usize * 2 <= source.len()
}

/// The most any one sample of `source` may be off: the tolerance, or one
/// level where `source` is flat. An encoder coding a block piece by piece
/// can give up on it as soon as one piece is further off.
pub fn sample_tolerance(source: &[u8], tolerance: i32) -> i32 {
    let (lowest, highest) = source.iter().fold((u8::MAX, u8::MIN), |(low, high), s| {
        (low.min(*s), high.max(*s))
    });
    if highest.saturating_sub(lowest) <= FLAT_SPREAD {
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

    /// Flat background shows any error as a pattern: one level at most.
    #[test]
    fn a_flat_block_allows_one_level_whatever_the_tolerance() {
        let flat = [40u8; 16];
        let mut trail = flat;
        trail[5] = 42;
        assert!(!is_faithful(&flat, &trail, 8), "two levels off on flat");
        trail[5] = 41;
        assert!(is_faithful(&flat, &trail, 8), "one level is fine");
    }
}
