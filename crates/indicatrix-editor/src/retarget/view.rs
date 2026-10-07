//! The Retarget dialog's toolkit-free half.
//!
//! Resolving its TARGET material from the dialog's combo/RI fields, the proposal view
//! model ([`retarget_view`]) that decides which of rows/notes/anchored-tier errors/solve
//! error a UI shows, and the per-row display text and risk badge color.

use super::{
    CrownShift, PlanRow, RetargetError, RetargetMode, RetargetPlan, RetargetProposal, RetargetRow,
    check::RetargetCheck, plan::tier_display_names, validity::ValidityStatus,
};
use crate::{
    material::{
        design_material_index_from_name, design_material_options, parse_design_material_form,
    },
    material_lookup::{EditorMaterialLookup, resolved_gem_material},
};
use indicatrix::{geometry::meet_solver::Block, optics::materials::GemMaterial};
use indicatrix_cut_core::{
    Design, MaterialSelection, ResolvedMaterial, Risk, optics_hints::AngleGuard,
};

/// The risk label of a row whose tier is listed but not changed (the table, the culet).
///
/// `ui/components/retarget_dialog.slint` greys a row by comparing against this exact text.
pub const NOT_CHANGED_LABEL: &str = "Not changed";

/// The risk label of a row whose angle follows a relation: it is not shifted on its own, it
/// moves with the tiers it reads.
///
/// `ui/components/retarget_dialog.slint` mutes the cell by comparing against this exact text.
pub const FOLLOWS_LABEL: &str = "Follows";

/// The line added to an invalid Shift verdict: the way out.
pub const OPTIMIZE_HINT: &str = "Shift alone is not valid here. Use Optimize: it also adjusts the crown and pavilion angles within a range you choose.";

/// The text in a cell that has no value for its row (a flat tier's margin, a row with no
/// risk). A dash, not a symbol glyph: the UI font draws it.
pub const NO_VALUE: &str = "\u{2014}";

/// Display view of one [`RetargetRow`] or [`PlanRow`].
#[derive(Debug, Clone)]
pub struct RetargetRowView {
    /// The tier's position in `design.tiers`.
    pub tier_index: usize,
    /// `"Crown"`/`"Pavilion"`/`"Girdle"`.
    pub block: &'static str,
    /// The tier's name. [`RetargetView::with_tier_names`] swaps an empty or old-style name
    /// (`1`, `A`) for the tier's standard code, like the tier table's NAME column.
    pub name: String,
    /// The current angle as a magnitude, two decimals and a degree sign: a person reads
    /// positive angles and the block column names the side.
    pub old_angle: String,
    /// The proposed angle as a magnitude, two decimals and a degree sign (plus `(held)`
    /// when the angle guard stopped the shift at a limit).
    pub new_angle: String,
    /// The signed margin over the target's critical angle, or [`NO_VALUE`]. A margin is a
    /// difference, not an angle, so it keeps its sign.
    pub margin: String,
    /// `"Safe"`/`"Marginal"`/`"Windows"`, [`NOT_CHANGED_LABEL`] for a tier the retarget
    /// leaves alone, or [`NO_VALUE`] when the row has no risk to show.
    pub risk_label: &'static str,
    /// The risk badge color, `(r, g, b)`. Meaningless (zero) when `risk_label` is
    /// [`NOT_CHANGED_LABEL`] or [`NO_VALUE`]: the dialog uses a theme colour for those.
    pub risk_rgb: (u8, u8, u8),
    /// `false` for a tier the retarget lists but does not change.
    pub changed: bool,
}

/// Display view of one [`super::build_proposal`] call -- exactly one of `rows`
/// (with `notes`), `anchored_errors` or `solve_error` is populated.
#[derive(Debug, Clone, Default)]
pub struct RetargetView {
    /// The proposal's rows (empty on either error).
    pub rows: Vec<RetargetRowView>,
    /// The proposal's notes (empty on either error).
    pub notes: Vec<String>,
    /// `#N "name"` per anchored tier when Optimize mode refused.
    pub anchored_errors: Vec<String>,
    /// The solve error's text when Optimize mode could not solve the design.
    pub solve_error: String,
}

