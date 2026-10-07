//! The per-tier tables of the native document: [`TierTable`] (one `[[tiers]]` entry) and
//! [`ConcaveTierTable`] (one concave tier of a self-contained design file). Split from
//! the parent module so the file stays readable; the types are unchanged.

use super::{NativeMeetConstraint, NativeTierTarget};

#[cfg(doc)]
use super::{NativeDesignFile, PreformTable};

/// One `[[tiers]]` entry: this native file's per-tier overlay -- or, for a
/// [`NativeDesignFile::draft`] save, the sole record of that tier at all.
///
/// Ordinarily NOT a full mirror of the editor's own tier type -- `angle_deg`/
/// `indices` stay canonical in the paired `.asc`; this otherwise only carries what
/// `.asc` cannot express (the authored [`NativeMeetConstraint`], which orbit members
/// are detached, and -- since the "gap this closes" module docs -- the meet
/// instruction and raw notes text a real `.asc` file's `G` field stated at import
/// time). `name` is purely a human-readable label for raw-TOML readers (e.g. `git
/// diff`); loading a paired (non-draft) file never reads it back into a design.
/// Tiers correlate to the paired `.asc`'s tier list by ARRAY POSITION alone -- see
/// the parent module's "The fingerprint" section for why a fingerprint mismatch
/// disables re-applying this overlay, and [`NativeDesignFile::draft`]'s own doc
/// comment for the one case where `angle_deg`/`indices` here ARE read back (a design
/// that does not currently solve has no reliable paired `.asc` to read them from at
/// all).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TierTable {
    /// Display name.
    pub name: String,
    /// Meet-point constraint.
    pub constraint: NativeMeetConstraint,
    /// Detached parameter values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<f64>,
    /// Mirrors `indicatrix_cut_core::design::ConstraintTier::angle_deg`. `#[serde(default)]`
    /// so an ordinary (non-draft) file saved before this field existed, or one whose
    /// tiers are only ever an overlay on a solvable paired `.asc`, still loads --
    /// `None` there is never read back into a design; see this type's own doc
    /// comment for the one caller (a draft reload) that does read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_deg: Option<f64>,
    /// Mirrors `indicatrix_cut_core::design::ConstraintTier::indices`. See
    /// [`Self::angle_deg`]'s own doc comment -- same rule, same one reader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indices: Option<Vec<f64>>,
    /// Mirrors `indicatrix_cut_core::design::ConstraintTier::imported_meet`: the meet
    /// instruction a real `.asc` file's `G` field actually stated for this tier at
    /// import time, preserved so a native reload can restore the editor's one-click
    /// "Adopt" action without needing to re-derive it from `.asc` text that (for a
    /// draft save) may not even be trustworthy. `#[serde(default)]` so a file saved
    /// before this field existed still loads, as `None` (nothing to adopt on
    /// reload from such a file, same as a tier `indicatrix_cut_core` never imported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_meet: Option<NativeMeetConstraint>,
    /// Mirrors `indicatrix_cut_core::design::ConstraintTier::original_notes`: the raw
    /// `.asc` `G`-field text this tier's file actually carried at import time,
    /// verbatim. `#[serde(default)]` for the same before-this-field-existed reason as
    /// [`Self::imported_meet`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_notes: Option<String>,
    /// A cutter-authored free-text note for this tier --
    /// unlike [`Self::original_notes`] (read-only imported `.asc` `G`-field text),
    /// this is a new authoring surface with no `.asc` counterpart to round-trip
    /// through, so it lives only here. `#[serde(default)]` so a file saved before
    /// this field existed still loads, as `None` (no note yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A per-tier "cheater"/azimuth-offset annotation, in degrees -- mirrors
    /// `indicatrix_cut_core::design::Design::cheater_offsets_deg`'s own map, keyed
    /// by this tier's array position. Like [`Self::note`],
    /// this is authored, undoable data with no `.asc` counterpart to round-trip
    /// through, so it lives only here. `#[serde(default)]` so a file saved before
    /// this field existed still loads, as `None` (no cheater offset recorded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cheater_offset_deg: Option<f64>,
    /// A stable per-tier identity, mirroring
    /// `indicatrix_cut_core::design::TierId::value` --
    /// `cheater_offset_deg`/`note` above are keyed by ARRAY POSITION
    /// (this whole table already documents that), which renumbers on add/remove/
    /// move; a `TierId` does not. `#[serde(default)]` so a file saved before this
    /// field existed still loads, as `None` -- the loader is expected to assign a
    /// fresh id to every such tier on load (old files have no stable identity to
    /// recover, only a fresh one to start from).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_id: Option<u64>,
    /// This tier's authoring-level target, if any -- see [`NativeTierTarget`].
    /// `#[serde(default)]` so a file saved before this field existed still loads,
    /// as `None` (no target authored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<NativeTierTarget>,
    /// The relation driving this tier's angle, in canonical text (`@3 - 2`: tiers by
    /// stable id, so a rename never breaks it) -- mirrors
    /// `indicatrix_cut_core::design::Design::tier_relations` one-to-one, and this
    /// crate does not interpret the text. `angle_deg` still holds the angle the
    /// relation gives. `#[serde(default, skip_serializing_if = ...)]` so a file with
    /// no relation never grows the key (its bytes do not change) and a file saved
    /// before relations existed loads as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_relation: Option<String>,
    /// See [`PreformTable::unknown`]'s doc comment -- the same rule, per tier.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

