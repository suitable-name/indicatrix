//! The Cut slider: where it stands, what it shows and what it is called.
//!
//! The slider walks a design's cutting steps ([`Design::preview_steps`]): position `0`
//! is the rough (the preform alone), position `k` the stone after the first `k` steps,
//! and the last position the finished design. Flat and concave tiers each count as one
//! step, in the order the cutting sheet lists them for every design
//! ([`Design::cutting_order`]: the pavilion and girdle tiers, the crown, the table last).
//!
//! `SolidPreviewModel.tier_cutoff` is the one source of truth, in these terms:
//! [`MODEL_FINISHED`] (`-1`) for the finished design, [`MODEL_ROUGH`] (`-2`) for the
//! rough, and `n >= 0` for the stone after steps `0..=n` -- the first `n + 1` steps of the
//! cutting order, which the Slice tool reads. [`CutPosition`] decodes it;
//! everything else here is derived from the position and the design, so the label, the
//! slider handle, the planner and every full redraw cannot disagree.
//!
//! The decisions are pure functions ([`CutPosition`], [`cut_label`], [`cut_geometry`]); the
//! few functions that touch the window ([`sync_model`], [`current_steps`],
//! [`reset_to_finished`]) only read and write the model.

use crate::{MainWindow, SolidPreviewModel};
use indicatrix::geometry::{GpuFacetPlane, meet_solver::SolvedTier};
use indicatrix_cut_core::{Design, design::TierRef, is_legacy_123_abc};
use indicatrix_solid::{
    live_update::{CutLimit, display_geometry},
    preview::StoneGeometryBuf,
};
use slint::ComponentHandle;

/// `SolidPreviewModel.tier_cutoff` for the finished design.
pub const MODEL_FINISHED: i32 = -1;

/// `SolidPreviewModel.tier_cutoff` for the rough, before any tier is cut.
pub const MODEL_ROUGH: i32 = -2;

/// Where the Cut slider sits for a design with a given number of steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CutPosition {
    /// The whole design: the slider at its right end.
    #[default]
    Finished,
    /// The preform alone, before any tier is cut: the slider at its left end.
    Rough,
    /// The stone after this many steps; always at least `1` and below the step count.
    After(usize),
}

impl CutPosition {
    /// The position "after `steps` steps" of a design with `step_count` steps: the
    /// rough for none, the finished design for all of them or more.
    #[must_use]
    pub const fn from_steps(steps: usize, step_count: usize) -> Self {
        if steps >= step_count {
            Self::Finished
        } else if steps == 0 {
            Self::Rough
        } else {
            Self::After(steps)
        }
    }

    /// Decodes `SolidPreviewModel.tier_cutoff` for a design with `step_count` steps.
    /// Anything below [`MODEL_ROUGH`] reads as finished, as does a stale cutoff past the
    /// last step (a tier was removed since it was set).
    #[must_use]
    pub fn from_model(tier_cutoff: i32, step_count: usize) -> Self {
        match tier_cutoff {
            MODEL_ROUGH => Self::from_steps(0, step_count),
            through if through >= 0 => usize::try_from(through).map_or(Self::Finished, |through| {
                Self::from_steps(through.saturating_add(1), step_count)
            }),
            _ => Self::Finished,
        }
    }

    /// The value `SolidPreviewModel.tier_cutoff` holds for this position.
    #[must_use]
    pub fn to_model(self) -> i32 {
        match self {
            Self::Finished => MODEL_FINISHED,
            Self::Rough => MODEL_ROUGH,
            Self::After(steps) => i32::try_from(steps).map_or(MODEL_FINISHED, |steps| steps - 1),
        }
    }

    /// How many steps are cut: `None` for the finished design (nothing is truncated),
    /// `Some(0)` for the rough.
    #[must_use]
    pub const fn steps(self) -> Option<usize> {
        match self {
            Self::Finished => None,
            Self::Rough => Some(0),
            Self::After(steps) => Some(steps),
        }
    }

    /// The slider handle's value: `0` for the rough up to `step_count` for the finished
    /// design.
    #[must_use]
    pub const fn slider_value(self, step_count: usize) -> f32 {
        match self {
            Self::Finished => step_count as f32,
            Self::Rough => 0.0,
            Self::After(steps) => steps as f32,
        }
    }

