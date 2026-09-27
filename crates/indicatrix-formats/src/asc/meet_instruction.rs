//! [`MeetInstruction`]: a parsed `G`-field cutting/meet instruction, and the
//! lenient free-text parser ([`parse_meet_instruction`]) behind
//! [`super::AscTier::meet_instruction`].

/// A parsed `G`-field cutting/meet instruction.
///
/// `GemCAD` schedules record these as free text after a tier's `a` record (e.g. `a
/// -90.000000 0.58736554 69 n G2 27 G Meet P1, P2, G1`); this is what
/// [`super::AscTier::meet_instruction`] parses that text into.
///
/// Parsing is deliberately lenient (case-insensitive keyword matching, not a strict
/// grammar) since these are hand-typed free-text notes -- an instruction this module
/// doesn't recognize lands in [`MeetInstruction::Other`] rather than erroring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeetInstruction {
    /// `"Meet <name>[, <name>...]"` -- explicit facet-name references this tier is
    /// stated to close against (e.g. `"Meet P1, P2, G1"` -> `["P1", "P2", "G1"]`).
    /// Names are kept exactly as written; resolving them against other tiers' names
    /// is the caller's job (this module has no notion of "other tiers").
    Meet(Vec<String>),
    /// `"Cut to centerpoint"` / `"Cut to TCP"` / `"Cut to PCP"` / a bare `"TCP"` /
    /// `"PCP"` -- meets at the crown or pavilion's central closing point. Distinct
    /// from [`Self::Meet`] only in that no specific facet names are given; it is
    /// still support-function tangency against the solid formed so far (see
    /// `indicatrix::geometry::meet_solver`'s module docs).
    CutToCenterpoint,
    /// `"GMP"` / `"Girdle meet point"` -- meets at the girdle edge. Same tangency
    /// semantics as [`Self::CutToCenterpoint`], just at a different point.
    GirdleMeetPoint,
    /// `"Level girdle[.]"` -- this facet is cut to bring the (already-mounted, still
    /// rough) girdle to a true, level plane. Conventionally one of the very first
    /// cuts made, before anything else exists to meet against, so its mast is a
    /// directly chosen/measured value rather than a meet-derived one.
    LevelGirdle,
    /// `"Set girdle width"` / `"Set girdle thickness"` / `"Establish girdle
    /// thickness"` / `"Set stone size"` -- an externally supplied scale choice, not
    /// derivable from other facets.
    ScaleReference,
    /// Any other free-text note that doesn't match a recognized instruction verb
    /// (e.g. `"Cut to mast depth X."`, `"Or continuous girdle"`, a stray comment).
    Other(String),
}

/// Parses one tier's raw `G`-field text into a [`MeetInstruction`]. See
/// [`MeetInstruction`]'s doc comment for the recognized verbs and real examples.
pub(super) fn parse_meet_instruction(notes: &str) -> Option<MeetInstruction> {
    let text = notes.trim();
    if text.is_empty() {
        return None;
    }
    let lower = text.to_ascii_lowercase();

    if lower.starts_with("meet") {
        return Some(MeetInstruction::Meet(extract_meet_names(text)));
    }
    if lower.contains("gmp")
        || (lower.contains("girdle") && lower.contains("meet") && lower.contains("point"))
    {
        return Some(MeetInstruction::GirdleMeetPoint);
    }
    if lower.contains("level") && lower.contains("girdle") {
        return Some(MeetInstruction::LevelGirdle);
    }
    if lower.contains("centerpoint")
        || lower.contains("center point")
        || lower == "tcp"
        || lower == "pcp"
        || lower.contains("cut to tcp")
        || lower.contains("cut to pcp")
    {
        return Some(MeetInstruction::CutToCenterpoint);
    }
    if lower.contains("set girdle")
        || lower.contains("set stone size")
        || lower.contains("girdle thickness")
        || lower.contains("girdle width")
        || lower.contains("establish girdle")
    {
        return Some(MeetInstruction::ScaleReference);
    }
    Some(MeetInstruction::Other(text.to_string()))
}

/// Extracts the facet-name list from a `"Meet ..."` instruction's text (original case
/// preserved). Tolerant of both comma- and whitespace-separated lists, and strips
/// stray leading/trailing punctuation from each name.
///
/// Also drops lowercase English connector words ("Meet 2 and the culet" -> `["2",
/// "culet"]`): measured against the corpus, these were 17% of every unresolved name
/// token, and since a caller resolving a `Meet` instruction typically requires every
/// listed name to resolve, one spurious "and" was silently sinking the whole tier.
/// Case-sensitive and restricted to unambiguous, multi-letter connector words only --
/// lowercase single-letter facet names are common here, so "a"/"an" are never
/// filtered despite also being articles.
fn extract_meet_names(text: &str) -> Vec<String> {
    // `text` is known (by the caller) to start with an ASCII case-insensitive match
    // of "meet", which is 4 ASCII bytes, so this slice is always on a char boundary.
    let after = text.get(4..).unwrap_or("");
    after
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|s| !s.is_empty() && !is_connector_word(s))
        .map(str::to_string)
        .collect()
}

/// Lowercase-only English connector words that appear in hand-typed `"Meet ..."`
/// prose but are never a facet name -- see [`extract_meet_names`]. Case-sensitive:
/// an uppercase `"And"`/`"THE"` never occurs in the sampled corpus's connector usage.
fn is_connector_word(token: &str) -> bool {
    matches!(token, "and" | "the" | "or")
}
