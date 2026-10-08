//! Formatting rough plan metrics for Slint display.
//!
//! Layout summaries (weight, yield, saw work, cut order), the live model readout and
//! weight check live here; the per-design metric list of a result card is in [`group`].
//! Everything returns plain strings, so it can be built on any thread.

mod group;

pub use group::{DesignFacts, ModelGeometry, SavedShape, group_metrics};

use indicatrix_cut_core::{
    rough_plan::{Axis, RoughLayout},
    yield_metrics::carat_weight,
};

/// Eight soft-contrast palette colors for distinguishing design groups in 3D and in result cards.
pub const PALETTE: [[u8; 3]; 8] = [
    [96, 165, 250],  // Blue
    [74, 222, 128],  // Green
    [251, 191, 36],  // Amber
    [244, 114, 182], // Pink
    [167, 139, 250], // Purple
    [45, 212, 191],  // Teal
    [251, 146, 60],  // Orange
    [148, 163, 184], // Slate
];

/// Returns the Slint palette color for design group at index `i`.
#[must_use]
pub const fn palette_swatch(i: usize) -> slint::Color {
    let [r, g, b] = PALETTE[i % PALETTE.len()];
    slint::Color::from_argb_u8(255, r, g, b)
}

/// Decodes raw PNG image bytes into a pixel buffer. Unlike a [`slint::Image`] the buffer
/// can cross to another thread, so worker threads decode and the UI thread wraps it with
/// `slint::Image::from_rgba8`.
#[must_use]
pub fn decode_preview_pixels(bytes: &[u8]) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
    let slice = buffer.make_mut_slice();
    let (pixels, _) = rgba.as_raw().as_chunks::<4>();
    for (src, dst) in pixels.iter().zip(slice.iter_mut()) {
        dst.r = src[0];
        dst.g = src[1];
        dst.b = src[2];
        dst.a = src[3];
    }
    Some(buffer)
}

/// Formats total weight in carats ("3.71 ct").
#[must_use]
pub fn format_total_carat(carat: f64) -> String {
    format!("{carat:.2} ct")
}

/// Formats yield of weighed rough ("38.9 % of 9.54 ct weighed"), or empty when no weight was given.
#[must_use]
pub fn format_weighed_yield(total_carat: f64, weighed_ct: Option<f64>) -> String {
    match weighed_ct {
        Some(w) if w > 0.0 => {
            let pct = (total_carat / w) * 100.0;
            format!("{pct:.1} % of {w:.2} ct weighed")
        }
        _ => String::new(),
    }
}

/// Formats the cut order ("slabs across Y, bars across X, pieces across Z").
#[must_use]
pub fn format_cut_order(layout: &RoughLayout) -> String {
    let [a, b, c] = layout.cut_order.axes().map(Axis::from_index);
    format!("slabs across {a}, bars across {b}, pieces across {c}")
}

/// Estimates saw work passes and maximum kerf loss ("5 cuts · kerf loss ≤ 0.21 ct").
///
/// Every pass is costed with the cross-section of the part it saws, taken from the
/// rough's bounding box, so a shaped rough is overstated (hence "≤"). An exact fit has no
/// saw plan, so its text is empty (the result card hides the row).
#[must_use]
pub fn format_saw_work(
    layout: &RoughLayout,
    rough_extents: [f64; 3],
    kerf_mm: f64,
    specific_gravity: f64,
) -> String {
    if layout.exact_fit {
        return String::new();
    }
    let axes = layout.cut_order.axes();
    let slabs = &layout.cut_plan.slabs;
    let slab_cuts = slabs.len().saturating_sub(1);

    let slab_cut_area = rough_extents[axes[1]] * rough_extents[axes[2]];
    let mut total_cut_area = (slab_cuts as f64) * slab_cut_area;
    let mut total_cuts = slab_cuts;

    for slab in slabs {
        let bar_cuts = slab.bars.len().saturating_sub(1);
        total_cuts += bar_cuts;
        total_cut_area =
            (bar_cuts as f64).mul_add(slab.thickness_mm * rough_extents[axes[2]], total_cut_area);

        for bar in &slab.bars {
            let piece_cuts = bar.pieces_mm.len().saturating_sub(1);
            total_cuts += piece_cuts;
            total_cut_area =
                (piece_cuts as f64).mul_add(slab.thickness_mm * bar.width_mm, total_cut_area);
        }
    }

    let kerf_volume_mm3 = total_cut_area * kerf_mm;
    let kerf_ct = carat_weight(kerf_volume_mm3, specific_gravity);

    format!(
        "{total_cuts} cut{} · kerf loss ≤ {kerf_ct:.2} ct",
        if total_cuts == 1 { "" } else { "s" }
    )
}