    /// The position a slider handle value stands for: rounded to the nearest step.
    #[must_use]
    pub const fn from_slider_value(value: f32, step_count: usize) -> Self {
        Self::from_steps(value.round().max(0.0) as usize, step_count)
    }
}

/// What the slider calls `step`.
///
/// The tier's code as the cutting sheet's label column shows it (`G1`, `P2`, `C3`, `T`, a
/// concave tier's `P4`), then its own name when that says more (`C2 Crown Main`). A tier with
/// no name, an old-style `123`/`ABC` one or a name equal to its code reads as the code alone. A
/// tier the design does not have reads "tier N" (flat) or "Concave tier N".
#[must_use]
pub fn step_name(design: &Design, step: TierRef) -> String {
    let codes = design.tier_codes();
    let (code, name, missing) = match step {
        TierRef::Flat(index) => (
            codes.flat.get(index).map(|label| label.code.as_str()),
            design.tiers.get(index).map(|tier| tier.name.trim()),
            format!("tier {}", index + 1),
        ),
        TierRef::Concave(index) => (
            codes.concave.get(index).map(|label| label.code.as_str()),
            design.concave_tiers.get(index).map(|tier| tier.name.trim()),
            format!("Concave tier {}", index + 1),
        ),
    };
    match (code.filter(|code| !code.is_empty()), name) {
        (Some(code), Some(name))
            if name.is_empty() || is_legacy_123_abc(name) || name.eq_ignore_ascii_case(code) =>
        {
            code.to_owned()
        }
        (Some(code), Some(name)) => format!("{code} {name}"),
        _ => missing,
    }
}

/// The slider's label for `position`: "Rough", "After <tier> (k of N)" or "Finished".
///
/// The tier is written as [`step_name`] gives it, `After C2 Crown Main (6 of 8)`. A position at
/// or past the design's last step reads "Finished".
#[must_use]
pub fn cut_label(design: &Design, position: CutPosition) -> String {
    match position {
        CutPosition::Finished => "Finished".to_string(),
        CutPosition::Rough => "Rough".to_string(),
        CutPosition::After(steps) => {
            let order = design.preview_steps();
            // `from_steps` never builds an `After` at or past the step count, but the label
            // agrees with it anyway: the last step is the finished design.
            if steps >= order.len() {
                return "Finished".to_string();
            }
            steps
                .checked_sub(1)
                .and_then(|last| order.get(last))
                .map_or_else(
                    || "Finished".to_string(),
                    |&step| {
                        format!(
                            "After {} ({steps} of {})",
                            step_name(design, step),
                            order.len()
                        )
                    },
                )
        }
    }
}

/// The geometry every full-plane redraw path draws for `design` at `steps` cutting steps.
///
/// `None` is the finished design. The result holds the planes, the concave tools cut into
/// them and the `(tier, placement)` of each tool. This is the one place the Cut slider is
/// applied outside the planner, so the background-solve push, the full refresh and the
/// optimise ghost agree with it, and the path tracer receives the same planes as the
/// solid raster.
///
/// `solved` is the design's own mast list, or `None` to solve it here (an unsolvable
/// design then draws nothing, except the rough, which needs no masts). Never panics: a
/// mast list that does not fit the design draws nothing.
///
/// `None` runs `Design::solve` on the calling thread, which takes seconds for a large
/// design: never pass it from the UI thread. Use [`cut_geometry_no_solve`] there.
#[must_use]
pub fn cut_geometry(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    steps: Option<usize>,
) -> StoneGeometryBuf {
    let own = if solved.is_none() {
        design.solve().unwrap_or_default()
    } else {
        Vec::new()
    };
    let masts = solved.unwrap_or(&own);
    let limit = steps.map_or(CutLimit::Finished, CutLimit::Steps);
    let geometry = display_geometry(design, masts, limit);
    StoneGeometryBuf {
        planes: geometry
            .planes
            .into_iter()
            .map(|(normal, offset)| GpuFacetPlane::new(normal.as_vec3(), -offset as f32))
            .collect(),
        tools: geometry.tools,
        placements: geometry.placements,
    }
}

