//! The Checks tab: the overall verdict and the findings behind it, the proportions, the
//! preform and yield figures, and the edits behind the preform fields and the one-step
//! fixes.
//!
//! The checks need a solve that matches the design, so [`checks`] reads
//! `AppState::solve` (`current_tiers` at the session's generation); without one it
//! returns a placeholder that says so. The result is cached for the generation it was
//! made for (the solid mesh behind the proportions takes a few milliseconds), so a refresh
//! that changed nothing costs a comparison.
//!
//! # Fixes
//!
//! A finding may carry a one-edit fix (snap to the gear, remove cut-away facets, steepen a
//! windowing pavilion, add a table...). Planning one solves candidate designs, so it is
//! split in three: [`fix_request`] copies what is needed (cheap, on the UI thread),
//! [`FixRequest::plan`] does the work (call it on a worker thread) and
//! [`apply_planned_fix`] applies the plan as one undoable edit -- unless the design moved
//! on meanwhile, which refuses the plan instead of applying it to the wrong design.

use std::panic::{AssertUnwindSafe, catch_unwind};

use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use indicatrix_cut_core::{ConstraintTier, Design, Edit, MaterialSelection, PreformShape};
use indicatrix_editor::{
    loading::{eval_number, preform_source_note},
    verdict::{self, FixAction, FixPlan, Reason, plan_fix},
    view_model::{
        row_format::representative_crown_and_pavilion_angles_deg,
        yield_report::{
            girdle_and_ratio_texts_from_solved, preform_mm_texts_from_solved, proportion_verdicts,
            proportions_texts_from_solved, yield_report_texts_from_solved,
        },
    },
};

use super::{materials::apply_selection, parse_optional_positive, stone::set_stone_width, trimmed};
use crate::app_state::AppState;

/// The text a proportion shows when there is nothing to measure.
const DASH: &str = "-";

/// Said when a fix is asked for before the design has a current solve.
const SOLVE_FIRST: &str = "Solve the design first.";

/// Said when the planner stopped unexpectedly.
const PLANNER_FAILED: &str = "The fix stopped unexpectedly. Nothing was changed.";

/// One finding of the verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarningRow {
    /// The tier to select for "Show" (a position in the design's tier list), if the
    /// finding is about one.
    pub tier: Option<usize>,
    /// One plain sentence.
    pub text: String,
    /// `1` check, `2` problem.
    pub level: i32,
    /// The Fix button's label, when a one-edit fix exists.
    pub fix_label: Option<String>,
    /// What the fix does, for the info sheet.
    pub fix_hint: Option<String>,
    /// A question to ask before the fix runs (a removal), `None` when it needs none.
    pub confirm: Option<String>,
}

/// One line of the Proportions table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProportionRow {
    /// What is measured.
    pub label: String,
    /// The figure as text (`-` when it cannot be measured).
    pub value: String,
    /// `0` within the usual range, `1` near its edge, `2` outside, `-1` not judged.
    pub level: i32,
    /// The reference window's explanation, empty when not judged.
    pub reason: String,
}

/// The preform and yield inputs as editable text, plus what they mean.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreformForm {
    /// `Block`, `Cylinder` or `Planned Piece`.
    pub shape_label: String,
    /// Half the rough's width, in the design's own units.
    pub half_width: String,
    /// The rough's length over its width.
    pub length_over_width: String,
    /// The rough's total height.
    pub depth: String,
    /// The half-width in millimetres (`≈ 1.234 mm`), empty without a solve and a width.
    pub half_width_mm: String,
    /// The depth in millimetres, empty without a solve and a width.
    pub depth_mm: String,
    /// The girdle diameter in millimetres (the same value as the Stone tab's width).
    pub girdle_mm: String,
    /// The specific-gravity override; empty when the material's own applies.
    pub specific_gravity: String,
    /// A sentence about a planned piece of rough, empty for a block or cylinder.
    pub source_note: String,
    /// Whether the size fields can be edited (not for a planned piece).
    pub editable: bool,
}

