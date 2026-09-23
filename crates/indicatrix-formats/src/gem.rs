//! Reader for `GemCAD`'s native `.gem` save format.
//!
//! `GemCAD` is Robert Strickland's faceting-design software (see [`crate::asc`]'s
//! module docs for the full affiliation note, which applies equally here). `.gem`
//! is its own binary save format -- distinct from the `.asc` text cutting
//! schedule `GemCAD` can also export -- and, unlike `.asc`, has no published
//! specification anywhere this module's author could find. This module is a
//! **partial, honestly-bounded** reverse-engineering of it, built from a real
//! corpus of 254 `.gem` files (`facet_diagrams.sqlite`'s `attached_files` table)
//! and, for 49 of those designs, a scraped `angle_settings` table (facet name,
//! angle, index positions, and cutting notes) that could serve as ground truth.
//!
//! # What is confirmed
//!
//! A `.gem` file has no fixed magic number, version marker, or other header --
//! every sample starts directly with what appears to be numeric data. What *is*
//! confirmed, byte-exactly, across every one of the 254 samples: the file embeds
//! **Pascal-style length-prefixed ASCII text fragments** -- a single byte `0..=255`
//! giving a length, immediately followed by exactly that many ASCII bytes (which
//! may include a literal tab character, `\t`, used as an internal separator -- see
//! below). This was confirmed by hand-computing the byte length of several
//! recovered fragments and finding it exactly equal to the byte immediately
//! preceding them -- most conclusively a 55-character footnote
//! (`"For quartz. Requires some carving, e.g. a \"bubble\" on 6"`) immediately
//! preceded by the byte `0x37` (55 decimal), in `attached_files` id 6616 (detail
//! 3163, `"Thank you Bernd.gem"`).
//!
//! [`parse_gem`] recovers every such fragment as a [`GemNote`]. Two things about
//! their content are independently confirmed against real data (not just
//! internally consistent):
//!
//! - **The free text matches `.asc`'s own cutting/meet-instruction vocabulary
//!   exactly** -- real recovered fragments include `"meet 3"`, `"level girdle"`,
//!   `"set girdle position"`, `"set girdle thickness"`, and `"cut stone
//!   outline"`, which is the same domain [`crate::asc::MeetInstruction`] parses
//!   out of `.asc`'s `G` field (e.g. "size stone,
//!   start level girdle" is exactly this style of text).
//! - **Design title and author strings are recoverable, usually as the last two
//!   or three fragments in the file**, and were checked against this crate's
//!   database independently of the file content itself: `attached_files` id 6616
//!   (detail 3163) embeds `"Thank you Bernd"` immediately followed by `"Marco
//!   Voltolini"`, and `diagram_details`/`diagram_entries` for that same detail id
//!   independently record the title `"Voltolini - Thank you Bernd"` and
//!   `designer_info` `"Voltolini"`. The same pattern -- design name then author,
//!   both untabbed, clustered at the end of the file -- was checked against four
//!   more samples (`"Easy Bar"` / `"by Dr. Hideki Ikeda"` against designer
//!   `"Ikeda"`; a `"PC 13.188  SMALL TRI RETRO (Revised)"` title against a design
//!   named `"Small Tri Retro, Revised"`; and two more) and held in every case.
//!
//! Many fragments are `"<facet-name>\t<instruction>"` (e.g. `"5\tmeet 3"`,
//! `"C\tmeet 3"`) or just `"<facet-name>\t"` with no instruction (e.g. `"g1\t"`,
//! `"pf1\t"`) -- [`GemNote::facet_name`]/[`GemNote::text`] split on that tab.
//! Facet names recovered this way (`"1"`, `"A"`, `"g1"`, `"pf1"`) match the exact
//! facet names the `angle_settings` table records for the same designs.
//!
//! # What is *not* confirmed -- read this before using [`GemNote::offset`]
//!
//! **The numeric encoding of facet angle, index/tooth position, and depth/mast
//! could not be located.** This is the one piece of ground truth
//! `angle_settings` could have given (49 designs' worth), and it was searched for
//! systematically against one such design (`attached_files` id 6669, detail
//! 3260, `"PentaStar"`, cross-checked against its 13-row `angle_settings` table)
//! without success:
//!
//! - Every `angle_settings` angle value (`75.00`, `43.00`, `41.00`, `72.00`, `40.00`,
//!   `23.90`, `40.36`, `45.00`, `90.00`, `40.41`) was searched for as a raw
//!   little-endian `f64` and `f32`, in degrees and in radians: no match.
//! - The same angles' `cos`/`sin` (a plane-normal-component encoding, by analogy
//!   with `.gcs`'s confirmed `nx`/`ny`/`nz` fields) were searched for as `f64` and
//!   `f32`: no match.
//! - The `angle_settings` table's literal tooth numbers (e.g. `120, 24, 48, 72, 96`
//!   for one row) were searched for as contiguous `i8`/`u8`/`i16`/`u16`/`i32`/`u32`
//!   sequences, and individually: no match beyond what plain chance predicts in a
//!   file this size.
//!
//! This module does not guess. No angle, index, mast, depth, or facet-geometry
//! field is exposed here -- only the text recovered by the length-prefix scan.
//! Everything else in the file (the numeric majority of its bytes) is simply
//! unaccounted for by this module; a caller who needs it has the original byte
//! slice they passed to [`parse_gem`] and [`GemNote::offset`] to see where the
//! recovered text sits within it, but this module makes no claim about what
//! surrounds it.
//!
//! # Recovery heuristic and its limits
//!
//! [`parse_gem`]'s scanner is necessarily a heuristic, not a grammar: a length
//! byte is only accepted when the bytes that follow are all printable ASCII (or a
//! tab) *and* contain enough letters to be plausible prose rather than a
//! coincidental run of print-range bytes inside numeric data (short fragments of
//! 4 bytes or fewer need only one letter; longer ones need roughly a third).
//! Across all 254 real samples this recovers 16,286 fragments (about 64 per
//! file), 61% of which contain a recognized instruction keyword (`"meet"`,
//! `"level"`, `"girdle"`, `"centerpoint"`, `"tcp"`, `"pcp"`, `"gmp"`, `"cam"`, or
//! `"set "`) -- the rest are short facet-name-only labels and other free text;
//! zero files produced no fragments at all. It can
//! still, in principle, both miss a genuine short fragment that happens to be
//! letter-sparse and manufacture a false one from numeric data that happens to
//! fall in the printable ASCII range with enough apparent letters -- no instance
//! of either was found in the sampled corpus, but neither can be ruled out for a
//! file this module has not seen.