/// One concave (fantasy-cut) tier of a self-contained design file: the two-line
/// faceting-diagram standard's facet line plus its tool line, stored in the
/// authored form so nothing is lost (plan §6.1).
///
/// A separate table from [`TierTable`] on purpose: a concave tier has no meet
/// constraint, mast or target, and keeping it apart means a build that predates
/// concave tiers cannot mistake one for a flat tier (it refuses the file by
/// version instead). Tool and motion are the standard's own strings so the file
/// reads like the diagram: `tool` is `"CYL"`, `"CON"`, `"CIR"`, `"DSC"` or
/// `"SPH"`, `motion` is `"reciprocating"` or `"plunge"`. This crate does not
/// interpret them; the loader in `indicatrix-cut-core` does and refuses a
/// string it does not know. `tool`, `diameter_ratio` and `motion` have no serde
/// default, so a record missing one is an error rather than a guessed tool.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConcaveTierTable {
    /// Facet name, free text.
    pub name: String,
    /// φ in degrees, signed like a flat tier's `angle_deg`.
    pub angle_deg: f64,
    /// Index-wheel positions of the placements.
    pub indices: Vec<f64>,
    /// Free-text cutting instructions, kept verbatim.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    /// The tool code.
    pub tool: String,
    /// θ: direction of the tool axis in the facet plane, degrees.
    pub tool_azimuth_deg: f64,
    /// X, Y, Z displacement as ratios of the stone width.
    pub displacement: [f64; 3],
    /// Tool diameter as a ratio of the stone width.
    pub diameter_ratio: f64,
    /// Included angle of a cone or disc; absent for the other tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_angle_deg: Option<f64>,
    /// How the tool moves while it cuts.
    pub motion: String,
    /// A stable per-tier identity, the concave counterpart of [`TierTable::tier_id`]: the
    /// cutting-mode marks of a concave step hang on it, so it must survive a save and a
    /// reopen (the position in this list does not: tiers get reordered). Drawn from the
    /// same counter as the flat ids, so a value never repeats within a file.
    /// `#[serde(default, skip_serializing_if = ...)]` so a file saved before this key
    /// existed still loads, as `None` (the loader hands such a tier a fresh id), and a
    /// record without an id never grows the key. No version bump is needed: a build that
    /// predates the key parks it in [`Self::unknown`] and writes it back unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concave_tier_id: Option<u64>,
    /// See [`PreformTable::unknown`]'s doc comment -- the same rule, per concave tier.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl TierTable {
    /// Builds a fresh entry with no unknown/future fields, and no `angle_deg`/
    /// `indices`/`imported_meet`/`original_notes`/`cheater_offset_deg` (all `None`)
    /// carried over -- see [`PreformTable::new`]'s own doc comment for why this
    /// lives here rather than in `indicatrix-cut-core`. A caller with one of those
    /// five to attach uses the matching `with_*` method afterward.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        constraint: NativeMeetConstraint,
        detached: Vec<f64>,
    ) -> Self {
        Self {
            name: name.into(),
            constraint,
            detached,
            angle_deg: None,
            indices: None,
            imported_meet: None,
            original_notes: None,
            note: None,
            cheater_offset_deg: None,
            tier_id: None,
            target: None,
            angle_relation: None,
            unknown: toml::Table::new(),
        }
    }

    /// Attaches [`Self::angle_relation`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_angle_relation(mut self, angle_relation: Option<String>) -> Self {
        self.angle_relation = angle_relation;
        self
    }

    /// Attaches [`Self::tier_id`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_tier_id(mut self, tier_id: Option<u64>) -> Self {
        self.tier_id = tier_id;
        self
    }

    /// Attaches [`Self::target`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_target(mut self, target: Option<NativeTierTarget>) -> Self {
        self.target = target;
        self
    }

    /// Attaches [`Self::angle_deg`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_angle_deg(mut self, angle_deg: Option<f64>) -> Self {
        self.angle_deg = angle_deg;
        self
    }

    /// Attaches [`Self::indices`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_indices(mut self, indices: Option<Vec<f64>>) -> Self {
        self.indices = indices;
        self
    }

    /// Attaches [`Self::imported_meet`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_imported_meet(mut self, imported_meet: Option<NativeMeetConstraint>) -> Self {
        self.imported_meet = imported_meet;
        self
    }

    /// Attaches [`Self::original_notes`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_original_notes(mut self, original_notes: Option<String>) -> Self {
        self.original_notes = original_notes;
        self
    }

    /// Attaches [`Self::note`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_note(mut self, note: Option<String>) -> Self {
        self.note = note;
        self
    }

    /// Attaches [`Self::cheater_offset_deg`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_cheater_offset_deg(mut self, cheater_offset_deg: Option<f64>) -> Self {
        self.cheater_offset_deg = cheater_offset_deg;
        self
    }
}