/// Everything the Checks tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksView {
    /// Whether a solve matching the design backs these checks.
    pub ready: bool,
    /// Why the checks are not ready, empty when they are.
    pub note: String,
    /// The verdict's one-line headline (or `note` while not ready).
    pub verdict_text: String,
    /// `0` good, `1` check, `2` problem.
    pub verdict_level: i32,
    /// The findings, worst first; the index is what [`fix_request`] takes.
    pub warnings: Vec<WarningRow>,
    /// The proportions table (empty for a design without tiers).
    pub proportions: Vec<ProportionRow>,
    /// The preform and yield inputs.
    pub preform: PreformForm,
    /// The yield, estimated weight and specific-gravity lines.
    pub yield_lines: Vec<String>,
    /// The estimated weight alone (`1.2345 ct (est.)`), when known.
    pub carat: Option<String>,
}

/// A computed [`ChecksView`] with what the fixes need.
struct Computed {
    view: ChecksView,
    reasons: Vec<Reason>,
    n_d: f64,
}

/// What a cached result was made for.
#[derive(Debug, Clone, PartialEq)]
struct CacheKey {
    generation: u64,
    tier_count: usize,
    material: MaterialSelection,
    custom_count: usize,
    girdle_mm: Option<f64>,
}

impl CacheKey {
    fn of(state: &AppState, generation: u64) -> Self {
        Self {
            generation,
            tier_count: state.session.design.tiers.len(),
            material: state.session.design.material.clone(),
            custom_count: state.props.custom_materials.len(),
            girdle_mm: state.session.design.girdle_diameter_mm,
        }
    }
}

/// The remembered result of [`checks`].
#[derive(Debug)]
pub(super) struct CachedChecks {
    key: CacheKey,
    view: ChecksView,
    reasons: Vec<Reason>,
    n_d: f64,
}

fn warning_row(reason: &Reason) -> WarningRow {
    WarningRow {
        tier: reason.tier,
        text: reason.text.clone(),
        level: reason.level.code(),
        fix_label: reason.fix.as_ref().map(|fix| fix.label().to_owned()),
        fix_hint: reason.fix.as_ref().map(|fix| fix.hint().to_owned()),
        confirm: reason.confirm.clone(),
    }
}

/// The proportions table for a solved design.
fn proportion_rows(design: &Design, solved: &[SolvedTier], n_d: f64) -> Vec<ProportionRow> {
    if design.tiers.is_empty() {
        return Vec::new();
    }
    let (table, crown_height, pavilion_depth, total_depth, length_to_width) =
        proportions_texts_from_solved(design, solved);
    let (girdle, crown_pct, pavilion_pct, girdle_pct) =
        girdle_and_ratio_texts_from_solved(design, solved);
    let [table_v, crown_v, pavilion_v, depth_v, girdle_v] = design
        .stone_proportions(solved)
        .map(|proportions| proportion_verdicts(design, &proportions, n_d))
        .map_or_else(
            || std::array::from_fn(|_| (-1, String::new())),
            |v| {
                [
                    v.table_pct,
                    v.crown_angle,
                    v.pavilion_angle,
                    v.total_depth_pct,
                    v.girdle_pct,
                ]
                .map(|judged| (judged.level, judged.reason))
            },
        );
    let (crown_angle, pavilion_angle) = representative_crown_and_pavilion_angles_deg(design);
    let angle_text = |angle: Option<f64>| {
        angle.map_or_else(|| DASH.to_owned(), |degrees| format!("{degrees:.1}\u{b0}"))
    };
    let row = |label: &str, value: String, (level, reason): (i32, String)| ProportionRow {
        label: label.to_owned(),
        value,
        level,
        reason,
    };
    let unjudged = || (-1, String::new());
    vec![
        row("Table", table, table_v),
        row("Crown Angle", angle_text(crown_angle), crown_v),
        row("Pavilion Angle", angle_text(pavilion_angle), pavilion_v),
        row("Crown Height", crown_height, unjudged()),
        row("Pavilion Depth", pavilion_depth, unjudged()),
        row("Girdle Thickness", girdle, girdle_v),
        row("Total Depth", total_depth, depth_v),
        row("Length / Width", length_to_width, unjudged()),
        row("Crown / Width", crown_pct, unjudged()),
        row("Pavilion / Width", pavilion_pct, unjudged()),
        row("Girdle / Width", girdle_pct, unjudged()),
    ]
}

