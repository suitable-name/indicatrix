//! Tests of the scenes: world frame, stone placement, cut planes and tooltips.

use super::*;
use crate::gui::rough_plan::view::design_mesh::DesignFacet;
use indicatrix_cut_core::rough_plan::{
    Axis, BarCut, BoxFace, CutOrder, CutPlan, RoughCut, SlabCut,
};

/// A cube design of side `side`, centred.
fn cube(side: f64) -> Arc<DesignMesh> {
    let faces = box_faces(DVec3::splat(-side / 2.0), DVec3::splat(side / 2.0));
    Arc::new(DesignMesh {
        facets: faces
            .iter()
            .map(|(normal, ring)| DesignFacet {
                normal: *normal,
                ring: ring.to_vec(),
            })
            .collect(),
    })
}

/// A unit cube design (side 1, centred).
fn unit_cube() -> Arc<DesignMesh> {
    cube(1.0)
}

/// Meshes by entry id; the ids in `unreadable` exist but cannot be read, any other
/// id that is missing is gone.
struct Stub {
    meshes: BTreeMap<i64, Arc<DesignMesh>>,
    unreadable: Vec<i64>,
}

impl Stub {
    fn of(meshes: BTreeMap<i64, Arc<DesignMesh>>) -> Self {
        Self {
            meshes,
            unreadable: Vec::new(),
        }
    }
}

impl MeshSource for Stub {
    fn mesh(&self, entry_id: i64) -> Result<Arc<DesignMesh>, MeshMiss> {
        if self.unreadable.contains(&entry_id) {
            return Err(MeshMiss::Unreadable);
        }
        self.meshes.get(&entry_id).cloned().ok_or(MeshMiss::Gone)
    }
}

fn pose(centre: [f64; 3]) -> StonePose {
    StonePose {
        center_mm: centre,
        axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        mm_per_unit: 2.0,
    }
}

fn stone(entry_id: i64, centre: [f64; 3]) -> PlacedStone {
    PlacedStone {
        entry_id,
        piece_origin_mm: [centre[0] - 1.5, centre[1] - 1.5, centre[2] - 1.5],
        piece_size_mm: [3.0; 3],
        stone_size_mm: [2.0; 3],
        table_axis: Axis::Y,
        carat: 0.62,
        volume_mm3: 8.0,
        pose: pose(centre),
    }
}

fn layout(stones: Vec<PlacedStone>, pieces_per_slab: &[usize]) -> RoughLayout {
    let slabs = pieces_per_slab
        .iter()
        .map(|&pieces| SlabCut {
            thickness_mm: 3.0,
            bars: vec![BarCut {
                width_mm: 3.0,
                pieces_mm: vec![3.0; pieces],
            }],
        })
        .collect();
    RoughLayout {
        cut_order: CutOrder::Xyz,
        stones,
        cut_plan: CutPlan { slabs },
        total_carat: 1.0,
        total_volume_mm3: 24.0,
        yield_fraction: 0.5,
        exact_fit: false,
    }
}

fn block(x: f64, y: f64, z: f64) -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: x,
            y_mm: y,
            z_mm: z,
        },
        Vec::new(),
    )
}

fn radius_of(mesh: &SolidMesh) -> f64 {
    mesh.rings
        .iter()
        .flat_map(|(_, ring)| ring.iter())
        .map(|p| p.length())
        .fold(0.0, f64::max)
}

#[test]
fn the_world_frame_puts_the_box_centre_at_the_origin_and_its_corner_at_radius_one() {
    let base = RoughBase::Block {
        x_mm: 6.0,
        y_mm: 8.0,
        z_mm: 10.0,
    };
    let frame = WorldFrame::for_base(&base);
    assert!(frame.point(DVec3::new(3.0, 4.0, 5.0)).length() < 1e-12);
    assert!((frame.point(DVec3::ZERO).length() - 1.0).abs() < 1e-12);
    assert!((frame.point(DVec3::new(6.0, 8.0, 10.0)).length() - 1.0).abs() < 1e-12);
}

