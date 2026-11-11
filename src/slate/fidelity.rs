//! When a coded block is close enough to what was drawn.
//!
//! Exact for the flat screens (tolerance 0). With a tolerance, two limits
//! hold: no sample may be further off than the tolerance, and the block as
//! a whole may not sit more than half a level off on average. The eye
//! hardly sees a few scattered samples off by two on soft light, but it
//! does see a whole square a level lighter than the one beside it: with
//! deblocking off, those squares are what a gradient shows as a grid.

/// `shown` (what a decoder will have) against `source` (what was drawn),
/// sample for sample.
pub fn is_faithful(source: &[u8], shown: &[u8], tolerance: i32) -> bool {
    if tolerance == 0 {
        return source == shown;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scattered_error_passes_but_a_shifted_block_does_not() {
        let drawn = [100u8; 16];
        let mut speckled = drawn;
        speckled[3] = 102;
        speckled[9] = 98;
        assert!(is_faithful(&drawn, &speckled, 2));
        let shifted = [101u8; 16];
        assert!(
            !is_faithful(&drawn, &shifted, 2),
            "a whole block a level off"
        );
        let half = [100, 101].repeat(8);
        assert!(is_faithful(&drawn, &half, 2), "half a level on average");
        assert!(!is_faithful(&drawn, &[103u8; 16], 2), "past the tolerance");
        assert!(!is_faithful(&drawn, &speckled, 0), "tolerance 0 is exact");
    }
}