/// Live model volume and carat readout ("Model 257 mm³ · 3.40 ct (Quartz, SG 2.65)" for
/// 256.6 mm³ of quartz: 256.6 × 2.65 / 200 = 3.40 ct).
#[must_use]
pub fn format_model_readout(volume_mm3: f64, specific_gravity: f64, material_name: &str) -> String {
    let ct = carat_weight(volume_mm3, specific_gravity);
    let vol_int = volume_mm3.round() as usize;
    let formatted_vol = super::format::group_thousands(vol_int);
    format!("Model {formatted_vol} mm³ · {ct:.2} ct ({material_name}, SG {specific_gravity:.2})")
}

/// Live weight check feedback against weighed rough.
///
/// Returns `(text, level)` where level is:
/// - 0: none (no weight entered, or no material picked so the model has no weight yet)
/// - 1: matches (within ±5 %, inclusive)
/// - 2: close (within ±15 %, inclusive)
/// - 3: check the model (beyond that)
///
/// The bands are applied to the percentage as shown, rounded to one decimal, so a model
/// that is exactly 5 % off in decimal terms ("0.735 ct against 0.7 ct") matches whatever
/// the last bit of the binary division says.
#[must_use]
pub fn format_weight_check(
    volume_mm3: f64,
    specific_gravity: f64,
    weighed_ct: Option<f64>,
) -> (String, i32) {
    let Some(weighed) = weighed_ct.filter(|&w| w > 0.0) else {
        return (String::new(), 0);
    };
    if !specific_gravity.is_finite() || specific_gravity <= 0.0 {
        return (String::new(), 0);
    }

    let model_ct = carat_weight(volume_mm3, specific_gravity);
    let diff_pct = ((model_ct - weighed) / weighed) * 100.0;
    let shown_pct = (diff_pct * 10.0).round() / 10.0;
    let abs_diff_pct = shown_pct.abs();
    let sign = if shown_pct < 0.0 { "-" } else { "+" };

    if abs_diff_pct <= 5.0 {
        (
            format!("matches weighed {weighed:.2} ct ({sign}{abs_diff_pct:.1} %)"),
            1,
        )
    } else if abs_diff_pct <= 15.0 {
        (format!("close to weighed ({sign}{abs_diff_pct:.1} %)"), 2)
    } else if shown_pct > 0.0 {
        (
            format!("check the model: {abs_diff_pct:.0} % heavier than weighed"),
            3,
        )
    } else {
        (
            format!("check the model: {abs_diff_pct:.0} % lighter than weighed"),
            3,
        )
    }
}

/// Layouts built by the real planner entry points, shared by the tests of this module and
/// of the plan text in `format`.
#[cfg(test)]
pub(in crate::gui::rough_plan) mod fixtures {
    use indicatrix_cut_core::rough_plan::{
        CandidateDesign, DesignHull, PlanSettings, RoughBlock, RoughLayout, SingleFit, StonePose,
        layout_from_single_fit, plan_rough,
    };

