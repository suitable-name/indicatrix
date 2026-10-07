//! The Angle Sweep dialog's form, without a window: which tiers the combo offers, the
//! range a tier starts with, and what the three number fields add up to.

use indicatrix_cut_core::Design;
use indicatrix_editor::{
    solve_policy::SolveCostEstimate,
    sweep::{
        DEFAULT_SWEEP_STEP_DEG, SweepError, SweepPlan, default_range, estimate_seconds,
        format_angle_input, format_duration_estimate, parse_sweep_range, plan_summary, plan_sweep,
        sweepable_tiers,
    },
};

/// One entry of the tier combo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TierChoice {
    /// The tier's position in the design.
    pub tier: usize,
    /// What the combo shows: `3. Crown Main (34.50°)`.
    pub label: String,
}

/// The three number fields of the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Fields {
    pub from: String,
    pub to: String,
    pub step: String,
}

/// What the form comes to.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Checked {
    /// A sweep that can run, with the line that tells the cutter what it will do.
    Ready { plan: SweepPlan, summary: String },
    /// Why it cannot run, in a sentence for the dialog.
    Refused(String),
}

/// Everything the dialog shows when it opens.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Opening {
    /// The combo's entries.
    pub choices: Vec<TierChoice>,
    /// The entry the combo starts on.
    pub chosen: usize,
    /// The range of that tier.
    pub fields: Fields,
    /// The hint about tiers left out; empty for none.
    pub notice: String,
    /// What the starting form comes to.
    pub checked: Checked,
}

/// The dialog's first view of `design`; `None` when no tier can be swept.
/// `selected_tier` is the tier selected in the tier table, if any.
pub(super) fn open(
    design: &Design,
    selected_tier: Option<usize>,
    tilt_average: bool,
    workers: usize,
) -> Option<Opening> {
    let choices = tier_choices(design);
    let chosen = default_choice(&choices, selected_tier);
    let tier = choices.get(chosen)?.tier;
    let fields = default_fields(design.tiers.get(tier)?.angle_deg);
    let checked = check(design, tier, &fields, tilt_average, workers);
    Some(Opening {
        choices,
        chosen,
        fields,
        notice: left_out_notice(design),
        checked,
    })
}

/// The range a tier starts with and what that comes to; `None` for a tier the design lacks.
pub(super) fn retier(
    design: &Design,
    tier: usize,
    tilt_average: bool,
    workers: usize,
) -> Option<(Fields, Checked)> {
    let fields = default_fields(design.tiers.get(tier)?.angle_deg);
    let checked = check(design, tier, &fields, tilt_average, workers);
    Some((fields, checked))
}

/// The tiers the combo offers, in design order.
pub(super) fn tier_choices(design: &Design) -> Vec<TierChoice> {
    sweepable_tiers(design)
        .into_iter()
        .map(|tier| TierChoice {
            tier,
            label: choice_label(design, tier),
        })
        .collect()
}

/// `3. Crown Main (34.50°)`: the row number the tier table shows, the name, the angle (a
/// magnitude: a pavilion tier stored at -41 reads `41.00°`).
fn choice_label(design: &Design, tier: usize) -> String {
    let angle_deg = design.tiers.get(tier).map_or(0.0, |t| t.angle_deg.abs());
    format!(
        "{}. {} ({angle_deg:.2}\u{b0})",
        tier + 1,
        indicatrix_editor::retarget::plan::tier_display_name(design, tier)
    )
}

/// Which combo entry the dialog opens on: the tier selected in the tier table when it can
/// be swept, else the first.
pub(super) fn default_choice(choices: &[TierChoice], selected_tier: Option<usize>) -> usize {
    selected_tier
        .and_then(|tier| choices.iter().position(|choice| choice.tier == tier))
        .unwrap_or(0)
}

/// The range and step a tier starts with: five degrees either side of its angle, in
/// half-degree steps. The numbers are magnitudes, flattest first (`36` to `46` for a
/// pavilion tier stored at -41): the side of the girdle comes from the tier.
pub(super) fn default_fields(angle_deg: f64) -> Fields {
    let (low, high) = default_range(angle_deg);
    Fields {
        from: format_angle_input(low),
        to: format_angle_input(high),
        step: format_angle_input(DEFAULT_SWEEP_STEP_DEG),
    }
}