/// The preform fields of `design`; the millimetre texts need the matching solve.
fn preform_form(design: &Design, solved: Option<&[SolvedTier]>) -> PreformForm {
    let preform = &design.preform;
    let (half_width_mm, depth_mm) = solved.map_or_else(
        || (String::new(), String::new()),
        |solved| preform_mm_texts_from_solved(design, solved),
    );
    PreformForm {
        shape_label: match &preform.shape {
            PreformShape::Block => "Block",
            PreformShape::Cylinder { .. } => "Cylinder",
            PreformShape::Planes(_) => "Planned Piece",
        }
        .to_owned(),
        half_width: trimmed(preform.half_width, 4),
        length_over_width: trimmed(preform.length_over_width, 4),
        depth: trimmed(preform.depth, 4),
        half_width_mm,
        depth_mm,
        girdle_mm: design
            .girdle_diameter_mm
            .map_or_else(String::new, |mm| trimmed(mm, 3)),
        specific_gravity: design
            .material
            .specific_gravity_override
            .map_or_else(String::new, |value| trimmed(value, 3)),
        source_note: preform_source_note(preform),
        editable: !matches!(preform.shape, PreformShape::Planes(_)),
    }
}

/// The checks of a solved `design`. Pure: tests call this directly.
fn compute(
    design: &Design,
    solved: &[SolvedTier],
    custom: &[GemMaterial],
    custom_sg: &[(String, f64)],
) -> Computed {
    let n_d = design.effective_refractive_index_with(custom);
    let inputs = verdict::gather(design, Some(solved), n_d, None);
    let judged = verdict::evaluate(&inputs);

    let (yield_text, carat_text, sg_text, fit_text) =
        yield_report_texts_from_solved(design, solved, custom_sg);
    let mut yield_lines = Vec::new();
    if !yield_text.is_empty() {
        yield_lines.push(format!("Yield: {yield_text} of the rough"));
    }
    if !carat_text.is_empty() {
        yield_lines.push(format!("Weight: {carat_text}"));
    }
    if !sg_text.is_empty() {
        yield_lines.push(format!("Specific gravity: {sg_text}"));
    }
    if !fit_text.is_empty() {
        yield_lines.push(fit_text);
    }

    let view = ChecksView {
        ready: true,
        note: String::new(),
        verdict_text: judged.headline.clone(),
        verdict_level: judged.level.code(),
        warnings: judged.reasons.iter().map(warning_row).collect(),
        proportions: proportion_rows(design, solved, n_d),
        preform: preform_form(design, Some(solved)),
        yield_lines,
        carat: (!carat_text.is_empty()).then_some(carat_text),
    };
    Computed {
        view,
        reasons: judged.reasons,
        n_d,
    }
}

/// The placeholder while no solve matches the design.
fn not_ready(state: &AppState) -> ChecksView {
    let (status, problem) = state.solve.status();
    let solving = state.solve.is_solving();
    let (note, level) = if solving {
        ("Solving\u{2026}".to_owned(), 1)
    } else if problem {
        (status, 2)
    } else {
        ("Solve the design to see its checks.".to_owned(), 1)
    };
    ChecksView {
        ready: false,
        note: note.clone(),
        verdict_text: note,
        verdict_level: level,
        warnings: Vec::new(),
        proportions: Vec::new(),
        preform: preform_form(&state.session.design, None),
        yield_lines: Vec::new(),
        carat: None,
    }
}

