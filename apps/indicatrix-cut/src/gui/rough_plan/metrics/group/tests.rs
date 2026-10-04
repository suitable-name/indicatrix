use super::*;
use crate::gui::rough_plan::{
    format::{DesignData, RowContext, group_rows},
    saved::dto::DesignShape,
};
use indicatrix_cut_core::rough_plan::{Axis, CutOrder, CutPlan, PlanSettings, StonePose};
use indicatrix_vault::model::tilt_curves::TILT_CURVE_POINTS_PER_AXIS;
use std::collections::BTreeMap;

const UP: [f64; 3] = [0.0, 1.0, 0.0];

fn extents() -> SolidExtents {
    SolidExtents {
        width_caliper: 1.0,
        length_caliper: 1.5,
        width_axis: 1.0,
        length_axis: 1.5,
        height: 0.7,
        volume: 0.9,
    }
}

/// A stone of `volume` mm³ in the piece `origin..origin + size` (its own box is 0.4 mm
/// smaller, as a sawn stone's is); its carat is a tenth of its volume.
fn stone(
    scale: f64,
    table_normal: [f64; 3],
    volume: f64,
    origin: [f64; 3],
    size: [f64; 3],
) -> PlacedStone {
    PlacedStone {
        entry_id: 1,
        piece_origin_mm: origin,
        piece_size_mm: size,
        stone_size_mm: size.map(|s| s - 0.4),
        table_axis: Axis::Y,
        carat: volume / 10.0,
        volume_mm3: volume,
        pose: StonePose {
            center_mm: [0.0; 3],
            axes: [[1.0, 0.0, 0.0], table_normal, [0.0, 0.0, 1.0]],
            mm_per_unit: scale,
        },
    }
}

fn layout(stones: Vec<PlacedStone>) -> RoughLayout {
    let total: f64 = stones.iter().map(|s| s.volume_mm3).sum();
    RoughLayout {
        cut_order: CutOrder::Xyz,
        stones,
        cut_plan: CutPlan { slabs: Vec::new() },
        total_carat: total / 10.0,
        total_volume_mm3: total,
        yield_fraction: 0.5,
        exact_fit: false,
    }
}

fn refs(layout: &RoughLayout) -> Vec<&PlacedStone> {
    layout.stones.iter().collect()
}

/// A 10 mm cube from the origin, as planes.
fn cube_planes() -> Vec<(DVec3, f64)> {
    vec![
        (DVec3::X, 10.0),
        (DVec3::NEG_X, 0.0),
        (DVec3::Y, 10.0),
        (DVec3::NEG_Y, 0.0),
        (DVec3::Z, 10.0),
        (DVec3::NEG_Z, 0.0),
    ]
}

fn model(volume_mm3: f64, planes: Vec<(DVec3, f64)>) -> ModelGeometry {
    ModelGeometry {
        volume_mm3,
        extents_mm: [10.0; 3],
        planes,
        mesh: None,
    }
}

fn cube() -> ModelGeometry {
    model(1000.0, cube_planes())
}

/// The cube with its (10, 10, 10) corner cut off by `x + y + z <= 27` (a tetrahedron of
/// 4.5 mm³ is gone).
fn cube_without_a_corner() -> ModelGeometry {
    let mut planes = cube_planes();
    let normal = DVec3::ONE.normalize();
    planes.push((normal, 27.0 / 3.0_f64.sqrt()));
    model(995.5, planes)
}

fn metric<'a>(metrics: &'a [(String, String)], label: &str) -> Option<&'a str> {
    metrics
        .iter()
        .find(|(name, _)| name == label)
        .map(|(_, value)| value.as_str())
}

fn fill(layout: &RoughLayout, model: &ModelGeometry) -> Option<String> {
    fill_value(&refs(layout), layout, model)
}

#[test]
fn a_piece_inside_the_rough_is_measured_by_its_box() {
    let layout = layout(vec![stone(1.0, UP, 32.0, [1.0; 3], [4.0; 3])]);
    assert_eq!(
        fill(&layout, &cube()).as_deref(),
        Some("stone uses 50 % of its piece")
    );
}