use std::fmt;

/// Everything that can go wrong parsing a `.gem` file with [`parse_gem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GemParseError {
    /// `content` was empty.
    EmptyInput,
}

impl fmt::Display for GemParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "empty input"),
        }
    }
}

impl std::error::Error for GemParseError {}

/// One length-prefixed ASCII text fragment recovered from a `.gem` file's byte
/// stream. See the module docs for what is (and is not) confirmed about these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GemNote {
    /// Byte offset of this fragment's length-prefix byte in the buffer passed to
    /// [`parse_gem`] -- kept so a caller cross-referencing other recovered data,
    /// or debugging a parse, can point back at the exact source bytes. Not
    /// claimed to be the start of any larger record; see the module docs.
    pub offset: usize,
    /// The text before a tab character, when this fragment contained exactly the
    /// shape of a facet-name label (short, alphanumeric, non-empty) followed by a
    /// tab. Confirmed against real facet names from the `angle_settings` table
    /// (`"1"`, `"A"`, `"g1"`, `"pf1"`) for several designs -- see module docs.
    /// `None` for fragments with no tab, fragments with an *empty* prefix before
    /// the tab (a confirmed real pattern -- the same instruction text sometimes
    /// recurs elsewhere in a file with no name ahead of it), and
    /// title/author/footnote-style fragments (which have no tab at all).
    pub facet_name: Option<String>,
    /// The fragment's text after the tab, or the whole fragment when there was no
    /// tab. Empty when a facet-name label had no instruction following it (e.g.
    /// `"g1\t"` recovers `facet_name: Some("g1")`, `text: ""`).
    pub text: String,
}

/// A `.gem` file's recovered contents. See the module docs: this is deliberately
/// not a full parse of the format, only the text this module could confirm.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GemDesign {
    /// Every recovered text fragment, in file order.
    pub notes: Vec<GemNote>,
}