impl RetargetView {
    /// This view with every row's name replaced by what the tier table shows for that tier
    /// of `design`: the tier's own name, or its standard code when the name is empty or
    /// old-style (`1`, `A`). A row whose tier is not in `design` keeps its name.
    #[must_use]
    pub fn with_tier_names(mut self, design: &Design) -> Self {
        let names = tier_display_names(design);
        for row in &mut self.rows {
            if let Some(name) = names.get(row.tier_index) {
                row.name.clone_from(name);
            }
        }
        self
    }
}

/// The text of an angle in a retarget row: the magnitude with two decimals and a degree
/// sign. The block column names the side, so no sign is shown.
fn angle_text(angle_deg: f64) -> String {
    format!("{:.2}\u{b0}", angle_deg.abs())
}

/// [`Risk`]'s label plus its RGB badge color -- chosen HERE, once, so the two can
/// never drift apart (the desktop theme's emerald/amber/ruby accents).
#[must_use]
pub const fn risk_label_and_rgb(risk: Risk) -> (&'static str, (u8, u8, u8)) {
    match risk {
        Risk::Safe => ("Safe", (0x10, 0xb9, 0x81)),
        Risk::Marginal => ("Marginal", (0xf5, 0x9e, 0x0b)),
        Risk::Windows => ("Windows", (0xf4, 0x3f, 0x5e)),
    }
}

/// The display view of one proposal row.
#[must_use]
pub fn row_view(row: &RetargetRow) -> RetargetRowView {
    let block = match row.block {
        Block::Crown => "Crown",
        Block::Pavilion => "Pavilion",
        // Never actually produced by `build_proposal` (girdle tiers are never
        // listed), but this stays total rather than panicking on a future change.
        Block::Girdle => "Girdle",
    };
    let (risk_label, risk_rgb) = risk_label_and_rgb(row.risk);
    RetargetRowView {
        tier_index: row.tier_index,
        block,
        name: row.name.clone(),
        old_angle: angle_text(row.old_angle),
        new_angle: angle_text(row.new_angle),
        margin: format!("{:+.2}\u{b0}", row.margin_deg),
        risk_label,
        risk_rgb,
        changed: true,
    }
}

/// The display view of one plan row.
///
/// - A flat tier (`moves == false`) reads [`NOT_CHANGED_LABEL`] with no margin.
/// - A row the angle guard held shows `(held)` after the new angle.
/// - A crown margin is the crown-window estimate, so it carries ` est.`.
/// - A row with no margin to show (a crown row of a design with no pavilion) reads
///   [`NO_VALUE`] for both the margin and the risk.
/// - A row that follows a relation reads `= <relation>` where the margin goes and
///   [`FOLLOWS_LABEL`] where the risk goes, with the angle the relation gives.
#[must_use]
pub fn plan_row_view(row: &PlanRow) -> RetargetRowView {
    let block = match row.block {
        Block::Crown => "Crown",
        Block::Pavilion => "Pavilion",
        Block::Girdle => "Girdle",
    };
    let held = if row.guard == AngleGuard::Within {
        ""
    } else {
        " (held)"
    };
    let new_angle = format!("{}{held}", angle_text(row.new_angle));
    let old_angle = angle_text(row.old_angle);
    if let Some(relation) = &row.follows {
        return RetargetRowView {
            tier_index: row.tier_index,
            block,
            name: row.name.clone(),
            old_angle,
            new_angle,
            margin: format!("= {relation}"),
            risk_label: FOLLOWS_LABEL,
            risk_rgb: (0, 0, 0),
            changed: (row.new_angle - row.old_angle).abs() > 1e-9,
        };
    }
    if !row.moves {
        return RetargetRowView {
            tier_index: row.tier_index,
            block,
            name: row.name.clone(),
            old_angle,
            new_angle,
            margin: NO_VALUE.to_string(),
            risk_label: NOT_CHANGED_LABEL,
            risk_rgb: (0, 0, 0),
            changed: false,
        };
    }
    let (margin, risk_label, risk_rgb) = match (row.margin_deg, row.risk) {
        (Some(margin), Some(risk)) => {
            let (label, rgb) = risk_label_and_rgb(risk);
            let suffix = if row.margin_is_estimate { " est." } else { "" };
            (format!("{margin:+.2}\u{b0}{suffix}"), label, rgb)
        }
        _ => (NO_VALUE.to_string(), NO_VALUE, (0, 0, 0)),
    };
    RetargetRowView {
        tier_index: row.tier_index,
        block,
        name: row.name.clone(),
        old_angle,
        new_angle,
        margin,
        risk_label,
        risk_rgb,
        changed: true,
    }
}