/// [`cut_geometry`] for a caller on the UI thread: it never solves.
///
/// `solved` is the mast list the caller already holds, and `None` means the design does
/// not solve (the caller's own solve failed). Retrying that failed solve here would only
/// fail again, on the UI thread, so the stone is drawn as for an empty mast list: nothing,
/// except the rough, which needs no masts.
#[must_use]
pub fn cut_geometry_no_solve(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    steps: Option<usize>,
) -> StoneGeometryBuf {
    cut_geometry(design, Some(solved.unwrap_or(&[])), steps)
}

/// How many more 16 ms ticks a Cut slider replan waits for the editor state to be free.
///
/// About 130 ms in all, then it gives up. A longer-lived writer (a blocking file dialog
/// that pumps the event loop) submits its own replan on its way out, and that replan
/// reads the slider fresh.
pub const REPLAN_RETRIES: u8 = 8;

/// The retries left after one more failed attempt, or `None` when they are used up.
#[must_use]
pub const fn retries_after_busy(retries_left: u8) -> Option<u8> {
    retries_left.checked_sub(1)
}

/// The stone to push when a redraw arrives with the finished stone in hand.
///
/// That is `finished` -- built for the whole design -- unless the slider has the design cut
/// back to `steps`, when it is the stone at that cut, so the cut survives the background
/// solve's result. With no cut the stone passes through untouched.
///
/// Runs on the UI thread, so it never solves: `solved` is `None` only when the solve that
/// produced `finished` failed (see [`cut_geometry_no_solve`]).
#[must_use]
pub fn stone_at_cut(
    finished: StoneGeometryBuf,
    design: &Design,
    solved: Option<&[SolvedTier]>,
    steps: Option<usize>,
) -> StoneGeometryBuf {
    steps.map_or(finished, |steps| {
        cut_geometry_no_solve(design, solved, Some(steps))
    })
}

/// The cut the viewport is showing for `design` right now; `None` is the finished design.
///
/// Reads the model without changing it, so it is safe on any redraw path, including the
/// Slice tool's provisional replans (whose design has one extra tier).
#[must_use]
pub fn current_steps(ui: &MainWindow, design: &Design) -> Option<usize> {
    let cutoff = ui.global::<SolidPreviewModel>().get_tier_cutoff();
    CutPosition::from_model(cutoff, design.preview_step_count()).steps()
}

/// Writes the model's slider fields for `design` and returns the position they show: the
/// number of steps (the slider's maximum), the slider handle, the label, and a cutoff that
/// no longer fits (a tier was removed) normalised back to finished. Setting a property to
/// the value it already has changes nothing, so this is cheap to call on every replan.
fn publish(ui: &MainWindow, design: &Design) -> CutPosition {
    let model = ui.global::<SolidPreviewModel>();
    let step_count = design.preview_step_count();
    let position = CutPosition::from_model(model.get_tier_cutoff(), step_count);
    model.set_cut_step_count(i32::try_from(step_count).unwrap_or(i32::MAX));
    model.set_tier_cutoff(position.to_model());
    model.set_cut_position(position.slider_value(step_count));
    model.set_cut_label(cut_label(design, position).into());
    position
}

/// Brings the model's slider fields in line with `design` (see [`publish`]) and returns
/// the cut to draw. Called for every replan of the committed design.
#[must_use]
pub fn sync_model(ui: &MainWindow, design: &Design) -> Option<usize> {
    publish(ui, design).steps()
}