#[test]
fn a_piece_crossing_a_cut_is_measured_by_the_part_inside_the_rough() {
    // The piece 6..10 in every axis loses the 4.5 mm³ corner: 59.5 mm³ are left, so
    // a 32 mm³ stone fills 54 %, not the 50 % its full box would give.
    let layout = layout(vec![stone(1.0, UP, 32.0, [6.0; 3], [4.0; 3])]);
    assert_eq!(
        fill(&layout, &cube_without_a_corner()).as_deref(),
        Some("stone uses 54 % of its piece")
    );
    let volume = piece_model_volume(&cube_without_a_corner(), [6.0; 3], [4.0; 3]);
    assert!(
        volume.is_some_and(|v| (v - 59.5).abs() < 1e-9),
        "clipped piece volume {volume:?}"
    );
    // A piece the cut plane only touches is still its full box.
    let clear = piece_model_volume(&cube_without_a_corner(), [0.0; 3], [4.0; 3]);
    assert_eq!(clear, Some(64.0));
}

#[test]
fn a_piece_over_a_notch_is_measured_by_the_mesh() {
    use crate::gui::rough_plan::obj_import::{C_SHAPE_OBJ, parse_obj_mesh};
    use indicatrix_cut_core::rough_plan::{RoughModel, import_mesh};
    let (points, triangles) = parse_obj_mesh(C_SHAPE_OBJ).expect("parses");
    let (base, _) = import_mesh(&points, &triangles).expect("imports");
    let geometry = ModelGeometry::of(&RoughModel::new(base, Vec::new())).expect("valid");
    assert!(geometry.mesh.is_some());
    // The piece x 8..12 over all of y and z loses the notch (x > 10, 5 < y < 15).
    let over = piece_model_volume(&geometry, [8.0, 0.0, 0.0], [4.0, 20.0, 20.0]);
    assert!(over.is_some_and(|v| (v - 1200.0).abs() < 1e-3), "{over:?}");
    let solid = piece_model_volume(&geometry, [0.0; 3], [10.0, 20.0, 20.0]);
    assert!(
        solid.is_some_and(|v| (v - 4000.0).abs() < 1e-3),
        "{solid:?}"
    );
    // A piece wholly inside the notch holds no material.
    assert_eq!(
        piece_model_volume(&geometry, [12.0, 6.0, 1.0], [4.0, 4.0, 4.0]),
        None
    );
}

#[test]
fn a_mesh_piece_volume_is_measured_once_per_box() {
    use crate::gui::rough_plan::obj_import::{C_SHAPE_OBJ, parse_obj_mesh};
    use indicatrix_cut_core::rough_plan::{RoughModel, import_mesh};
    let (points, triangles) = parse_obj_mesh(C_SHAPE_OBJ).expect("parses");
    let (base, _) = import_mesh(&points, &triangles).expect("imports");
    let geometry = ModelGeometry::of(&RoughModel::new(base, Vec::new())).expect("valid");
    let cache = || {
        geometry
            .mesh
            .as_ref()
            .expect("a mesh")
            .volumes
            .lock()
            .unwrap()
            .len()
    };
    assert_eq!(cache(), 0);
    let first = piece_model_volume(&geometry, [8.0, 0.0, 0.0], [4.0, 20.0, 20.0]);
    assert_eq!(cache(), 1);
    // The same box again, and a box with no material, are answered from the cache, bit
    // for bit; a different box is measured on its own.
    let again = piece_model_volume(&geometry, [8.0, 0.0, 0.0], [4.0, 20.0, 20.0]);
    assert_eq!(first.map(f64::to_bits), again.map(f64::to_bits));
    assert_eq!(cache(), 1);
    assert_eq!(
        piece_model_volume(&geometry, [12.0, 6.0, 1.0], [4.0, 4.0, 4.0]),
        None
    );
    assert_eq!(
        piece_model_volume(&geometry, [12.0, 6.0, 1.0], [4.0, 4.0, 4.0]),
        None
    );
    assert_eq!(cache(), 2);
}

#[test]
fn a_piece_outside_the_rough_has_no_fill_figure() {
    let mut planes = cube_planes();
    planes.push((DVec3::X, 5.0));
    let halved = model(500.0, planes);
    let layout = layout(vec![stone(1.0, UP, 10.0, [6.0, 0.0, 0.0], [4.0; 3])]);
    assert_eq!(fill(&layout, &halved), None);
    assert_eq!(piece_model_volume(&halved, [1.0; 3], [0.0, 1.0, 1.0]), None);
}

#[test]
fn a_single_fit_is_measured_against_the_whole_model() {
    let mut layout = layout(vec![stone(1.0, UP, 100.0, [1.0; 3], [4.0; 3])]);
    layout.exact_fit = true;
    assert_eq!(
        fill(&layout, &model(400.0, cube_planes())).as_deref(),
        Some("stone uses 25 % of the model")
    );
}

