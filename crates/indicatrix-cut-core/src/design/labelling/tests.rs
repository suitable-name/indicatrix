//! Tests for [`super`].

use super::*;
use crate::{
    design::{ConcaveTool, ScheduleMeta, ToolMotion},
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

fn pinned(name: &str, angle_deg: f64) -> ConstraintTier {
    meeting(name, angle_deg, MeetConstraint::ScaleReference(0.5))
}

fn meeting(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_owned(),
        indices: vec![0.0],
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn design(tiers: Vec<ConstraintTier>) -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        tiers,
    )
}

fn concave(name: &str, angle_deg: f64) -> ConcaveTier {
    ConcaveTier {
        name: name.to_owned(),
        angle_deg,
        indices: vec![0.0, 4.0],
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: 0.0,
        displacement: [0.0, 0.0, 0.1],
        diameter_ratio: 0.5,
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    }
}

fn codes(labels: &[TierLabelInfo]) -> Vec<&str> {
    labels.iter().map(|label| label.code.as_str()).collect()
}

#[test]
fn test_is_legacy_123_abc() {
    assert!(is_legacy_123_abc("1"));
    assert!(is_legacy_123_abc("2"));
    assert!(is_legacy_123_abc("12"));
    assert!(is_legacy_123_abc("1A"));
    assert!(is_legacy_123_abc("A"));
    assert!(is_legacy_123_abc("B"));
    assert!(is_legacy_123_abc("G"));
    assert!(is_legacy_123_abc("T"));

    assert!(!is_legacy_123_abc("P1"));
    assert!(!is_legacy_123_abc("PF1"));
    assert!(!is_legacy_123_abc("C1"));
    assert!(!is_legacy_123_abc("G1"));
    assert!(!is_legacy_123_abc("Table"));
    assert!(!is_legacy_123_abc("Pavilion Main"));
}

#[test]
fn test_convert_legacy_facet_name() {
    assert_eq!(convert_legacy_facet_name("1", Block::Pavilion, 1), "P1");
    assert_eq!(convert_legacy_facet_name("2", Block::Pavilion, 2), "P2");
    assert_eq!(convert_legacy_facet_name("A", Block::Crown, 1), "C1");
    assert_eq!(convert_legacy_facet_name("B", Block::Crown, 2), "C2");
    assert_eq!(convert_legacy_facet_name("G", Block::Girdle, 1), "G1");
    assert_eq!(
        convert_legacy_facet_name("T", Block::Crown, 1),
        "T",
        "the table's code is T"
    );
    assert_eq!(convert_legacy_facet_name("Table", Block::Crown, 1), "T");
}

/// The Standard Round Brilliant fixture is stored top-down (table first). Cut in order it
/// is the girdle, the pavilion main, the lower girdle, the culet, the star, the crown main,
/// the upper girdle and the table: `G1`, `P1`, `P2`, `Culet`, `C1`, `C2`, `C3`, `T`. The
/// list below is in stored order, so each code sits at the tier's own index.
#[test]
fn test_standard_round_brilliant_canonical_labels() {
    let tiers = ConstraintTier::standard_round_brilliant();
    let labels = compute_tier_labels(&tiers);

    assert_eq!(labels.len(), 8);
    assert_eq!(labels[0].code, "T");
    assert_eq!(labels[0].display_name, "Table");

    assert_eq!(labels[1].code, "C1");
    assert_eq!(labels[1].display_name, "C1 (Star)");

    assert_eq!(labels[2].code, "C2");
    assert_eq!(labels[2].display_name, "C2 (Crown Main)");

    assert_eq!(labels[3].code, "C3");
    assert_eq!(labels[3].display_name, "C3 (Upper Girdle)");

    assert_eq!(labels[4].code, "G1");
    assert_eq!(labels[4].display_name, "G1 (Girdle)");

    assert_eq!(labels[5].code, "P1");
    assert_eq!(labels[5].display_name, "P1 (Pavilion Main)");

    assert_eq!(labels[6].code, "P2");
    assert_eq!(labels[6].display_name, "P2 (Lower Girdle)");

    assert_eq!(labels[7].code, "Culet");
    assert_eq!(labels[7].display_name, "Culet");
}