/// Puts the slider back to the finished design and refreshes the label for `design`: a
/// new, opened or loaded design starts uncut, and the label and the view must agree.
pub fn reset_to_finished(ui: &MainWindow, design: &Design) {
    ui.global::<SolidPreviewModel>()
        .set_tier_cutoff(MODEL_FINISHED);
    publish(ui, design);
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta, compute_tier_labels};

    fn round_brilliant() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    #[test]
    fn the_model_encoding_round_trips_for_every_position() {
        for step_count in [1_usize, 2, 7, 12] {
            let mut positions = vec![CutPosition::Rough, CutPosition::Finished];
            positions.extend((1..step_count).map(CutPosition::After));
            for position in positions {
                assert_eq!(
                    CutPosition::from_model(position.to_model(), step_count),
                    position,
                    "{position:?} of {step_count}"
                );
            }
        }
    }

    #[test]
    fn the_model_values_mean_what_the_slice_tool_reads() {
        // `-1` is the whole design, `n >= 0` is "through step n": the Slice tool's
        // `cut_hides_tier` compares exactly that.
        assert_eq!(CutPosition::from_model(-1, 8), CutPosition::Finished);
        assert_eq!(CutPosition::from_model(-2, 8), CutPosition::Rough);
        assert_eq!(CutPosition::from_model(0, 8), CutPosition::After(1));
        assert_eq!(CutPosition::from_model(5, 8), CutPosition::After(6));
        assert_eq!(CutPosition::After(6).to_model(), 5);
    }

    #[test]
    fn a_cutoff_past_the_last_step_or_below_rough_is_finished() {
        assert_eq!(CutPosition::from_model(7, 8), CutPosition::Finished);
        assert_eq!(CutPosition::from_model(99, 8), CutPosition::Finished);
        assert_eq!(CutPosition::from_model(-3, 8), CutPosition::Finished);
        assert_eq!(CutPosition::from_model(i32::MIN, 8), CutPosition::Finished);
    }

    #[test]
    fn a_design_without_steps_has_only_a_finished_position() {
        assert_eq!(
            CutPosition::from_model(MODEL_ROUGH, 0),
            CutPosition::Finished
        );
        assert_eq!(CutPosition::from_model(0, 0), CutPosition::Finished);
        assert_eq!(CutPosition::from_steps(0, 0), CutPosition::Finished);
    }

    #[test]
    fn steps_are_none_for_finished_and_zero_for_the_rough() {
        assert_eq!(CutPosition::Finished.steps(), None);
        assert_eq!(CutPosition::Rough.steps(), Some(0));
        assert_eq!(CutPosition::After(4).steps(), Some(4));
    }

    #[test]
    fn the_slider_spans_rough_to_finished_in_whole_steps() {
        let count = 8;
        assert_eq!(CutPosition::Rough.slider_value(count), 0.0);
        assert_eq!(CutPosition::After(3).slider_value(count), 3.0);
        assert_eq!(CutPosition::Finished.slider_value(count), 8.0);
        for step in 0..=count {
            let value = CutPosition::from_steps(step, count).slider_value(count);
            assert_eq!(
                CutPosition::from_slider_value(value, count),
                CutPosition::from_steps(step, count)
            );
        }
    }

    #[test]
    fn a_slider_value_rounds_to_the_nearest_step_and_clamps() {
        assert_eq!(
            CutPosition::from_slider_value(2.4, 8),
            CutPosition::After(2)
        );
        assert_eq!(
            CutPosition::from_slider_value(2.6, 8),
            CutPosition::After(3)
        );
        assert_eq!(CutPosition::from_slider_value(-3.0, 8), CutPosition::Rough);
        assert_eq!(CutPosition::from_slider_value(0.4, 8), CutPosition::Rough);
        assert_eq!(
            CutPosition::from_slider_value(7.6, 8),
            CutPosition::Finished
        );
        assert_eq!(
            CutPosition::from_slider_value(50.0, 8),
            CutPosition::Finished
        );
        assert_eq!(
            CutPosition::from_slider_value(f32::NAN, 8),
            CutPosition::Rough
        );
    }

    #[test]
    fn labels_name_the_last_cut_tier_and_count_the_steps() {
        let design = round_brilliant();
        let count = design.preview_step_count();
        assert_eq!(cut_label(&design, CutPosition::Rough), "Rough");
        assert_eq!(cut_label(&design, CutPosition::Finished), "Finished");
        // The fixture is stored top-down, the slider walks the cutting order: the girdle is
        // the first step, the upper girdle the last but one (the table is last).
        let order = design.preview_steps();
        let first = step_name(&design, order[0]);
        assert_eq!(first, "G1 Girdle");
        assert_eq!(
            cut_label(&design, CutPosition::After(1)),
            "After G1 Girdle (1 of 8)"
        );
        let last_but_one = step_name(&design, order[count - 2]);
        assert_eq!(last_but_one, "C3 Upper Girdle");
        assert_eq!(
            cut_label(&design, CutPosition::After(count - 1)),
            format!("After {last_but_one} ({} of {count})", count - 1)
        );
    }

    #[test]
    fn a_position_past_the_last_step_reads_finished() {
        let design = round_brilliant();
        assert_eq!(cut_label(&design, CutPosition::After(999)), "Finished");
        assert_eq!(cut_label(&design, CutPosition::After(0)), "Finished");
    }

    #[test]
    fn an_unnamed_or_legacy_named_tier_reads_as_its_canonical_code() {
        let mut design = round_brilliant();
        let named = design.tiers[2].name.clone();
        let code = compute_tier_labels(&design.tiers)[2].code.clone();
        assert_eq!(code, "C2", "the crown main is the second crown tier cut");
        assert_eq!(
            step_name(&design, TierRef::Flat(2)),
            format!("{code} {named}"),
            "a descriptive name follows the code"
        );
        for unnamed in ["", "  ", "123", "c2"] {
            design.tiers[2].name = unnamed.to_string();
            let code = compute_tier_labels(&design.tiers)[2].code.clone();
            assert!(!code.is_empty(), "a tier always has a canonical code");
            assert_eq!(
                step_name(&design, TierRef::Flat(2)),
                code,
                "name {unnamed:?}"
            );
        }
    }

    /// The table's code is `T`; the slider names it with its own name after it. The table is
    /// the last step, so no "After" label ever names it (the last position is "Finished").
    #[test]
    fn the_table_reads_as_t_and_its_name() {
        let design = round_brilliant();
        assert_eq!(step_name(&design, TierRef::Flat(0)), "T Table");
        let count = design.preview_step_count();
        assert_eq!(design.preview_steps()[count - 1], TierRef::Flat(0));
        assert_eq!(cut_label(&design, CutPosition::After(count)), "Finished");
    }

    #[test]
    fn a_missing_tier_has_a_positional_name() {
        let design = round_brilliant();
        assert_eq!(step_name(&design, TierRef::Flat(99)), "tier 100");
        assert_eq!(step_name(&design, TierRef::Concave(0)), "Concave tier 1");
    }

    #[test]
    fn concave_steps_count_and_are_labelled_in_cutting_order() {
        let design = Design::concave_fixture();
        let count = design.preview_step_count();
        assert_eq!(count, design.tiers.len() + design.concave_tiers.len());
        // Cutting order: three flat pavilion/girdle tiers, the groove, two crowns, the dimple.
        assert_eq!(
            cut_label(&design, CutPosition::After(4)),
            format!("After P3 Groove (4 of {count})"),
            "the groove continues the P count of the two flat pavilion tiers"
        );
        assert_eq!(
            cut_label(&design, CutPosition::After(count - 1)),
            format!(
                "After {} ({} of {count})",
                step_name(&design, TierRef::Flat(4)),
                count - 1
            )
        );
    }

    #[test]
    fn the_rough_geometry_is_the_preform_alone_with_or_without_masts() {
        let design = round_brilliant();
        let preform = design.preform.planes().len();
        let solved = design.solve().expect("every tier is pinned");
        assert_eq!(
            cut_geometry(&design, Some(&solved), Some(0)).planes.len(),
            preform
        );
        assert_eq!(cut_geometry(&design, None, Some(0)).planes.len(), preform);
        assert_eq!(
            cut_geometry(&design, Some(&[]), Some(0)).planes.len(),
            preform
        );
    }

    #[test]
    fn the_finished_geometry_matches_the_uncut_conversion() {
        let design = round_brilliant();
        let solved = design.solve().expect("every tier is pinned");
        let expected =
            indicatrix_editor::solve_policy::design_to_gpu_planes_from_solved(&design, &solved);
        let finished = cut_geometry(&design, Some(&solved), None);
        assert_eq!(finished.planes, expected);
        assert!(finished.tools.is_empty() && finished.placements.is_empty());
        assert_eq!(
            cut_geometry(&design, None, None).planes,
            expected,
            "without masts the design is solved here"
        );
    }

    #[test]
    fn a_cut_geometry_grows_one_tier_at_a_time() {
        let design = round_brilliant();
        let solved = design.solve().expect("every tier is pinned");
        let mut previous = cut_geometry(&design, Some(&solved), Some(0)).planes.len();
        for steps in 1..=design.preview_step_count() {
            let now = cut_geometry(&design, Some(&solved), Some(steps))
                .planes
                .len();
            assert!(now >= previous, "step {steps}");
            previous = now;
        }
        assert_eq!(
            previous,
            cut_geometry(&design, Some(&solved), None).planes.len()
        );
    }

    #[test]
    fn an_unsolvable_design_draws_nothing_but_its_rough() {
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            vec![ConstraintTier {
                angle_deg: 30.0,
                name: "C1".to_string(),
                indices: vec![0.0],
                constraint: indicatrix::geometry::meet_solver::MeetConstraint::MeetExisting,
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            }],
        );
        assert!(
            cut_geometry(&design, None, None).planes.is_empty(),
            "an unsolvable design has no finished stone"
        );
        assert!(
            cut_geometry(&design, None, Some(1)).planes.is_empty(),
            "nor one cut back after a step"
        );
        assert_eq!(
            cut_geometry(&design, None, Some(0)).planes.len(),
            design.preform.planes().len()
        );
    }

    /// The UI-thread form never solves: with masts it is `cut_geometry`, and without masts
    /// (a design whose solve failed) the stone is the rough and nothing else.
    #[test]
    fn the_no_solve_form_draws_the_rough_for_a_design_without_masts() {
        let design = round_brilliant();
        let solved = design.solve().expect("every tier is pinned");
        for steps in [None, Some(0), Some(3)] {
            assert_eq!(
                cut_geometry_no_solve(&design, Some(&solved), steps),
                cut_geometry(&design, Some(&solved), steps),
                "with masts, cut {steps:?}"
            );
        }
        // The same design, but the caller holds no masts: `cut_geometry(.., None, ..)` would
        // solve it here and draw the whole stone, which is exactly what this must not do.
        assert!(
            !cut_geometry(&design, None, None).planes.is_empty(),
            "the solving form draws the whole stone"
        );
        assert!(
            cut_geometry_no_solve(&design, None, None).planes.is_empty(),
            "no masts, no stone"
        );
        assert!(
            cut_geometry_no_solve(&design, None, Some(3))
                .planes
                .is_empty(),
            "nor one cut back to step 3"
        );
        assert_eq!(
            cut_geometry_no_solve(&design, None, Some(0)).planes.len(),
            design.preform.planes().len()
        );
    }

    /// A background solve arrives with the finished stone: with no cut it is pushed as it
    /// is, with a cut the truncated stone replaces it (the regression: the finished gem
    /// overwrote the cut after every edit).
    #[test]
    fn a_background_solves_stone_is_cut_back_only_when_the_slider_is() {
        let design = round_brilliant();
        let solved = design.solve().expect("every tier is pinned");
        let finished = cut_geometry(&design, Some(&solved), None);
        let untouched = stone_at_cut(finished.clone(), &design, Some(&solved), None);
        assert_eq!(untouched, finished);

        let cut = stone_at_cut(finished.clone(), &design, Some(&solved), Some(3));
        assert!(cut.planes.len() < finished.planes.len());
        assert_eq!(cut, cut_geometry(&design, Some(&solved), Some(3)));

        let rough = stone_at_cut(finished, &design, Some(&solved), Some(0));
        assert_eq!(rough.planes.len(), design.preform.planes().len());
    }

    #[test]
    fn a_busy_editor_is_retried_a_bounded_number_of_times() {
        let mut left = REPLAN_RETRIES;
        let mut retries = 0;
        while let Some(next) = retries_after_busy(left) {
            left = next;
            retries += 1;
        }
        assert_eq!(retries, REPLAN_RETRIES);
        assert_eq!(retries_after_busy(0), None);
        assert_eq!(retries_after_busy(1), Some(0));
    }

    #[test]
    fn the_concave_geometry_carries_the_tools_of_the_cut_steps() {
        let design = Design::concave_fixture();
        let solved = design.solve().expect("the fixture solves");
        let count = design.preview_step_count();
        assert!(
            cut_geometry(&design, Some(&solved), Some(3))
                .tools
                .is_empty(),
            "the groove is step 4"
        );
        let grooved = cut_geometry(&design, Some(&solved), Some(4));
        assert_eq!(grooved.tools.len(), 8);
        assert_eq!(grooved.placements.len(), 8);
        let finished = cut_geometry(&design, Some(&solved), None);
        assert_eq!(finished.tools.len(), 12);
        assert_eq!(
            cut_geometry(&design, Some(&solved), Some(count))
                .tools
                .len(),
            12
        );
    }
}
