//! Unit tests for [`SolidRasterizer`]'s coverage/depth ordering, pick buffer,
//! overlay styling, and (ignored) visual/timing smoke tests against real fixtures.

use super::*;
use indicatrix::{
    geometry::stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh},
    optics::raytracer::Camera,
};
// Only `rbc_planes`/`crackotto_step_planes` (below) need these, and both are
// themselves `#[cfg(not(target_arch = "wasm32"))]`.
#[cfg(not(target_arch = "wasm32"))]
use indicatrix::geometry::{cuts::StandardGemCuts, meet_solver, plane::GpuFacetPlane};

/// The `stone_metrics.rs` "plain box" fixture: `x,z in [-1,1]`, `y in [-0.6,0.6]`,
/// six axis-aligned facets, indices `0..=5` (`+X, -X, +Y, -Y, +Z, -Z`).
fn unit_box_mesh() -> SolidMesh {
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("box fixture must close: {other:?}"),
    }
}

#[test]
fn unit_cube_renders_expected_coverage_and_depth_ordering() {
    // Camera on +Z looking down -Z: only the +Z facet (index 4) can be nearest.
    let mesh = unit_box_mesh();
    let camera = Camera::new(0.0, 0.0, 5.0, 42.0);
    let style = SolidStyle::default();
    let mut rasterizer = SolidRasterizer::new(64, 64);
    rasterizer.render(&mesh, &camera, &style);

    assert_eq!(
        rasterizer.pick_at(32, 32),
        Some(4),
        "screen center must show the +Z facet"
    );
    assert_eq!(
        rasterizer.pick_at(0, 0),
        None,
        "image corner must be outside the box's silhouette"
    );

    // The box is convex, so back-face culling + depth test only ever leave the
    // facet directly facing the camera -- never the opposite (-Z) facet.
    let mut painted = 0;
    for &p in &rasterizer.pick {
        if p != 0 {
            painted += 1;
            assert_eq!(p, 5, "every painted pixel must be the +Z facet (id 4)");
        }
    }
    assert!(
        painted > 100,
        "expected a real silhouette, got {painted} px"
    );
}