#[test]
fn only_a_layout_marked_as_an_exact_fit_is_measured_against_the_model() {
    // A lone sawn stone whose box happens to equal its piece is still measured against the
    // piece: what decides is the layout's marker, not how the sizes come out.
    let mut snug = stone(1.0, UP, 32.0, [1.0; 3], [4.0; 3]);
    snug.stone_size_mm = snug.piece_size_mm;
    let sawn = layout(vec![snug]);
    assert!(!sawn.exact_fit);
    assert_eq!(
        fill(&sawn, &model(400.0, cube_planes())).as_deref(),
        Some("stone uses 50 % of its piece")
    );
}

#[test]
fn a_group_of_unlike_fills_shows_the_range() {
    let layout = layout(vec![
        stone(1.0, UP, 40.0, [0.0; 3], [5.0, 5.0, 4.0]),
        stone(1.0, UP, 60.0, [5.0, 0.0, 0.0], [5.0, 5.0, 4.0]),
    ]);
    assert_eq!(
        fill(&layout, &cube()).as_deref(),
        Some("stones use 40-60 % of their pieces")
    );
    let first = &refs(&layout)[..1];
    let one = group_metrics(first, &layout, Some(&cube()), &DesignFacts::default());
    assert_eq!(metric(&one, "Fill"), Some("stone uses 40 % of its piece"));
}

#[test]
fn the_size_is_in_the_stones_own_terms_and_a_group_shows_the_range() {
    let one = layout(vec![stone(5.0, UP, 10.0, [0.0; 3], [5.0; 3])]);
    let size = size_value(&refs(&one), Some(&extents()), None);
    assert_eq!(size.as_deref(), Some("L x W x D = 7.50 x 5.00 x 3.50 mm"));

    let two = layout(vec![
        stone(4.0, UP, 10.0, [0.0; 3], [5.0; 3]),
        stone(5.0, UP, 10.0, [0.0; 3], [5.0; 3]),
    ]);
    assert_eq!(
        size_value(&refs(&two), Some(&extents()), None).as_deref(),
        Some("L x W x D = 6.00-7.50 x 4.00-5.00 x 2.80-3.50 mm")
    );
    assert_eq!(size_value(&refs(&two), None, None), None, "no size known");
}

/// The design of [`extents`] as a plan recorded it: width 1.0 and ratios L/W 1.5,
/// H/W 0.7 (so length 1.5 and height 0.7 model units).
fn saved_shape() -> SavedShape {
    SavedShape {
        width_caliper: 1.0,
        fingerprint: [1.5, 0.7, 0.9],
    }
}

#[test]
fn a_design_record_gives_its_size_only_when_it_stored_a_width() {
    let mut record = SavedDesignDto {
        entry_id: 1,
        title: "Barion Oval".to_string(),
        fingerprint: [1.5, 0.7, 0.9],
        width_caliper: Some(1.0),
        extents_version: 1,
    };
    assert_eq!(SavedShape::try_from(&record), Ok(saved_shape()));
    record.width_caliper = None;
    assert_eq!(SavedShape::try_from(&record), Err(()));
}

#[test]
fn the_size_follows_the_design_as_it_was_saved_and_says_when_it_was_redrawn() {
    let one = layout(vec![stone(5.0, UP, 10.0, [0.0; 3], [5.0; 3])]);
    // The library's design is now twice as large (width 2, length 3, height 1.4). The stone
    // was fitted at scale 5 to the width-1 design: 5 x 1.5 x 0.7 is what it measures.
    let redrawn = SolidExtents {
        width_caliper: 2.0,
        length_caliper: 3.0,
        width_axis: 2.0,
        length_axis: 3.0,
        height: 1.4,
        volume: 7.2,
    };
    let saved = saved_shape();
    assert_eq!(
        size_value(&refs(&one), Some(&redrawn), Some(&saved)).as_deref(),
        Some("L x W x D = 7.50 x 5.00 x 3.50 mm (design rescaled)")
    );
    // Without the record the line would multiply today's extents by the scale: twice the
    // stone that was planned.
    assert_eq!(
        size_value(&refs(&one), Some(&redrawn), None).as_deref(),
        Some("L x W x D = 15.00 x 10.00 x 7.00 mm")
    );
    // The same design as saved: no flag.
    assert_eq!(
        size_value(&refs(&one), Some(&extents()), Some(&saved)).as_deref(),
        Some("L x W x D = 7.50 x 5.00 x 3.50 mm")
    );
    // A design the library no longer measures still shows the size it was planned at.
    assert_eq!(
        size_value(&refs(&one), None, Some(&saved)).as_deref(),
        Some("L x W x D = 7.50 x 5.00 x 3.50 mm")
    );
    // A record with no known ratios cannot give a size: today's extents are used.
    let unknown = SavedShape {
        fingerprint: [0.0; 3],
        ..saved
    };
    assert_eq!(
        size_value(&refs(&one), Some(&extents()), Some(&unknown)).as_deref(),
        Some("L x W x D = 7.50 x 5.00 x 3.50 mm")
    );
}

