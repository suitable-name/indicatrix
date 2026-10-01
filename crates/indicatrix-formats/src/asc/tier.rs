//! [`AscTier`]: one `a` record -- a single facet tier at a given angle and
//! mast (height) setting, occurring at one or more index-wheel positions.

use super::meet_instruction::{MeetInstruction, parse_meet_instruction};

/// One `a` record: a single facet tier at a given angle and mast (height) setting,
/// occurring at one or more index-wheel positions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AscTier {
    /// Signed angle from the girdle plane, in degrees. `GemCAD` convention: negative is
    /// pavilion, positive is crown, and a zero angle is a flat facet whose side the
    /// SIGN of the zero records:
    ///
    /// - a sign-negative zero (`-0.0`, [`f64::is_sign_negative`]) is the pavilion
    ///   side -- the culet;
    /// - a positive zero (`0.0`) is always the crown side -- the table.
    ///
    /// The file itself states the culet as angle `0` with a NEGATIVE distance ("This
    /// number will be positive unless the facet is a culet (0° pavilion) facet",
    /// `GemCAD` for Windows manual p.21), or as a `-0.000000` angle token.
    /// [`super::parse_asc`] folds both spellings into this one representation
    /// (sign-negative zero angle, positive [`Self::mast`]), so no consumer ever
    /// needs the file order to tell a culet from a table.
    pub angle_deg: f64,
    /// The "mast" / height setting: how far the facet plane is cut from the stone's
    /// center. [`super::parse_asc`] turns the culet's documented negative distance
    /// into a positive one (moving the sign onto [`Self::angle_deg`]'s zero); any
    /// other value is stored as `GemCAD` wrote it, and callers should take the
    /// magnitude when turning this into a plane offset.
    pub mast: f64,
    /// Facet name(s) as written in the file (e.g. "P1", "C7", "G1", "1", "U"). Empty
    /// when the file leaves the facet unnamed, which is common -- roughly two-thirds
    /// of real tier records never name a facet at all. When more than one distinct
    /// name shares a single tier (rare, but real), they are joined with `/`, in
    /// first-seen order with consecutive repeats collapsed. This folded label is
    /// what every tier-level consumer (meet resolution, cut-sheet labels) reads;
    /// [`Self::index_names`] keeps the per-facet placement.
    pub name: String,
    /// Every index-wheel position at which this tier's facet occurs. Usually
    /// integers, but `GemCAD` allows fractional positions (about 0.2% of index tokens
    /// in the sampled corpus) for angles that don't land exactly on a gear tooth.
    pub indices: Vec<f64>,
    /// Every `n <name>` group exactly where the file put it, as `(position, name)`
    /// pairs whose position indexes [`Self::indices`]: `GemCAD` binds a name to the index written just
    /// before it ("Facet names are given after the n character after the
    /// corresponding index number", manual p.21), and its diagram labels that facet.
    /// A name written before any index gets position 0. Repeats are kept (a tier
    /// can label two facets with the same name).
    ///
    /// [`super::to_asc_string`] writes names back at these positions. Empty for a
    /// tier built by hand (e.g. exported from the editor): the writer then puts
    /// [`Self::name`] after the FIRST index, `GemCAD`'s own default labelling.
    pub index_names: Vec<(usize, String)>,
    /// Free-text notes after the `G` marker, if any (e.g. "Cut to mast depth X.").
    pub notes: String,
}

impl AscTier {
    /// Every distinct name this tier is known by, split back out of the joined
    /// `name` field (see that field's doc comment for why more than one name folds
    /// into a single `/`-joined string). Empty if the tier is unnamed.
    ///
    /// `/` is the separator and is never escaped: a name that itself contains `/`
    /// (typed by hand, for instance) is indistinguishable from a group of names and
    /// is split into several here.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        if self.name.is_empty() {
            Vec::new()
        } else {
            self.name.split('/').collect()
        }
    }

    /// `true` for a zero-angle tier on the pavilion side (the culet): a
    /// sign-negative zero [`Self::angle_deg`]. See that field's doc comment.
    #[must_use]
    pub const fn is_culet(&self) -> bool {
        self.angle_deg == 0.0 && self.angle_deg.is_sign_negative()
    }

    /// Parses `notes` (the raw text after this tier's `G` marker, if any) into a
    /// structured [`MeetInstruction`]. Returns `None` when there are no notes at all.
    ///
    /// This is computed on demand from the same text [`super::to_asc_string`] writes back
    /// out verbatim -- there is no separate stored field, so there is nothing that
    /// could drift out of sync with the raw text or put the round-trip property at
    /// risk.
    #[must_use]
    pub fn meet_instruction(&self) -> Option<MeetInstruction> {
        parse_meet_instruction(&self.notes)
    }
}
