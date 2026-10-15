//! A culet (a sign-negative zero angle -- see
//! `indicatrix_formats::asc::AscTier::angle_deg`) must stay a culet through `.asc`
//! import, `.asc` export, the design file save/load and the catalogue `.asc` a save
//! stores; if the sign were lost anywhere, the shared side rule would put it on the
//! crown as a second table.

use crate::{
    design::Design,
    native::{DesignExtras, catalogue_asc, design_from_str, design_to_string},
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// Girdle, table, then the culet in `GemCAD`'s documented form (zero angle,
/// NEGATIVE distance) with no index.
const CULET_ASC: &str = "GemCad 5.0\n\
     g 4 0.0\n\
     y 1 n\n\
     I 1.62\n\
     a 90.000000 1.00000000 0 1 2 3 G Set girdle thickness\n\
     a 0.000000 0.60000000 G Set stone size\n\
     a 0.000000 -0.55000000 G Set stone size\n";

const CULET_TIER: usize = 2;

fn culet_design() -> Design {
    let schedule = indicatrix_formats::asc::parse_asc(CULET_ASC).expect("fixture must parse");
    Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule)
}

#[test]
fn asc_import_keeps_the_culet_sign_and_a_positive_mast() {
    let design = culet_design();
    let culet = &design.tiers[CULET_TIER];
    assert_eq!(culet.angle_deg, 0.0);
    assert!(culet.angle_deg.is_sign_negative());
    assert_eq!(culet.constraint, MeetConstraint::ScaleReference(0.55));
    let table = &design.tiers[1];
    assert!(!table.angle_deg.is_sign_negative());
}

#[test]
fn asc_export_writes_the_culet_in_gemcads_form() {
    let design = culet_design();
    let schedule = design.to_asc_schedule().expect("every tier is pinned");
    let text = indicatrix_formats::asc::to_asc_string(&schedule).expect("must write");
    assert!(
        text.contains("a -0 -0.55 4 G Set stone size\r\n"),
        "exported text:\n{text}"
    );
    let reparsed = indicatrix_formats::asc::parse_asc(&text).expect("must reparse");
    assert!(reparsed.tiers[CULET_TIER].is_culet());
}

#[test]
fn the_culet_sign_survives_a_design_file_save_and_load() {
    let design = culet_design();
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("must serialize");
    let loaded = design_from_str(&text).expect("must load");
    let culet = &loaded.design.tiers[CULET_TIER];
    assert_eq!(culet.angle_deg, 0.0);
    assert!(
        culet.angle_deg.is_sign_negative(),
        "TOML lost the sign of the culet's -0.0 angle:\n{text}"
    );
    assert!(!loaded.design.tiers[1].angle_deg.is_sign_negative());
}

#[test]
fn the_culet_sign_survives_the_catalogue_asc_a_save_stores() {
    let design = culet_design();
    let saved = catalogue_asc(&design, None, None).expect("must write");
    let reparsed = indicatrix_formats::asc::parse_asc(&saved.asc_text).expect("must reparse");
    assert!(reparsed.tiers[CULET_TIER].is_culet());
}
