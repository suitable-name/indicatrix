//! Tests of the frame renderer: frame keys, pick publishing, frame buffers and the reset pose.

use super::*;
use crate::gui::rough_plan::view::scene::box_faces;
use glam::DVec3;

/// A fit scene of one cube stone of half size 0.3 at the origin, in a rough box of half
/// size 0.5.
fn cube_scene() -> Arc<Scene> {
    let mut fit = FitScene::default();
    for (id, (normal, ring)) in box_faces(DVec3::splat(-0.3), DVec3::splat(0.3))
        .into_iter()
        .enumerate()
    {
        fit.stones.facet_id.push(id);
        fit.stones.normals.push(normal);
        fit.stones.rings.push((id, ring.to_vec()));
        fit.facet_colors.push([200, 60, 60]);
    }
    let mut rough = SolidMesh::default();
    for (id, (normal, ring)) in box_faces(DVec3::splat(-0.5), DVec3::splat(0.5))
        .into_iter()
        .enumerate()
    {
        rough.facet_id.push(id);
        rough.normals.push(normal);
        rough.rings.push((id, ring.to_vec()));
    }
    fit.rough = Arc::new(rough);
    fit.stone_starts = vec![0, 6];
    fit.stone_group = vec![0];
    Arc::new(Scene::new(SceneKind::Fit(Box::new(fit))))
}

fn request(scene: &Arc<Scene>, options: RenderOptions) -> RenderRequest {
    RenderRequest {
        generation: 1,
        scene: Arc::clone(scene),
        pose: CameraPose {
            yaw: 0.6,
            pitch: 0.45,
            distance: 3.0,
        },
        size: (64, 48),
        options,
    }
}

fn pixel(pixels: &Pixels, x: usize, y: usize) -> [u8; 3] {
    let bytes = pixels.as_bytes();
    let at = (y * pixels.width() as usize + x) * 4;
    [bytes[at], bytes[at + 1], bytes[at + 2]]
}

#[test]
fn the_model_style_outlines_the_selected_cut_and_tints_only_cut_faces() {
    use crate::gui::rough_plan::{shape_worker::centred_mesh, view::scene::CUT_TINT};
    use indicatrix_cut_core::rough_plan::{BoxFace, RoughBase, RoughCut, RoughModel};

    let mut model = RoughModel::new(
        RoughBase::Block {
            x_mm: 10.0,
            y_mm: 8.0,
            z_mm: 6.0,
        },
        Vec::new(),
    );
    model.cuts.push(RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Front],
        setbacks_mm: [1.0, 1.0],
    });
    model.cuts.push(RoughCut::Corner {
        faces: [BoxFace::Bottom, BoxFace::Back, BoxFace::Left],
        setbacks_mm: [1.0, 1.0, 1.0],
    });
    let centred = centred_mesh(&model).expect("a valid model");
    let scene = ModelScene::new(&centred, &model);
    let options = RenderOptions {
        selected_cut: Some(1),
        hover: Hover::Facet(3),
        ..RenderOptions::default()
    };

    let style = model_style(&scene, &options);
    // Six base planes and two cuts: only the second cut's face is selected.
    assert_eq!(style.selected.len(), 8);
    assert_eq!(style.selected.iter().filter(|&&on| on).count(), 1);
    assert!(style.selected[7]);
    assert_eq!(style.hovered, Some(3));
    assert_eq!(style.facet_base_colors[5], BASE_GREY);
    assert_eq!(style.facet_base_colors[6], CUT_TINT);
    assert_eq!(style.facet_base_colors[7], CUT_TINT);

    let plain = model_style(
        &scene,
        &RenderOptions {
            show_cut_faces: false,
            selected_cut: None,
            hover: Hover::Box(BoxTarget::Edge(0)),
            ..options
        },
    );
    assert!(plain.selected.iter().all(|&on| !on));
    assert_eq!(plain.hovered, None);
    assert_eq!(plain.facet_base_colors[6], BASE_GREY);
}

#[test]
fn the_stone_is_drawn_in_the_middle_on_the_background() {
    let scene = cube_scene();
    let mut renderer = ViewRenderer::default();
    let options = RenderOptions {
        show_rough: false,
        show_saw: false,
        ..RenderOptions::default()
    };
    let pixels = renderer.render(&request(&scene, options), None);
    assert_eq!(
        pixel(&pixels, 0, 0),
        [0x12, 0x14, 0x1c],
        "background corner"
    );
    let middle = pixel(&pixels, 32, 24);
    assert!(middle[0] > middle[1], "a red stone face, got {middle:?}");
}