/// The rows and notes of a plan, as a [`RetargetView`] (no errors).
#[must_use]
pub fn plan_view(plan: &RetargetPlan) -> RetargetView {
    RetargetView {
        rows: plan.rows.iter().map(plan_row_view).collect(),
        notes: plan.notes.clone(),
        anchored_errors: Vec::new(),
        solve_error: String::new(),
    }
}

/// Where a retarget check stands, as the dialog needs to know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CheckState {
    /// No check is relevant (Optimize mode, an error, nothing to apply).
    #[default]
    None,
    /// A check is running; Apply is disabled until it reports.
    Checking,
    /// The retargeted stone passed.
    Valid,
    /// The retargeted stone failed; Apply is disabled.
    Invalid,
    /// The current design could not be compared; Apply stays available.
    Unchecked,
}

impl CheckState {
    /// The number `ui/models/retarget.slint` uses for this state
    /// (`0` none, `1` checking, `2` valid, `3` invalid, `4` unchecked).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Checking => 1,
            Self::Valid => 2,
            Self::Invalid => 3,
            Self::Unchecked => 4,
        }
    }

    /// `true` when the dialog's Apply button may be enabled.
    #[must_use]
    pub const fn allows_apply(self) -> bool {
        matches!(self, Self::None | Self::Valid | Self::Unchecked)
    }
}

/// The text of the dialog's validity block and metrics table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckView {
    /// Which state the block is in.
    pub state: CheckState,
    /// The one-line verdict (`Checking...` while a check runs).
    pub headline: String,
    /// Further reasons and warnings, one line each.
    pub details: Vec<String>,
    /// One sentence on how the masts were chosen ([`super::validity::RetargetStrategy::label`]).
    pub strategy: String,
    /// The nine metrics cells, row-major (windowing, brilliance, extinction; current,
    /// current in target, retargeted in target). Empty when there is nothing to show.
    pub metrics: Vec<String>,
}

impl CheckView {
    /// The view while a check is running.
    #[must_use]
    pub fn checking() -> Self {
        Self {
            state: CheckState::Checking,
            headline: "Checking...".to_string(),
            ..Self::default()
        }
    }

    /// This view with [`OPTIMIZE_HINT`] added when the retarget is not valid (the way out
    /// for Shift mode). Any other state is returned unchanged.
    #[must_use]
    pub fn with_optimize_hint(mut self) -> Self {
        if self.state == CheckState::Invalid {
            self.details.push(OPTIMIZE_HINT.to_string());
        }
        self
    }

    /// The view for a finished check.
    #[must_use]
    pub fn from_check(check: &RetargetCheck) -> Self {
        let state = match check.validity.status {
            ValidityStatus::Valid => CheckState::Valid,
            ValidityStatus::Invalid => CheckState::Invalid,
            ValidityStatus::Unchecked => CheckState::Unchecked,
        };
        let metrics = if check.metrics.any() {
            check.metrics.cells()
        } else {
            Vec::new()
        };
        Self {
            state,
            headline: check.validity.headline(),
            details: check.validity.detail_lines(),
            strategy: check.validity.strategy.label().to_string(),
            metrics,
        }
    }
}