#[test]
fn a_unit_cube_is_moved_scaled_and_turned_by_its_pose() {
    // Local x maps to -z, local z to +x, local y stays up; 2 mm per unit, so the cube
    // is 2 mm wide; the frame maps 2 mm to one world unit.
    let pose = StonePose {
        center_mm: [5.0, 5.0, 5.0],
        axes: [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
        mm_per_unit: 2.0,
    };
    let frame = WorldFrame {
        centre: DVec3::new(5.0, 5.0, 5.0),
        scale: 0.5,
    };
    let cube = unit_cube();
    let plus_x = &cube.facets[0];
    assert_eq!(plus_x.normal, DVec3::X);
    assert_eq!(normal_to_world(plus_x.normal, &pose), DVec3::NEG_Z);
    for &corner in &plus_x.ring {
        let world = point_to_world(corner, &pose, &frame);
        assert!((world.z + 0.5).abs() < 1e-12, "{world:?}");
        assert!((world.x.abs() - 0.5).abs() < 1e-12, "{world:?}");
        assert!((world.y.abs() - 0.5).abs() < 1e-12, "{world:?}");
    }
}

#[test]
fn stones_take_the_group_number_of_their_design_row() {
    let stones = vec![
        stone(9, [0.0; 3]),
        stone(4, [3.0, 0.0, 0.0]),
        stone(9, [6.0, 0.0, 0.0]),
        stone(2, [9.0, 0.0, 0.0]),
    ];
    let layout = layout(stones, &[4]);
    let (titles, mesh_ids) = (BTreeMap::new(), BTreeMap::new());
    let meshes = Stub::of(BTreeMap::from([
        (9, unit_cube()),
        (4, unit_cube()),
        (2, unit_cube()),
    ]));
    let scene = build_fit_scene(
        &FitInputs {
            layout: &layout,
            rough: &RoughMesh::new(block(12.0, 3.0, 3.0)),
            titles: &titles,
            mesh_ids: &mesh_ids,
        },
        &meshes,
    );
    // Two stones of design 9 make the first row, then the single stones by id: 2, 4.
    assert_eq!(scene.stone_group, vec![0, 2, 0, 1]);
    assert_eq!(scene.facet_colors[0], PALETTE[0]);
    assert_eq!(scene.facet_colors[6], PALETTE[2]);
    assert_eq!(scene.facet_colors[18], PALETTE[1]);
}

#[test]
fn a_stone_hint_names_the_slab_bar_and_piece_of_the_cut_plan() {
    // Slab 1 has two bars (2 and 1 pieces), slab 2 one bar with one piece, so the
    // four stones come from (slab, bar, piece) = (1,1,1), (1,1,2), (1,2,1), (2,1,1).
    let stones: Vec<PlacedStone> = (0..4).map(|i| stone(1, [f64::from(i) * 3.0; 3])).collect();
    let mut layout = layout(stones, &[]);
    let bar = |pieces: usize| BarCut {
        width_mm: 3.0,
        pieces_mm: vec![3.0; pieces],
    };
    layout.cut_plan = CutPlan {
        slabs: vec![
            SlabCut {
                thickness_mm: 3.0,
                bars: vec![bar(2), bar(1)],
            },
            SlabCut {
                thickness_mm: 3.0,
                bars: vec![bar(1)],
            },
        ],
    };
    let titles = BTreeMap::from([(1, "Barion Oval".to_string())]);
    let scene = build_fit_scene(
        &FitInputs {
            layout: &layout,
            rough: &RoughMesh::new(block(12.0, 3.0, 3.0)),
            titles: &titles,
            mesh_ids: &BTreeMap::new(),
        },
        &Stub::of(BTreeMap::from([(1, unit_cube())])),
    );
    let positions: Vec<Option<PiecePosition>> =
        scene.info.iter().map(|info| info.position).collect();
    let at = |slab, bar, piece| Some(PiecePosition { slab, bar, piece });
    assert_eq!(
        positions,
        vec![at(1, 1, 1), at(1, 1, 2), at(1, 2, 1), at(2, 1, 1)]
    );
    assert_eq!(
        scene.info[2].hint(),
        "Barion Oval \u{00B7} 0.62 ct \u{00B7} slab 1, bar 2, piece 1"
    );
}

#[test]
fn a_layout_without_a_matching_cut_plan_gets_no_invented_position() {
    // Three stones but a plan of two pieces: the positions are unknown.
    let stones = vec![stone(1, [0.0; 3]), stone(1, [3.0; 3]), stone(1, [6.0; 3])];
    let layout = layout(stones, &[2]);
    let scene = build_fit_scene(
        &FitInputs {
            layout: &layout,
            rough: &RoughMesh::new(block(12.0, 3.0, 3.0)),
            titles: &BTreeMap::new(),
            mesh_ids: &BTreeMap::new(),
        },
        &Stub::of(BTreeMap::from([(1, unit_cube())])),
    );
    assert!(scene.info.iter().all(|info| info.position.is_none()));
    assert_eq!(scene.info[0].hint(), "Design #1 \u{00B7} 0.62 ct");
}

#[test]
fn a_fit_scene_merges_the_stones_with_offset_facet_ids_and_group_colours() {
    let stones = vec![
        stone(1, [1.5, 1.5, 1.5]),
        stone(1, [4.5, 1.5, 1.5]),
        stone(2, [7.5, 1.5, 1.5]),
    ];
    let layout = layout(stones, &[2, 1]);
    let rough = RoughMesh::new(block(9.0, 3.0, 3.0));
    let titles = BTreeMap::from([(1, "Barion Oval".to_string())]);
    // Design 2 is gone: its stone becomes a grey box.
    let mesh_ids = BTreeMap::from([(1, StoneDraw::Design(1)), (2, StoneDraw::Deleted)]);
    let meshes = Stub::of(BTreeMap::from([(1, unit_cube())]));
    let scene = build_fit_scene(
        &FitInputs {
            layout: &layout,
            rough: &rough,
            titles: &titles,
            mesh_ids: &mesh_ids,
        },
        &meshes,
    );

    assert_eq!(scene.stone_starts, vec![0, 6, 12, 18]);
    assert_eq!(scene.stones.rings.len(), 18);
    assert_eq!(scene.facet_colors.len(), 18);
    assert_eq!(scene.facet_colors[0], PALETTE[0]);
    assert_eq!(scene.facet_colors[11], PALETTE[0]);
    assert_eq!(scene.facet_colors[12], DELETED_GREY);
    assert_eq!(scene.stone_group, vec![0, 0, 1]);
    assert_eq!(scene.stone_at_facet(0), Some(0));
    assert_eq!(scene.stone_at_facet(7), Some(1));
    assert_eq!(scene.stone_at_facet(17), Some(2));
    assert_eq!(scene.stone_at_facet(18), None);
    assert_eq!(scene.facets_of_group(0).iter().filter(|&&f| f).count(), 12);
    // 3 pieces of 6 faces, and the rough of the block.
    assert_eq!(scene.saw.rings.len(), 18);
    assert_eq!(scene.rough.rings.len(), 6);
    assert!(radius_of(&scene.rough) <= 1.0 + 1e-9);
    assert_eq!(scene.info[2].title, "Design #2");
    assert_eq!(
        scene.info[1].hint(),
        "Barion Oval \u{00B7} 0.62 ct \u{00B7} slab 1, bar 1, piece 2"
    );
    assert_eq!(
        scene.info[2].position,
        Some(PiecePosition {
            slab: 2,
            bar: 1,
            piece: 1
        })
    );
    assert_eq!(
        scene.info[2].hint(),
        "Design #2 \u{00B7} 0.62 ct \u{00B7} slab 2, bar 1, piece 1 \u{00B7} design deleted"
    );
}

#[test]
fn a_stone_of_a_matched_design_draws_the_resolved_mesh() {
    let layout = layout(vec![stone(5, [1.5; 3])], &[1]);
    let rough = RoughMesh::new(block(3.0, 3.0, 3.0));
    let titles = BTreeMap::new();
    let mesh_ids = BTreeMap::from([(5, StoneDraw::Matched(77))]);
    let meshes = Stub::of(BTreeMap::from([(77, unit_cube())]));
    let scene = build_fit_scene(
        &FitInputs {
            layout: &layout,
            rough: &rough,
            titles: &titles,
            mesh_ids: &mesh_ids,
        },
        &meshes,
    );
    assert_eq!(
        scene.facet_colors[0], PALETTE[0],
        "a real design, not the grey box"
    );
    assert!(scene.info[0].note.starts_with(NOTE_MATCHED));
}

/// The x extent of the stone drawn for the first stone of `scene`, in world units.
fn drawn_width(scene: &FitScene) -> f64 {
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    for (_, ring) in &scene.stones.rings {
        for point in ring {
            low = low.min(point.x);
            high = high.max(point.x);
        }
    }
    high - low
}

#[test]
fn a_changed_design_is_drawn_at_the_width_the_plan_recorded() {
    // The plan cut a 2 mm wide stone (unit cube at 2 mm per unit). The design has since
    // become a cube of side 4: at the old scale it would be 8 mm wide, scaled to the
    // recorded width (0.5 mm per unit) it is 2 mm wide. The block is 3 mm, whose
    // world scale is 1 / (sqrt(27) / 2).
    let layout = layout(vec![stone(5, [1.5; 3])], &[1]);
    let rough = RoughMesh::new(block(3.0, 3.0, 3.0));
    let scale = 2.0 / 27.0_f64.sqrt();
    let meshes = Stub::of(BTreeMap::from([(5, cube(4.0))]));
    let draw = |draw: StoneDraw| {
        build_fit_scene(
            &FitInputs {
                layout: &layout,
                rough: &rough,
                titles: &BTreeMap::new(),
                mesh_ids: &BTreeMap::from([(5, draw)]),
            },
            &meshes,
        )
    };
    let changed = draw(StoneDraw::Changed(5));
    assert!(2.0_f64.mul_add(-scale, drawn_width(&changed)).abs() < 1e-9);
    assert_eq!(
        changed.info[0].note,
        "design changed since saved, drawn at the saved width"
    );
    let as_planned = draw(StoneDraw::Design(5));
    assert!(8.0_f64.mul_add(-scale, drawn_width(&as_planned)).abs() < 1e-9);
    assert_eq!(as_planned.info[0].note, "");
}

#[test]
fn a_pose_turned_off_the_axes_keeps_the_plans_scale() {
    let mut turned = stone(5, [1.5; 3]);
    turned.pose.axes[0] = [0.6, 0.8, 0.0];
    assert_eq!(recorded_width_mm(&turned), None);
    let (pose, scaled) = rescaled_pose(&turned, 4.0);
    assert!(!scaled);
    assert!((pose.mm_per_unit - 2.0).abs() < f64::EPSILON);
    // On the axes the width is the recorded size along the width axis.
    let mut wide = stone(5, [1.5; 3]);
    wide.stone_size_mm = [2.0, 3.0, 5.0];
    wide.pose.axes[0] = [0.0, 0.0, 1.0];
    assert_eq!(recorded_width_mm(&wide), Some(5.0));
    let (pose, scaled) = rescaled_pose(&wide, 10.0);
    assert!(scaled && (pose.mm_per_unit - 0.5).abs() < 1e-12);
    // A mesh without width cannot be scaled.
    assert!(!rescaled_pose(&wide, 0.0).1);
}

#[test]
fn a_design_that_cannot_be_loaded_is_told_apart_from_a_deleted_one() {
    let layout = layout(vec![stone(5, [1.5; 3]), stone(6, [4.5; 3])], &[2]);
    let rough = RoughMesh::new(block(9.0, 3.0, 3.0));
    let meshes = Stub {
        meshes: BTreeMap::new(),
        unreadable: vec![5],
    };
    let scene = build_fit_scene(
        &FitInputs {
            layout: &layout,
            rough: &rough,
            titles: &BTreeMap::new(),
            mesh_ids: &BTreeMap::new(),
        },
        &meshes,
    );
    // Design 5 exists but failed to load; design 6 is gone.
    assert_eq!(scene.facet_colors[0], UNREADABLE_TINT);
    assert_eq!(scene.facet_colors[6], DELETED_GREY);
    assert_eq!(scene.info[0].note, "design could not be loaded");
    assert_eq!(scene.info[1].note, "design deleted");
}

#[test]
fn a_model_scene_knows_its_cut_faces_and_stays_inside_the_unit_sphere() {
    let mut model = block(10.0, 8.0, 6.0);
    model.cuts.push(RoughCut::Corner {
        faces: [BoxFace::Top, BoxFace::Front, BoxFace::Right],
        setbacks_mm: [3.0, 3.0, 3.0],
    });
    let centred = centred_mesh(&model).expect("valid model");
    let scene = ModelScene::new(&centred, &model);
    assert_eq!(scene.base_facets, 6);
    assert_eq!(scene.cut_of_facet(6), Some(0));
    assert_eq!(scene.cut_of_facet(5), None);
    assert_eq!(scene.cut_of_facet(7), None);
    assert!(scene.block);
    // The one cut is a half-space of the world frame that takes the uncut box's
    // Top-Front-Right corner away and keeps the centre.
    assert_eq!(scene.cut_planes.len(), 1);
    let (plane_normal, plane_offset) = scene.cut_planes[0];
    assert!(plane_normal.dot(Vec3::from(scene.half_extents)) > plane_offset);
    assert!(plane_offset > 0.0);
    assert!(radius_of(&scene.mesh) <= 1.0 + 1e-9);
    // The corner cut has a face, and its normal leans towards +x, +y, +z.
    let normal = scene.facet_normal(6).expect("the cut face is in the mesh");
    assert!(
        normal.x > 0.0 && normal.y > 0.0 && normal.z > 0.0,
        "{normal:?}"
    );
    // The base box is 10 x 8 x 6 mm: its half diagonal is sqrt(200) / 2.
    let half_diagonal = 200.0_f64.sqrt() / 2.0;
    assert!((f64::from(scene.half_extents[0]) - 5.0 / half_diagonal).abs() < 1e-6);
    let colours = scene.facet_colors(true);
    assert_eq!(colours.len(), 7);
    assert_eq!(colours[0], BASE_GREY);
    assert_eq!(colours[6], CUT_TINT);
    assert_eq!(scene.facet_colors(false)[6], BASE_GREY);
}