#[test]
fn hiding_the_stones_leaves_only_the_background_and_no_pick() {
    let scene = cube_scene();
    let mut renderer = ViewRenderer::default();
    let shared: SharedPick = Arc::default();
    let options = RenderOptions {
        show_rough: false,
        show_saw: false,
        show_stones: false,
        ..RenderOptions::default()
    };
    let pixels = renderer.render(&request(&scene, options), Some(&shared));
    assert_eq!(pixel(&pixels, 32, 24), [0x12, 0x14, 0x1c]);
    let snapshot = shared.lock().expect("lock");
    assert_eq!(snapshot.facet_at(0.5, 0.5), None);
}

#[test]
fn the_published_pick_names_the_facet_under_a_pixel() {
    let scene = cube_scene();
    let mut renderer = ViewRenderer::default();
    let shared: SharedPick = Arc::default();
    let options = RenderOptions {
        show_rough: false,
        show_saw: false,
        ..RenderOptions::default()
    };
    renderer.render(&request(&scene, options), Some(&shared));
    let snapshot = shared.lock().expect("lock");
    assert_eq!(snapshot.serial, scene.serial);
    let SceneKind::Fit(fit) = &scene.kind else {
        panic!("a fit scene");
    };
    let facet = snapshot
        .facet_at(0.5, 0.5)
        .expect("the stone is in the middle");
    assert_eq!(fit.stone_at_facet(facet), Some(0));
    assert_eq!(snapshot.facet_at(0.01, 0.01), None);
    assert_eq!(snapshot.facet_at(1.5, 0.5), None, "outside the image");
}

#[test]
fn the_glass_rough_tints_the_background_it_covers_and_draws_edges() {
    let scene = cube_scene();
    let mut renderer = ViewRenderer::default();
    let plain = RenderOptions {
        show_rough: false,
        show_saw: false,
        show_stones: false,
        ..RenderOptions::default()
    };
    let glass = RenderOptions {
        show_rough: true,
        ..plain
    };
    let without = renderer.render(&request(&scene, plain), None);
    let with = renderer.render(&request(&scene, glass), None);
    // The middle of the frame is inside the rough's silhouette: brighter than the
    // background by the glass tint.
    let (before, after) = (pixel(&without, 32, 24), pixel(&with, 32, 24));
    assert!(after[2] > before[2], "tinted: {before:?} -> {after:?}");
    // Some edge pixel differs by much more than the tint.
    let edge_pixels = (0..64 * 48)
        .filter(|index| {
            let (col, row) = (index % 64, index / 64);
            let (old, new) = (pixel(&without, col, row), pixel(&with, col, row));
            new[2].abs_diff(old[2]) > 60
        })
        .count();
    assert!(edge_pixels > 20, "edge pixels: {edge_pixels}");
}

#[test]
fn only_the_hover_marker_differs_and_the_raster_is_reused() {
    let scene = cube_scene();
    let model = ModelScene {
        mesh: SolidMesh::default(),
        base_facets: 0,
        cut_count: 0,
        half_extents: [0.6, 0.5, 0.4],
        block: true,
        cut_planes: Vec::new(),
    };
    let model_scene = Arc::new(Scene::new(SceneKind::Model(model)));
    let mut renderer = ViewRenderer::default();
    let calm = RenderOptions::default();
    let marked = RenderOptions {
        hover: Hover::Box(BoxTarget::Corner(1)),
        ..calm
    };
    assert_eq!(
        FrameKey::of(&request(&model_scene, calm)),
        FrameKey::of(&request(&model_scene, marked)),
        "a box hover is not part of the raster key"
    );
    assert_ne!(
        FrameKey::of(&request(&model_scene, calm)),
        FrameKey::of(&request(
            &model_scene,
            RenderOptions {
                hover: Hover::Facet(2),
                ..calm
            }
        )),
        "a facet hover is"
    );
    assert_ne!(
        FrameKey::of(&request(&scene, calm)).serial,
        FrameKey::of(&request(&model_scene, calm)).serial
    );
    let plain = renderer.render(&request(&model_scene, calm), None);
    let with_marker = renderer.render(&request(&model_scene, marked), None);
    let (plain_pixels, _) = plain.as_bytes().as_chunks::<4>();
    let (marked_pixels, _) = with_marker.as_bytes().as_chunks::<4>();
    let differing = plain_pixels
        .iter()
        .zip(marked_pixels)
        .filter(|(left, right)| left != right)
        .count();
    assert!(
        differing > 20,
        "the corner dot was drawn: {differing} pixels"
    );
    // The marker sits where the corner projects.
    let camera = camera_for(request(&model_scene, calm).pose);
    let at = project_point(
        &camera,
        BoxTarget::Corner(1).points([0.6, 0.5, 0.4])[0],
        64,
        48,
    )
    .expect("in front");
    assert_eq!(
        pixel(&with_marker, at.0.round() as usize, at.1.round() as usize),
        HOVER_MARK
    );
}

