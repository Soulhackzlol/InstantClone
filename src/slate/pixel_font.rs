//! 5x7 pixel font for the Arcade theme. Each glyph is seven rows, top to
//! bottom, with the five pixels of a row in the low bits (bit 4 is the
//! leftmost column). Letters are uppercase only; `to_pixel_text` folds
//! lowercase and turns anything unsupported into a space.

pub const GLYPH_COLUMNS: usize = 5;
pub const GLYPH_ROWS: usize = 7;
/// Glyph plus one blank column between letters.
pub const ADVANCE_COLUMNS: usize = 6;

const GLYPHS: [(char, [u8; GLYPH_ROWS]); 47] = [
    ('A', [14, 17, 17, 31, 17, 17, 17]),
    ('B', [30, 17, 17, 30, 17, 17, 30]),
    ('C', [14, 17, 16, 16, 16, 17, 14]),
    ('D', [30, 17, 17, 17, 17, 17, 30]),
    ('E', [31, 16, 16, 30, 16, 16, 31]),
    ('F', [31, 16, 16, 30, 16, 16, 16]),
    ('G', [14, 17, 16, 23, 17, 17, 15]),
    ('H', [17, 17, 17, 31, 17, 17, 17]),
    ('I', [14, 4, 4, 4, 4, 4, 14]),
    ('J', [7, 2, 2, 2, 2, 18, 12]),
    ('K', [17, 18, 20, 24, 20, 18, 17]),
    ('L', [16, 16, 16, 16, 16, 16, 31]),
    ('M', [17, 27, 21, 21, 17, 17, 17]),
    ('N', [17, 17, 25, 21, 19, 17, 17]),
    ('O', [14, 17, 17, 17, 17, 17, 14]),
    ('P', [30, 17, 17, 30, 16, 16, 16]),
    ('Q', [14, 17, 17, 17, 21, 18, 13]),
    ('R', [30, 17, 17, 30, 20, 18, 17]),
    ('S', [15, 16, 16, 14, 1, 1, 30]),
    ('T', [31, 4, 4, 4, 4, 4, 4]),
    ('U', [17, 17, 17, 17, 17, 17, 14]),
    ('V', [17, 17, 17, 17, 17, 10, 4]),
    ('W', [17, 17, 17, 21, 21, 21, 10]),
    ('X', [17, 17, 10, 4, 10, 17, 17]),
    ('Y', [17, 17, 10, 4, 4, 4, 4]),
    ('Z', [31, 1, 2, 4, 8, 16, 31]),
    ('0', [14, 17, 19, 21, 25, 17, 14]),
    ('1', [4, 12, 4, 4, 4, 4, 14]),
    ('2', [14, 17, 1, 2, 4, 8, 31]),
    ('3', [31, 2, 4, 2, 1, 17, 14]),
    ('4', [2, 6, 10, 18, 31, 2, 2]),
    ('5', [31, 16, 30, 1, 1, 17, 14]),
    ('6', [6, 8, 16, 30, 17, 17, 14]),
    ('7', [31, 1, 2, 4, 8, 8, 8]),
    ('8', [14, 17, 17, 14, 17, 17, 14]),
    ('9', [14, 17, 17, 15, 1, 2, 12]),
    ('.', [0, 0, 0, 0, 0, 12, 12]),
    (',', [0, 0, 0, 0, 12, 4, 8]),
    ('!', [4, 4, 4, 4, 4, 0, 4]),
    ('?', [14, 17, 1, 2, 4, 0, 4]),
    ('-', [0, 0, 0, 31, 0, 0, 0]),
    (':', [0, 12, 12, 0, 12, 12, 0]),
    ('\'', [4, 4, 8, 0, 0, 0, 0]),
    ('/', [1, 1, 2, 4, 8, 16, 16]),
    ('(', [2, 4, 8, 8, 8, 4, 2]),
    (')', [8, 4, 2, 2, 2, 4, 8]),
    (' ', [0, 0, 0, 0, 0, 0, 0]),
];

/// Rows for `c`, or None when the font has no glyph for it.
pub fn glyph(c: char) -> Option<&'static [u8; GLYPH_ROWS]> {
    GLYPHS
        .iter()
        .find(|(glyph_char, _)| *glyph_char == c)
        .map(|(_, rows)| rows)
}

/// Uppercase `text` and replace characters the font lacks with spaces,
/// so layout can count columns without looking glyphs up twice.
pub fn to_pixel_text(text: &str) -> Vec<char> {
    text.chars()
        .flat_map(char::to_uppercase)
        .map(|c| if glyph(c).is_some() { c } else { ' ' })
        .collect()
}

/// Whether column `column` (0 = left) of `row` is lit.
pub fn is_lit(row: u8, column: usize) -> bool {
    row & (1 << (GLYPH_COLUMNS - 1 - column)) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercase_folds_and_unknown_becomes_space() {
        assert_eq!(to_pixel_text("Hi é!"), vec!['H', 'I', ' ', ' ', '!']);
    }

    #[test]
    fn every_row_fits_in_five_columns() {
        for (c, rows) in GLYPHS {
            assert!(
                rows.iter().all(|row| *row < 32),
                "glyph {c:?} is wider than 5 columns"
            );
        }
    }

    #[test]
    fn leftmost_column_is_bit_four() {
        assert!(is_lit(16, 0));
        assert!(!is_lit(16, 4));
        assert!(is_lit(1, 4));
    }
}