/// Two axis-facing quads at different depths (0 nearer/smaller, 1 farther/bigger),
/// built directly so the exact expected pixels are known.
fn two_quads_mesh() -> SolidMesh {
    let mut mesh = SolidMesh::default();
    for (facet_id, z, half_extent) in [(0usize, 0.0f64, 0.5f64), (1usize, -2.0f64, 1.0f64)] {
        let normal = DVec3::new(0.0, 0.0, 1.0);
        let corners = [
            DVec3::new(-half_extent, -half_extent, z),
            DVec3::new(half_extent, -half_extent, z),
            DVec3::new(half_extent, half_extent, z),
            DVec3::new(-half_extent, half_extent, z),
        ];
        let base = mesh.positions.len() as u32;
        for c in corners {
            mesh.positions.push(c);
            mesh.normals.push(normal);
            mesh.facet_id.push(facet_id);
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        mesh.rings.push((facet_id, corners.to_vec()));
    }
    mesh
}

/// Camera at `(0,0,5)` looking down `-Z`. Near quad's right edge projects to
/// `screen_x ~= 40.3`, far quad's to `~= 43.9`: `32` sees both (near wins),
/// `42` sees only the far quad, `60` sees neither.
fn two_quads_camera() -> Camera {
    Camera::new(0.0, 0.0, 5.0, 42.0)
}

#[test]
fn a_facet_hidden_behind_another_never_paints() {
    let mesh = two_quads_mesh();
    let camera = two_quads_camera();
    let style = SolidStyle::default();
    let mut rasterizer = SolidRasterizer::new(64, 64);
    rasterizer.render(&mesh, &camera, &style);

    // Where both quads cover the same pixel, the nearer one (facet 0) must win.
    assert_eq!(rasterizer.pick_at(32, 32), Some(0));
}

#[test]
fn pick_buffer_returns_the_right_facet_id_at_known_pixels() {
    let mesh = two_quads_mesh();
    let camera = two_quads_camera();
    let style = SolidStyle::default();
    let mut rasterizer = SolidRasterizer::new(64, 64);
    rasterizer.render(&mesh, &camera, &style);

    assert_eq!(
        rasterizer.pick_at(32, 32),
        Some(0),
        "center: covered by both, near facet (0) wins"
    );
    assert_eq!(
        rasterizer.pick_at(42, 32),
        Some(1),
        "just outside the near quad's footprint, inside the far quad's"
    );
    assert_eq!(
        rasterizer.pick_at(60, 32),
        None,
        "outside both quads' footprints"
    );
}

/// The orientation marker (a bright line from the stone's centre along +x) is drawn over
/// the fill on the very row these pixels sit on, so the color tests turn it off.
#[test]
fn two_facet_mesh_with_two_colors_paints_two_different_fills() {
    let mesh = two_quads_mesh();
    let camera = two_quads_camera();
    let style = SolidStyle {
        facet_base_colors: vec![[220, 40, 40], [40, 220, 40]],
        show_orientation_marker: false,
        ..SolidStyle::default()
    };
    let mut rasterizer = SolidRasterizer::new(64, 64);
    rasterizer.render(&mesh, &camera, &style);

    assert_eq!(rasterizer.pick_at(32, 32), Some(0));
    assert_eq!(rasterizer.pick_at(42, 32), Some(1));

    let idx0 = ((32 * 64 + 32) * 4) as usize;
    let color0 = [
        rasterizer.color[idx0],
        rasterizer.color[idx0 + 1],
        rasterizer.color[idx0 + 2],
    ];
    let idx1 = ((32 * 64 + 42) * 4) as usize;
    let color1 = [
        rasterizer.color[idx1],
        rasterizer.color[idx1 + 1],
        rasterizer.color[idx1 + 2],
    ];

    assert_ne!(
        color0, color1,
        "facets with different base colors must produce different fills"
    );
    // Facet 0 has reddish base color, facet 1 has greenish base color
    assert!(color0[0] > color0[1]);
    assert!(color1[1] > color1[0]);
}

#[test]
fn a_facet_index_past_the_end_of_facet_base_colors_falls_back_to_base_color() {
    let mesh = two_quads_mesh();
    let camera = two_quads_camera();
    let render_pixels = |style: &SolidStyle| {
        let mut rasterizer = SolidRasterizer::new(64, 64);
        rasterizer.render(&mesh, &camera, style);
        let pixel = |x: usize| {
            let i = (32 * 64 + x) * 4;
            [
                rasterizer.color[i],
                rasterizer.color[i + 1],
                rasterizer.color[i + 2],
            ]
        };
        (pixel(32), pixel(42))
    };

    // Only facet 0 has an entry; facet 1 is past the end of the vector.
    let (short_near, short_far) = render_pixels(&SolidStyle {
        facet_base_colors: vec![[220, 40, 40]],
        show_orientation_marker: false,
        ..SolidStyle::default()
    });
    let (plain_near, plain_far) = render_pixels(&SolidStyle {
        show_orientation_marker: false,
        ..SolidStyle::default()
    });
    assert_ne!(short_near, plain_near, "facet 0 must use its own color");
    assert_eq!(
        short_far, plain_far,
        "facet 1 is past the end and must be painted with base_color"
    );

    // The fallback is exactly the color an explicit entry of base_color gives.
    let base_color = SolidStyle::default().base_color;
    let (_, explicit_far) = render_pixels(&SolidStyle {
        facet_base_colors: vec![[220, 40, 40], base_color],
        show_orientation_marker: false,
        ..SolidStyle::default()
    });
    assert_eq!(short_far, explicit_far);
}

#[test]
fn project_point_matches_internal_project() {
    let camera = Camera::new(0.5, 0.3, 4.0, 42.0);
    let p_vec = glam::Vec3::new(0.5, -0.2, 0.8);
    let p_dvec = DVec3::new(0.5, -0.2, 0.8);
    let width = 128u32;
    let height = 96u32;

    let res_pub = project_point(&camera, p_vec, width, height).expect("must project");
    let res_int =
        render::project(&camera, p_dvec, width as f32, height as f32).expect("must project");

    // The public wrapper delegates, so the two agree exactly, not just closely.
    assert_eq!(res_pub.0.to_bits(), res_int.0.to_bits());
    assert_eq!(res_pub.1.to_bits(), res_int.1.to_bits());
    assert_eq!(res_pub.2.to_bits(), res_int.2.to_bits());

    // Behind camera returns None
    let behind = glam::Vec3::new(0.0, 0.0, 10.0);
    assert!(project_point(&camera, behind, width, height).is_none());
    assert!(project_point(&camera, p_vec, 0, height).is_none());
}

#[test]
fn flagged_facets_are_visibly_hatched() {
    let mesh = unit_box_mesh();
    let mut flagged = vec![false; 6];
    flagged[4] = true; // +Z facet, the only one visible here
    let hatched_style = SolidStyle {
        flagged,
        ..SolidStyle::default()
    };

    let mut plain = SolidRasterizer::new(64, 64);
    plain.render(
        &mesh,
        &Camera::new(0.0, 0.0, 5.0, 42.0),
        &SolidStyle::default(),
    );
    let mut hatched = SolidRasterizer::new(64, 64);
    hatched.render(&mesh, &Camera::new(0.0, 0.0, 5.0, 42.0), &hatched_style);

    assert_ne!(
        plain.color, hatched.color,
        "hatching a visible facet must change at least one pixel"
    );
}

#[test]
fn selected_facets_get_a_visible_tint_and_the_selected_edge_color() {
    let mesh = unit_box_mesh();
    let mut selected = vec![false; 6];
    selected[4] = true; // +Z facet, the only one visible here
    let style = SolidStyle {
        selected,
        selected_color: [0, 0, 255],
        edge_color: [10, 10, 10],
        ..SolidStyle::default()
    };

    let mut plain = SolidRasterizer::new(64, 64);
    plain.render(
        &mesh,
        &Camera::new(0.0, 0.0, 5.0, 42.0),
        &SolidStyle::default(),
    );
    let mut tinted = SolidRasterizer::new(64, 64);
    tinted.render(&mesh, &Camera::new(0.0, 0.0, 5.0, 42.0), &style);

    assert_ne!(
        plain.color, tinted.color,
        "tinting a selected facet must change at least one pixel"
    );
    let found_selected_edge = tinted
        .color
        .as_chunks::<4>()
        .0
        .iter()
        .any(|px| px[0] == 0 && px[1] == 0 && px[2] == 255);
    assert!(
        found_selected_edge,
        "expected the selected edge color somewhere on screen"
    );
}

#[test]
fn pending_facets_get_the_pending_edge_color() {
    let mesh = unit_box_mesh();
    let mut pending = vec![false; 6];
    pending[4] = true;
    let style = SolidStyle {
        pending,
        pending_color: [255, 0, 0],
        edge_color: [10, 10, 10],
        ..SolidStyle::default()
    };
    let mut rasterizer = SolidRasterizer::new(64, 64);
    rasterizer.render(&mesh, &Camera::new(0.0, 0.0, 5.0, 42.0), &style);

    // The box's silhouette edge must be drawn in the pending color somewhere.
    let found_pending_edge = rasterizer
        .color
        .as_chunks::<4>()
        .0
        .iter()
        .any(|px| px[0] == 255 && px[1] == 0 && px[2] == 0);
    assert!(
        found_pending_edge,
        "expected the pending edge color somewhere on screen"
    );
}

/// The unit box seen from an off-axis camera (`+Z`, id 4, and one side facet are
/// visible): returns the camera, the side facet's id, and the pixel at the middle of
/// the vertical edge the two facets share. With zero pitch that edge projects to an
/// exactly vertical screen line, so its middle pixel is on it whichever pass drew it.
fn box_shared_edge() -> (Camera, usize, (i32, i32)) {
    let mesh = unit_box_mesh();
    let camera = Camera::new(0.5, 0.0, 5.0, 42.0);
    let mut probe = SolidRasterizer::new(128, 128);
    probe.render(
        &mesh,
        &camera,
        &SolidStyle {
            show_orientation_marker: false,
            ..SolidStyle::default()
        },
    );
    let side = usize::from(!probe.pick.contains(&1));
    assert!(probe.pick.contains(&5), "the +Z facet must be visible");
    assert!(
        probe.pick.contains(&(side as u32 + 1)),
        "one of the side facets must be visible"
    );
    let x = if side == 0 { 1.0 } else { -1.0 };
    let (sx, sy, _) = render::project(&camera, DVec3::new(x, 0.0, 1.0), 128.0, 128.0)
        .expect("the edge midpoint is in front of the camera");
    (camera, side, (sx.round() as i32, sy.round() as i32))
}

/// Renders the unit box from [`box_shared_edge`]'s camera with `style`.
fn render_box_edge(camera: &Camera, style: SolidStyle) -> SolidRasterizer {
    let mut rasterizer = SolidRasterizer::new(128, 128);
    rasterizer.render(
        &unit_box_mesh(),
        camera,
        &SolidStyle {
            show_orientation_marker: false,
            ..style
        },
    );
    rasterizer
}

/// Whether any pixel in the three columns around `(x, y)` is exactly `color`.
fn edge_window_has(rasterizer: &SolidRasterizer, (x, y): (i32, i32), color: [u8; 3]) -> bool {
    (x - 1..=x + 1).any(|column| {
        let index = (y as usize * rasterizer.width as usize + column as usize) * 4;
        rasterizer.color[index..index + 3] == color
    })
}

#[test]
fn provisional_facets_overpaint_a_shared_edge_against_a_selected_facet() {
    let (camera, side, spot) = box_shared_edge();
    let defaults = SolidStyle::default();
    for (selected_id, provisional_id) in [(4_usize, side), (side, 4_usize)] {
        let mut selected = vec![false; 6];
        selected[selected_id] = true;
        let rasterizer = render_box_edge(
            &camera,
            SolidStyle {
                selected,
                provisional: vec![provisional_id as u32],
                ..SolidStyle::default()
            },
        );
        assert!(
            edge_window_has(&rasterizer, spot, defaults.provisional_color),
            "the provisional outline must own the shared edge (selected {selected_id})"
        );
        assert!(
            !edge_window_has(&rasterizer, spot, defaults.selected_color),
            "the selected outline must not survive on the shared edge (selected {selected_id})"
        );
    }
    let mut pending = vec![false; 6];
    pending[side] = true;
    let rasterizer = render_box_edge(
        &camera,
        SolidStyle {
            pending,
            provisional: vec![4],
            ..SolidStyle::default()
        },
    );
    assert!(edge_window_has(
        &rasterizer,
        spot,
        defaults.provisional_color
    ));
    assert!(!edge_window_has(&rasterizer, spot, defaults.pending_color));
}

#[test]
fn moved_facets_are_outlined_but_lose_a_shared_edge_to_a_selected_facet() {
    let (camera, side, spot) = box_shared_edge();
    let defaults = SolidStyle::default();
    let lone = render_box_edge(
        &camera,
        SolidStyle {
            moved: vec![4],
            ..SolidStyle::default()
        },
    );
    assert!(
        edge_window_has(&lone, spot, defaults.moved_color),
        "a moved facet beside an ordinary one must show the moved outline"
    );
    for (selected_id, moved_id) in [(side, 4_usize), (4_usize, side)] {
        let mut selected = vec![false; 6];
        selected[selected_id] = true;
        let rasterizer = render_box_edge(
            &camera,
            SolidStyle {
                selected,
                moved: vec![moved_id as u32],
                ..SolidStyle::default()
            },
        );
        assert!(
            edge_window_has(&rasterizer, spot, defaults.selected_color),
            "the selected outline must own the shared edge (selected {selected_id})"
        );
        assert!(
            !edge_window_has(&rasterizer, spot, defaults.moved_color),
            "the moved outline must lose the shared edge (selected {selected_id})"
        );
    }
}

#[test]
fn provisional_and_moved_colors_have_their_documented_defaults() {
    let style = SolidStyle::default();
    assert_eq!(style.provisional_color, [74, 222, 128]);
    assert_eq!(style.moved_color, [251, 146, 60]);
    assert!(style.provisional.is_empty() && style.moved.is_empty());
}

#[test]
fn preform_facets_are_tinted_and_can_be_hidden() {
    // Facet 4 (+Z, the only one visible from this camera) counts as a
    // preform plane under this style (`preform_plane_count: 5`).
    let mesh = unit_box_mesh();
    let camera = Camera::new(0.0, 0.0, 5.0, 42.0);

    let mut plain = SolidRasterizer::new(64, 64);
    plain.render(
        &mesh,
        &camera,
        &SolidStyle {
            show_orientation_marker: false,
            ..SolidStyle::default()
        },
    );

    let mut tinted = SolidRasterizer::new(64, 64);
    tinted.render(
        &mesh,
        &camera,
        &SolidStyle {
            preform_plane_count: 5,
            show_orientation_marker: false,
            ..SolidStyle::default()
        },
    );
    assert_eq!(
        tinted.pick_at(32, 32),
        Some(4),
        "a shown preform facet must still render and still be pickable"
    );
    assert_ne!(
        plain.color, tinted.color,
        "a tinted preform facet must look different from an ordinary one"
    );

    let mut hidden = SolidRasterizer::new(64, 64);
    hidden.render(
        &mesh,
        &camera,
        &SolidStyle {
            preform_plane_count: 5,
            show_preform: false,
            show_orientation_marker: false,
            ..SolidStyle::default()
        },
    );
    assert_eq!(
        hidden.pick_at(32, 32),
        None,
        "a hidden preform facet must be culled entirely, like a back-face"
    );
}

#[test]
fn a_big_enough_facet_label_changes_the_rendered_pixels() {
    let mesh = unit_box_mesh();
    // A 128 px frame and a closer camera on purpose: `draw_facet_labels` skips
    // any facet whose projected span is under `MIN_LABEL_SPAN` (26 px), and in a
    // 64x64 frame at distance 5 this box lands under it, so the label would be
    // skipped by design and the assertion below would prove nothing.
    let camera = Camera::new(0.0, 0.0, 4.0, 42.0);
    let mut plain = SolidRasterizer::new(128, 128);
    plain.render(
        &mesh,
        &camera,
        &SolidStyle {
            show_orientation_marker: false,
            ..SolidStyle::default()
        },
    );
    // Label whichever facet actually faces the camera at the frame centre. A
    // hard-coded id is a trap here: on a box at this pose the top face is
    // edge-on, its projected span falls below `MIN_LABEL_SPAN`, and
    // `draw_facet_labels` skips it by design -- so the test would fail for a
    // reason that has nothing to do with labelling.
    let visible = plain
        .pick_at(64, 64)
        .expect("the box must paint the frame centre") as usize;
    let mut labels = vec![String::new(); 6];
    labels[visible] = "T".to_string();

    let mut labeled = SolidRasterizer::new(128, 128);
    labeled.render(
        &mesh,
        &camera,
        &SolidStyle {
            facet_labels: labels,
            show_orientation_marker: false,
            ..SolidStyle::default()
        },
    );

    assert_ne!(
        plain.color, labeled.color,
        "a facet label on a facet this large (the box fills most of a 64x64 \
         frame) must change at least one pixel"
    );
}

#[test]
fn orientation_marker_draws_a_crown_pavilion_label_and_can_be_turned_off() {
    let mesh = unit_box_mesh();
    // Pitch above the pole: the camera sits above the girdle plane, looking
    // down onto the crown side.
    let camera = Camera::new(0.0, 0.6, 5.0, 42.0);

    let mut with_marker = SolidRasterizer::new(64, 64);
    with_marker.render(&mesh, &camera, &SolidStyle::default());
    let mut without_marker = SolidRasterizer::new(64, 64);
    without_marker.render(
        &mesh,
        &camera,
        &SolidStyle {
            show_orientation_marker: false,
            ..SolidStyle::default()
        },
    );

    assert_ne!(
        with_marker.color, without_marker.color,
        "the orientation marker must actually draw something when enabled"
    );
}

/// Mirrors `stone_metrics.rs`'s `planes_from_asc_schedule` helper: converts
/// `GpuFacetPlane`'s `n . x + d <= 0` convention to the `n . x <= m` pairs
/// `build_solid_mesh` takes.
///
/// `#[cfg(not(target_arch = "wasm32"))]`: only the three `#[ignore]`d
/// perf/smoke tests below call this, and they are themselves gated the same way
/// (see their own doc comments) -- gating the helper too keeps it from becoming
/// dead code under `--target wasm32-unknown-unknown`.
#[cfg(not(target_arch = "wasm32"))]
fn rbc_planes() -> Vec<(DVec3, f64)> {
    StandardGemCuts::standard_round_brilliant()
        .into_iter()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect()
}

/// The 103-tier "CrackOtto-Step" design (`indicatrix-cut-core`'s benchmark
/// fixture, PC 05.115), embedded via `include_str!` so there is exactly one
/// copy in the workspace. Every tier is genuinely meet-derived, so one
/// `ScaleReference` bootstrap per block (crown/pavilion/girdle) is added here,
/// reimplemented over `indicatrix::geometry::meet_solver` so this crate's test
/// does not need to depend on `indicatrix-cut-core`.
///
/// `#[cfg(not(target_arch = "wasm32"))]`: same reasoning as [`rbc_planes`] --
/// its only caller (`timing_crackotto_step_103_tier_800x600`) is gated the same
/// way.
#[cfg(not(target_arch = "wasm32"))]
fn crackotto_step_planes() -> Vec<(DVec3, f64)> {
    const TEXT: &str =
        include_str!("../../../indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc");
    let schedule = indicatrix_formats::asc::parse_asc(TEXT).expect("fixture must parse");
    let mut inputs = meet_solver::meet_tier_inputs_from_asc(&schedule);
    let blocks = meet_solver::classify_blocks(&inputs);
    for block in [
        meet_solver::Block::Crown,
        meet_solver::Block::Pavilion,
        meet_solver::Block::Girdle,
    ] {
        let anchored = inputs.iter().zip(&blocks).any(|(t, &b)| {
            b == block && matches!(t.constraint, meet_solver::MeetConstraint::ScaleReference(_))
        });
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint =
                meet_solver::MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }
    let normals = meet_solver::tier_instance_normals(schedule.gear_teeth_abs(), &inputs);
    let solved = meet_solver::solve_meet_points(schedule.gear_teeth_abs(), &inputs);
    normals
        .iter()
        .zip(solved.iter().map(|s| s.mast))
        .flat_map(|(ns, m)| ns.iter().map(move |&n| (n, m)))
        .collect()
}

/// Writes a PNG to the system temp dir -- `#[cfg(not(target_arch = "wasm32"))]`
/// (on top of `#[ignore]`) since this crate must contain no `std::fs` use at all,
/// even in a test that never runs on `wasm32-unknown-unknown` -- see the crate
/// README.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "writes a PNG to the system temp dir -- a visual smoke test, not a \
            correctness check; run explicitly with --ignored"]
fn renders_the_standard_round_brilliant_without_panicking() {
    let planes = rbc_planes();
    let mesh = match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("RBC-445 must close: {other:?}"),
    };
    let camera = Camera::new(0.6, 0.35, 3.0, 42.0);
    let style = SolidStyle::default();
    let mut rasterizer = SolidRasterizer::new(800, 600);
    rasterizer.render(&mesh, &camera, &style);

    let visible = rasterizer.pick.iter().filter(|&&p| p != 0).count();
    assert!(
        visible > 1000,
        "expected a substantial visible silhouette, got {visible} px"
    );

    let image = image::RgbaImage::from_raw(800, 600, rasterizer.color.clone())
        .expect("color buffer length must match 800x600 RGBA8");
    let path = std::env::temp_dir().join("indicatrix_cut_solid_preview_rbc.png");
    image
        .save(&path)
        .expect("PNG write to the temp dir must succeed");
    println!("wrote {}", path.display());
}

