//! Tier-list row-builder tests: `tier_items`/`tier_items_stale`, the meet-adoption
//! resolve-stays-solved regression, `tier_margin_and_risk`, `constraint_kind_and_text`,
//! `index_chip_items`, and `tier_matches_filter`.

use crate::{
    EditorSession,
    view_model::{row_format::*, rows::*, solid_status::*},
};
use indicatrix::geometry::meet_solver::{Block, MeetConstraint};
use indicatrix_cut_core::{ConstraintTier, Design, Edit, TierTarget};

#[test]
fn tier_items_reflects_position_and_fields_in_order() {
    let mut state = EditorSession::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: -41.0,
                name: "P1".to_string(),
                indices: vec![0.0, 24.0],
                constraint: MeetConstraint::ScaleReference(0.65),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    let items = tier_items(&state.design, state.design.effective_refractive_index());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].index, 0);
    assert_eq!(items[0].angle_deg.as_str(), "41.00");
    assert_eq!(items[0].mast.as_str(), "0.6500");
    assert_eq!(items[0].name.as_str(), "P1");
    assert_eq!(items[0].indices.as_str(), "0, 24");
}

/// A complete orbit reads "orbit x4" and is never flagged incomplete; detaching it
/// must flip [`crate::view_model::TierRow::is_detached`] without changing the orbit shape
/// shown.
#[test]
fn tier_items_reports_orbit_status_and_detached_state() {
    let mut state = EditorSession::fresh(); // symmetry_order 8, see `EditorSession::fresh`
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: -41.0,
                name: "P1".to_string(),
                indices: vec![0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0],
                constraint: MeetConstraint::ScaleReference(0.65),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    let items = tier_items_stale(&state.design, state.design.effective_refractive_index());
    assert_eq!(items[0].orbit_status.as_str(), "orbit x8");
    assert!(!items[0].orbit_incomplete);
    assert!(!items[0].is_detached);

    let edit = state.design.detach_all_in_tier(0).unwrap();
    state.apply(edit).unwrap();
    let items = tier_items_stale(&state.design, state.design.effective_refractive_index());
    assert!(items[0].is_detached);
    assert_eq!(
        items[0].orbit_status.as_str(),
        "orbit x8",
        "detaching must not change the reported orbit shape, only is_detached"
    );
}

/// The no-solve tier list must still reflect authored fields exactly like
/// [`tier_items`] does -- only mast/strategy differ, always the fixed "not solved"
/// placeholder flagged uncertain, regardless of whether the design would actually
/// solve cleanly. This function must never touch `Design::solve`.
#[test]
fn tier_items_stale_reflects_authored_fields_without_solving() {
    let mut state = EditorSession::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: -41.0,
                name: "P1".to_string(),
                indices: vec![0.0, 24.0],
                constraint: MeetConstraint::ScaleReference(0.65),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    let items = tier_items_stale(&state.design, state.design.effective_refractive_index());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].index, 0);
    assert_eq!(items[0].angle_deg.as_str(), "41.00");
    assert_eq!(items[0].name.as_str(), "P1");
    assert_eq!(items[0].indices.as_str(), "0, 24");
    // Never a real mast, always flagged uncertain, even though this exact tier is a
    // `ScaleReference` that would solve instantly and exactly.
    assert_eq!(items[0].mast.as_str(), "-");
    assert!(items[0].strategy_is_uncertain);
}

