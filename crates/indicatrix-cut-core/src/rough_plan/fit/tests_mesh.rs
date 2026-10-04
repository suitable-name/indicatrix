//! The exact single-stone fit in a non-convex rough: the C-shaped mesh of
//! [`C_SHAPE_OBJ`] against its convex hull.

use glam::DVec3;

use super::{SingleFit, fit_single_stones, fit_single_stones_with};
use crate::rough_plan::{
    DesignHull, RoughLayout, RoughModel,
    plan::layout_from_single_fit,
    shape::mesh_fixture::{
        C_SHAPE_OBJ, box_hull, import_obj, import_parts, noisy_c_shape, stone_enters_notch,
    },
    shaped::ShapedCtx,
    tests::settings_with,
};

/// The best fit of `hull` in the C-shaped rough, with or without its mesh.
fn best_fit(with_mesh: bool, hull: &DesignHull) -> (SingleFit, RoughLayout) {
    best_fit_in(
        &RoughModel::new(import_obj(C_SHAPE_OBJ), Vec::new()),
        with_mesh,
        hull,
    )
}

/// The best fit of `hull` in `model`, with or without its mesh.
fn best_fit_in(model: &RoughModel, with_mesh: bool, hull: &DesignHull) -> (SingleFit, RoughLayout) {
    let settings = settings_with(1, 0.3, 0.2, 1.0);
    let ctx = ShapedCtx::new(model, &settings).expect("ctx");
    let coarse = model
        .canonical_coarse_usable_halfspaces(ctx.inset_mm)
        .expect("coarse region");
    let mesh = with_mesh.then(|| ctx.fit_mesh()).flatten();
    let fits = fit_single_stones_with(
        &ctx.usable,
        &coarse,
        std::slice::from_ref(hull),
        &settings,
        1,
        mesh,
        &mut |_| true,
    )
    .expect("not cancelled");
    let fit = fits.into_iter().next().expect("the C holds a stone");
    let layout = layout_from_single_fit(&fit, hull, ctx.model_volume);
    (fit, layout)
}

#[test]
fn single_fit_avoids_notch() {
    let hull = box_hull(1, 1.0, 1.0, 1.0);
    let hulls = std::slice::from_ref(&hull);

    // The control: against the hull alone the best cube fills the notch.
    let (open, open_layout) = best_fit(false, &hull);
    assert!(
        stone_enters_notch(&open_layout, &open_layout.stones[0], hulls),
        "the unguarded stone should reach into the notch"
    );

    let (fit, layout) = best_fit(true, &hull);
    assert!(
        !stone_enters_notch(&layout, &layout.stones[0], hulls),
        "the guarded stone reaches into the notch: centre {:?}, scale {}",
        fit.pose.center_mm,
        fit.pose.mm_per_unit
    );
    assert!(fit.volume_mm3 < open.volume_mm3);
    // The left block alone holds a 9.6 mm cube (10 mm less the clearance on both sides).
    assert!(
        fit.volume_mm3 > 0.8 * 9.6_f64.powi(3),
        "only {} mm^3 fit",
        fit.volume_mm3
    );
    // And the stone lies in the cube, clear of the outer faces.
    let centre = DVec3::from(fit.pose.center_mm);
    let reach = 0.5 * fit.pose.mm_per_unit * 3.0_f64.sqrt();
    assert!(centre.min_element() > 0.2 - 1e-6 && centre.max_element() < 19.8 + 1e-6);
    assert!(reach > 0.0);
}

#[test]
fn without_a_mesh_the_with_variant_is_the_plain_fit() {
    let hull = box_hull(1, 1.0, 1.0, 1.0);
    let settings = settings_with(1, 0.3, 0.2, 1.0);
    let model = RoughModel::new(import_obj(C_SHAPE_OBJ), Vec::new());
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    let coarse = model
        .canonical_coarse_usable_halfspaces(ctx.inset_mm)
        .expect("coarse region");
    let hulls = std::slice::from_ref(&hull);
    let plain = fit_single_stones(&ctx.usable, &coarse, hulls, &settings, 1, &mut |_| true);
    let with = fit_single_stones_with(&ctx.usable, &coarse, hulls, &settings, 1, None, &mut |_| {
        true
    });
    assert_eq!(plain, with);
}

#[test]
fn single_fit_beside_a_noisy_scanned_notch() {
    let hull = box_hull(1, 1.0, 1.0, 1.0);
    let hulls = std::slice::from_ref(&hull);
    let (points, tris) = noisy_c_shape(3, 0.8);
    let model = RoughModel::new(import_parts(&points, &tris), Vec::new());
    let (fit, layout) = best_fit_in(&model, true, &hull);
    assert!(
        !stone_enters_notch(&layout, &layout.stones[0], hulls),
        "the stone reaches into the notch: centre {:?}",
        fit.pose.center_mm
    );
    // The left block, less the noise and the clearance, still holds a good cube.
    assert!(
        fit.volume_mm3 > 0.5 * 9.6_f64.powi(3),
        "{} mm^3",
        fit.volume_mm3
    );
}
