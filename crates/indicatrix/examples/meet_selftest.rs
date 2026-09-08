//! TEMPORARY runner mirroring `meet_solver`'s unit tests, so their scenarios can be
//! exercised without invoking the workspace test suite. Prints PASS/FAIL per check.
//!
//! Run: `cargo run --profile probe -p indicatrix --example meet_selftest`

use indicatrix::geometry::meet_solver::{
    MeetConstraint, MeetTierInput, SolveStrategy, meet_tier_inputs_from_asc, solve_meet_points,
};

fn check(name: &str, ok: bool, detail: &str) -> bool {
    println!(
        "{}: {name}{}",
        if ok { "PASS" } else { "FAIL" },
        if ok {
            String::new()
        } else {
            format!(" -- {detail}")
        }
    );
    ok
}

#[allow(clippy::too_many_lines)]
fn main() {
    let mut all_ok = true;

    // 1. Rank-1 vertex level against a capped box.
    {
        let tiers = vec![
            MeetTierInput {
                angle_deg: 90.0,
                indices: vec![0.0, 1.0, 2.0, 3.0],
                constraint: MeetConstraint::ScaleReference(1.0),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: 0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.6),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: -0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.6),
                names: vec![],
            },
            MeetTierInput {
                angle_deg: 45.0,
                indices: vec![0.5],
                constraint: MeetConstraint::MeetExisting,
                names: vec![],
            },
        ];
        let solved = solve_meet_points(4, &tiers);
        let expected = std::f64::consts::FRAC_1_SQRT_2.mul_add(-0.6, 1.0);
        all_ok &= check(
            "rank-1 level against capped box",
            solved[3].strategy == SolveStrategy::DependencyOrder
                && (solved[3].mast - expected).abs() < 1e-6,
            &format!(
                "expected {expected}, got {} ({:?}: {})",
                solved[3].mast, solved[3].strategy, solved[3].detail
            ),
        );
    }

    // 2. Stated named meet overrides the rank-1 prior.
    {
        use glam::{DMat3, DVec3};
        let tiers = vec![
            MeetTierInput {
                angle_deg: 90.0,
                indices: vec![0.0, 1.0, 2.0, 3.0],
                constraint: MeetConstraint::ScaleReference(1.0),
                names: vec!["girdle".to_string()],
            },
            MeetTierInput {
                angle_deg: 0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.6),
                names: vec!["table".to_string()],
            },
            MeetTierInput {
                angle_deg: -0.0,
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.6),
                names: vec!["culet".to_string()],
            },
            MeetTierInput {
                angle_deg: 45.0,
                indices: vec![0.5],
                constraint: MeetConstraint::MeetNamed(vec![
                    "Y".to_string(),
                    "girdle".to_string(),
                    "table".to_string(),
                ]),
                names: vec!["X".to_string()],
            },
            MeetTierInput {
                angle_deg: 60.0,
                indices: vec![0.5],
                constraint: MeetConstraint::MeetExisting,
                names: vec!["Y".to_string()],
            },
        ];
        let solved = solve_meet_points(4, &tiers);
        let x = &solved[3];
        let y = &solved[4];
        // Vertex of {Y, girdle at azimuth 0, table}:
        let phi = std::f64::consts::FRAC_PI_4;
        let (tx, ty) = (45.0_f64.to_radians(), 60.0_f64.to_radians());
        let n_x = DVec3::new(tx.sin() * phi.cos(), tx.cos(), tx.sin() * phi.sin());
        let n_y = DVec3::new(ty.sin() * phi.cos(), ty.cos(), ty.sin() * phi.sin());
        let m = DMat3::from_cols(n_y, DVec3::X, DVec3::Y).transpose();
        let v = m.inverse() * DVec3::new(y.mast, 1.0, 0.6);
        all_ok &= check(
            "named meet overrides rank-1 prior",
            x.strategy == SolveStrategy::DependencyOrder
                && x.detail.contains("named reference")
                && (n_x.dot(v) - x.mast).abs() < 1e-6,
            &format!(
                "X mast {} vs n_x.v {} ({:?}: {})",
                x.mast,
                n_x.dot(v),
                x.strategy,
                x.detail
            ),
        );
    }

    // 3. Determinism on a real design (Round Trichecker-12).
    {
        let schedule = indicatrix_formats::asc::parse_asc(
            "GemCad 5.0\n\
             g 96 0.0\n\
             y 6 y\n\
             I 1.72\n\
             H PC 45.149  Round Trichecker-12\n\
             a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
             a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
             a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
             a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
             a 10.000000 0.48799664 96 n C 16 32 48 64 80\n",
        )
        .expect("must parse");
        let mut tiers = meet_tier_inputs_from_asc(&schedule);
        tiers[0].constraint = MeetConstraint::ScaleReference(schedule.tiers[0].mast);
        tiers[1].constraint = MeetConstraint::ScaleReference(schedule.tiers[1].mast);
        tiers[2].constraint = MeetConstraint::ScaleReference(schedule.tiers[2].mast);

        let a = solve_meet_points(schedule.gear_teeth_abs(), &tiers);
        let b = solve_meet_points(schedule.gear_teeth_abs(), &tiers);
        let identical = a.len() == b.len()
            && a.iter().zip(&b).all(|(x, y)| {
                x.mast.to_bits() == y.mast.to_bits()
                    && x.strategy == y.strategy
                    && x.detail == y.detail
            });
        all_ok &= check(
            "bitwise-identical repeat solve",
            identical,
            "results differ",
        );

        // And the solved tiers land near this design's true masts (B and C are
        // meet-derived here; loose bound, this is a smoke check not a corpus gate).
        let errs: Vec<f64> = (3..5)
            .map(|i| (a[i].mast - schedule.tiers[i].mast.abs()).abs() / schedule.tiers[i].mast)
            .collect();
        all_ok &= check(
            "Trichecker meet-derived tiers plausible (<20%)",
            errs.iter().all(|e| *e < 0.20),
            &format!("errors {errs:?}"),
        );
    }

    if all_ok {
        println!("\nALL CHECKS PASSED");
    } else {
        println!("\nSOME CHECKS FAILED");
        std::process::exit(1);
    }
}
