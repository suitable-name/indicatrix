//! Tests for [`super`]: the arithmetic parser, the two text forms of a relation, and
//! the evaluation of a design's relations.

use super::*;
use crate::{design::ConstraintTier, preform::PreformSpec};
use indicatrix::geometry::meet_solver::MeetConstraint;

fn tier(name: &str, angle_deg: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![0.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// A design whose tiers have these names and angles, with ids 0, 1, 2, ...
fn design_with(tiers: &[(&str, f64)]) -> Design {
    let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers = tiers
        .iter()
        .map(|&(name, angle)| tier(name, angle))
        .collect();
    design.ensure_tier_ids();
    design
}

/// Gives the tier at `index` the relation `text` (read against the design).
fn relate(design: &mut Design, index: usize, text: &str) {
    let relation = design.parse_relation(text).expect("relation reads");
    let id = design.tier_ids[index];
    design.tier_relations.insert(id, relation);
}

fn calc(text: &str) -> Result<f64, ExprError> {
    Expr::<RawName>::parse_syntax(text, MAX_RELATION_CHARS)?
        .eval(&mut |_| Err(ExprError::UnknownName("no names here".to_owned())))
}

// --- the parser ---------------------------------------------------------------------

#[test]
fn arithmetic_follows_the_usual_precedence() {
    assert_eq!(calc("2 + 3 * 4"), Ok(14.0));
    assert_eq!(calc("(2 + 3) * 4"), Ok(20.0));
    assert_eq!(calc("10 - 4 - 3"), Ok(3.0));
    assert_eq!(calc("8 / 4 / 2"), Ok(1.0));
    assert_eq!(calc("2 * 3 + 4 * 5"), Ok(26.0));
    assert_eq!(calc("1e1 + .5"), Ok(10.5));
}

#[test]
fn unary_signs_bind_tighter_than_multiplication() {
    assert_eq!(calc("-2 * 3"), Ok(-6.0));
    assert_eq!(calc("2 * -3"), Ok(-6.0));
    assert_eq!(calc("--3"), Ok(3.0));
    assert_eq!(calc("+3"), Ok(3.0));
    assert_eq!(calc("-(2 + 3)"), Ok(-5.0));
    assert_eq!(calc("5 - -2"), Ok(7.0));
}

#[test]
fn names_are_collected_in_reading_order() {
    let tree = Expr::<RawName>::parse_syntax("P1 - [Crown Main] + @3", MAX_RELATION_CHARS)
        .expect("parses");
    assert_eq!(
        tree.refs(),
        vec![
            &RawName::Bare("P1".to_owned()),
            &RawName::Bracketed("Crown Main".to_owned()),
            &RawName::Id(3)
        ]
    );
}

#[test]
fn broken_text_says_what_is_wrong() {
    let syntax = |text: &str| match Expr::<RawName>::parse_syntax(text, MAX_RELATION_CHARS) {
        Err(ExprError::Syntax(message)) => message,
        other => panic!("{text:?}: expected a syntax error, got {other:?}"),
    };
    assert!(syntax("1 +").contains("missing at the end"));
    assert!(syntax("(1").contains("never closed"));
    assert!(syntax("1)").contains("without a matching"));
    assert!(syntax("1 2").contains("put +"));
    assert!(syntax("* 3").contains("missing before '*'"));
    assert!(syntax("3 $ 4").contains("'$' is not understood here"));
    assert!(syntax("[x").contains("never closed"));
    assert!(syntax("[  ]").contains("no tier name"));
    assert!(syntax("@").contains("must be followed by a tier number"));
    assert!(syntax("1e400").contains("not a usable number"));
    for blank in ["", "   "] {
        assert_eq!(
            Expr::<RawName>::parse_syntax(blank, MAX_RELATION_CHARS),
            Err(ExprError::Empty)
        );
    }
    assert_eq!(calc("1 / 0"), Err(ExprError::DivisionByZero));
    assert_eq!(calc("1e308 * 10"), Err(ExprError::NotFinite));
}

#[test]
fn the_length_and_nesting_limits_hold() {
    let long = "1".repeat(MAX_RELATION_CHARS + 1);
    assert_eq!(
        Expr::<RawName>::parse_syntax(&long, MAX_RELATION_CHARS),
        Err(ExprError::TooLong {
            max: MAX_RELATION_CHARS
        })
    );
    let exactly = "1".repeat(MAX_RELATION_CHARS);
    assert!(Expr::<RawName>::parse_syntax(&exactly, MAX_RELATION_CHARS).is_ok());

    let nested = |depth: usize| format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
    assert_eq!(calc(&nested(MAX_NESTING)), Ok(1.0));
    assert_eq!(calc(&nested(MAX_NESTING + 1)), Err(ExprError::TooDeep));
    let signs = |count: usize| format!("{}1", "-".repeat(count));
    assert!(calc(&signs(MAX_NESTING)).is_ok());
    assert_eq!(calc(&signs(MAX_NESTING + 1)), Err(ExprError::TooDeep));
}

#[test]
fn a_tree_with_too_many_nodes_or_a_huge_constant_is_refused() {
    let big = (0..600).fold(RelationExpr::Number(1.0), |sum, _| {
        RelationExpr::Add(Box::new(sum), Box::new(RelationExpr::Number(1.0)))
    });
    assert!(matches!(big.validate(), Err(RelationError::Parse(_))));
    let huge = RelationExpr::Number(2.0 * MAX_NUMBER);
    assert!(matches!(huge.validate(), Err(RelationError::Parse(_))));
    assert_eq!(RelationExpr::Number(MAX_NUMBER).validate(), Ok(()));
}

// --- the two text forms -------------------------------------------------------------

#[test]
fn canonical_text_reads_back_and_writes_the_same_text() {
    for text in [
        "@0 - 2",
        "(@0 + @1) / 2",
        "@0 - (@1 - 2)",
        "@0 - @1 - 2",
        "-(@0 + 1)",
        "2 * (3 + @0)",
        "@0 / (2 * 3)",
        "@0 / 2 / 3",
        "@0 * -2",
        "@0 - -2",
        "-2 * @0",
        "-(-@0)",
        "1e-7 + @0",
        "0.5 * @1",
        "@12",
    ] {
        let relation = TierRelation::parse_canonical(text).expect(text);
        assert_eq!(relation.to_canonical(), text);
    }
}

#[test]
fn redundant_brackets_are_dropped_from_the_canonical_text() {
    for (typed, canonical) in [
        ("((@0))", "@0"),
        ("(@0 * 2) + 1", "@0 * 2 + 1"),
        ("(@0 - 1) - 2", "@0 - 1 - 2"),
        ("+@0", "@0"),
        ("@0 + 3.0", "@0 + 3"),
    ] {
        let relation = TierRelation::parse_canonical(typed).expect(typed);
        assert_eq!(relation.to_canonical(), canonical, "{typed}");
    }
}

#[test]
fn canonical_text_names_tiers_by_number_only() {
    match TierRelation::parse_canonical("P1 - 2") {
        Err(RelationError::Parse(message)) => assert!(message.contains("by number"), "{message}"),
        other => panic!("expected a parse error, got {other:?}"),
    }
    match TierRelation::parse_canonical("@0 +") {
        Err(RelationError::Parse(message)) => {
            assert!(
                message.starts_with("The saved relation cannot be read"),
                "{message}"
            );
        }
        other => panic!("expected a parse error, got {other:?}"),
    }
    let too_long = format!("@0{}", " + 1".repeat(300));
    assert!(TierRelation::parse_canonical(&too_long).is_err());
}

#[test]
fn user_text_resolves_names_and_survives_renames() {
    let mut design = design_with(&[("P1", -40.0), ("P2", -38.0), ("P3", -30.0)]);
    let relation = TierRelation::parse_user("P1 - 2", &design).expect("reads");
    assert_eq!(relation.to_canonical(), "@0 - 2");
    assert_eq!(relation.to_display(&design), "P1 - 2");
    assert_eq!(relation.references(), BTreeSet::from([TierId(0)]));

    // A rename changes the display, never the stored relation.
    design.tiers[0].name = "Main".to_owned();
    assert_eq!(relation.to_display(&design), "Main - 2");
    assert_eq!(
        TierRelation::parse_canonical("@0 - 2").expect("reads"),
        relation
    );
    // A name with a space is written in brackets, and reads back.
    design.tiers[0].name = "Crown Main".to_owned();
    let shown = relation.to_display(&design);
    assert_eq!(shown, "[Crown Main] - 2");
    assert_eq!(
        TierRelation::parse_user(&shown, &design),
        Ok(relation.clone())
    );
    // Two tiers with one name cannot be told apart: the id is written instead.
    design.tiers[1].name = "Crown Main".to_owned();
    assert_eq!(relation.to_display(&design), "@0 - 2");
}

#[test]
fn names_resolve_exactly_first_then_ignoring_case() {
    let design = design_with(&[("p1", 40.0), ("P1", 38.0), ("Table", 0.0)]);
    let reads = |text: &str| {
        TierRelation::parse_user(text, &design)
            .expect(text)
            .references()
    };
    assert_eq!(reads("P1"), BTreeSet::from([TierId(1)]));
    assert_eq!(reads("p1"), BTreeSet::from([TierId(0)]));
    assert_eq!(reads("TABLE"), BTreeSet::from([TierId(2)]));
    assert_eq!(reads("@1"), BTreeSet::from([TierId(1)]));
    // A leading `=` is allowed and ignored.
    assert_eq!(
        TierRelation::parse_user("= P1 - 2", &design),
        TierRelation::parse_user("P1 - 2", &design)
    );
}

#[test]
fn user_text_that_cannot_be_used_says_why() {
    let design = design_with(&[("A", 40.0), ("B", 38.0), ("B", 30.0)]);
    let message = |text: &str| match TierRelation::parse_user(text, &design) {
        Err(RelationError::Parse(message)) => message,
        other => panic!("{text:?}: expected a parse error, got {other:?}"),
    };
    assert!(message("Q9 - 2").starts_with("This relation cannot be read:"));
    assert!(message("Q9 - 2").contains("there is no tier called 'Q9'"));
    assert!(message("B + 1").contains("more than one tier is called 'B'"));
    assert!(message("@77").contains("there is no tier @77"));
    assert!(message("").contains("Type a relation"));
    assert!(message("=").contains("Type a relation"));
    assert!(message("A +").contains("missing at the end"));
}

#[test]
fn an_offset_from_a_tier_writes_a_plain_sum() {
    assert_eq!(
        RelationExpr::offset_from(TierId(5), -2.0).to_canonical(),
        "@5 - 2"
    );
    assert_eq!(
        RelationExpr::offset_from(TierId(5), 3.5).to_canonical(),
        "@5 + 3.5"
    );
    assert_eq!(
        RelationExpr::offset_from(TierId(5), 0.0).to_canonical(),
        "@5"
    );
}

// --- evaluation ---------------------------------------------------------------------

#[test]
fn a_design_without_relations_evaluates_to_nothing() {
    let design = design_with(&[("A", 40.0), ("B", 38.0)]);
    assert_eq!(design.evaluate_relations(), Ok(Vec::new()));
}

#[test]
fn a_relation_gives_the_driven_tier_its_angle() {
    let mut design = design_with(&[("C1", 40.0), ("C2", 10.0), ("C3", 30.0)]);
    relate(&mut design, 1, "C1 - 2");
    assert_eq!(design.evaluate_relations(), Ok(vec![(1, 38.0)]));
    assert!(design.is_tier_driven(1));
    assert!(!design.is_tier_driven(0));
    assert_eq!(design.relation_text(1).as_deref(), Some("C1 - 2"));
    assert_eq!(design.relation_text(0), None);
}

#[test]
fn float_noise_is_snapped_off_the_result() {
    let mut design = design_with(&[("C1", 40.0), ("C2", 10.0)]);
    relate(&mut design, 1, "C1 + 3 * 0.1");
    let updates = design.evaluate_relations().expect("evaluates");
    assert_eq!(updates[0].1.to_bits(), 40.3_f64.to_bits());
}

#[test]
fn a_driven_tier_comes_after_the_driven_tiers_it_reads() {
    // A reads B, B reads C: the order is B then A whatever the positions say.
    let mut design = design_with(&[("A", 1.0), ("B", 1.0), ("C", 10.0)]);
    relate(&mut design, 0, "B + 1");
    relate(&mut design, 1, "C + 1");
    assert_eq!(design.evaluate_relations(), Ok(vec![(1, 11.0), (0, 12.0)]));
    assert_eq!(design.relation_drivers(0), vec![1]);
    assert_eq!(design.relation_dependants(1), vec![0]);
    assert_eq!(design.relation_dependants(2), vec![1]);
    assert_eq!(design.relation_drivers(2), Vec::<usize>::new());
}

#[test]
fn several_drivers_are_listed_in_tier_order() {
    let mut design = design_with(&[("A", 1.0), ("B", 20.0), ("C", 30.0)]);
    relate(&mut design, 0, "(C + B) / 2");
    assert_eq!(design.relation_drivers(0), vec![1, 2]);
    assert_eq!(design.evaluate_relations(), Ok(vec![(0, 25.0)]));
}

#[test]
fn a_reference_is_a_magnitude_and_the_driven_tier_keeps_its_side() {
    let mut design = design_with(&[("P1", -40.0), ("P2", -10.0), ("C1", 10.0)]);
    relate(&mut design, 1, "P1 - 2");
    relate(&mut design, 2, "P1 - 2");
    // The pavilion tier stays a pavilion tier, the crown tier a crown tier.
    assert_eq!(design.evaluate_relations(), Ok(vec![(1, -38.0), (2, 38.0)]));
}

#[test]
fn tiers_that_read_each_other_are_a_loop() {
    let mut design = design_with(&[("A", 10.0), ("B", 10.0), ("C", 10.0)]);
    relate(&mut design, 0, "B");
    relate(&mut design, 1, "A");
    let error = design.evaluate_relations().expect_err("a loop");
    assert_eq!(
        error,
        RelationError::Cycle(vec!["A".to_owned(), "B".to_owned()])
    );
    assert_eq!(error.to_string(), "A and B refer to each other in a loop.");

    relate(&mut design, 1, "C");
    relate(&mut design, 2, "A");
    let error = design.evaluate_relations().expect_err("a loop of three");
    assert_eq!(
        error.to_string(),
        "A, B and C refer to each other in a loop."
    );
}

#[test]
fn a_tier_that_reads_itself_is_a_loop() {
    let mut design = design_with(&[("A", 10.0)]);
    relate(&mut design, 0, "A + 1");
    let error = design.evaluate_relations().expect_err("a loop");
    assert_eq!(error, RelationError::Cycle(vec!["A".to_owned()]));
    assert_eq!(error.to_string(), "A refers to itself.");
}

#[test]
fn a_result_outside_the_facet_range_is_refused_by_name() {
    let mut design = design_with(&[("A", 10.0), ("B", 40.0)]);
    relate(&mut design, 0, "B + 60");
    let error = design.evaluate_relations().expect_err("too steep");
    assert_eq!(
        error,
        RelationError::OutOfRange {
            tier: "A".to_owned(),
            value: 100.0
        }
    );
    assert_eq!(
        error.to_string(),
        "A would come out at 100.00\u{b0}, but a facet angle must be more than 0\u{b0} and at \
         most 90\u{b0}."
    );
    // Exactly 90 is a girdle facet and allowed; 0 and below are not.
    relate(&mut design, 0, "B + 50");
    assert_eq!(design.evaluate_relations(), Ok(vec![(0, 90.0)]));
    relate(&mut design, 0, "B - 40");
    assert!(matches!(
        design.evaluate_relations(),
        Err(RelationError::OutOfRange { value, .. }) if value == 0.0
    ));
    relate(&mut design, 0, "B - 50");
    assert!(matches!(
        design.evaluate_relations(),
        Err(RelationError::OutOfRange { value, .. }) if value < 0.0
    ));
}

#[test]
fn arithmetic_that_cannot_be_done_is_refused_by_name() {
    let mut design = design_with(&[("A", 10.0), ("B", 40.0)]);
    relate(&mut design, 0, "1 / (B - 40)");
    assert_eq!(
        design.evaluate_relations(),
        Err(RelationError::DivisionByZero {
            tier: "A".to_owned()
        })
    );
    // A relation may only hold constants up to MAX_NUMBER (1e9), so one literal cannot
    // overflow; forty of them multiplied together (1e360) can, and that overflow must be
    // named when the relations are evaluated.
    let overflowing = format!("B{}", " * 1e9".repeat(40));
    assert!(overflowing.chars().count() <= MAX_RELATION_CHARS);
    relate(&mut design, 0, &overflowing);
    assert_eq!(
        design.evaluate_relations(),
        Err(RelationError::NotFinite {
            tier: "A".to_owned()
        })
    );
}

#[test]
fn a_literal_beyond_the_largest_allowed_number_is_refused_when_typed() {
    let design = design_with(&[("A", 10.0), ("B", 40.0)]);
    // The old overflow test relied on 1e308 literals; the limit is deliberate, so they
    // are refused up front with the plain-English reason.
    let too_large = "This relation is too big, or uses a number that is too large.";
    for text in ["B * 1e308 * 1e308", "B * 1e10", "B + 2e9", "-1e10 + B"] {
        assert_eq!(
            design.parse_relation(text),
            Err(RelationError::Parse(too_large.to_owned())),
            "{text}"
        );
    }
    // Exactly the limit is still fine.
    assert!(design.parse_relation("B * 1e9").is_ok());
}

#[test]
fn a_relation_reading_a_tier_that_is_gone_is_refused() {
    let mut design = design_with(&[("A", 10.0), ("B", 40.0)]);
    let id = design.tier_ids[0];
    design.tier_relations.insert(
        id,
        TierRelation::new(RelationExpr::offset_from(TierId(99), 1.0)),
    );
    let error = design.evaluate_relations().expect_err("missing tier");
    assert_eq!(
        error,
        RelationError::MissingTier {
            tier: "A".to_owned()
        }
    );
    assert_eq!(
        error.to_string(),
        "A refers to a tier that is no longer in the design."
    );
    // A relation whose own tier is gone is simply not evaluated.
    design.tier_relations.clear();
    design.tier_relations.insert(
        TierId(77),
        TierRelation::new(RelationExpr::offset_from(design.tier_ids[1], 1.0)),
    );
    assert_eq!(design.evaluate_relations(), Ok(Vec::new()));
}

#[test]
fn only_a_facet_tier_may_follow_a_relation() {
    let design = design_with(&[
        ("T", 0.0),
        ("Culet", -0.0),
        ("G", 90.0),
        ("PG", -90.0),
        ("C1", 40.0),
    ]);
    for index in [0, 1] {
        assert!(matches!(
            design.check_relation_target(index),
            Err(RelationError::HorizontalTier { .. })
        ));
    }
    for index in [2, 3] {
        assert!(matches!(
            design.check_relation_target(index),
            Err(RelationError::GirdleTier { .. })
        ));
    }
    assert_eq!(design.check_relation_target(4), Ok(()));
    assert_eq!(
        design.check_relation_target(9),
        Err(RelationError::NoSuchTier { index: 9 })
    );
    assert_eq!(
        design.check_relation_target(0).unwrap_err().to_string(),
        "T is flat, so its angle cannot follow a relation."
    );
    assert_eq!(
        design.check_relation_target(2).unwrap_err().to_string(),
        "G is a girdle facet (90\u{b0}), so its angle cannot follow a relation."
    );
    assert_eq!(
        RelationError::NoSuchTier { index: 9 }.to_string(),
        "There is no tier 10."
    );
}

#[test]
fn designs_compare_relations_by_tier_position() {
    let mut a = design_with(&[("A", 10.0), ("B", 40.0)]);
    let b = a.clone();
    assert_eq!(a, b);
    relate(&mut a, 0, "B - 2");
    assert_ne!(a, b);
    let mut c = b;
    relate(&mut c, 0, "B - 2");
    assert_eq!(a, c);
    relate(&mut c, 0, "B - 3");
    assert_ne!(a, c);
}

#[test]
fn the_next_tier_id_counts_tiers_not_yet_given_one() {
    let mut design = design_with(&[("A", 10.0), ("B", 40.0), ("C", 30.0)]);
    assert_eq!(design.peek_next_tier_id(), TierId(3));
    design.tiers.push(tier("D", 20.0));
    assert_eq!(design.peek_next_tier_id(), TierId(4));
}

#[test]
fn a_tier_label_falls_back_to_its_number() {
    let design = design_with(&[("", 10.0), ("B", 40.0)]);
    assert_eq!(design.relation_label(0), "tier 1");
    assert_eq!(design.relation_label(1), "B");
    assert_eq!(design.relation_label(5), "tier 6");
}

#[test]
fn a_tier_is_found_by_name() {
    let design = design_with(&[
        ("P1", 10.0),
        ("Crown/Main", 40.0),
        ("Dup", 1.0),
        ("dup", 2.0),
    ]);
    assert_eq!(design.tier_position_by_name("P1"), Ok(0));
    assert_eq!(design.tier_position_by_name("p1"), Ok(0));
    assert_eq!(design.tier_position_by_name("Main"), Ok(1));
    assert_eq!(design.tier_position_by_name("Dup"), Ok(2));
    assert!(design.tier_position_by_name("Nope").is_err());
}
