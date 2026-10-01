//! Byte-identity pins for the solid renderer and 2D diagram, taken BEFORE the
//! `crates/indicatrix-solid` extraction (see the W1-b work-lane brief and
//! `wasm_web_app_development_plan_2026-09-27.md` §3.1 item 1: `raster`, `diagram2d`,
//! `facet_map`, `mesh_cache` move out of `apps/indicatrix-cut/src/gui/solid_preview`
//! into a shared crate, and the desktop re-exports them at their old paths).
//!
//! Every test below hashes (FNV-1a, the same convention `mesh_cache::hash_planes`
//! already uses internally) the RGBA/depth/pick buffers
//! `raster::SolidRasterizer::render`/`render_prepared` and
//! `diagram2d::render_diagram`/`render_diagram_single_panel` produce for fixed
//! designs/camera poses/sizes/styles, plus `facet_map::FacetMap::from_design`'s
//! build output (read back through its public accessors, since this is an
//! integration test crate and cannot reach a `pub(crate)` field). Only PUBLIC API
//! is used throughout, on purpose: this file's own module path never moves (only
//! the modules it calls through `indicatrix_cut::gui::solid_preview::*` do), so an
//! unchanged pass after the move is direct proof the move changed no behaviour.
//!
//! Three fixed designs:
//! - **CrackOtto-Step** (`indicatrix-cut-core`'s 103-tier probe fixture): the full
//!   pipeline (`Design::solve` -> `planes_from_solved` -> `build_solid_mesh` ->
//!   `MeshCache`/`SolidRasterizer` -> `diagram2d` -> `FacetMap::from_design`).
//! - **RBC-445** (`StandardGemCuts::standard_round_brilliant`): raw planes, no
//!   `Design`, exercised through the rasterizer and the diagram renderer (which
//!   only needs a `SolidMesh`, not a `Design`).
//! - **Box**: a minimal 6-plane cube, the cheapest possible closed solid, exercised
//!   through the rasterizer only.

use glam::Vec3;
use indicatrix::{
    geometry::{
        cuts::StandardGemCuts,
        meet_solver::{Block, MeetConstraint, classify_blocks, meet_tier_inputs_from_asc},
        stone_metrics::{SolidStatus, build_solid_mesh},
    },
    optics::raytracer::Camera,
};
use indicatrix_cut::gui::solid_preview::{
    diagram2d::{self, DiagramConfig, DiagramStyle, PanelKind},
    facet_map::FacetMap,
    mesh_cache::MeshCache,
    raster::{SolidRasterizer, SolidStyle},
};
use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};

/// FNV-1a over raw bytes -- the shared `mesh_cache::fnv1a_64` that `hash_planes` uses
/// internally (bit-pattern hashing, not `std::hash::Hash`, so the pin is exactly as
/// deterministic as that module's own cache key), applied here to any byte slice.
fn fnv1a(bytes: &[u8]) -> u64 {
    indicatrix_cut::gui::solid_preview::mesh_cache::fnv1a_64(bytes.iter().copied())
}

/// Folds several already-computed sub-hashes into one combined pin -- keeps this
/// file to one constant per test rather than one per buffer.
fn combine(hashes: &[u64]) -> u64 {
    let bytes: Vec<u8> = hashes.iter().flat_map(|h| h.to_le_bytes()).collect();
    fnv1a(&bytes)
}

fn hash_f32_slice(values: &[f32]) -> u64 {
    let bytes: Vec<u8> = values
        .iter()
        .flat_map(|v| v.to_bits().to_le_bytes())
        .collect();
    fnv1a(&bytes)
}

fn hash_u32_slice(values: &[u32]) -> u64 {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    fnv1a(&bytes)
}

/// A [`SolidRasterizer`] frame's RGBA + depth + pick buffers, combined into one hash.
fn hash_rasterizer(r: &SolidRasterizer) -> u64 {
    combine(&[
        fnv1a(&r.color),
        hash_f32_slice(&r.depth),
        hash_u32_slice(&r.pick),
    ])
}