#[test]
fn the_fixtures_codes_read_in_cutting_order() {
    let tiers = ConstraintTier::standard_round_brilliant();
    let labels = compute_tier_labels(&tiers);
    let in_cut_order: Vec<&str> = flat_cutting_order(&tiers)
        .into_iter()
        .map(|index| labels[index].code.as_str())
        .collect();
    assert_eq!(
        in_cut_order,
        ["G1", "P1", "P2", "Culet", "C1", "C2", "C3", "T"]
    );
}

/// `P` and `G` count independently even when they interleave, and the table is `T`.
#[test]
fn p_and_g_count_independently_when_they_interleave() {
    let tiers = vec![
        pinned("Pav A", -40.0),
        pinned("Pav B", -38.2),
        pinned("Girdle A", 90.0),
        pinned("Girdle B", 90.0),
        pinned("Pav C", -56.58),
        pinned("Table", 0.0),
    ];
    let labels = compute_tier_labels(&tiers);
    assert_eq!(codes(&labels), ["P1", "P2", "G1", "G2", "P3", "T"]);
    assert_eq!(labels[0].display_name, "P1 (Pav A)");
    assert_eq!(labels[5].display_name, "Table");
}

/// A concave pavilion tier continues the `P` count (the template's `P4`) and a concave crown
/// tier the `C` count; the flat codes do not change because concave tiers exist.
#[test]
fn concave_tiers_continue_their_letter() {
    let mut design = design(vec![
        pinned("Pav A", -40.0),
        pinned("Pav B", -38.2),
        pinned("Girdle A", 90.0),
        pinned("Girdle B", 90.0),
        pinned("Pav C", -56.58),
        pinned("Crown A", 35.0),
        pinned("Crown B", 30.0),
        pinned("Table", 0.0),
    ]);
    design.concave_tiers = vec![concave("Dimple", 36.0), concave("Groove", -30.0)];

    let all = design.tier_codes();
    assert_eq!(
        codes(&all.flat),
        ["P1", "P2", "G1", "G2", "P3", "C1", "C2", "T"]
    );
    assert_eq!(all.flat, compute_tier_labels(&design.tiers));
    assert_eq!(
        codes(&all.concave),
        ["C3", "P4"],
        "stored order: Dimple, Groove"
    );
    assert_eq!(all.concave[0].display_name, "C3 (Dimple)");
    assert_eq!(all.concave[1].display_name, "P4 (Groove)");
}

#[test]
fn a_design_without_concave_tiers_has_no_concave_codes() {
    let design = design(ConstraintTier::standard_round_brilliant());
    let all = design.tier_codes();
    assert_eq!(codes(&all.concave), Vec::<&str>::new());
    assert_eq!(all.flat.len(), 8);
}

/// A facet that meets one cut later moves after it, and its code follows: here `Alpha` meets
/// `Gamma`, so the cutting order is Beta, Gamma, Alpha and the codes are P1, P2, P3 in that
/// order -- `Alpha`, stored first, is `P3`.
#[test]
fn a_forward_meet_changes_the_numbering() {
    let tiers = vec![
        meeting(
            "Alpha",
            -41.0,
            MeetConstraint::MeetNamed(vec!["Gamma".to_owned()]),
        ),
        pinned("Beta", -42.0),
        pinned("Gamma", -43.0),
    ];
    let labels = compute_tier_labels(&tiers);
    assert_eq!(codes(&labels), ["P3", "P1", "P2"]);
}

#[test]
fn the_preform_rule_is_unchanged() {
    let tiers = vec![pinned("PF Base", -50.0), pinned("Main", -41.0)];
    let labels = compute_tier_labels(&tiers);
    assert_eq!(codes(&labels), ["PF1", "P1"]);
}

#[test]
fn the_culet_and_a_table_named_by_hand_keep_their_codes() {
    let tiers = vec![
        pinned("Keel", -0.0),
        pinned("table", 12.0),
        pinned("Crown", 40.0),
    ];
    let labels = compute_tier_labels(&tiers);
    assert_eq!(codes(&labels), ["Culet", "T", "C1"]);
}