#[test]
fn the_frame_key_ignores_the_options_its_scene_kind_does_not_draw() {
    let fit = cube_scene();
    let model = Arc::new(Scene::new(SceneKind::Model(ModelScene {
        mesh: SolidMesh::default(),
        base_facets: 0,
        cut_count: 0,
        half_extents: [0.6, 0.5, 0.4],
        block: true,
        cut_planes: Vec::new(),
    })));
    let base = RenderOptions::default();
    let key = |scene: &Arc<Scene>, options: RenderOptions| FrameKey::of(&request(scene, options));

    // Selecting a cut or tinting cut faces does not redraw a result ...
    let cut_selected = RenderOptions {
        selected_cut: Some(2),
        show_cut_faces: false,
        ..base
    };
    assert_eq!(key(&fit, base), key(&fit, cut_selected));
    // ... but it does redraw the model, and so does its facet hover.
    assert_ne!(key(&model, base), key(&model, cut_selected));
    let facet_hover = RenderOptions {
        hover: Hover::Facet(1),
        ..base
    };
    assert_eq!(key(&fit, base), key(&fit, facet_hover));
    assert_ne!(key(&model, base), key(&model, facet_hover));

    // Outlining a design group or hiding a layer redraws a result, not the model.
    let outlined = RenderOptions {
        highlight_group: Some(0),
        show_saw: false,
        ..base
    };
    assert_ne!(key(&fit, base), key(&fit, outlined));
    assert_eq!(key(&model, base), key(&model, outlined));
}

#[test]
fn a_published_pick_buffer_is_swapped_in_and_a_reader_keeps_the_one_it_took() {
    let scene = cube_scene();
    let mut renderer = ViewRenderer::default();
    let shared: SharedPick = Arc::default();
    let options = RenderOptions {
        show_rough: false,
        show_saw: false,
        ..RenderOptions::default()
    };
    renderer.render(&request(&scene, options), Some(&shared));
    let taken = shared.lock().expect("lock").clone();
    assert_eq!(taken.pick.len(), 64 * 48);

    let mut moved = request(&scene, options);
    moved.pose.yaw = 1.2;
    renderer.render(&moved, Some(&shared));
    let now = shared.lock().expect("lock").clone();
    assert!(
        !Arc::ptr_eq(&taken.pick, &now.pick),
        "a new buffer was swapped in"
    );
    assert_eq!(taken.pick.len(), now.pick.len());
    assert_ne!(
        taken.pick, now.pick,
        "the reader's buffer still shows the old frame"
    );
}

#[test]
fn two_frame_buffers_take_turns_and_a_frame_the_window_holds_is_never_overwritten() {
    let scene = cube_scene();
    let mut renderer = ViewRenderer::default();
    let options = RenderOptions {
        show_rough: false,
        show_saw: false,
        ..RenderOptions::default()
    };
    let first = renderer.render(&request(&scene, options), None);
    let first_bytes = first.as_bytes().as_ptr();
    drop(first);
    let second = renderer.render(&request(&scene, options), None);
    // The third frame reuses the first buffer: nothing holds it any more.
    let third = renderer.render(&request(&scene, options), None);
    assert_eq!(third.as_bytes().as_ptr(), first_bytes);
    // The fourth would reuse the second, which the window still holds: it is copied.
    let fourth = renderer.render(&request(&scene, options), None);
    assert_ne!(fourth.as_bytes().as_ptr(), second.as_bytes().as_ptr());
    assert_eq!(fourth.as_bytes(), second.as_bytes());
}

#[test]
fn a_resized_image_keeps_its_zoom_relative_to_the_framing() {
    let wide = reset_pose(1.5).distance;
    // A square image needs the same distance as a wide one; a tall one, twice at 0.5.
    assert!((refit_distance(wide, 1.5, 1.0) - wide).abs() < 1e-6);
    assert!((refit_distance(wide, 1.5, 0.75) - reset_pose(0.75).distance).abs() < 1e-5);
    assert!(
        wide.mul_add(-(1.0 / 0.75), refit_distance(wide, 1.5, 0.75))
            .abs()
            < 1e-5
    );
    // Back to the wide image it came from.
    let tall = reset_pose(0.75).distance;
    assert!((refit_distance(tall, 0.75, 1.5) - wide).abs() < 1e-5);
    // Whatever the ratio, the result stays inside the orbit limits.
    let (near, far) = orbit_distance_bounds(super::super::scene::SCENE_RADIUS);
    assert!(refit_distance(far, 1.5, 0.1) <= far);
    assert!(refit_distance(near, 0.1, 1.5) >= near);
}

#[test]
fn the_reset_view_frames_the_scene_and_backs_off_for_a_tall_image() {
    let wide = reset_pose(1.5);
    let tall = reset_pose(0.5);
    let (near, far) = orbit_distance_bounds(super::super::scene::SCENE_RADIUS);
    assert!(wide.distance >= near && wide.distance <= far);
    assert!(tall.distance > wide.distance);
    assert!(tall.distance <= far);
    assert_eq!(reset_pose(f32::NAN).distance, reset_pose(1.0).distance);
}