/// A [`diagram2d::DiagramFrame`]'s RGBA + per-pixel pick/panel/tooth readback,
/// combined into one hash. `pick_at`/`panel_at`/`tooth_at` are the only way an
/// external crate (this integration test) can read those buffers -- see the
/// module doc comment.
fn hash_diagram_frame(frame: &diagram2d::DiagramFrame) -> u64 {
    let mut pick = Vec::with_capacity((frame.width * frame.height) as usize);
    let mut panel = Vec::with_capacity((frame.width * frame.height) as usize);
    let mut tooth = Vec::with_capacity((frame.width * frame.height) as usize);
    for y in 0..frame.height {
        for x in 0..frame.width {
            pick.push(frame.pick_at(x, y).map_or(0, |v| v + 1));
            panel.push(match frame.panel_at(x, y) {
                None => 0u32,
                Some(PanelKind::Crown) => 1,
                Some(PanelKind::Pavilion) => 2,
                Some(PanelKind::Profile) => 3,
            });
            tooth.push(frame.tooth_at(x, y).map_or(0, |v| v + 1));
        }
    }
    combine(&[
        fnv1a(&frame.color),
        hash_u32_slice(&pick),
        hash_u32_slice(&panel),
        hash_u32_slice(&tooth),
    ])
}

fn closed_mesh(planes: &[(Vec3, f32)]) -> indicatrix::geometry::stone_metrics::SolidMesh {
    let widened: Vec<(glam::DVec3, f64)> = planes
        .iter()
        .map(|&(n, m)| {
            (
                glam::DVec3::new(f64::from(n.x), f64::from(n.y), f64::from(n.z)),
                f64::from(m),
            )
        })
        .collect();
    match build_solid_mesh(&widened) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("fixture must close: {other:?}"),
    }
}

fn rbc_planes() -> Vec<(Vec3, f32)> {
    StandardGemCuts::standard_round_brilliant()
        .into_iter()
        .map(|plane| (Vec3::from(plane.normal), -plane.d))
        .collect()
}

fn box_planes() -> Vec<(Vec3, f32)> {
    vec![
        (Vec3::X, 1.0),
        (Vec3::NEG_X, 1.0),
        (Vec3::Y, 0.6),
        (Vec3::NEG_Y, 0.6),
        (Vec3::Z, 1.0),
        (Vec3::NEG_Z, 1.0),
    ]
}

/// `indicatrix-cut-core`'s "CrackOtto-Step" fixture (PC 05.115, 103 tiers), built as
/// a real [`Design`] -- mirrors `solid_preview::live_update`'s own test fixture of
/// the same name (`crackotto_step_design`), duplicated here rather than shared so
/// this pin file depends on nothing this campaign is about to move.
const CRACKOTTO_STEP_ASC: &str =
    include_str!("../../../crates/indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc");