    /// The exact fit of a 2 mm cube (design 7, corners at +-1 model unit, 1 mm per unit)
    /// whose frame is turned about Z with the table normal `(0.6, 0.8, 0)`, centred at
    /// `(5, 4, 3)` mm in a 100 mm^3 rough.
    ///
    /// By hand: the table normal is 0.8 along +Y, so the table is tilted
    /// `acos(0.8) = 36.87` degrees (37 rounded) from the Top face; the cube's corners
    /// reach `0.8 + 0.6 = 1.4` mm from the centre along X and Y and 1 mm along Z, so the
    /// stone box is 2.8 x 2.8 x 2.0 mm.
    pub(in crate::gui::rough_plan) fn exact_fit_layout() -> RoughLayout {
        let corner = |bit: usize, at: usize| if (bit >> at) & 1 == 0 { -1.0 } else { 1.0 };
        let hull = DesignHull {
            entry_id: 7,
            vertices: (0..8)
                .map(|bit| [corner(bit, 0), corner(bit, 1), corner(bit, 2)])
                .collect(),
            volume: 8.0,
            width: 2.0,
        };
        let fit = SingleFit {
            entry_id: 7,
            pose: StonePose {
                center_mm: [5.0, 4.0, 3.0],
                axes: [[0.8, -0.6, 0.0], [0.6, 0.8, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            },
            volume_mm3: 8.0,
            carat: 0.106,
        };
        layout_from_single_fit(&fit, &hull, 100.0)
    }

    /// The four-stone layout the planner returns for equal cubes in a 10 x 10 x 5 mm
    /// block with no kerf, allowance or skin.
    ///
    /// By hand: a cube stone is limited by the 5 mm height, so four cubes of 5 mm fill
    /// the block exactly (500 mm^3, yield 1), which no layout of fewer stones reaches and
    /// the cap of four stones forbids more. How the two 5 mm splits are assigned to
    /// slab, bar and piece depends on the cut order the planner prefers, so callers rely
    /// only on the stone count.
    pub(in crate::gui::rough_plan) fn planned_four_stone_layout() -> RoughLayout {
        let rough = RoughBlock {
            x_mm: 10.0,
            y_mm: 10.0,
            z_mm: 5.0,
        };
        let settings = PlanSettings {
            count: 4,
            min_count: 1,
            kerf_mm: 0.0,
            allowance_mm: 0.0,
            skin_mm: 0.0,
            min_width_mm: 0.01,
            specific_gravity: 2.65,
        };
        let cube = CandidateDesign {
            entry_id: 1,
            width: 1.0,
            length: 1.0,
            height: 1.0,
            volume: 1.0,
        };
        plan_rough(&rough, &settings, &[cube], &mut |_| true)
            .expect("the plan is not cancelled")
            .into_iter()
            .find(|layout| layout.stone_count() == 4)
            .expect("four cubes fill the block, so a four-stone layout is ranked")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::{BarCut, CutOrder, CutPlan, SlabCut};

    /// The weight check for a model of `model_ct` against a 10 ct rough (SG 2, so the
    /// volume is `model_ct * 100` mm³).
    fn check(model_ct: f64) -> (String, i32) {
        format_weight_check(model_ct * 100.0, 2.0, Some(10.0))
    }

    #[test]
    fn no_weighed_carat_means_no_check() {
        assert_eq!(format_weight_check(1000.0, 2.0, None), (String::new(), 0));
        assert_eq!(
            format_weight_check(1000.0, 2.0, Some(0.0)),
            (String::new(), 0)
        );
    }

    #[test]
    fn exactly_five_percent_still_matches_and_just_over_is_close() {
        assert_eq!(
            check(10.5),
            ("matches weighed 10.00 ct (+5.0 %)".to_string(), 1)
        );
        assert_eq!(
            check(9.5),
            ("matches weighed 10.00 ct (-5.0 %)".to_string(), 1)
        );
        assert_eq!(
            check(10.0),
            ("matches weighed 10.00 ct (+0.0 %)".to_string(), 1)
        );
        assert_eq!(check(10.51), ("close to weighed (+5.1 %)".to_string(), 2));
        assert_eq!(check(9.49), ("close to weighed (-5.1 %)".to_string(), 2));
    }

    #[test]
    fn exactly_fifteen_percent_is_still_close_and_just_over_needs_a_check() {
        assert_eq!(check(11.5), ("close to weighed (+15.0 %)".to_string(), 2));
        assert_eq!(check(8.5), ("close to weighed (-15.0 %)".to_string(), 2));
        assert_eq!(
            check(11.51),
            ("check the model: 15 % heavier than weighed".to_string(), 3)
        );
        assert_eq!(
            check(13.1),
            ("check the model: 31 % heavier than weighed".to_string(), 3)
        );
        assert_eq!(
            check(6.0),
            ("check the model: 40 % lighter than weighed".to_string(), 3)
        );
    }

    #[test]
    fn the_readout_names_volume_weight_material_and_gravity() {
        // 1284 * 2.65 / 200 = 17.013.
        assert_eq!(
            format_model_readout(1284.0, 2.65, "Quartz"),
            "Model 1,284 mm³ · 17.01 ct (Quartz, SG 2.65)"
        );
        // The documented example: 256.6 * 2.65 / 200 = 3.39995, shown as 3.40 ct.
        assert_eq!(
            format_model_readout(256.6, 2.65, "Quartz"),
            "Model 257 mm³ · 3.40 ct (Quartz, SG 2.65)"
        );
    }

    #[test]
    fn a_model_without_a_material_has_no_weight_to_check() {
        assert_eq!(
            format_weight_check(1000.0, 0.0, Some(5.0)),
            (String::new(), 0)
        );
        assert_eq!(
            format_weight_check(1000.0, -2.65, Some(5.0)),
            (String::new(), 0)
        );
    }

    /// The weight check of a model of `model_ct` against a rough weighing `weighed_ct`
    /// (SG 2, so the volume is `model_ct * 100` mm³).
    fn check_against(model_ct: f64, weighed_ct: f64) -> (String, i32) {
        format_weight_check(model_ct * 100.0, 2.0, Some(weighed_ct))
    }

    #[test]
    fn a_decimal_five_percent_matches_whatever_the_binary_division_says() {
        // model = weighed * 1.05 (and * 0.95), written out in decimals.
        for (weighed, over, under) in [
            (0.7, 0.735, 0.665),
            (3.0, 3.15, 2.85),
            (9.54, 10.017, 9.063),
        ] {
            let (text, level) = check_against(over, weighed);
            assert_eq!((level, text.ends_with("(+5.0 %)")), (1, true), "{text}");
            let (text, level) = check_against(under, weighed);
            assert_eq!((level, text.ends_with("(-5.0 %)")), (1, true), "{text}");
        }
        // 0.7 * 1.0514 is 5.14 % over: shown as 5.1 %, so it is close, not a match.
        assert_eq!(
            check_against(0.7 * 1.0514, 0.7),
            ("close to weighed (+5.1 %)".to_string(), 2)
        );
    }

    #[test]
    fn a_decimal_fifteen_percent_is_still_close() {
        // 0.7 * 1.15 = 0.805 and 0.7 * 0.85 = 0.595.
        assert_eq!(
            check_against(0.805, 0.7),
            ("close to weighed (+15.0 %)".to_string(), 2)
        );
        assert_eq!(
            check_against(0.595, 0.7),
            ("close to weighed (-15.0 %)".to_string(), 2)
        );
    }

    #[test]
    fn an_exact_fit_has_no_saw_work() {
        let fit = fixtures::exact_fit_layout();
        assert!(fit.exact_fit);
        assert_eq!(format_saw_work(&fit, [10.0, 8.0, 6.0], 0.3, 2.65), "");
    }

    #[test]
    fn a_planned_layout_needs_one_cut_fewer_than_it_has_stones() {
        // A three-stage guillotine plan with P pieces costs (slabs - 1) + (bars - slabs)
        // + (P - bars) = P - 1 cuts, so four stones need 3 cuts; with no kerf nothing is
        // lost.
        let layout = fixtures::planned_four_stone_layout();
        assert!(!layout.exact_fit);
        assert_eq!(
            format_saw_work(&layout, [10.0, 10.0, 5.0], 0.0, 2.65),
            "3 cuts · kerf loss ≤ 0.00 ct"
        );
    }

    #[test]
    fn carat_and_weighed_yield_texts() {
        assert_eq!(format_total_carat(3.714), "3.71 ct");
        assert_eq!(
            format_weighed_yield(3.71, Some(9.54)),
            "38.9 % of 9.54 ct weighed"
        );
        assert_eq!(format_weighed_yield(3.71, None), "");
        assert_eq!(format_weighed_yield(3.71, Some(-1.0)), "");
    }

    fn layout(order: CutOrder, slabs: Vec<SlabCut>) -> RoughLayout {
        RoughLayout {
            cut_order: order,
            stones: Vec::new(),
            cut_plan: CutPlan { slabs },
            total_carat: 0.0,
            total_volume_mm3: 0.0,
            yield_fraction: 0.0,
            exact_fit: false,
        }
    }

    fn slab(thickness_mm: f64, bars: &[(f64, &[f64])]) -> SlabCut {
        SlabCut {
            thickness_mm,
            bars: bars
                .iter()
                .map(|&(width_mm, pieces)| BarCut {
                    width_mm,
                    pieces_mm: pieces.to_vec(),
                })
                .collect(),
        }
    }

    #[test]
    fn saw_work_counts_every_pass_with_the_cross_section_it_saws() {
        // Two slabs; the first has two bars (the first of two pieces), the second one bar
        // with one piece: 1 slab cut + 1 bar cut + 1 piece cut.
        let plan = vec![
            slab(4.0, &[(3.0, &[2.0, 3.0]), (4.0, &[5.0])]),
            slab(5.0, &[(8.0, &[6.0])]),
        ];
        let rough = [10.0, 8.0, 6.0];
        // Xyz: the slab cut saws y * z = 48, the bar cut 4 (thickness) * 6 (z) = 24, the
        // piece cut 4 (thickness) * 3 (bar width) = 12: 84 mm² of kerf at 0.5 mm and
        // SG 2 is 84 * 0.5 * 2 / 200 = 0.42 ct.
        let xyz = layout(CutOrder::Xyz, plan.clone());
        assert_eq!(
            format_saw_work(&xyz, rough, 0.5, 2.0),
            "3 cuts · kerf loss ≤ 0.42 ct"
        );
        // Yxz: slabs across y, so the slab cut saws x * z = 60 and the bar cut 4 * 6.
        let yxz = layout(CutOrder::Yxz, plan);
        assert_eq!(
            format_saw_work(&yxz, rough, 0.5, 2.0),
            "3 cuts · kerf loss ≤ 0.48 ct"
        );
    }

    #[test]
    fn a_single_piece_needs_no_cut_and_one_cut_is_singular() {
        let uncut = layout(CutOrder::Xyz, vec![slab(5.0, &[(4.0, &[3.0])])]);
        assert_eq!(
            format_saw_work(&uncut, [5.0, 4.0, 3.0], 0.3, 2.65),
            "0 cuts · kerf loss ≤ 0.00 ct"
        );
        let one = layout(CutOrder::Xyz, vec![slab(5.0, &[(4.0, &[3.0, 3.0])])]);
        assert!(format_saw_work(&one, [5.0, 4.0, 6.0], 0.3, 2.65).starts_with("1 cut ·"));
    }

    #[test]
    fn the_cut_order_names_the_axes_of_the_three_stages() {
        let layout = layout(CutOrder::Yxz, Vec::new());
        assert_eq!(
            format_cut_order(&layout),
            "slabs across Y, bars across X, pieces across Z"
        );
    }

    #[test]
    fn the_palette_repeats_after_eight_groups() {
        assert_eq!(palette_swatch(0), palette_swatch(8));
        assert_ne!(palette_swatch(0), palette_swatch(1));
    }
}