/// The hint under the form about tiers the combo leaves out because their angles follow a
/// relation (a table, a culet and a girdle are plain to see); empty when there are none.
pub(super) fn left_out_notice(design: &Design) -> String {
    let driven: Vec<String> = (0..design.tiers.len())
        .filter(|&tier| design.is_tier_driven(tier))
        .map(|tier| indicatrix_editor::retarget::plan::tier_display_name(design, tier))
        .collect();
    if driven.is_empty() {
        String::new()
    } else {
        format!(
            "Not listed, because their angles follow a relation: {}.",
            driven.join(", ")
        )
    }
}

/// Reads the form: the three fields, then the plan of the sweep, then the line that says
/// how many angles it tries and how long that takes. Cheap (it never solves), so the dialog
/// calls it on every keystroke.
pub(super) fn check(
    design: &Design,
    tier: usize,
    fields: &Fields,
    tilt_average: bool,
    workers: usize,
) -> Checked {
    if tier >= design.tiers.len() {
        return Checked::Refused(
            SweepError::NoSuchTier {
                tier,
                tier_count: design.tiers.len(),
            }
            .to_string(),
        );
    }
    let range = match parse_sweep_range(&fields.from, &fields.to, &fields.step) {
        Ok(range) => range,
        Err(error) => return Checked::Refused(error.to_string()),
    };
    match plan_sweep(design, tier, range) {
        Ok(plan) => {
            let seconds = estimate_seconds(
                plan.angles.len(),
                tilt_average,
                workers,
                SolveCostEstimate::of(design),
            );
            let summary = plan_summary(&plan, &format_duration_estimate(seconds));
            Checked::Ready { plan, summary }
        }
        Err(error) => Checked::Refused(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

    const CROWN_MAIN: usize = 2;
    const PAVILION_MAIN: usize = 5;

    fn round_brilliant() -> Design {
        let mut design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        design.ensure_tier_ids();
        design
    }

    fn fields(from: &str, to: &str, step: &str) -> Fields {
        Fields {
            from: from.to_owned(),
            to: to.to_owned(),
            step: step.to_owned(),
        }
    }

    #[test]
    fn the_combo_lists_the_sweepable_tiers_with_their_row_numbers_and_angles() {
        let choices = tier_choices(&round_brilliant());
        let tiers: Vec<usize> = choices.iter().map(|choice| choice.tier).collect();
        assert_eq!(tiers, vec![1, CROWN_MAIN, 3, PAVILION_MAIN, 6]);
        let crown = choices
            .iter()
            .find(|choice| choice.tier == CROWN_MAIN)
            .expect("the crown main");
        assert_eq!(crown.label, "3. Crown Main (34.50\u{b0})");
        let pavilion = choices
            .iter()
            .find(|choice| choice.tier == PAVILION_MAIN)
            .expect("the pavilion main");
        assert_eq!(
            pavilion.label, "6. Pavilion Main (41.00\u{b0})",
            "the stored -41 reads as 41"
        );
    }

    #[test]
    fn the_dialog_opens_on_the_selected_tier_when_it_can_be_swept() {
        let choices = tier_choices(&round_brilliant());
        assert_eq!(
            choices[default_choice(&choices, Some(PAVILION_MAIN))].tier,
            PAVILION_MAIN
        );
        // The table cannot be swept; so does no selection at all: the first entry.
        assert_eq!(default_choice(&choices, Some(0)), 0);
        assert_eq!(default_choice(&choices, None), 0);
        assert_eq!(default_choice(&[], Some(3)), 0);
    }

    #[test]
    fn a_tier_starts_with_five_degrees_either_side_as_positive_numbers() {
        assert_eq!(
            default_fields(34.5),
            fields("29.5", "39.5", "0.5"),
            "a crown tier"
        );
        assert_eq!(
            default_fields(-41.0),
            fields("36", "46", "0.5"),
            "a pavilion tier stored at -41 starts with positive numbers"
        );
    }

    #[test]
    fn a_relation_is_named_in_the_hint_under_the_form() {
        let design = round_brilliant();
        assert_eq!(left_out_notice(&design), "");
        let mut session = indicatrix_editor::EditorSession::with_history(
            design,
            indicatrix_cut_core::History::new(),
        );
        session
            .set_tier_relation(3, "[Crown Main] + 6.5")
            .expect("a relation");
        let notice = left_out_notice(&session.design);
        assert_eq!(
            notice,
            "Not listed, because their angles follow a relation: Upper Girdle."
        );
    }

    #[test]
    fn opening_starts_on_the_selected_tier_with_its_range_and_a_ready_form() {
        let design = round_brilliant();
        let opening = open(&design, Some(PAVILION_MAIN), false, 4).expect("tiers to sweep");
        assert_eq!(opening.choices[opening.chosen].tier, PAVILION_MAIN);
        assert_eq!(opening.fields, fields("36", "46", "0.5"));
        assert_eq!(opening.notice, "");
        assert!(matches!(opening.checked, Checked::Ready { .. }));

        let (fields_for_crown, checked) =
            retier(&design, CROWN_MAIN, false, 4).expect("the crown main");
        assert_eq!(fields_for_crown, fields("29.5", "39.5", "0.5"));
        assert!(matches!(checked, Checked::Ready { .. }));
        assert!(retier(&design, 99, false, 4).is_none());
    }

    #[test]
    fn a_design_with_nothing_to_sweep_does_not_open() {
        let mut design = round_brilliant();
        design.tiers.clear();
        assert!(open(&design, None, false, 4).is_none());
    }

    #[test]
    fn a_good_form_gives_the_plan_and_a_summary_line() {
        let design = round_brilliant();
        let Checked::Ready { plan, summary } =
            check(&design, PAVILION_MAIN, &fields("39", "43", "1"), false, 4)
        else {
            panic!("the form is fine");
        };
        // The tier is stored at -41, so its angles carry the pavilion side; the summary
        // is read by a person and shows positive numbers, flattest first.
        assert_eq!(plan.angles, vec![-39.0, -40.0, -41.0, -42.0, -43.0]);
        assert!(
            summary.starts_with("5 angles from 39.00 to 43.00 degrees."),
            "{summary}"
        );
        assert!(summary.contains("This takes"), "{summary}");
    }

    #[test]
    fn a_signed_entry_and_an_unsigned_entry_make_the_same_sweep() {
        let design = round_brilliant();
        let ready = |from: &str, to: &str| match check(
            &design,
            PAVILION_MAIN,
            &fields(from, to, "1"),
            false,
            4,
        ) {
            Checked::Ready { plan, summary } => (plan, summary),
            Checked::Refused(message) => panic!("the form should run: {message}"),
        };
        assert_eq!(ready("39", "43"), ready("-39", "-43"));
        assert_eq!(ready("39", "43"), ready("43", "39"));
        // The same numbers are fine for a crown tier: the side comes from the tier.
        assert!(matches!(
            check(&design, CROWN_MAIN, &fields("-33", "-36", "0.5"), false, 4),
            Checked::Ready { .. }
        ));
    }

    #[test]
    fn a_form_that_cannot_run_says_why_in_words() {
        let design = round_brilliant();
        let refusal = |tier: usize, from: &str, to: &str, step: &str| match check(
            &design,
            tier,
            &fields(from, to, step),
            false,
            4,
        ) {
            Checked::Refused(message) => message,
            Checked::Ready { .. } => panic!("the form should be refused"),
        };
        assert!(refusal(PAVILION_MAIN, "abc", "-39", "1").starts_with("From"));
        assert_eq!(
            refusal(PAVILION_MAIN, "-43", "-39", "0"),
            "The step must be more than 0."
        );
        assert!(refusal(PAVILION_MAIN, "-43", "-39", "0.001").contains("at most 200"));
        assert!(refusal(CROWN_MAIN, "95", "39", "1").contains("outside what a sweep takes"));
        assert!(refusal(99, "-43", "-39", "1").contains("not in the design"));
    }
}
