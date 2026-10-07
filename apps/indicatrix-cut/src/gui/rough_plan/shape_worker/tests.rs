use super::*;
use indicatrix_cut_core::rough_plan::{BoxFace, RoughCut};

/// Evaluates `request` from scratch, as the worker does for its first request.
fn evaluate(request: &ShapeRequest) -> ShapeResult {
    evaluate_reusing(request, None).0
}

fn request(model: RoughModel) -> ShapeRequest {
    ShapeRequest {
        revision: 7,
        model,
        specific_gravity: 2.65,
        material_name: "Quartz".to_string(),
        weighed_ct: None,
        selected_cut: None,
    }
}

fn block() -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 10.0,
            y_mm: 8.0,
            z_mm: 6.0,
        },
        Vec::new(),
    )
}

/// The block with a corner cut of setbacks 3, 4, 6 mm, which removes a tetrahedron of
/// 3 * 4 * 6 / 6 = 12 mm^3.
fn cornered_block() -> RoughModel {
    let mut model = block();
    model.cuts.push(RoughCut::Corner {
        faces: [BoxFace::Top, BoxFace::Front, BoxFace::Right],
        setbacks_mm: [4.0, 6.0, 3.0],
    });
    model
}

#[test]
fn a_plain_block_has_its_box_volume_and_a_centred_mesh() {
    let result = evaluate(&request(block()));
    assert_eq!(result.revision, 7);
    let output = result.outcome.expect("a plain block is valid");
    assert!((output.measure.volume_mm3 - 480.0).abs() < 1e-6);
    assert!(
        (output.specific_gravity - 2.65).abs() < 1e-12,
        "the gravity is carried for the removed-volume text"
    );
    assert!(
        output.model_text.contains("480 mm³"),
        "{}",
        output.model_text
    );
    assert!(
        output.model_text.contains("Quartz"),
        "{}",
        output.model_text
    );
    assert_eq!(output.check_level, 0);

    let mut low = DVec3::splat(f64::INFINITY);
    let mut high = DVec3::splat(f64::NEG_INFINITY);
    for &p in &output.mesh.positions {
        low = low.min(p);
        high = high.max(p);
    }
    assert!((low + high).length() < 1e-9, "the box centre is the origin");
    assert!((high - low - DVec3::new(10.0, 8.0, 6.0)).length() < 1e-9);
}

/// The C-shaped mesh rough (a 20 mm cube with a 10 x 10 x 20 mm notch in its `+x`
/// face) with its top 2 mm sawn off by a face cut.
fn cut_c_shape() -> RoughModel {
    RoughModel::new(
        crate::gui::rough_plan::obj_import::mesh_base_of(
            crate::gui::rough_plan::obj_import::C_SHAPE_OBJ,
        ),
        vec![RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 2.0,
        }],
    )
}

#[test]
fn a_mesh_rough_is_drawn_from_its_clipped_surface_with_cut_faces_by_cut() {
    let model = cut_c_shape();
    let output = evaluate(&request(model))
        .outcome
        .expect("a valid mesh rough");
    // The mesh volume, less the 20 x 2 x 20 mm slab: the notch is not in the slab.
    assert!(
        (output.measure.volume_mm3 - 5200.0).abs() < 1e-6,
        "{}",
        output.measure.volume_mm3
    );
    let mesh = &output.mesh;
    let pieces = mesh.piece_normals.as_ref().expect("one normal per ring");
    let visible = mesh.edge_visible.as_ref().expect("one flag list per ring");
    assert_eq!(pieces.len(), mesh.rings.len());
    assert_eq!(visible.len(), mesh.rings.len());
    assert!(visible.iter().all(|flags| flags.len() == 3));
    // The cube has six hull planes, so the cut's face is facet 6 and faces +y; every
    // other triangle is the surface, facet 0.
    let cap: Vec<usize> = (0..mesh.rings.len())
        .filter(|&i| mesh.rings[i].0 == 6)
        .collect();
    assert!(!cap.is_empty(), "the cut leaves a face");
    assert!(cap.iter().all(|&i| (pieces[i] - DVec3::Y).length() < 1e-9));
    assert!(
        mesh.rings.iter().all(|(id, _)| *id == 0 || *id == 6),
        "ids: {:?}",
        mesh.rings
            .iter()
            .map(|(id, _)| *id)
            .collect::<BTreeSet<_>>()
    );
    // Flat runs hide their seams; creases and cap boundaries are drawn.
    let drawn = visible.iter().flatten().filter(|&&d| d).count();
    let hidden = visible.iter().flatten().filter(|&&d| !d).count();
    assert!(drawn > 0 && hidden > 0, "drawn {drawn}, hidden {hidden}");
    // The notch's back wall is in the mesh: x = 10 in the rough frame, 0 centred, over
    // 5 < y < 15 (-5 < y < 5 centred).
    assert!(mesh.rings.iter().any(|(id, ring)| {
        *id == 0
            && ring
                .iter()
                .all(|p| p.x.abs() < 1e-9 && p.y.abs() < 5.0 + 1e-9)
    }));
}

#[test]
fn the_selected_cut_reports_the_volume_it_removes() {
    let mut ask = request(cornered_block());
    ask.selected_cut = Some(0);
    let output = evaluate(&ask).outcome.expect("valid corner cut");
    assert!((output.measure.volume_mm3 - 468.0).abs() < 1e-6);
    let removed = output.removed_mm3.expect("the selected cut has a volume");
    assert!((removed - 12.0).abs() < 1e-6, "removed {removed}");
    assert_eq!(output.selected_cut, Some(0), "the result names its cut");

    ask.selected_cut = Some(5);
    assert!(
        evaluate(&ask)
            .outcome
            .expect("still valid")
            .removed_mm3
            .is_none()
    );
    ask.selected_cut = None;
    let output = evaluate(&ask).outcome.expect("still valid");
    assert!(output.removed_mm3.is_none());
    assert_eq!(output.selected_cut, None);
}

