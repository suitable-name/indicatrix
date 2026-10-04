//! The Retarget dialog's toolkit-free half.
//!
//! Resolving its TARGET material from the dialog's combo/RI fields, the proposal view
//! model ([`retarget_view`]) that decides which of rows/notes/anchored-tier errors/solve
//! error a UI shows, and the per-row display text and risk badge color.

use super::{CrownShift, RetargetError, RetargetMode, RetargetProposal, RetargetRow};
use crate::{
    material::{
        design_material_index_from_name, design_material_options, parse_design_material_form,
    },
    material_lookup::{EditorMaterialLookup, resolved_gem_material},
};
use indicatrix::{geometry::meet_solver::Block, optics::materials::GemMaterial};
use indicatrix_cut_core::{Design, MaterialSelection, ResolvedMaterial, Risk};

/// Display view of one [`RetargetRow`].
#[derive(Debug, Clone)]
pub struct RetargetRowView {
    /// The tier's position in `design.tiers`.
    pub tier_index: usize,
    /// `"Crown"`/`"Pavilion"`/`"Girdle"`.
    pub block: &'static str,
    /// The tier's name.
    pub name: String,
    /// The current angle, two decimals and a degree sign.
    pub old_angle: String,
    /// The proposed angle, two decimals and a degree sign.
    pub new_angle: String,
    /// The signed margin over the target's critical angle.
    pub margin: String,
    /// `"Safe"`/`"Marginal"`/`"Windows"`.
    pub risk_label: &'static str,
    /// The risk badge color, `(r, g, b)`.
    pub risk_rgb: (u8, u8, u8),
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
        old_angle: format!("{:.2}\u{b0}", row.old_angle),
        new_angle: format!("{:.2}\u{b0}", row.new_angle),
        margin: format!("{:+.2}\u{b0}", row.margin_deg),
        risk_label,
        risk_rgb,
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
            };
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