impl GemDesign {
    /// Every recovered text fragment, in file order.
    #[must_use]
    pub fn notes(&self) -> &[GemNote] {
        &self.notes
    }

    /// Best-effort split of the trailing run of fragments that have no
    /// [`GemNote::facet_name`] (title/author/footnote-style text -- see the
    /// module docs' confirmed examples), by scanning backward from the end of
    /// [`Self::notes`] and stopping at the first fragment that does have one.
    /// This is a grouping over already-recovered text, not a new decoding step,
    /// and it is a heuristic: a design whose last real facet happens to have no
    /// instruction text of its own is indistinguishable, by this rule, from one
    /// whose metadata block is one fragment longer. Returns fragments in file
    /// order (oldest first).
    #[must_use]
    pub fn probable_metadata(&self) -> Vec<&GemNote> {
        let mut out: Vec<&GemNote> = self
            .notes
            .iter()
            .rev()
            .take_while(|note| note.facet_name.is_none())
            .collect();
        out.reverse();
        out
    }
}

/// Below this many bytes, a single letter is enough to accept a fragment as
/// plausible text (a bare 1-3 character facet-name label like `"T"` or `"g1"`
/// would otherwise be rejected by a proportional letter-density check).
const SHORT_FRAGMENT_LEN: usize = 4;

/// Returns whether `chunk` is plausible recovered text: every byte is either a
/// tab (the facet-name/instruction separator -- see module docs) or in the
/// printable ASCII range, and it contains enough letters not to be a coincidental
/// run of print-range bytes inside numeric data. See the module docs' "Recovery
/// heuristic and its limits" for what this trades off.
fn is_plausible_text(chunk: &[u8]) -> bool {
    if chunk.is_empty() {
        return false;
    }
    if !chunk
        .iter()
        .all(|&b| b == b'\t' || (0x20..0x7f).contains(&b))
    {
        return false;
    }
    let letters = chunk.iter().filter(|b| b.is_ascii_alphabetic()).count();
    let min_letters = if chunk.len() <= SHORT_FRAGMENT_LEN {
        1
    } else {
        (chunk.len() / 3).max(2)
    };
    letters >= min_letters
}

/// Longest text a leading facet-name label is accepted to be, before the tab.
/// Every real facet name seen in the corpus (`"1"`, `"A"`, `"g1"`, `"pf1"`,
/// `"C4"`) is 3 characters or fewer; this is deliberately more generous than
/// that so an unseen-but-plausible longer name is not silently discarded into
/// `text` instead, while still being far short of ordinary prose.
const MAX_FACET_NAME_LEN: usize = 12;

/// Splits one recovered fragment's text into a facet-name/instruction pair, when
/// it has that shape. See [`GemNote::facet_name`]'s doc comment for the exact
/// rule.
fn split_note(offset: usize, text: &str) -> GemNote {
    if let Some((name, rest)) = text.split_once('\t') {
        // An empty prefix before the tab is itself a confirmed real pattern (the
        // same instruction text recurs elsewhere in a file with no facet-name
        // label ahead of it) -- still split it, just with no recognized name.
        if name.is_empty() {
            return GemNote {
                offset,
                facet_name: None,
                text: rest.to_string(),
            };
        }
        let looks_like_facet_name =
            name.len() <= MAX_FACET_NAME_LEN && name.chars().all(|c| c.is_ascii_alphanumeric());
        if looks_like_facet_name {
            return GemNote {
                offset,
                facet_name: Some(name.to_string()),
                text: rest.to_string(),
            };
        }
        // A non-empty prefix that doesn't look like a facet name: safer to keep
        // the fragment whole than to guess which side of the tab is meaningful.
    }
    GemNote {
        offset,
        facet_name: None,
        text: text.to_string(),
    }
}