#[test]
fn a_request_that_only_changes_the_cut_reuses_the_mesh_and_measure() {
    let first = request(cornered_block());
    let (first_result, kept) = evaluate_reusing(&first, None);
    let first_output = first_result.outcome.expect("valid");
    assert!(first_output.removed_mm3.is_none());

    let mut second = first;
    second.revision = 8;
    second.selected_cut = Some(0);
    let (second_result, kept) = evaluate_reusing(&second, kept);
    let second_output = second_result.outcome.expect("valid");
    assert!(
        Arc::ptr_eq(&first_output.mesh, &second_output.mesh),
        "the same model keeps its mesh"
    );
    assert_eq!(first_output.measure, second_output.measure);
    let removed = second_output.removed_mm3.expect("the cut has a volume");
    assert!((removed - 12.0).abs() < 1e-6, "removed {removed}");

    // The figure is remembered with the cut it was measured for.
    let kept = kept.expect("the solid is kept");
    assert_eq!(kept.removed, Some((0, second_output.removed_mm3)));

    // A different model measures and meshes afresh.
    let mut third = second;
    third.model = block();
    third.selected_cut = None;
    let (third_result, _) = evaluate_reusing(&third, Some(kept));
    let third_output = third_result.outcome.expect("valid");
    assert!(!Arc::ptr_eq(&second_output.mesh, &third_output.mesh));
    assert!((third_output.measure.volume_mm3 - 480.0).abs() < 1e-6);
}

#[test]
fn a_bad_cut_is_reported_with_its_row() {
    let mut model = block();
    model.cuts.push(RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Front],
        setbacks_mm: [1.0, 1.0],
    });
    model.cuts.push(RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Front],
        setbacks_mm: [-1.0, 1.0],
    });
    let (result, kept) = evaluate_reusing(&request(model), None);
    let failure = result.outcome.expect_err("negative setback");
    assert!(kept.is_none(), "an invalid model keeps nothing");
    assert!(
        matches!(
            failure,
            ShapeFailure::Invalid(ShapeError::BadSetback { index: 1, .. })
        ),
        "{failure:?}"
    );
    assert_eq!(failure.row(), Some(1));
    assert_eq!(error_row(&ShapeError::NothingLeft), None);
}

#[test]
fn an_internal_failure_names_no_row_and_tells_the_user_to_edit() {
    let failure = ShapeFailure::Internal;
    assert_eq!(failure.row(), None);
    assert_eq!(
        failure.message(),
        "The model could not be evaluated (internal error); edit the model to retry."
    );
    assert_eq!(
        ShapeFailure::Invalid(ShapeError::NothingLeft).message(),
        ShapeError::NothingLeft.to_string()
    );
}

#[test]
fn the_weight_check_uses_the_weighed_carat() {
    let mut ask = request(block());
    // 480 mm^3 * 2.65 / 200 = 6.36 ct.
    ask.weighed_ct = Some(6.4);
    let output = evaluate(&ask).outcome.expect("valid");
    assert_eq!(output.check_level, 1);
    ask.weighed_ct = Some(3.0);
    assert_eq!(evaluate(&ask).outcome.expect("valid").check_level, 3);
}

#[test]
fn without_a_material_the_model_is_not_accused_of_the_wrong_weight() {
    let mut ask = request(block());
    ask.specific_gravity = 0.0;
    ask.material_name = String::new();
    ask.weighed_ct = Some(6.4);
    let output = evaluate(&ask).outcome.expect("valid");
    assert_eq!(output.check_level, 0);
    assert!(output.check_text.is_empty(), "{}", output.check_text);
    assert_eq!(output.model_text, "Model 480 mm³");
}

#[test]
fn a_bad_weighed_field_replaces_only_the_weight_check_line() {
    // A valid or empty field leaves the evaluation's line alone.
    assert_eq!(
        weight_check_lines("", "close".to_string(), 2),
        ("close".to_string(), 2)
    );
    assert_eq!(
        weight_check_lines("8,9", "matches".to_string(), 1),
        ("matches".to_string(), 1)
    );
    // Anything else is reported at the "check the model" level, whatever the model says.
    for typed in ["abc", "0", "-1", "inf"] {
        let (text, level) = weight_check_lines(typed, "matches".to_string(), 1);
        assert_eq!(level, 3, "{typed}");
        assert!(text.contains("Weighed carat"), "{typed}: {text}");
    }
    // The model is still evaluated with such a field: the request carries no weighed
    // carat and a block's figures come out as usual.
    let output = evaluate(&request(block())).outcome.expect("valid block");
    assert_eq!(output.check_level, 0);
    assert!(output.measure.volume_mm3 > 0.0);
}

#[test]
fn requests_that_differ_only_in_the_revision_are_the_same_inputs() {
    let a = request(block());
    let mut b = a.clone();
    b.revision += 1;
    assert!(a.same_inputs(&b));
    b.selected_cut = Some(0);
    assert!(!a.same_inputs(&b));
    let mut c = a.clone();
    c.weighed_ct = Some(1.0);
    assert!(!a.same_inputs(&c));
}

#[test]
fn the_removed_line_names_volume_and_carats() {
    // 84 * 2.6 / 200 = 1.092 ct.
    assert_eq!(removed_text(84.0, 2.6), "removes 84 mm³ (1.09 ct)");
}