/// The "Size" line of the result row for a one-stone layout (entry id 1, scale 5), built by
/// `group_rows` with the library's current `extents` and the given planned `shapes`.
fn row_size_line(shapes: &BTreeMap<i64, DesignShape>, extents: SolidExtents) -> Option<String> {
    let layout = layout(vec![stone(5.0, UP, 10.0, [0.0; 3], [5.0; 3])]);
    let designs = DesignData {
        extents: BTreeMap::from([(1, extents)]),
        ..DesignData::default()
    };
    let (titles, statuses) = (BTreeMap::new(), BTreeMap::new());
    let settings = PlanSettings::default();
    let ctx = RowContext {
        titles: &titles,
        statuses: &statuses,
        shapes,
        settings: &settings,
        weighed_ct: None,
        model: None,
        rough_extents: None,
        designs: &designs,
    };
    let rows = group_rows(&layout, &ctx);
    assert_eq!(rows.len(), 1);
    metric(&rows[0].metrics, "Size").map(str::to_string)
}

/// The result row reads the planned shape of the stone's entry id. With a recorded width 1
/// and ratios 1.5 and 0.7 at stone scale 5 the line is 5 x 1.5 = 7.50, 5 x 1 = 5.00 and
/// 5 x 0.7 = 3.50 mm, whatever the library holds now; a design now twice as large (width 2,
/// length 3, height 1.4) only adds the rescaled flag (see `size_value`).
#[test]
fn the_row_size_comes_from_the_planned_shape_and_flags_a_rescaled_design() {
    let planned = DesignShape {
        fingerprint: [1.5, 0.7, 0.9],
        width_caliper: Some(1.0),
        extents_version: 1,
    };
    let shapes = BTreeMap::from([(1, planned)]);
    let doubled = SolidExtents {
        width_caliper: 2.0,
        length_caliper: 3.0,
        width_axis: 2.0,
        length_axis: 3.0,
        height: 1.4,
        volume: 7.2,
    };
    assert_eq!(
        row_size_line(&shapes, extents()).as_deref(),
        Some("L x W x D = 7.50 x 5.00 x 3.50 mm")
    );
    assert_eq!(
        row_size_line(&shapes, doubled).as_deref(),
        Some("L x W x D = 7.50 x 5.00 x 3.50 mm (design rescaled)")
    );
}

/// Without a planned shape (an empty map, or a shape that stored no width) the row follows
/// the current extents: at scale 5 a width-2, length-3, height-1.4 design gives 15.00 x
/// 10.00 x 7.00 mm, and there is nothing to call rescaled.
#[test]
fn a_missing_planned_shape_leaves_the_row_size_to_the_current_extents() {
    let doubled = SolidExtents {
        width_caliper: 2.0,
        length_caliper: 3.0,
        width_axis: 2.0,
        length_axis: 3.0,
        height: 1.4,
        volume: 7.2,
    };
    let expected = Some("L x W x D = 15.00 x 10.00 x 7.00 mm");
    assert_eq!(
        row_size_line(&BTreeMap::new(), doubled).as_deref(),
        expected
    );
    let without_width = BTreeMap::from([(1, DesignShape::default())]);
    assert_eq!(row_size_line(&without_width, doubled).as_deref(), expected);
}

#[test]
fn the_ratios_belong_to_the_design_and_ignore_the_stones_scale() {
    assert_eq!(
        ratios_value(&extents()).as_deref(),
        Some("L/W 1.50 · D/W 70 %")
    );
    // Width and length stored the other way round read the same.
    let swapped = SolidExtents {
        width_caliper: 1.5,
        length_caliper: 1.0,
        ..extents()
    };
    assert_eq!(ratios_value(&swapped), ratios_value(&extents()));
    let flat = SolidExtents {
        width_caliper: 0.0,
        length_caliper: 0.0,
        ..extents()
    };
    assert_eq!(ratios_value(&flat), None);
}