/// Scans `bytes` for length-prefixed text fragments (see module docs), greedily
/// consuming each accepted fragment so a real fragment's own interior bytes are
/// never re-examined as if they were a fresh length prefix.
fn scan_notes(bytes: &[u8]) -> Vec<GemNote> {
    let mut notes = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let len = usize::from(bytes[i]);
        let end = i + 1 + len;
        if len >= 1 && end <= bytes.len() {
            let chunk = &bytes[i + 1..end];
            if is_plausible_text(chunk) {
                // `is_plausible_text` only accepts tab or 0x20..0x7f, both valid UTF-8.
                let text = std::str::from_utf8(chunk).unwrap_or_default();
                notes.push(split_note(i, text));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    notes
}

/// Recovers every confirmed-recoverable text fragment from a `.gem` file's raw
/// bytes. See the module docs for exactly what this does and does not decode.
///
/// # Errors
///
/// Returns `Err` only when `content` is empty -- there is no format grammar here
/// to violate (see module docs), so this can't fail the way [`crate::asc::parse_asc`]
/// or [`crate::gcs::parse_gcs`] can.
pub fn parse_gem(content: &[u8]) -> Result<GemDesign, GemParseError> {
    if content.is_empty() {
        return Err(GemParseError::EmptyInput);
    }
    Ok(GemDesign {
        notes: scan_notes(content),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real bytes, `attached_files` id 6616 (detail 3163, `"Thank you
    /// Bernd.gem"`), offset 24..48: a 4-byte field (unaccounted for by this
    /// module -- see module docs) followed by the length-prefixed fragment
    /// `"1\tcut stone outline"` (length byte `0x13` = 19 = `len("1\tcut stone
    /// outline")`).
    const GEM_NAMED_NOTE: &[u8] = &[
        0x01, 0x00, 0x00, 0x00, 0x13, 0x31, 0x09, 0x63, 0x75, 0x74, 0x20, 0x73, 0x74, 0x6f, 0x6e,
        0x65, 0x20, 0x6f, 0x75, 0x74, 0x6c, 0x69, 0x6e, 0x65,
    ];
    const _: () = assert!(GEM_NAMED_NOTE.len() == 24);

    /// Real bytes, same file, offset 190..212: the same instruction text
    /// recurring elsewhere in the file but with no facet-name prefix before the
    /// tab (length byte `0x12` = 18 = `len("\tcut stone outline")`).
    const GEM_UNNAMED_NOTE: &[u8] = &[
        0x00, 0x00, 0x12, 0x09, 0x63, 0x75, 0x74, 0x20, 0x73, 0x74, 0x6f, 0x6e, 0x65, 0x20, 0x6f,
        0x75, 0x74, 0x6c, 0x69, 0x6e, 0x65, 0x01,
    ];
    const _: () = assert!(GEM_UNNAMED_NOTE.len() == 22);

    /// Real bytes, same file, offset 1805..2063 (the file's last 258 bytes): a
    /// named instruction fragment (`"C\tmeet 3"`), roughly 150 bytes of
    /// unaccounted-for numeric data, then the file's trailing three untabbed
    /// fragments -- design name, author, and footnote -- exactly as confirmed
    /// against this detail id's own `diagram_details`/`diagram_entries` rows
    /// (title `"Voltolini - Thank you Bernd"`, `designer_info` `"Voltolini"`) in
    /// the module docs.
    const GEM_TAIL: &[u8] = &[
        0x08, 0x43, 0x09, 0x6d, 0x65, 0x65, 0x74, 0x20, 0x33, 0x01, 0x00, 0x00, 0x00, 0xcf, 0xa7,
        0x25, 0x00, 0xbc, 0xd5, 0xe8, 0x3f, 0x69, 0x60, 0xc9, 0x26, 0xc9, 0xb4, 0xe6, 0xbf, 0xce,
        0x86, 0xa9, 0x97, 0xfd, 0x27, 0xc2, 0x3f, 0x01, 0x00, 0x00, 0x00, 0xd0, 0xa7, 0x25, 0x00,
        0xbc, 0xd5, 0xe8, 0x3f, 0xff, 0x56, 0x46, 0xdc, 0x96, 0x07, 0xf1, 0x3f, 0xa6, 0x86, 0xa9,
        0x97, 0xfd, 0x27, 0xc2, 0x3f, 0x01, 0x00, 0x00, 0x00, 0x76, 0xb4, 0xc4, 0xdc, 0xae, 0xf6,
        0xda, 0xbf, 0xff, 0x56, 0x46, 0xdc, 0x96, 0x07, 0xf1, 0x3f, 0xa0, 0x86, 0xa9, 0x97, 0xfd,
        0x27, 0xc2, 0x3f, 0x01, 0x00, 0x00, 0x00, 0x75, 0xb4, 0xc4, 0xdc, 0xae, 0xf6, 0xda, 0xbf,
        0x6d, 0x60, 0xc9, 0x26, 0xc9, 0xb4, 0xe6, 0xbf, 0xa9, 0x86, 0xa9, 0x97, 0xfd, 0x27, 0xc2,
        0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x69, 0xf8, 0xc0, 0x01, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa0, 0xff, 0xff, 0xff, 0xa4, 0x70, 0x3d, 0x0a, 0xd7,
        0xa3, 0xf8, 0x3f, 0xff, 0x7f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x0f, 0x54, 0x68, 0x61, 0x6e, 0x6b, 0x20, 0x79, 0x6f, 0x75, 0x20, 0x42, 0x65, 0x72, 0x6e,
        0x64, 0x0f, 0x4d, 0x61, 0x72, 0x63, 0x6f, 0x20, 0x56, 0x6f, 0x6c, 0x74, 0x6f, 0x6c, 0x69,
        0x6e, 0x69, 0x00, 0x00, 0x37, 0x46, 0x6f, 0x72, 0x20, 0x71, 0x75, 0x61, 0x72, 0x74, 0x7a,
        0x2e, 0x20, 0x52, 0x65, 0x71, 0x75, 0x69, 0x72, 0x65, 0x73, 0x20, 0x73, 0x6f, 0x6d, 0x65,
        0x20, 0x63, 0x61, 0x72, 0x76, 0x69, 0x6e, 0x67, 0x2c, 0x20, 0x65, 0x2e, 0x67, 0x2e, 0x20,
        0x61, 0x20, 0x22, 0x62, 0x75, 0x62, 0x62, 0x6c, 0x65, 0x22, 0x20, 0x6f, 0x6e, 0x20, 0x36,
        0x00, 0x00, 0x00,
    ];
    const _: () = assert!(GEM_TAIL.len() == 258);

    #[test]
    fn recovers_named_facet_note() {
        let design = parse_gem(GEM_NAMED_NOTE).expect("real bytes must parse");
        assert_eq!(design.notes.len(), 1);
        assert_eq!(design.notes[0].facet_name.as_deref(), Some("1"));
        assert_eq!(design.notes[0].text, "cut stone outline");
        assert_eq!(design.notes[0].offset, 4);
    }

    #[test]
    fn recovers_unnamed_note() {
        let design = parse_gem(GEM_UNNAMED_NOTE).expect("real bytes must parse");
        assert_eq!(design.notes.len(), 1);
        assert_eq!(design.notes[0].facet_name, None);
        assert_eq!(design.notes[0].text, "cut stone outline");
    }

    #[test]
    fn recovers_tail_metadata_after_a_named_note() {
        let design = parse_gem(GEM_TAIL).expect("real bytes must parse");
        assert_eq!(design.notes.len(), 4);

        assert_eq!(design.notes[0].facet_name.as_deref(), Some("C"));
        assert_eq!(design.notes[0].text, "meet 3");

        assert_eq!(design.notes[1].facet_name, None);
        assert_eq!(design.notes[1].text, "Thank you Bernd");
        assert_eq!(design.notes[2].facet_name, None);
        assert_eq!(design.notes[2].text, "Marco Voltolini");
        assert_eq!(design.notes[3].facet_name, None);
        assert_eq!(
            design.notes[3].text,
            "For quartz. Requires some carving, e.g. a \"bubble\" on 6"
        );
    }

    #[test]
    fn probable_metadata_stops_at_the_last_named_note() {
        let design = parse_gem(GEM_TAIL).expect("real bytes must parse");
        let metadata = design.probable_metadata();
        assert_eq!(metadata.len(), 3);
        assert_eq!(metadata[0].text, "Thank you Bernd");
        assert_eq!(metadata[1].text, "Marco Voltolini");
        assert!(metadata[2].text.starts_with("For quartz"));
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse_gem(&[]).is_err());
    }

    #[test]
    fn does_not_panic_on_arbitrary_garbage() {
        let samples: &[&[u8]] = &[
            &[0xff; 32],
            &[0x00; 32],
            b"no length prefixes here at all, just prose",
            &[0x05, b'h', b'i'], // length byte claims more bytes than exist
        ];
        for s in samples {
            let _ = parse_gem(s);
        }
    }
}