/// Runs `read` on the cached checks of the current solve, making them first when the cache
/// is stale. `None` when no solve matches the design (or the cache lock is poisoned).
fn with_computed<R>(state: &AppState, read: impl FnOnce(&CachedChecks) -> R) -> Option<R> {
    let generation = state.session.current_generation();
    let solved = state.solve.current_tiers(generation)?;
    let key = CacheKey::of(state, generation);
    let mut cache = state.props.checks_cache.lock().ok()?;
    if cache.as_ref().is_none_or(|cached| cached.key != key) {
        let custom_sg = state.props.custom_specific_gravity();
        let computed = compute(
            &state.session.design,
            solved,
            &state.props.custom_materials,
            &custom_sg,
        );
        *cache = Some(CachedChecks {
            key,
            view: computed.view,
            reasons: computed.reasons,
            n_d: computed.n_d,
        });
    }
    cache.as_ref().map(read)
}

/// The Checks tab for the design as it stands: the verdict and the rest when a solve
/// matches the design, a placeholder that says what is missing otherwise.
#[must_use]
pub fn checks(state: &AppState) -> ChecksView {
    with_computed(state, |cached| cached.view.clone()).unwrap_or_else(|| not_ready(state))
}

/// A preform or yield field of the Checks tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreformField {
    /// Half the rough's width.
    HalfWidth,
    /// The rough's length over its width.
    LengthOverWidth,
    /// The rough's total height.
    Depth,
    /// The girdle diameter in millimetres.
    GirdleMm,
    /// The specific-gravity override.
    SpecificGravity,
}

impl PreformField {
    /// The field the UI calls `id` (`half-width`, `length-width`, `depth`, `girdle-mm`,
    /// `sg`).
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "half-width" => Some(Self::HalfWidth),
            "length-width" => Some(Self::LengthOverWidth),
            "depth" => Some(Self::Depth),
            "girdle-mm" => Some(Self::GirdleMm),
            "sg" => Some(Self::SpecificGravity),
            _ => None,
        }
    }

    /// The field's name in an error sentence.
    const fn label(self) -> &'static str {
        match self {
            Self::HalfWidth => "Half-width",
            Self::LengthOverWidth => "Length/width",
            Self::Depth => "Depth",
            Self::GirdleMm => "Girdle diameter",
            Self::SpecificGravity => "Specific gravity",
        }
    }
}

/// Sets one preform or yield field from `text` (arithmetic allowed) as one undoable edit.
///
/// The girdle diameter and the specific gravity may be blank (cleared); the three size
/// fields must be positive numbers.
///
/// # Errors
///
/// A sentence when the text is not an acceptable number, the rough is a planned piece (its
/// size is fixed), or the editor refuses.
pub fn set_preform_field(
    state: &mut AppState,
    field: PreformField,
    text: &str,
) -> Result<bool, String> {
    match field {
        PreformField::GirdleMm => set_stone_width(state, text),
        PreformField::SpecificGravity => {
            let value = parse_optional_positive(field.label(), text)?;
            let next = state
                .session
                .design
                .material
                .with_specific_gravity_override(value);
            apply_selection(state, next)
        }
        PreformField::HalfWidth | PreformField::LengthOverWidth | PreformField::Depth => {
            let design = &state.session.design;
            if matches!(design.preform.shape, PreformShape::Planes(_)) {
                return Err(
                    "This design's rough is a planned piece; its size is fixed here.".to_owned(),
                );
            }
            let label = field.label();
            let value = eval_number(text, None).map_err(|error| error.message(label, text))?;
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{label} must be a positive, finite number."));
            }
            let mut preform = design.preform.clone();
            match field {
                PreformField::HalfWidth => preform.half_width = value,
                PreformField::LengthOverWidth => preform.length_over_width = value,
                _ => preform.depth = value,
            }
            if preform == design.preform {
                return Ok(false);
            }
            state
                .session
                .apply(Edit::SetPreform { preform })
                .map(|_| true)
                .map_err(|error| error.to_string())
        }
    }
}

/// A fix ready to be planned: a copy of the design and what to do to it. Plan it with
/// [`Self::plan`] on a worker thread.
#[derive(Debug, Clone)]
pub struct FixRequest {
    design: Design,
    action: FixAction,
    n_d: f64,
    reference: Vec<ConstraintTier>,
}

