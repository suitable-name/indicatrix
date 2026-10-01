//! Cross-checks the `VertexAccumulator`-indexed dedup against the linear-scan
//! reference in [`super`], on the module's hand-built fixtures and on real
//! cutting instructions.

use glam::DVec3;

use super::{assert_dedup_matches_reference, planes_from_asc_schedule};

#[test]
fn dedup_matches_linear_scan_reference_on_module_fixtures() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    assert_dedup_matches_reference(
        &[
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.6),
            (DVec3::NEG_Y, 0.6),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ],
        "plain box",
    );
    assert_dedup_matches_reference(
        &[
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
            (DVec3::NEG_Y, 0.5),
            (DVec3::new(s, s, 0.0), s),
            (DVec3::new(-s, s, 0.0), s),
            (DVec3::new(0.0, s, s), s),
            (DVec3::new(0.0, s, -s), s),
        ],
        "hip-roofed block",
    );
    assert_dedup_matches_reference(
        &[
            (DVec3::new(s, 0.0, s), 1.0),
            (DVec3::new(-s, 0.0, s), 1.0),
            (DVec3::new(s, 0.0, -s), 1.0),
            (DVec3::new(-s, 0.0, -s), 1.0),
            (DVec3::Y, 0.5),
            (DVec3::NEG_Y, 0.5),
        ],
        "rotated square girdle",
    );
}

#[test]
fn dedup_matches_linear_scan_reference_on_real_schedules() {
    // Same fixture text as `examples/simd_bench.rs`'s solver benchmark
    // ("Bench design" / pgo_train.rs's "Train A"): a 96-tooth round with
    // two crown tiers, table, and two pavilion tiers plus culet.
    assert_dedup_matches_reference(
        &planes_from_asc_schedule(
            "GemCad 5.0\n\
             g 96 0.0\n\
             y 6 y\n\
             I 1.72\n\
             H Bench design\n\
             a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
             a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
             a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
             a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
             a 10.000000 0.48799664 96 n C 16 32 48 64 80\n\
             a 0.000000 0.44000000 n T\n",
        ),
        "real schedule: Train A (96-tooth round)",
    );
    // pgo_train.rs's "Train B": mixed 96/6-tooth tiers, a heavier real
    // schedule with a different symmetry split.
    assert_dedup_matches_reference(
        &planes_from_asc_schedule(
            "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\nH Train B\n\
             a -43.000000 0.70000000 96 n P1 12 24 36 48 60 72 84\n\
             a -41.000000 0.68000000 6 n P2 18 30 42 54 66 78 90\n\
             a -90.000000 1.00000000 96 n G 12 24 36 48 60 72 84\n\
             a -90.000000 1.00000000 6 n G2 18 30 42 54 66 78 90\n\
             a 42.000000 0.72000000 96 n C1 12 24 36 48 60 72 84\n\
             a 27.000000 0.62000000 6 n C2 18 30 42 54 66 78 90\n\
             a 0.000000 0.40000000 n T\n",
        ),
        "real schedule: Train B (mixed 96/6-tooth)",
    );
    // pgo_train.rs's "Train C": a simpler 4-fold real schedule.
    assert_dedup_matches_reference(
        &planes_from_asc_schedule(
            "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.62\nH Train C\n\
             a -45.000000 0.75000000 96 n 1 24 48 72\n\
             a -40.000000 0.70000000 12 n 2 36 60 84\n\
             a -90.000000 1.05000000 96 n G 24 48 72\n\
             a -90.000000 1.05000000 12 n G2 36 60 84\n\
             a 35.000000 0.70000000 96 n 3 24 48 72\n\
             a 20.000000 0.58000000 12 n 4 36 60 84\n\
             a 0.000000 0.42000000 n T\n",
        ),
        "real schedule: Train C (4-fold)",
    );
}