/// Builds a proposal and its display view in one call. The second element is the
/// real [`RetargetProposal`] to hold for Apply, `None` for either
/// [`RetargetError`] variant.
///
/// `custom_materials` resolves the design's CURRENT refractive index through the
/// same lookup the rest of the editor uses -- every proposed angle is a shift from
/// that number.
#[must_use]
pub fn retarget_view(
    design: &Design,
    target: &ResolvedMaterial,
    crown: CrownShift,
    mode: RetargetMode,
    custom_materials: &[GemMaterial],
) -> (RetargetView, Option<RetargetProposal>) {
    match super::build_proposal(design, target, crown, mode, custom_materials) {
        Ok(proposal) => {
            let rows = proposal.rows.iter().map(row_view).collect();
            let view = RetargetView {
                rows,
                notes: proposal.notes.clone(),
                anchored_errors: Vec::new(),
                solve_error: String::new(),
            }
            .with_tier_names(design);
            (view, Some(proposal))
        }
        Err(RetargetError::AnchoredTiers(tiers)) => {
            let anchored_errors = tiers
                .iter()
                .map(|(index, name)| format!("#{index} \"{name}\""))
                .collect();
            let view = RetargetView {
                rows: Vec::new(),
                notes: Vec::new(),
                anchored_errors,
                solve_error: String::new(),
            };
            (view, None)
        }
        Err(RetargetError::Solve(err)) => {
            let view = RetargetView {
                rows: Vec::new(),
                notes: Vec::new(),
                anchored_errors: Vec::new(),
                solve_error: err.to_string(),
            };
            (view, None)
        }
    }
}

/// The dialog's target [`MaterialSelection`] from its combo index and RI-override
/// text, carrying the design's own specific-gravity override through.
///
/// # Errors
///
/// [`parse_design_material_form`]'s own message when `ri_override_text` is
/// non-empty and does not parse as a finite refractive index greater than 1.0 --
/// see [`target_material_selection`] for the lenient wrapper.
pub fn resolve_target_selection(
    design: &Design,
    custom: &[GemMaterial],
    combo_index: i32,
    ri_override_text: &str,
) -> Result<MaterialSelection, String> {
    let options = design_material_options(custom);
    parse_design_material_form(combo_index, ri_override_text, &options, &design.material)
}

/// [`resolve_target_selection`], falling back to `design.material` unchanged on a
/// parse error rather than surfacing it -- for a caller that needs SOME target
/// selection unconditionally.
#[must_use]
pub fn target_material_selection(
    design: &Design,
    custom: &[GemMaterial],
    combo_index: i32,
    ri_override_text: &str,
) -> MaterialSelection {
    resolve_target_selection(design, custom, combo_index, ri_override_text)
        .unwrap_or_else(|_| design.material.clone())
}

/// Resolves a [`MaterialSelection`] into a [`ResolvedMaterial`] against `custom`.
///
/// Using [`resolved_gem_material`] for the `gem` so an RI override shows up in the
/// target's own dispersion (it matters for `RetargetMode::Optimize`'s objective).
#[must_use]
pub fn resolved_material_from_selection(
    selection: &MaterialSelection,
    custom: &[GemMaterial],
) -> ResolvedMaterial {
    let lookup = EditorMaterialLookup::new(custom);
    let resolved = selection.resolve(&lookup);
    let gem = resolved_gem_material(selection, &lookup);
    ResolvedMaterial { gem, ..resolved }
}

/// The readout label for a retarget target: the name when one was picked, `"Custom RI"`
/// for a typed override with no name, `"(none)"` for neither.
///
/// Never the Diamond fallback the resolved `gem` carries for both.
#[must_use]
pub fn target_display_name(selection: &MaterialSelection) -> String {
    selection.name.clone().unwrap_or_else(|| {
        if selection.refractive_index_override.is_some() {
            "Custom RI".to_string()
        } else {
            "(none)".to_string()
        }
    })
}

/// The target combo's initial index for `material`.
///
/// The design's own current material by name, except a name-less selection carrying an RI
/// override (the "Custom RI..." case) which has no name to look up and must instead seed
/// the trailing sentinel entry [`design_material_options`] always appends.
#[must_use]
pub fn initial_target_index(material: &MaterialSelection, options: &[String]) -> i32 {
    if material.name.is_none() && material.refractive_index_override.is_some() {
        return i32::try_from(options.len()).unwrap_or(i32::MAX) - 1;
    }
    design_material_index_from_name(material.name.as_deref(), options)
}
