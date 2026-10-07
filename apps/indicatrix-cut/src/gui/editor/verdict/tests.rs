//! The desktop glue's pure parts: the rows the popover lists, the guards in front of a Fix
//! and the worker's job. (The verdict itself and every fix are tested in
//! `indicatrix_editor::verdict`.)

use super::{
    FIX_RUNNING, OUT_OF_DATE, SLOT, Slot, failure_text, pending_fix,
    present::{ReasonRow, rows_of},
    work::compute,
};
use indicatrix::optics::LightingPreset;
use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};
use indicatrix_editor::verdict::{FixAction, Level, Reason, ReasonKind, SolveFacts, Verdict};

fn brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

fn reason(
    level: Level,
    tier: Option<usize>,
    fix: Option<FixAction>,
    confirm: Option<&str>,
) -> Reason {
    Reason {
        kind: ReasonKind::OffGear,
        level,
        text: "A tier has a problem.".to_string(),
        tier,
        fix,
        confirm: confirm.map(str::to_string),
    }
}

fn verdict_of(reasons: Vec<Reason>) -> Verdict {
    Verdict {
        level: Level::Check,
        headline: "Check things.".to_string(),
        reasons,
    }
}

/// Puts `verdict` into the slot as the one on screen for `design`.
fn show(verdict: Verdict, design: &Design, current: bool) {
    SLOT.with(|cell| {
        *cell.borrow_mut() = Slot {
            serial: 1,
            verdict: Some(verdict),
            tiers: design.tiers.clone(),
            n_d: 1.76,
            current,
            busy: false,
        };
    });
}

#[test]
fn rows_carry_the_level_the_tier_and_the_fix_words() {
    let verdict = verdict_of(vec![
        reason(
            Level::Problem,
            Some(3),
            Some(FixAction::SnapToTeeth { tier: 3 }),
            None,
        ),
        reason(Level::Check, None, None, None),
    ]);
    let rows = rows_of(&verdict);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        ReasonRow {
            level: 2,
            text: "A tier has a problem.".to_string(),
            tier: 3,
            fix_label: FixAction::SnapToTeeth { tier: 3 }.label().to_string(),
            fix_hint: FixAction::SnapToTeeth { tier: 3 }.hint().to_string(),
        }
    );
    // A reason about the whole stone selects nothing and has no button.
    assert_eq!(rows[1].level, 1);
    assert_eq!(rows[1].tier, -1);
    assert_eq!(rows[1].fix_label, "");
    assert_eq!(rows[1].fix_hint, "");
}

#[test]
fn the_editor_status_line_explains_a_failed_solve_only() {
    assert_eq!(
        failure_text("failed", "  P2 never meets anything.  ").as_deref(),
        Some("P2 never meets anything.")
    );
    assert_eq!(failure_text("failed", "   "), None);
    // The "Not solved" marker an edit leaves is not a reason.
    assert_eq!(failure_text("stale", "Not solved -- click Solve"), None);
    assert_eq!(failure_text("solved", "Closed solid"), None);
}

#[test]
fn a_current_verdict_hands_over_its_fix_and_its_question() {
    let design = brilliant();
    show(
        verdict_of(vec![
            reason(
                Level::Check,
                Some(1),
                Some(FixAction::RemoveVanished { tier: 1 }),
                Some("Remove it?"),
            ),
            reason(
                Level::Check,
                Some(2),
                Some(FixAction::SnapToTeeth { tier: 2 }),
                None,
            ),
        ]),
        &design,
        true,
    );
    assert_eq!(
        pending_fix(0, &design.tiers),
        Ok((
            FixAction::RemoveVanished { tier: 1 },
            Some("Remove it?".to_string())
        ))
    );
    assert_eq!(
        pending_fix(1, &design.tiers),
        Ok((FixAction::SnapToTeeth { tier: 2 }, None))
    );
}

#[test]
fn a_fix_waits_for_a_verdict_that_describes_the_design() {
    let design = brilliant();
    let verdict = verdict_of(vec![reason(
        Level::Check,
        Some(1),
        Some(FixAction::SnapToTeeth { tier: 1 }),
        None,
    )]);

    // An edit since the verdict landed.
    show(verdict.clone(), &design, false);
    assert_eq!(pending_fix(0, &design.tiers), Err(OUT_OF_DATE.to_string()));

    // The verdict says current, but the design in front of it is another one.
    show(verdict.clone(), &design, true);
    let mut edited = design.clone();
    edited.tiers.pop();
    assert_eq!(pending_fix(0, &edited.tiers), Err(OUT_OF_DATE.to_string()));

    // A second fix while one is running.
    show(verdict, &design, true);
    SLOT.with(|cell| cell.borrow_mut().busy = true);
    assert_eq!(pending_fix(0, &design.tiers), Err(FIX_RUNNING.to_string()));
}

#[test]
fn a_reason_without_a_tool_or_out_of_range_has_no_fix() {
    let design = brilliant();
    show(
        verdict_of(vec![reason(Level::Check, Some(1), None, None)]),
        &design,
        true,
    );
    assert!(pending_fix(0, &design.tiers).is_err());
    assert!(pending_fix(7, &design.tiers).is_err());
}

#[test]
fn the_worker_job_checks_geometry_and_measures_only_with_a_material() {
    let mut design = brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let lighting = LightingPreset::RingLights;

    // No custom list handed over: geometry only.
    let inputs = compute(&design, Some(&solved), 1.76, lighting, None);
    assert_eq!(inputs.solve, SolveFacts::Closed);
    assert!(inputs.optics.is_none());

    // A custom list, but the design names no material: still nothing to measure.
    let inputs = compute(&design, Some(&solved), 1.76, lighting, Some(&[]));
    assert!(inputs.optics.is_none());

    // A named preset: the three figures are in range.
    design.material.name = Some("Sapphire".to_string());
    let inputs = compute(&design, Some(&solved), 1.76, lighting, Some(&[]));
    let column = inputs.optics.expect("a named material is measured");
    for value in [
        column.windowing_pct,
        column.brilliance_pct,
        column.extinction_pct,
    ] {
        assert!((0.0..=100.0).contains(&value), "{value}");
    }
}

#[test]
fn a_design_that_did_not_solve_gets_a_verdict_input_without_optics() {
    let mut design = brilliant();
    design.material.name = Some("Sapphire".to_string());
    let inputs = compute(&design, None, 1.76, LightingPreset::RingLights, Some(&[]));
    assert!(matches!(inputs.solve, SolveFacts::DoesNotSolve(_)));
    assert!(inputs.optics.is_none());
}
