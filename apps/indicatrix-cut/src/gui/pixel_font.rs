//! A minimal built-in 5x7 bitmap font: a fixed glyph lookup shared by every place
//! in this app that stamps text straight into an RGBA8 pixel buffer rather than
//! going through Slint's own text layout -- `gui::solid_preview::diagram2d`'s
//! crown/pavilion/profile panel labels and `gui::tilt::video_export::overlay`'s
//! tilt-performance readout. Both call sites keep their own scaling, drawing, and
//! colour code (`draw_text`/`draw_glyph`/`text_size`/`text_width`); only the glyph
//! DATA -- the character-to-bitmap lookup -- lives here, so the two copies this
//! module replaces could never drift against each other one character at a time.
//!
//! Covers every character either caller actually draws: space, `-`, `.`, `'`,
//! `/`, `_`, `%`, `:`, the digits, and every uppercase letter A-Z (case-folded --
//! see [`glyph`]). An unsupported character renders as a blank cell rather than a
//! placeholder glyph.

/// Glyph width in pixels.
pub(in crate::gui) const GLYPH_WIDTH: usize = 5;
/// Glyph height in pixels.
pub(in crate::gui) const GLYPH_HEIGHT: usize = 7;

/// Converts a 5-character `'X'`/`'.'` row into a bitmask (bit 4 = leftmost column).
fn row_bits(row: &str) -> u8 {
    let mut bits = 0u8;
    for (i, ch) in row.chars().take(GLYPH_WIDTH).enumerate() {
        if ch != '.' {
            bits |= 1 << (GLYPH_WIDTH - 1 - i);
        }
    }
    bits
}

/// The punctuation/whitespace arms of [`glyph_rows`]'s lookup, split out purely to
/// keep that function under clippy's function-length lint -- `c` is already
/// uppercased by the caller.
const fn glyph_rows_symbols(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        ' ' => [
            ".....", ".....", ".....", ".....", ".....", ".....", ".....",
        ],
        '-' => [
            ".....", ".....", ".....", "XXXXX", ".....", ".....", ".....",
        ],
        '.' => [
            ".....", ".....", ".....", ".....", ".....", ".XX..", ".XX..",
        ],
        '\'' => [
            ".X...", ".X...", ".....", ".....", ".....", ".....", ".....",
        ],
        // Needed for the multi-name tier join convention (`crates/
        // indicatrix-cut-core/src/design/tier.rs`) and for an underscore in a
        // tier name: `glyph` falls back to a blank cell for anything
        // `glyph_rows` returns `None` for, so both characters need their own
        // entries here to render instead of vanishing.
        '/' => [
            "....X", "...X.", "..X..", "..X..", ".X...", "X....", ".....",
        ],
        '_' => [
            ".....", ".....", ".....", ".....", ".....", ".....", "XXXXX",
        ],
        // Needed by the tilt video-export overlay's percentage readouts and its
        // "HH:MM"-shaped ones -- diagram2d's own labels never use either, but the
        // glyph shapes are identical wherever both callers happen to draw the
        // same character (see the module doc comment).
        '%' => [
            "X...X", "...X.", "..X..", "..X..", ".X...", "X...X", ".....",
        ],
        ':' => [
            ".....", ".XX..", ".XX..", ".....", ".XX..", ".XX..", ".....",
        ],
        _ => return None,
    })
}

/// The digit arms of [`glyph_rows`]'s lookup, split out purely to keep that function
/// under clippy's function-length lint -- `c` is already uppercased by the caller.
const fn glyph_rows_digits(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        '0' => [
            "XXXXX", "X...X", "X..XX", "X.X.X", "XX..X", "X...X", "XXXXX",
        ],
        '1' => [
            "..X..", ".XX..", "..X..", "..X..", "..X..", "..X..", ".XXX.",
        ],
        '2' => [
            ".XXX.", "X...X", "....X", "...X.", "..X..", ".X...", "XXXXX",
        ],
        '3' => [
            "XXXXX", "...X.", "..X..", "...X.", "....X", "X...X", ".XXX.",
        ],
        '4' => [
            "...X.", "..XX.", ".X.X.", "X..X.", "XXXXX", "...X.", "...X.",
        ],
        '5' => [
            "XXXXX", "X....", "XXXX.", "....X", "....X", "X...X", ".XXX.",
        ],
        '6' => [
            "..XX.", ".X...", "X....", "XXXX.", "X...X", "X...X", ".XXX.",
        ],
        '7' => [
            "XXXXX", "....X", "...X.", "..X..", ".X...", ".X...", ".X...",
        ],
        '8' => [
            ".XXX.", "X...X", "X...X", ".XXX.", "X...X", "X...X", ".XXX.",
        ],
        '9' => [
            ".XXX.", "X...X", "X...X", ".XXXX", "....X", "...X.", ".XX..",
        ],
        _ => return None,
    })
}

/// The `A`-`M` letter arms of [`glyph_rows`]'s lookup, split out purely to keep that
/// function under clippy's function-length lint -- `c` is already uppercased by the
/// caller.
const fn glyph_rows_letters_a_to_m(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        'A' => [
            "..X..", ".X.X.", "X...X", "X...X", "XXXXX", "X...X", "X...X",
        ],
        'B' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X...X", "X...X", "XXXX.",
        ],
        'C' => [
            ".XXXX", "X....", "X....", "X....", "X....", "X....", ".XXXX",
        ],
        'D' => [
            "XXXX.", "X...X", "X...X", "X...X", "X...X", "X...X", "XXXX.",
        ],
        'E' => [
            "XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "XXXXX",
        ],
        'F' => [
            "XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "X....",
        ],
        'G' => [
            ".XXXX", "X....", "X....", "X.XXX", "X...X", "X...X", ".XXXX",
        ],
        'H' => [
            "X...X", "X...X", "X...X", "XXXXX", "X...X", "X...X", "X...X",
        ],
        'I' => [
            "XXXXX", "..X..", "..X..", "..X..", "..X..", "..X..", "XXXXX",
        ],
        'J' => [
            "....X", "....X", "....X", "....X", "X...X", "X...X", ".XXX.",
        ],
        'K' => [
            "X...X", "X..X.", "X.X..", "XX...", "X.X..", "X..X.", "X...X",
        ],
        'L' => [
            "X....", "X....", "X....", "X....", "X....", "X....", "XXXXX",
        ],
        'M' => [
            "X...X", "XX.XX", "X.X.X", "X...X", "X...X", "X...X", "X...X",
        ],
        _ => return None,
    })
}

