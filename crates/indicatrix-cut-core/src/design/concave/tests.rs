//! Tests for [`super`].

use super::{
    TierRef::{Concave as C, Flat as F},
    *,
};
use crate::{design::ScheduleMeta, preform::PreformSpec};
use indicatrix::geometry::meet_solver::MeetConstraint;

fn sample() -> ConcaveTier {
    ConcaveTier {
        name: "Groove".to_owned(),
        angle_deg: -40.0,
        indices: vec![0.0, 24.0, 48.0, 72.0],
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: 0.0,
        displacement: [0.0, 0.0, 0.1],
        diameter_ratio: 0.5,
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    }
}

fn flat(name: &str, angle_deg: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_owned(),
        indices: vec![0.0],
        constraint: MeetConstraint::MeetExisting,
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

#[test]
fn concave_tool_codes_round_trip_through_from_str_case_insensitively() {
    for tool in ConcaveTool::ALL {
        assert_eq!(tool.code().parse::<ConcaveTool>(), Ok(tool));
        assert_eq!(tool.to_string(), tool.code());
        let lower = tool.code().to_ascii_lowercase();
        assert_eq!(format!("  {lower}\t").parse::<ConcaveTool>(), Ok(tool));
    }
    assert_eq!(
        "XYZ".parse::<ConcaveTool>(),
        Err(UnknownToolCode("XYZ".to_owned()))
    );
}

#[test]
fn second_line_fields_match_the_reference_template() {
    let cylinder = ConcaveTier {
        tool_azimuth_deg: 10.0,
        displacement: [0.0; 3],
        diameter_ratio: 0.4,
        ..sample()
    };
    assert_eq!(
        cylinder.second_line_fields(),
        [
            "CYL",
            "+10.00°",
            "X = 0.000, Y = 0.000, Z = 0.000",
            "D/W = 0.400, reciprocating"
        ]
    );
    let cone = ConcaveTier {
        tool: ConcaveTool::Cone,
        tool_angle_deg: Some(60.0),
        tool_azimuth_deg: 0.0,
        displacement: [-0.25, 0.12, 0.05],
        diameter_ratio: 0.6,
        ..sample()
    };
    assert_eq!(
        cone.second_line_fields(),
        [
            "CON",
            "0.00°",
            "X = -0.250, Y = 0.120, Z = 0.050",
            "D/W = 0.600, angle = 60.00°, reciprocating"
        ]
    );
    let negative = ConcaveTier {
        tool: ConcaveTool::Disc,
        tool_angle_deg: Some(90.0),
        tool_azimuth_deg: -15.0,
        displacement: [-0.0, 0.0, 0.0],
        ..sample()
    };
    assert_eq!(negative.second_line_fields()[1], "-15.00°");
    assert_eq!(
        negative.second_line_fields()[2],
        "X = 0.000, Y = 0.000, Z = 0.000"
    );
    let plunge = ConcaveTier {
        tool: ConcaveTool::Sphere,
        motion: ToolMotion::Plunge,
        diameter_ratio: 0.25,
        ..sample()
    };
    assert_eq!(plunge.second_line_fields()[0], "SPH");
    assert_eq!(plunge.second_line_fields()[3], "D/W = 0.250, plunge");
}

/// The ASCII columns are the Unicode ones with ` deg` for each degree sign, nothing
/// else: one formatter, two spellings of the unit.
#[test]
fn the_ascii_second_line_differs_only_in_the_spelling_of_the_degree() {
    let cone = ConcaveTier {
        tool: ConcaveTool::Cone,
        tool_angle_deg: Some(60.0),
        tool_azimuth_deg: -15.0,
        displacement: [-0.25, 0.12, 0.05],
        diameter_ratio: 0.6,
        ..sample()
    };
    assert_eq!(
        cone.second_line_fields_ascii(),
        [
            "CON",
            "-15.00 deg",
            "X = -0.250, Y = 0.120, Z = 0.050",
            "D/W = 0.600, angle = 60.00 deg, reciprocating"
        ]
    );
    for tier in [sample(), cone] {
        let unicode = tier
            .second_line_fields()
            .map(|field| field.replace('°', " deg"));
        assert_eq!(tier.second_line_fields_ascii(), unicode);
        assert!(tier.second_line_fields_ascii().iter().all(|f| f.is_ascii()));
    }
}

#[test]
fn concave_tier_validate_rejects_each_hostile_field() {
    const GEAR: i32 = 96;
    assert_eq!(sample().validate(GEAR), Ok(()));
    let bad = |edit: &dyn Fn(&mut ConcaveTier)| {
        let mut tier = sample();
        edit(&mut tier);
        tier.validate(GEAR)
            .expect_err("hostile tier must be rejected")
    };
    assert!(matches!(
        bad(&|t| t.angle_deg = f64::NAN),
        ConcaveTierError::NonFinite {
            field: "angle_deg",
            ..
        }
    ));
    assert!(matches!(
        bad(&|t| t.displacement[1] = f64::INFINITY),
        ConcaveTierError::NonFinite {
            field: "displacement",
            ..
        }
    ));
    assert!(matches!(
        bad(&|t| t.angle_deg = 0.0),
        ConcaveTierError::AngleOutOfRange { .. }
    ));
    assert!(matches!(
        bad(&|t| t.angle_deg = 90.0),
        ConcaveTierError::AngleOutOfRange { .. }
    ));
    assert_eq!(bad(&|t| t.indices.clear()), ConcaveTierError::NoIndices);
    // 96 is the same ring position as 0 on a 96-tooth gear: valid. 97 and -1 are not.
    for index in [97.0, 96.5, -1.0] {
        assert!(matches!(
            bad(&|t| t.indices = vec![24.0, index]),
            ConcaveTierError::IndexOutOfRange { gear_teeth: 96, .. }
        ));
    }
    let mut closing = sample();
    closing.indices = vec![96.0, 24.0];
    assert_eq!(closing.validate(GEAR), Ok(()));
    assert!(matches!(
        bad(&|t| t.diameter_ratio = 0.0),
        ConcaveTierError::DiameterNotPositive { .. }
    ));
    assert!(matches!(
        bad(&|t| t.diameter_ratio = 1e300),
        ConcaveTierError::DiameterTooLarge { .. }
    ));
    assert!(matches!(
        bad(&|t| t.diameter_ratio = MAX_DIAMETER_RATIO + 0.5),
        ConcaveTierError::DiameterTooLarge { .. }
    ));
    for axis in 0..3 {
        for value in [1e300, -1e300, -(MAX_DISPLACEMENT_RATIO + 0.5)] {
            assert!(matches!(
                bad(&|t| t.displacement[axis] = value),
                ConcaveTierError::DisplacementTooLarge { .. }
            ));
        }
    }
    // The bounds themselves are allowed.
    let mut at_limit = sample();
    at_limit.diameter_ratio = MAX_DIAMETER_RATIO;
    at_limit.displacement = [MAX_DISPLACEMENT_RATIO, -MAX_DISPLACEMENT_RATIO, 0.0];
    assert_eq!(at_limit.validate(GEAR), Ok(()));
    assert_eq!(
        bad(&|t| t.tool = ConcaveTool::Cone),
        ConcaveTierError::ToolAngleMissing {
            tool: ConcaveTool::Cone
        }
    );
    assert_eq!(
        bad(&|t| {
            t.tool = ConcaveTool::Sphere;
            t.tool_angle_deg = Some(60.0);
        }),
        ConcaveTierError::ToolAngleUnexpected {
            tool: ConcaveTool::Sphere
        }
    );
    assert!(matches!(
        bad(&|t| {
            t.tool = ConcaveTool::Disc;
            t.tool_angle_deg = Some(180.0);
        }),
        ConcaveTierError::ToolAngleOutOfRange { .. }
    ));

    let mut clashing = design(vec![flat("Pavilion Main", -41.0)]);
    clashing.concave_tiers.push(ConcaveTier {
        name: "Pavilion Main".to_owned(),
        ..sample()
    });
    assert_eq!(
        clashing.validate_concave_tiers(),
        Err((
            0,
            ConcaveTierError::NameClash {
                name: "Pavilion Main".to_owned()
            }
        ))
    );
}

#[test]
fn cutting_order_places_concave_groups_at_the_end_of_each_section_before_the_table() {
    let mut d = design(vec![
        flat("table", 0.0),
        flat("crown", 35.0),
        flat("pav", -41.0),
        flat("girdle", 90.0),
        flat("culet", -0.0),
        flat("crown2", 20.0),
    ]);
    let concave_at = |angle_deg: f64| ConcaveTier {
        angle_deg,
        ..sample()
    };
    d.concave_tiers = vec![
        concave_at(30.0),
        concave_at(-30.0),
        concave_at(10.0),
        concave_at(-10.0),
    ];
    assert_eq!(
        d.cutting_order(),
        vec![F(2), F(3), F(4), C(1), C(3), F(1), F(5), C(0), C(2), F(0)]
    );
    assert_eq!(d.concave_placement_count(), 16);
    assert!(d.tiers[0].is_table());
    assert!(!d.tiers[4].is_table());
}

#[test]
fn design_equality_ignores_concave_tier_ids() {
    let mut a = design(vec![flat("pav", -41.0)]);
    a.concave_tiers.push(sample());
    let mut b = a.clone();
    a.ensure_concave_tier_ids();
    assert_eq!(a.concave_tier_ids.len(), 1);
    assert_eq!(b.concave_tier_ids, [] as [crate::design::TierId; 0]);
    assert_eq!(a, b);
    b.concave_tiers[0].diameter_ratio = 0.6;
    assert_ne!(a, b);
}