/// A planned fix: the edit, and the tiers of the design it was planned for.
#[derive(Debug)]
pub struct PlannedFix {
    reference: Vec<ConstraintTier>,
    plan: FixPlan,
}

/// What an applied fix reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixApplied {
    /// What changed, as one sentence.
    pub message: String,
    /// The tier now selected (a position in the new tier list), if the fix names one.
    pub select: Option<usize>,
}

/// The fix request for the finding at `index` of the current [`ChecksView::warnings`].
fn request_from(
    design: &Design,
    reasons: &[Reason],
    n_d: f64,
    index: usize,
) -> Result<FixRequest, String> {
    let action = reasons
        .get(index)
        .and_then(|reason| reason.fix.clone())
        .ok_or("There is no fix for this finding.")?;
    Ok(FixRequest {
        design: design.clone(),
        action,
        n_d,
        reference: design.tiers.clone(),
    })
}

/// Prepares the fix of the finding at `index` of the current [`ChecksView::warnings`].
///
/// # Errors
///
/// A sentence when the design has no current solve or the finding has no fix.
pub fn fix_request(state: &AppState, index: usize) -> Result<FixRequest, String> {
    let (reasons, n_d) =
        with_computed(state, |cached| (cached.reasons.clone(), cached.n_d)).ok_or(SOLVE_FIRST)?;
    request_from(&state.session.design, &reasons, n_d, index)
}

impl FixRequest {
    /// Plans the fix. Solves candidate designs, so run it off the UI thread; the request is
    /// `Send`.
    ///
    /// # Errors
    ///
    /// The planner's sentence when no safe edit exists, or a notice that the planner
    /// stopped unexpectedly.
    pub fn plan(self) -> Result<PlannedFix, String> {
        let Self {
            design,
            action,
            n_d,
            reference,
        } = self;
        let plan = catch_unwind(AssertUnwindSafe(|| plan_fix(&design, &action, n_d)))
            .unwrap_or_else(|_| Err(PLANNER_FAILED.to_owned()))?;
        Ok(PlannedFix { reference, plan })
    }
}

/// Applies a planned fix as one undoable edit and selects the tier it names.
///
/// # Errors
///
/// A sentence when the design changed since the plan was made (nothing is applied then) or
/// the editor refuses the edit.
pub fn apply_planned_fix(state: &mut AppState, planned: PlannedFix) -> Result<FixApplied, String> {
    if state.session.design.tiers != planned.reference {
        return Err(
            "The design changed while the fix was being planned. Nothing was changed.".to_owned(),
        );
    }
    let FixPlan {
        edit,
        message,
        select,
    } = planned.plan;
    state
        .session
        .apply(edit)
        .map_err(|error| error.to_string())?;
    let tier_count = state.session.design.tiers.len();
    let select = select.filter(|&tier| tier < tier_count);
    state.selected_tier = select.or_else(|| state.selected_tier.filter(|&tier| tier < tier_count));
    Ok(FixApplied { message, select })
}

#[cfg(test)]
pub(super) mod test_support {
    //! Entry points for the tests in `super::tests`, which cannot reach the private items.

    use super::{ChecksView, Computed, FixRequest, compute, request_from};
    use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
    use indicatrix_cut_core::Design;

    /// The checks of a solved design, with the reasons behind its warning rows.
    pub fn compute_view(design: &Design, solved: &[SolvedTier]) -> (ChecksView, usize) {
        let custom: Vec<GemMaterial> = Vec::new();
        let Computed { view, reasons, .. } = compute(design, solved, &custom, &[]);
        let fixable = reasons.iter().filter(|reason| reason.fix.is_some()).count();
        (view, fixable)
    }

    /// A fix request for the first fixable finding of a solved design, if it has one.
    pub fn first_fix_request(design: &Design, solved: &[SolvedTier]) -> Option<FixRequest> {
        let custom: Vec<GemMaterial> = Vec::new();
        let Computed { reasons, n_d, .. } = compute(design, solved, &custom, &[]);
        let index = reasons.iter().position(|reason| reason.fix.is_some())?;
        request_from(design, &reasons, n_d, index).ok()
    }
}