/// The `N`-`Z` letter arms of [`glyph_rows`]'s lookup, split out purely to keep that
/// function under clippy's function-length lint -- `c` is already uppercased by the
/// caller.
const fn glyph_rows_letters_n_to_z(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        'N' => [
            "X...X", "XX..X", "X.X.X", "X..XX", "X...X", "X...X", "X...X",
        ],
        'O' => [
            ".XXX.", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX.",
        ],
        'P' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X....", "X....", "X....",
        ],
        'Q' => [
            ".XXX.", "X...X", "X...X", "X...X", "X.X.X", "X..X.", ".XX.X",
        ],
        'R' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X.X..", "X..X.", "X...X",
        ],
        'S' => [
            ".XXXX", "X....", "X....", ".XXX.", "....X", "....X", "XXXX.",
        ],
        'T' => [
            "XXXXX", "..X..", "..X..", "..X..", "..X..", "..X..", "..X..",
        ],
        'U' => [
            "X...X", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX.",
        ],
        'V' => [
            "X...X", "X...X", "X...X", "X...X", "X...X", ".X.X.", "..X..",
        ],
        'W' => [
            "X...X", "X...X", "X...X", "X.X.X", "X.X.X", "XX.XX", "X...X",
        ],
        'X' => [
            "X...X", "X...X", ".X.X.", "..X..", ".X.X.", "X...X", "X...X",
        ],
        'Y' => [
            "X...X", "X...X", ".X.X.", "..X..", "..X..", "..X..", "..X..",
        ],
        'Z' => [
            "XXXXX", "....X", "...X.", "..X..", ".X...", "X....", "XXXXX",
        ],
        _ => return None,
    })
}

/// Returns the glyph rows for `c` (case-insensitive letters), or `None` for an
/// unsupported character -- callers skip it rather than drawing a placeholder.
/// Dispatches across [`glyph_rows_symbols`]/[`glyph_rows_digits`]/
/// [`glyph_rows_letters_a_to_m`]/[`glyph_rows_letters_n_to_z`], split out purely to
/// keep this lookup table's own function under clippy's function-length lint.
const fn glyph_rows(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    let c = c.to_ascii_uppercase();
    if let Some(rows) = glyph_rows_symbols(c) {
        return Some(rows);
    }
    if let Some(rows) = glyph_rows_digits(c) {
        return Some(rows);
    }
    if let Some(rows) = glyph_rows_letters_a_to_m(c) {
        return Some(rows);
    }
    glyph_rows_letters_n_to_z(c)
}

/// The bitmap for `c` (case-insensitive), one packed row per byte (bit 4 =
/// leftmost column) -- a blank cell for any character [`glyph_rows`] does not
/// cover.
pub(in crate::gui) fn glyph(c: char) -> [u8; GLYPH_HEIGHT] {
    let rows = glyph_rows(c).unwrap_or([
        ".....", ".....", ".....", ".....", ".....", ".....", ".....",
    ]);
    std::array::from_fn(|i| row_bits(rows[i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_bits_packs_x_as_1_and_dot_as_0_leftmost_bit_first() {
        assert_eq!(row_bits("X...."), 0b1_0000);
        assert_eq!(row_bits("....X"), 0b0_0001);
        assert_eq!(row_bits("....."), 0);
        assert_eq!(row_bits("XXXXX"), 0b1_1111);
    }

    #[test]
    fn glyph_is_blank_for_an_unsupported_character() {
        assert_eq!(glyph('@'), [0u8; GLYPH_HEIGHT]);
    }

    #[test]
    fn glyph_is_case_insensitive() {
        assert_eq!(glyph('a'), glyph('A'));
        assert_eq!(glyph('z'), glyph('Z'));
    }

    /// The two characters `diagram2d`'s own table never carried before this module
    /// unified the two callers' glyph data -- the tilt video-export overlay draws
    /// both (percentages, and a colon in "HH:MM"-shaped labels), so the merged
    /// table must still cover them.
    #[test]
    fn glyph_covers_the_percent_and_colon_symbols_the_overlay_needs() {
        assert_ne!(glyph('%'), [0u8; GLYPH_HEIGHT]);
        assert_ne!(glyph(':'), [0u8; GLYPH_HEIGHT]);
    }

    /// Every letter/digit/symbol either real caller actually draws must render
    /// (never fall back to blank) -- this is the exhaustive set `diagram2d.rs`'s
    /// `text_size`/`draw_text_into_buffer` and `tilt::video_export::overlay`'s
    /// `text_width`/`draw_text` between them ever pass to [`glyph`].
    #[test]
    fn every_character_either_caller_draws_renders_a_non_blank_glyph() {
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-.'/_%:".chars() {
            assert_ne!(
                glyph(c),
                [0u8; GLYPH_HEIGHT],
                "expected a real glyph for {c:?}"
            );
        }
        // Space is the one deliberately blank glyph in that set.
        assert_eq!(glyph(' '), [0u8; GLYPH_HEIGHT]);
    }
}
