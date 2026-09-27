//! Shared test fixtures for [`super`]'s topic modules: a real, small,
//! genuinely meet-derived design ("RBC-445") used across the free-tier,
//! candidate, search and optimize-outcome test suites.

use crate::{
    design::{ConstraintTier, Design, ScheduleMeta},
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::{
    Block, MeetConstraint, classify_blocks, meet_tier_inputs_from_asc,
};

/// "RBC-445" (PC 13.156), 12 tiers, real prose `G` instructions ("G TCP"/"G PCP")
/// and named references that partially resolve -- byte-identical to the fixture
/// embedded in `design.rs`'s own `real_fixtures` test module, duplicated here
/// (rather than imported) because that module is a private `#[cfg(test)]` fixture
/// of a different module, not a reusable export. A real, small, genuinely
/// meet-derived design -- see this module's own doc comment's "Cost first" section
/// for why a SMALL meet-derived fixture (not the 103-tier CrackOtto-Step, which is
/// reserved for the cost probe) is the right size for exercising the search itself
/// quickly and repeatedly.
pub(super) const RBC_445: &str = "GemCad 5.0\ng 96 0.0\ny 3 y\nI 1.54\n\
     H PC 13.156  RBC-445\n\
     H Richard B Conley, Facets, Oct 2013 p10\n\
     a -50.400000 0.61624001 92 n 1 68 60 36 28 4 G TCP\n\
     a -48.900000 0.63552822 88 n 2 72 56 40 24 8 G TCP\n\
     a -90.000000 0.91252100 92 n 3 68 60 36 28 4\n\
     a -90.000000 0.96225045 88 n 4 72 56 40 24 8\n\
     a -47.200000 0.59908304 95 n 5 65 63 33 31 1\n\
     a -43.000000 0.64485570 86 n 6 74 54 42 22 10 G PCP\n\
     a -47.266965 0.63949242 87 n 7 73 55 41 23 9\n\
     a 31.000000 0.62509296 4 n A 28 36 60 68 92\n\
     a 29.000000 0.62477630 8 n B 24 40 56 72 88\n\
     a 28.100000 0.60078994 2 n C 30 34 62 66 94\n\
     a 20.940747 0.56272062 14 n D 18 46 50 78 82\n\
     a 0.000000 0.40674031 96 n E\n";

/// Builds a real [`Design`] with GENUINE meet-derived structure from a raw `.asc`
/// text: every tier keeps its file's own real [`MeetConstraint`] (`MeetExisting`/
/// `MeetNamed`), except that each crown/pavilion/girdle [`Block`] present gets
/// EXACTLY ONE bootstrapped [`MeetConstraint::ScaleReference`] (that block's own
/// first tier's real recorded mast, when the file stated no explicit anchor of its
/// own) -- otherwise [`Design::solve`] could never produce a mast for that block at
/// all (see `design.rs`'s own module doc comment on scale anchoring). Same
/// technique as `design.rs`'s own private `design_with_real_meet_structure` test
/// helper, reimplemented here since that one is private to a different module.
pub(super) fn design_with_real_meet_structure(text: &str) -> Design {
    let schedule = indicatrix_formats::asc::parse_asc(text).expect("fixture must parse");
    let mut inputs = meet_tier_inputs_from_asc(&schedule);
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        let anchored = inputs
            .iter()
            .zip(&blocks)
            .any(|(t, &b)| b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_)));
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }
    let tiers = inputs
        .into_iter()
        .zip(&schedule.tiers)
        .map(|(input, original)| ConstraintTier {
            angle_deg: input.angle_deg,
            name: original.name.clone(),
            indices: input.indices,
            constraint: input.constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: schedule.gemcad_version.clone(),
            gear_teeth: schedule.gear_teeth,
            gear_reference_angle: schedule.gear_reference_angle,
            symmetry_order: schedule.symmetry_order,
            mirror: schedule.mirror,
            refractive_index: schedule.refractive_index,
            headers: schedule.headers.clone(),
            footnotes: schedule.footnotes,
        },
        tiers,
    )
}

/// [`design_with_real_meet_structure`] applied to [`RBC_445`] -- the small
/// meet-derived fixture most of this module's tests exercise.
pub(super) fn rbc_445() -> Design {
    design_with_real_meet_structure(RBC_445)
}