/// `std::time::Instant`-based perf measurement -- `#[cfg(not(target_arch =
/// "wasm32"))]` (on top of `#[ignore]`) since this crate must contain no
/// `Instant::now` at all, even in a test that never runs on
/// `wasm32-unknown-unknown` -- see the crate README.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "timing measurement, not a correctness check -- run with \
            --release --ignored --nocapture"]
fn timing_rbc_445_800x600() {
    let planes = rbc_planes();
    let mesh = match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("RBC-445 must close: {other:?}"),
    };
    let camera = Camera::new(0.6, 0.35, 3.0, 42.0);
    let style = SolidStyle::default();
    let mut rasterizer = SolidRasterizer::new(800, 600);
    rasterizer.render(&mesh, &camera, &style); // warm-up

    let iters = 200u32;
    let start = std::time::Instant::now();
    for _ in 0..iters {
        rasterizer.render(&mesh, &camera, &style);
    }
    let per_frame = start.elapsed() / iters;
    println!("RBC-445 800x600: {per_frame:?} per frame over {iters} iterations (target < 1 ms)");
}

/// `std::time::Instant`-based perf measurement -- `#[cfg(not(target_arch =
/// "wasm32"))]` (on top of `#[ignore]`), same reasoning as
/// [`timing_rbc_445_800x600`].
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "timing measurement, not a correctness check -- run with \
            --release --ignored --nocapture"]