/// Reproduces the "Solve, Adopt, and it says not solved again" bug: build a design
/// with a real crown anchor and one imported, not-yet-adopted tier pinned to exactly
/// the mast its real meet constraint would derive, confirm it already solves, then
/// apply the exact `Edit::SetConstraint` "Adopt" issues, and confirm the design is
/// STILL solvable -- never falling back to `MissingAnchor` -- converging to the same
/// mast. The acceptance criterion for `setup_adopt_meet_callback` calling a real
/// re-solve rather than showing a fixed "Not solved" banner.
#[test]
fn adopting_a_suggested_meet_keeps_the_design_solved_at_the_same_mast() {
    // A proven-solvable crown anchor + `MeetNamed` pair, not a guessed shape.
    let mut state = EditorSession::fresh();
    state.design = Design::fresh(
        indicatrix_cut_core::PreformSpec::block(1.0, 1.0, 2.0),
        96,
        4,
        1.62,
    );

    // Crown anchor: gives the block something to solve the meet-derived tier
    // against.
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: 30.0,
                name: "A".to_string(),
                indices: vec![0.0, 24.0, 48.0, 72.0],
                constraint: MeetConstraint::ScaleReference(0.5),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        })
        .unwrap();

    // What the real meet constraint derives for a same-block, same-index-shape tier
    // meeting A -- computed once up front so the "pinned" tier below can be pinned to
    // EXACTLY this, and the post-Adopt assertion has a real number to compare against.
    let mut derived = state.design.clone();
    derived.tiers.push(ConstraintTier {
        angle_deg: 45.0,
        name: "B".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetNamed(vec!["A".to_string()]),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    let derived_mast = derived
        .solve()
        .expect("A anchors the crown; B must solve against it")[1]
        .mast;

    // Import policy: B is PINNED to that same real mast, with the file's actual meet
    // instruction preserved alongside for one-click Adopt.
    state
        .apply(Edit::AddTier {
            index: 1,
            tier: ConstraintTier {
                angle_deg: 45.0,
                name: "B".to_string(),
                indices: vec![0.0, 24.0, 48.0, 72.0],
                constraint: MeetConstraint::ScaleReference(derived_mast),
                imported_meet: Some(MeetConstraint::MeetNamed(vec!["A".to_string()])),
                original_notes: None,
                detached: Vec::new(),
            },
        })
        .unwrap();

    // The explicit "Solve" action's own path: already solved, already reporting B's
    // real (pinned) mast.
    let items = tier_items(&state.design, state.design.effective_refractive_index());
    assert_eq!(items[1].mast.as_str(), format!("{derived_mast:.4}"));
    let (status, is_problem) = status_text_and_is_problem(&state.design);
    assert!(
        !is_problem,
        "pinned design must already read as solved: {status}"
    );

    // "Adopt" itself: the exact `Edit::SetConstraint` call `setup_adopt_meet_callback`
    // issues, switching B over to its `imported_meet`.
    let constraint = state.design.tiers[1].imported_meet.clone().unwrap();
    state
        .apply(Edit::SetConstraint {
            index: 1,
            constraint,
        })
        .expect("adopt must apply");

    // The bug: after Adopt, the design must still solve -- a real "Solve" click must
    // never report "not solved" against a design that actually does.
    let (status, is_problem) = status_text_and_is_problem(&state.design);
    assert!(!is_problem, "adopted design must still solve: {status}");
    let items = tier_items(&state.design, state.design.effective_refractive_index());
    assert!(
        !items[1].strategy_is_uncertain,
        "the adopted meet must resolve to a real, trusted strategy, not an estimate"
    );
    // And it must converge to the SAME mast Adopt was suggesting in the first place --
    // not silently move the geometry.
    let adopted_mast = state
        .design
        .solve()
        .expect("the adopted design must still solve")[1]
        .mast;
    assert!(
        (adopted_mast - derived_mast).abs() < 1e-9,
        "adopting should reproduce the exact same mast the suggestion was showing: \
         {adopted_mast} vs {derived_mast}"
    );
    assert_eq!(
        items[1].mast.as_str(),
        format!("{derived_mast:.4}"),
        "and the row must display that same mast, rounded for the table"
    );
}

// --- tier_margin_and_risk ---

#[test]
fn tier_margin_and_risk_is_not_applicable_for_a_crown_tier_with_no_partner() {
    // A crown tier with no pavilion partner to estimate against reads
    // "nothing to show".
    assert_eq!(
        tier_margin_and_risk(30.0, Block::Crown, 2.417, None),
        (String::new(), -1)
    );
}

#[test]
fn tier_margin_and_risk_is_not_applicable_for_a_girdle_tier() {
    // A girdle tier is classified by BLOCK (magnitude ~90 degrees), not
    // by `angle == 0.0` -- the old check treated an exact-zero table/culet
    // tier as girdle and a real +/-90 girdle tier as crown/pavilion, backwards
    // on both counts. `Block::Girdle` always reads "nothing to show"
    // regardless of the angle's own value.
    assert_eq!(
        tier_margin_and_risk(90.0, Block::Girdle, 2.417, None),
        (String::new(), -1)
    );
}

#[test]
fn tier_margin_and_risk_classifies_a_comfortably_safe_pavilion_tier() {
    // Diamond's critical angle is ~24.4 degrees; -40 sits well past it.
    let (text, risk) = tier_margin_and_risk(-40.0, Block::Pavilion, 2.417, None);
    assert_eq!(risk, 0);
    assert!(text.starts_with('+'), "{text}");
}

#[test]
fn tier_margin_and_risk_classifies_a_windowing_pavilion_tier() {
    // -20 is well below diamond's ~24.4 degree critical angle.
    let (text, risk) = tier_margin_and_risk(-20.0, Block::Pavilion, 2.417, None);
    assert_eq!(risk, 2);
    assert!(text.starts_with('-'), "{text}");
}

#[test]
fn tier_margin_and_risk_classifies_a_marginal_pavilion_tier() {
    // A pavilion angle exactly at the critical angle has zero margin -- Marginal,
    // not Safe or Windows.
    let n_d = 2.417;
    let critical = indicatrix_cut_core::critical_angle_deg(n_d);
    let (_, risk) = tier_margin_and_risk(-critical, Block::Pavilion, n_d, None);
    assert_eq!(risk, 1);
}

#[test]
fn tier_margin_and_risk_classifies_a_crown_tier_as_an_estimate_against_a_pavilion_partner() {
    // Diamond's critical angle is ~24.4 degrees; a 34-degree crown facet
    // against a comfortably-safe 41-degree pavilion partner should itself
    // read Safe, suffixed "(est.)" to mark it as an estimate.
    let (text, risk) = tier_margin_and_risk(34.0, Block::Crown, 2.417, Some(41.0));
    assert_eq!(risk, 0);
    assert!(text.ends_with("(est.)"), "{text}");
}

// --- constraint_kind_and_text ---

#[test]
fn constraint_kind_and_text_reads_the_three_plain_constraint_kinds_with_no_target() {
    assert_eq!(
        constraint_kind_and_text(&MeetConstraint::MeetExisting, None),
        (0, String::new())
    );
    assert_eq!(
        constraint_kind_and_text(
            &MeetConstraint::MeetNamed(vec!["P1".to_string(), "G1".to_string()]),
            None
        ),
        (1, "P1, G1".to_string())
    );
    assert_eq!(
        constraint_kind_and_text(&MeetConstraint::ScaleReference(0.65), None),
        (2, "0.65".to_string())
    );
}

#[test]
fn constraint_kind_and_text_prefers_a_target_over_the_scale_reference_placeholder() {
    // `Design::resolved_meet_tier_inputs` always leaves a target-bearing tier's
    // own `constraint` as a `ScaleReference(0.0)` placeholder -- this must read
    // back the TARGET (kind 3/4/5), never "kind 2, text '0'".
    let placeholder = MeetConstraint::ScaleReference(0.0);
    assert_eq!(
        constraint_kind_and_text(&placeholder, Some(TierTarget::DepthMm(3.2))),
        (3, "3.2".to_string())
    );
    assert_eq!(
        constraint_kind_and_text(&placeholder, Some(TierTarget::GirdleThicknessMm(0.25))),
        (4, "0.25".to_string())
    );
    assert_eq!(
        constraint_kind_and_text(&placeholder, Some(TierTarget::TableWidthMm(4.1))),
        (5, "4.1".to_string())
    );
}

// --- index_chip_items ---

#[test]
fn index_chip_items_builds_one_chip_per_index_flagging_only_the_detached_ones() {
    let indices = vec![0.0, 24.0, 48.0, 72.0];
    let detached = vec![24.0];
    let chips = index_chip_items(&indices, &detached);
    assert_eq!(chips.len(), 4);
    assert_eq!(chips[0].position, 0.0);
    assert_eq!(chips[0].label.as_str(), "0");
    assert!(!chips[0].detached);
    assert_eq!(chips[1].position, 24.0);
    assert!(chips[1].detached);
    assert!(!chips[2].detached);
    assert!(!chips[3].detached);
}

#[test]
fn index_chip_items_formats_a_fractional_position_with_two_decimals() {
    let chips = index_chip_items(&[12.5], &[]);
    assert_eq!(chips[0].label.as_str(), "12.50");
}

#[test]
fn index_chip_items_is_empty_for_an_empty_tier() {
    assert_eq!(index_chip_items(&[], &[]).len(), 0);
}

// --- tier_matches_filter ---

#[test]
fn tier_matches_filter_is_case_insensitive_and_matches_a_substring_anywhere() {
    assert!(tier_matches_filter("Girdle Facet G1", "girdle"));
    assert!(tier_matches_filter("Girdle Facet G1", "FACET"));
    assert!(tier_matches_filter("Girdle Facet G1", "g1"));
    assert!(!tier_matches_filter("Girdle Facet G1", "pavilion"));
}

#[test]
fn tier_matches_filter_treats_a_blank_or_whitespace_only_filter_as_matching_everything() {
    assert!(tier_matches_filter("anything", ""));
    assert!(tier_matches_filter("anything", "   "));
    assert!(tier_matches_filter("", ""));
}

#[test]
fn cutting_rows_emit_second_lines_in_cutting_order() {
    use crate::view_model::yield_report::cutting_instructions_rows;
    use indicatrix_cut_core::design::TierRef;

    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let rows = cutting_instructions_rows(&design, &solved);
    let order = design.cutting_order();
    assert_eq!(rows.len(), order.len());
    for (position, (row, tier_ref)) in rows.iter().zip(&order).enumerate() {
        assert_eq!(row.order_idx, position as i32);
        match *tier_ref {
            TierRef::Flat(_) => assert_eq!(row.second_line, None),
            TierRef::Concave(i) => assert_eq!(
                row.second_line,
                Some(design.concave_tiers[i].second_line_fields())
            ),
        }
    }
    assert_eq!(
        rows.iter().filter(|r| r.second_line.is_some()).count(),
        design.concave_tiers.len()
    );
    // The pavilion tool line follows the pavilion flat rows and precedes the first
    // flat crown row; the crown tool line closes the sheet (this fixture has no table
    // tier to sit above).
    let pavilion_tool = rows
        .iter()
        .position(|r| r.second_line.as_ref().is_some_and(|l| l[0] == "CYL"));
    let first_crown = rows
        .iter()
        .position(|r| r.side == 1 && r.second_line.is_none());
    assert!(pavilion_tool < first_crown);
    assert!(
        rows.last()
            .is_some_and(|r| r.side == 1 && r.second_line.is_some())
    );
}

#[test]
fn unnamed_flat_cutting_rows_are_labelled_by_cutting_position() {
    use crate::view_model::yield_report::cutting_instructions_rows;

    let mut design = Design::concave_fixture();
    for tier in &mut design.tiers {
        tier.name.clear();
    }
    let solved = design.solve().expect("the fixture's flat tiers solve");
    let rows = cutting_instructions_rows(&design, &solved);
    let flat: Vec<_> = rows.iter().filter(|r| r.second_line.is_none()).collect();
    assert_eq!(flat.len(), design.tiers.len());
    for row in flat {
        assert_eq!(row.facet, format!("#{}", row.order_idx + 1));
    }
    // The first crown tier is stored fourth but cut fifth, after the groove.
    let first_crown = rows
        .iter()
        .find(|r| r.side == 1 && r.second_line.is_none())
        .expect("a flat crown row");
    assert_eq!(first_crown.facet, "#5");
}

/// A planar design's schedule is in cutting order too, no longer in stored order: the fixture
/// is stored top-down (table first), the rows open with the girdle and the pavilion and close
/// with the table, each row's side the solver's classification of the tier it shows.
#[test]
fn cutting_rows_of_a_planar_design_follow_the_cutting_order() {
    use crate::view_model::yield_report::cutting_instructions_rows;

    let design = Design::new(
        indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
        indicatrix_cut_core::ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    let solved = design.solve().expect("every tier is pinned");
    let rows = cutting_instructions_rows(&design, &solved);
    let facets: Vec<&str> = rows.iter().map(|row| row.facet.as_str()).collect();
    assert_eq!(
        facets,
        [
            "Girdle",
            "Pavilion Main",
            "Lower Girdle",
            "Culet",
            "Star",
            "Crown Main",
            "Upper Girdle",
            "Table"
        ]
    );
    let sides: Vec<i32> = rows.iter().map(|row| row.side).collect();
    assert_eq!(sides, [0, -1, -1, -1, 1, 1, 1, 1]);
    let positions: Vec<i32> = rows.iter().map(|row| row.order_idx).collect();
    assert_eq!(positions, [0, 1, 2, 3, 4, 5, 6, 7]);
}

/// The tier table keeps the stored order, but each row's code is the one the cutting order
/// gives it: the fixture is stored table first, so the table row reads `T` and the girdle
/// (stored fifth) reads `G1`; a concave tier's code continues its letter's count.
#[test]
fn tier_rows_carry_their_code_in_cutting_order() {
    let planar = Design::new(
        indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
        indicatrix_cut_core::ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    let rows = tier_items(&planar, planar.effective_refractive_index());
    let codes: Vec<&str> = rows.iter().map(|row| row.code.as_str()).collect();
    assert_eq!(codes, ["T", "C1", "C2", "C3", "G1", "P1", "P2", "Culet"]);
    let stale = tier_items_stale(&planar, planar.effective_refractive_index());
    assert_eq!(
        stale
            .iter()
            .map(|row| row.code.as_str())
            .collect::<Vec<_>>(),
        codes,
        "the stale rows carry the same codes"
    );

    let concave = Design::concave_fixture();
    let rows = tier_items(&concave, concave.effective_refractive_index());
    let (flat, tools) = rows.split_at(concave.tiers.len());
    let flat_codes: Vec<&str> = flat.iter().map(|row| row.code.as_str()).collect();
    assert_eq!(flat_codes, ["G1", "P1", "P2", "C1", "C2"]);
    let tool_codes: Vec<&str> = tools.iter().map(|row| row.code.as_str()).collect();
    assert_eq!(
        tool_codes,
        ["P3", "C3"],
        "the groove follows the two flat pavilion tiers, the dimple the two crown tiers"
    );
}

#[test]
fn tier_rows_append_concave_rows_with_their_tool_line() {
    use crate::view_model::TierRowKind;

    let design = Design::concave_fixture();
    let rows = tier_items(&design, design.effective_refractive_index());
    assert_eq!(rows.len(), design.tiers.len() + design.concave_tiers.len());
    let (flat, concave) = rows.split_at(design.tiers.len());
    assert!(
        flat.iter()
            .all(|r| r.kind == TierRowKind::Flat && r.tool_line.is_empty())
    );
    for (i, row) in concave.iter().enumerate() {
        assert_eq!(row.kind, TierRowKind::Concave);
        assert_eq!(row.index, i as i32);
        assert_eq!(row.tool_line, concave_tool_line(&design.concave_tiers[i]));
    }
    assert_eq!(concave[0].block, "Pavilion");
    assert_eq!(concave[1].block, "Crown");
}

/// The tier table's link badge and the inspector note read `relation_text`: empty for a
/// free tier, the relation as a cutter reads it for a driven one, and gone again once the
/// relation is cleared. The concave rows never carry one.
#[test]
fn a_driven_tier_row_carries_its_relation_text() {
    let mut state = EditorSession::fresh();
    for (index, (name, angle)) in [("P1", -41.0), ("P2", -39.0)].into_iter().enumerate() {
        state
            .apply(Edit::AddTier {
                index,
                tier: ConstraintTier {
                    angle_deg: angle,
                    name: name.to_string(),
                    indices: vec![0.0, 24.0],
                    constraint: MeetConstraint::ScaleReference(0.65),
                    imported_meet: None,
                    original_notes: None,
                    detached: Vec::new(),
                },
            })
            .unwrap();
    }
    let n_d = state.design.effective_refractive_index();
    let rows = tier_items_stale(&state.design, n_d);
    assert!(rows.iter().all(|row| row.relation_text.is_empty()));

    state.set_tier_relation(1, "=P1 - 2").unwrap();
    let rows = tier_items_stale(&state.design, n_d);
    assert_eq!(rows[0].relation_text, "");
    assert_eq!(rows[1].relation_text, "P1 - 2");
    assert_eq!(rows[1].angle_deg.as_str(), "39.00", "the value still shows");

    state.clear_tier_relation(1).unwrap();
    let rows = tier_items_stale(&state.design, n_d);
    assert_eq!(rows[1].relation_text, "");

    let concave = Design::concave_fixture();
    let rows = tier_items(&concave, concave.effective_refractive_index());
    assert!(rows.iter().all(|row| row.relation_text.is_empty()));
}