#[test]
fn orientation_names_the_face_and_the_tilt_and_a_group_lists_its_variants() {
    assert_eq!(orientation_phrase(UP), "faces Top (+Y)");
    assert_eq!(orientation_phrase([-1.0, 0.0, 0.0]), "faces Left (-X)");
    let angle = 23.0_f64.to_radians();
    assert_eq!(
        orientation_phrase([angle.sin(), angle.cos(), 0.0]),
        "tilted 23° from Top"
    );

    let alike = layout(vec![
        stone(1.0, UP, 1.0, [0.0; 3], [2.0; 3]),
        stone(1.0, UP, 1.0, [0.0; 3], [2.0; 3]),
    ]);
    assert_eq!(orientation_value(&refs(&alike)), "table faces Top (+Y)");

    let mixed = layout(vec![
        stone(1.0, [1.0, 0.0, 0.0], 1.0, [0.0; 3], [2.0; 3]),
        stone(1.0, UP, 1.0, [0.0; 3], [2.0; 3]),
        stone(1.0, UP, 1.0, [0.0; 3], [2.0; 3]),
    ]);
    assert_eq!(
        orientation_value(&refs(&mixed)),
        "table: 2 × faces Top (+Y), 1 × faces Right (+X)"
    );
}

fn curves(face_up: [[f32; 3]; 4]) -> TiltPerformanceCurves {
    let axis = |[brilliance, extinction, windowing]: [f32; 3]| {
        let mut axis = AxisTiltCurves {
            brilliance_pct: [1.0; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [2.0; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [3.0; TILT_CURVE_POINTS_PER_AXIS],
        };
        axis.brilliance_pct[FACE_UP] = brilliance;
        axis.extinction_pct[FACE_UP] = extinction;
        axis.windowing_pct[FACE_UP] = windowing;
        axis
    };
    TiltPerformanceCurves {
        axes: face_up.map(axis),
    }
}

#[test]
fn optics_average_the_axes_at_the_face_up_sample_not_the_first() {
    assert_eq!(FACE_UP, 90);
    let curves = curves([
        [70.0, 10.0, 2.0],
        [72.0, 12.0, 4.0],
        [74.0, 14.0, 4.0],
        [76.0, 16.0, 6.0],
    ]);
    assert_eq!(
        format_optics(Some(&curves), Some("Quartz")),
        "brilliance 73 % · extinction 13 % · windowing 4 % (preview material: Quartz)"
    );
    // No recorded material: no parenthesis at all, and no invented name.
    assert_eq!(
        format_optics(Some(&curves), None),
        "brilliance 73 % · extinction 13 % · windowing 4 %"
    );
    assert_eq!(
        format_optics(Some(&curves), Some("  ")),
        "brilliance 73 % · extinction 13 % · windowing 4 %"
    );
    assert_eq!(format_optics(None, Some("Quartz")), "not generated yet");
}

#[test]
fn optics_leave_out_samples_that_are_not_numbers() {
    // Brilliance: 70, NaN, 74, 76 -> mean of the three finite ones, 220 / 3 = 73.3.
    // Extinction: 10, 12, infinity, 16 -> 38 / 3 = 12.7. Windowing has four numbers: 4.
    let mixed = curves([
        [70.0, 10.0, 2.0],
        [f32::NAN, 12.0, 4.0],
        [74.0, f32::INFINITY, 4.0],
        [76.0, 16.0, 6.0],
    ]);
    assert_eq!(
        format_optics(Some(&mixed), None),
        "brilliance 73 % · extinction 13 % · windowing 4 %"
    );
    // A metric with no number at all says so; the others still show.
    let partly = curves([
        [f32::NAN, 10.0, 2.0],
        [f32::NAN, 12.0, 4.0],
        [f32::NAN, 14.0, 4.0],
        [f32::NAN, 16.0, 6.0],
    ]);
    assert_eq!(
        format_optics(Some(&partly), None),
        "brilliance n/a · extinction 13 % · windowing 4 %"
    );
    // Nothing usable at all: the same words as for curves that do not exist yet.
    let broken = curves([[f32::NAN; 3]; 4]);
    assert_eq!(
        format_optics(Some(&broken), Some("Quartz")),
        "not generated yet"
    );
}

#[test]
fn the_metric_list_follows_the_card_order_and_leaves_out_what_is_unknown() {
    let layout = layout(vec![stone(5.0, UP, 32.0, [1.0; 3], [4.0; 3])]);
    let curves = curves([[50.0; 3]; 4]);
    let design_extents = extents();
    let facts = DesignFacts {
        extents: Some(&design_extents),
        curves: Some(&curves),
        preview_material: Some("Quartz"),
        saved: None,
    };
    let full = group_metrics(&refs(&layout), &layout, Some(&cube()), &facts);
    let labels: Vec<&str> = full.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Weight",
            "Size",
            "Ratios",
            "Volume",
            "Orientation",
            "Fill",
            "Optics"
        ]
    );
    assert_eq!(
        metric(&full, "Weight"),
        Some("3.20 ct · 100 % of the total")
    );
    assert_eq!(metric(&full, "Volume"), Some("32.0 mm³"));

    let bare = group_metrics(&refs(&layout), &layout, None, &DesignFacts::default());
    let labels: Vec<&str> = bare.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels, ["Weight", "Volume", "Orientation", "Optics"]);
    assert_eq!(metric(&bare, "Optics"), Some("not generated yet"));

    assert_eq!(
        group_metrics(&[], &layout, None, &facts),
        Vec::<(String, String)>::new()
    );
}