fn timing_crackotto_step_103_tier_800x600() {
    let planes = crackotto_step_planes();
    let mesh = match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("CrackOtto-Step must close: {other:?}"),
    };
    let camera = Camera::new(0.6, 0.35, 3.0, 42.0);
    let style = SolidStyle::default();
    let mut rasterizer = SolidRasterizer::new(800, 600);
    rasterizer.render(&mesh, &camera, &style); // warm-up

    let iters = 100u32;
    let start = std::time::Instant::now();
    for _ in 0..iters {
        rasterizer.render(&mesh, &camera, &style);
    }
    let per_frame = start.elapsed() / iters;
    println!(
        "CrackOtto-Step (103 tiers) 800x600: {per_frame:?} per frame over {iters} \
         iterations (target < 5 ms)"
    );
}

/// Two rings of one facet id with opposite per-ring normals, side by side at `z = 0`
/// (camera at `+z`). The per-facet table (`mesh.normals`) says both face the camera,
/// as it would if it were read for a facet whose pieces differ.
fn two_pieces_one_facet_mesh(visible: bool) -> SolidMesh {
    let mut mesh = SolidMesh::default();
    let mut piece_normals = Vec::new();
    for (x0, x1, normal_z) in [(-1.0_f64, -0.2_f64, 1.0_f64), (0.2, 1.0, -1.0)] {
        let corners = [
            DVec3::new(x0, -0.5, 0.0),
            DVec3::new(x1, -0.5, 0.0),
            DVec3::new(x1, 0.5, 0.0),
            DVec3::new(x0, 0.5, 0.0),
        ];
        let base = mesh.positions.len() as u32;
        for c in corners {
            mesh.positions.push(c);
            mesh.normals.push(DVec3::Z);
            mesh.facet_id.push(0);
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        mesh.rings.push((0, corners.to_vec()));
        piece_normals.push(DVec3::new(0.0, 0.0, normal_z));
    }
    mesh.piece_normals = Some(piece_normals);
    mesh.edge_visible = Some(vec![vec![visible; 4]; 2]);
    mesh
}

#[test]
fn concave_pieces_are_culled_by_their_own_ring_normal_not_the_facet_table() {
    let mesh = two_pieces_one_facet_mesh(true);
    let mut rasterizer = SolidRasterizer::new(64, 64);
    rasterizer.render(&mesh, &two_quads_camera(), &SolidStyle::default());
    assert_eq!(
        rasterizer.pick_at(20, 32),
        Some(0),
        "the camera-facing piece paints"
    );
    assert_eq!(
        rasterizer.pick_at(44, 32),
        None,
        "the piece facing away is culled although the facet table says it faces the camera"
    );
}

#[test]
fn concave_prepared_render_matches_the_direct_render() {
    // The cached path must read the same per-ring normals and edge flags as the
    // direct one, even though the cache re-indexes rings after simplification.
    let mesh = two_pieces_one_facet_mesh(true);
    let camera = two_quads_camera();
    let style = SolidStyle::default();
    let mut direct = SolidRasterizer::new(64, 64);
    direct.render(&mesh, &camera, &style);
    let prepared = crate::mesh_cache::CachedMesh::build(mesh, 1);
    let mut via_cache = SolidRasterizer::new(64, 64);
    via_cache.render_prepared(&prepared, &camera, &style);
    assert_eq!(direct.color, via_cache.color);
    assert_eq!(direct.pick, via_cache.pick);
}

#[test]
fn edges_flagged_invisible_are_not_stroked() {
    let camera = two_quads_camera();
    let style = SolidStyle::default();
    let count_edge_pixels = |visible: bool| {
        let mut rasterizer = SolidRasterizer::new(64, 64);
        rasterizer.render(&two_pieces_one_facet_mesh(visible), &camera, &style);
        rasterizer
            .color
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[..3] == style.edge_color)
            .count()
    };
    assert!(count_edge_pixels(true) > 0, "visible edges are drawn");
    assert_eq!(count_edge_pixels(false), 0, "seam edges are skipped");
}

#[test]
fn simplified_edge_flags_merge_the_flags_of_dropped_collinear_points() {
    let ring = [
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(0.5, 0.0, 0.0),
        DVec3::new(1.0, 0.0, 0.0),
        DVec3::new(1.0, 1.0, 0.0),
        DVec3::new(0.0, 1.0, 0.0),
    ];
    let (mut dedup, mut simplified) = (Vec::new(), Vec::new());
    simplify_ring(&ring, &mut dedup, &mut simplified);
    assert_eq!(
        simplified.len(),
        4,
        "the midpoint of the bottom edge is dropped"
    );
    // Only the second half of the bottom edge is a real boundary.
    let flags = simplified_edge_flags(&ring, &simplified, &[false, true, false, false, false]);
    assert_eq!(flags, vec![true, false, false, false]);
    assert_eq!(
        simplified_edge_flags(&ring, &simplified, &[false; 5]),
        vec![false; 4]
    );
}