fn crackotto_step_design() -> Design {
    let schedule =
        indicatrix_formats::asc::parse_asc(CRACKOTTO_STEP_ASC).expect("fixture must parse");
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

fn pin_camera() -> Camera {
    Camera::new(0.6, 0.35, 3.0, 42.0)
}

const PIN_WIDTH: u32 = 200;
const PIN_HEIGHT: u32 = 150;

#[test]
fn solid_identity_pin_crackotto_step_full_pipeline() {
    let design = crackotto_step_design();
    assert_eq!(
        design.tiers.len(),
        103,
        "fixture must have its real tier count"
    );
    let solved = design.solve().expect("fixture must solve");
    let planes_f64 = design.planes_from_solved(&solved);
    let planes: Vec<(Vec3, f32)> = planes_f64
        .into_iter()
        .map(|(n, m)| (Vec3::new(n.x as f32, n.y as f32, n.z as f32), m as f32))
        .collect();

    let mut cache = MeshCache::default();
    let prepared = cache.get_or_build(&planes).expect("must close");
    let style = SolidStyle::default();

    let mut plain = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    plain.render(&prepared.mesh, &pin_camera(), &style);
    let plain_hash = hash_rasterizer(&plain);

    let mut fast = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    fast.render_prepared(prepared, &pin_camera(), &style);
    let prepared_hash = hash_rasterizer(&fast);

    let diagram_config = DiagramConfig {
        width: PIN_WIDTH,
        height: PIN_HEIGHT,
        gear_teeth: design.meta.gear_teeth_abs(),
        gear_reference_angle: design.meta.gear_reference_angle as f32,
        symmetry_order: design.meta.symmetry_order,
        mirror: design.meta.mirror,
    };
    let diagram_style = DiagramStyle::default();
    let three_panel = diagram2d::render_diagram(&prepared.mesh, &diagram_config, &diagram_style);
    let three_panel_hash = hash_diagram_frame(&three_panel);
    let single_panel = diagram2d::render_diagram_single_panel(
        &prepared.mesh,
        &diagram_config,
        &diagram_style,
        PanelKind::Pavilion,
    );
    let single_panel_hash = hash_diagram_frame(&single_panel);

    let facet_map = FacetMap::from_design(&design, &solved);
    let mut facet_map_bytes: Vec<u8> = Vec::new();
    facet_map_bytes.extend_from_slice(&(facet_map.facet_count() as u64).to_le_bytes());
    facet_map_bytes.extend_from_slice(&(facet_map.preform_plane_count() as u64).to_le_bytes());
    for facet_id in 0..facet_map.facet_count() {
        facet_map_bytes.extend_from_slice(
            &facet_map
                .tier_of(facet_id)
                .map_or(u64::MAX, |t| t as u64)
                .to_le_bytes(),
        );
        facet_map_bytes.extend_from_slice(&facet_map.index_on_gear(facet_id).to_le_bytes());
        facet_map_bytes.extend_from_slice(facet_map.facet_label(facet_id).as_bytes());
    }
    let facet_map_hash = fnv1a(&facet_map_bytes);

    let combined = combine(&[
        plain_hash,
        prepared_hash,
        three_panel_hash,
        single_panel_hash,
        facet_map_hash,
    ]);
    assert_eq!(
        combined, 0x185a_50a0_ea1a_363b,
        "CrackOtto-Step solid/diagram/facet_map pipeline hash changed -- \
         plain={plain_hash:#x} prepared={prepared_hash:#x} three_panel={three_panel_hash:#x} \
         single_panel={single_panel_hash:#x} facet_map={facet_map_hash:#x}"
    );
}

#[test]
fn solid_identity_pin_rbc_445() {
    let planes = rbc_planes();
    let mesh = closed_mesh(&planes);
    let style = SolidStyle::default();

    let mut cache = MeshCache::default();
    let prepared = cache.get_or_build(&planes).expect("RBC-445 must close");

    let mut plain = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    plain.render(&mesh, &pin_camera(), &style);
    let plain_hash = hash_rasterizer(&plain);

    let mut fast = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    fast.render_prepared(prepared, &pin_camera(), &style);
    let prepared_hash = hash_rasterizer(&fast);

    let diagram_config = DiagramConfig {
        width: PIN_WIDTH,
        height: PIN_HEIGHT,
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        mirror: false,
    };
    let diagram_style = DiagramStyle::default();
    let three_panel = diagram2d::render_diagram(&mesh, &diagram_config, &diagram_style);
    let three_panel_hash = hash_diagram_frame(&three_panel);

    let combined = combine(&[plain_hash, prepared_hash, three_panel_hash]);
    assert_eq!(
        combined, 0x3a3d_abe7_d3e9_4c87,
        "RBC-445 solid/diagram hash changed -- plain={plain_hash:#x} \
         prepared={prepared_hash:#x} three_panel={three_panel_hash:#x}"
    );
}

#[test]
fn solid_identity_pin_box() {
    let planes = box_planes();
    let mesh = closed_mesh(&planes);
    let style = SolidStyle::default();
    let camera = Camera::new(0.4, 0.3, 4.0, 40.0);

    let mut plain = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    plain.render(&mesh, &camera, &style);
    let plain_hash = hash_rasterizer(&plain);

    let mut cache = MeshCache::default();
    let prepared = cache.get_or_build(&planes).expect("box must close");
    let mut fast = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    fast.render_prepared(prepared, &camera, &style);
    let prepared_hash = hash_rasterizer(&fast);

    let combined = combine(&[plain_hash, prepared_hash]);
    assert_eq!(
        combined, 0xdd3f_cd8d_48f7_3025,
        "Box solid hash changed -- plain={plain_hash:#x} prepared={prepared_hash:#x}"
    );
}