#[test]
fn weight_shows_one_value_a_common_value_or_the_range() {
    let one = layout(vec![stone(1.0, UP, 62.0, [0.0; 3], [4.0; 3])]);
    assert_eq!(
        weight_value(&refs(&one), &one),
        "6.20 ct · 100 % of the total"
    );

    let common = layout(vec![
        stone(1.0, UP, 62.0, [0.0; 3], [4.0; 3]),
        stone(1.0, UP, 62.0, [0.0; 3], [4.0; 3]),
    ]);
    assert_eq!(
        weight_value(&refs(&common), &common),
        "6.20 ct each · 100 % of the total"
    );

    let mut all = layout(vec![
        stone(1.0, UP, 48.0, [0.0; 3], [4.0; 3]),
        stone(1.0, UP, 62.0, [0.0; 3], [4.0; 3]),
        stone(1.0, [1.0, 0.0, 0.0], 100.0, [0.0; 3], [4.0; 3]),
    ]);
    all.stones[2].entry_id = 2;
    let group: Vec<&PlacedStone> = all.stones.iter().filter(|s| s.entry_id == 1).collect();
    // 11 of the 21 ct in total.
    assert_eq!(
        weight_value(&group, &all),
        "4.80-6.20 ct · 52 % of the total"
    );
}

#[test]
fn weights_that_print_alike_are_one_weight_each_and_not_a_range_of_one_figure() {
    // 0.6201 and 0.6215 ct are 0.62 ct at two decimals; the old 1e-4 test called them a range
    // and printed "0.62-0.62 ct".
    let alike = layout(vec![
        stone(1.0, UP, 6.201, [0.0; 3], [4.0; 3]),
        stone(1.0, UP, 6.215, [0.0; 3], [4.0; 3]),
    ]);
    assert_eq!(
        weight_value(&refs(&alike), &alike),
        "0.62 ct each · 100 % of the total"
    );
    // 0.6249 and 0.6251 ct print as 0.62 and 0.63: a range.
    let apart = layout(vec![
        stone(1.0, UP, 6.249, [0.0; 3], [4.0; 3]),
        stone(1.0, UP, 6.251, [0.0; 3], [4.0; 3]),
    ]);
    assert_eq!(
        weight_value(&refs(&apart), &apart),
        "0.62-0.63 ct · 100 % of the total"
    );
}

#[test]
fn volumes_that_print_alike_are_one_volume_each() {
    let alike = layout(vec![
        stone(1.0, UP, 10.01, [0.0; 3], [4.0; 3]),
        stone(1.0, UP, 10.04, [0.0; 3], [4.0; 3]),
    ]);
    assert_eq!(volume_value(&refs(&alike)), "10.0 mm³ each");
    let apart = layout(vec![
        stone(1.0, UP, 10.04, [0.0; 3], [4.0; 3]),
        stone(1.0, UP, 10.06, [0.0; 3], [4.0; 3]),
    ]);
    assert_eq!(volume_value(&refs(&apart)), "10.0-10.1 mm³");
    let one = layout(vec![stone(1.0, UP, 10.04, [0.0; 3], [4.0; 3])]);
    assert_eq!(volume_value(&refs(&one)), "10.0 mm³");
}
