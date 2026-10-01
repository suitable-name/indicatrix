//! Plane-parallel uniaxial slab, e-mode forced. Drives
//! `compute_bounce_refraction_geometry`/`apply_partial_fresnel_bounce` directly
//! (bypassing the full stochastic bounce loop) at exactly two facets -- the slab's
//! entry and exit faces -- so the resulting `k`/`S` at each step can be inspected.

use super::super::{
    super::{
        NUM_CHANNELS,
        camera::Ray,
        environment::{EnvironmentSource, LightingPreset},
        intersect::{build_plane_soa, intersect_polyhedron},
        refraction::{
            BounceContext, BounceRay, BounceState, ExitEvent, ExitSplitCtx, PathModeState, RngDraw,
            apply_partial_fresnel_bounce, build_ray_wavelength_cache,
            compute_bounce_refraction_geometry,
        },
    },
    inner::build_ray_material_context,
};
use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::{CrystalSystem, GemMaterial},
        polarization::StokesVector,
    },
};
use glam::Vec3;

/// Two real slab faces (`+-Y`, `y` in `[-HALF_THICKNESS, HALF_THICKNESS]`) plus
/// four far blank planes so `intersect_polyhedron` sees a bounded polyhedron --
/// the measured walk-off displacement never reaches them.
const HALF_THICKNESS: f32 = 1.0;
fn slab_planes() -> Vec<GpuFacetPlane> {
    vec![
        GpuFacetPlane::new(Vec3::Y, -HALF_THICKNESS),
        GpuFacetPlane::new(Vec3::NEG_Y, -HALF_THICKNESS),
        GpuFacetPlane::new(Vec3::X, -1000.0),
        GpuFacetPlane::new(Vec3::NEG_X, -1000.0),
        GpuFacetPlane::new(Vec3::Z, -1000.0),
        GpuFacetPlane::new(Vec3::NEG_Z, -1000.0),
    ]
}

/// Drives the entry facet (bounce 0) and exit facet (bounce 1) directly via
/// [`compute_bounce_refraction_geometry`]/[`apply_partial_fresnel_bounce`],
/// searching `rng_seed` for the first value that selects the extraordinary
/// eigenmode at entry and transmits at both facets. Returns `(entry_k, entry_s,
/// exit_k, exit_s, lateral_displacement)`.
///
/// # Panics
///
/// Panics if no seed in `0..SEED_SEARCH_LIMIT` satisfies every condition.
#[expect(
    clippy::too_many_lines,
    reason = "the fixed (enabled: false) ExitSplitCtx both \
              apply_partial_fresnel_bounce calls require; the search loop body \
              itself is unchanged"
)]
fn trace_forced_extraordinary_slab(
    material: &GemMaterial,
    incident_origin: Vec3,
    incident_dir: Vec3,
) -> (Vec3, Vec3, Vec3, Vec3, f32) {
    const SEED_SEARCH_LIMIT: u32 = 20_000;
    let planes = slab_planes();
    let lambdas = [589.3f32; NUM_CHANNELS]; // sodium D line, every channel (direction-only test)
    let mat_ctx = build_ray_material_context(material, lambdas, 0, false);
    let cache = build_ray_wavelength_cache(&mat_ctx);

    // The entry hit point depends only on the fixed incident ray/geometry, not on
    // `seed` -- computed once, outside the search loop.
    let entry_hit = intersect_polyhedron(
        Ray {
            origin: incident_origin,
            dir: incident_dir,
        },
        &planes,
    )
    .expect("incident ray must hit the slab's top face");
    let entry_point = incident_origin + entry_hit.t * incident_dir;

    // This test's assertions only read k1/s1/k2/s2 (direction, unaffected by
    // splitting) -- `enabled: false` keeps every other side effect out of the way,
    // avoiding any dependence on this synthetic slab's `EnvironmentSource` choice.
    let plane_soa = build_plane_soa(&planes);
    let mut split_radiance = [0.0f32; NUM_CHANNELS];
    let mut exit_ctx = ExitSplitCtx {
        plane_soa: &plane_soa,
        environment: EnvironmentSource::Studio {
            preset: LightingPreset::RingLights,
            exposure: 1.0,
            light_yaw: 0.0,
            light_pitch: 0.85,
            backdrop: 0.0,
        },
        studio_rig: None,
        observer: Vec3::ZERO,
        split_radiance: &mut split_radiance,
        enabled: false,
        compat: [u8::MAX; NUM_CHANNELS],
        split_mis_weight: 1.0,
    };

    for seed in 0..SEED_SEARCH_LIMIT {
        let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
        let mut path_pdf = [1.0f32; NUM_CHANNELS];

        // Entry facet: normal +Y (top of the slab), current_k == current_ray.dir ==
        // incident_dir (air, isotropic -- k == S trivially before any interface).
        let entry_normal = Vec3::Y;
        let geo0 = compute_bounce_refraction_geometry(
            &mat_ctx,
            &cache,
            entry_normal,
            incident_dir,
            false,
            false,
        );
        if geo0.sin2_t > 1.0 {
            continue; // TIR is geometrically unreachable entering from air (n1==1), but stay defensive.
        }
        // `stokes` is unpolarized here, so `entry_eigenmode_selection` returns
        // `None` regardless -- computed properly anyway for clarity.
        let entry_plane_normal = incident_dir.cross(entry_normal).normalize_or_zero();
        let bctx0 = BounceContext {
            ctx: &mat_ctx,
            cache: &cache,
            geo: &geo0,
        };
        let mut state0 = BounceState {
            stokes: &mut stokes,
            path_pdf: &mut path_pdf,
        };
        let mut exit_event0 = ExitEvent {
            exit: &mut exit_ctx,
            hit_point: entry_point,
        };
        let (k1, s1, inside_gem_1, is_extraordinary_update, _) = apply_partial_fresnel_bounce(
            &bctx0,
            BounceRay {
                k_hat: incident_dir,
                normal: entry_normal,
            },
            PathModeState {
                current_plane_normal: entry_plane_normal,
                inside_gem: false,
                is_extraordinary: false,
            },
            RngDraw {
                rng_seed: seed,
                bounce: 0,
            },
            &mut state0,
            &mut exit_event0,
        );
        let entered_extraordinary = is_extraordinary_update == Some(true);
        if !inside_gem_1 || !entered_extraordinary {
            continue; // reflected off the top face, or entered the ORDINARY mode -- keep searching.
        }

        // Advance geometrically along S (k1/s1 as computed) to the exit facet.
        let post_entry_origin = entry_point + s1 * 1e-4;
        let exit_ray = Ray {
            origin: post_entry_origin,
            dir: s1,
        };
        let Some(exit_hit) = intersect_polyhedron(exit_ray, &planes) else {
            continue;
        };
        let exit_point = post_entry_origin + exit_hit.t * s1;
        // `intersect_polyhedron`'s hit normal is outward-facing; flip it to match
        // `apply_interior_segment`'s convention.
        let exit_normal = -exit_hit.normal;

        let geo1 =
            compute_bounce_refraction_geometry(&mat_ctx, &cache, exit_normal, k1, true, true);
        if geo1.sin2_t > 1.0 {
            continue; // Would TIR back into the slab -- keep searching for a seed that transmits.
        }
        // Exiting the crystal (`inside_gem == true`), so `entering_anisotropic`
        // is false at this call regardless -- this value is never consulted.
        let exit_plane_normal = k1.cross(exit_normal).normalize_or_zero();
        let bctx1 = BounceContext {
            ctx: &mat_ctx,
            cache: &cache,
            geo: &geo1,
        };
        let mut state1 = BounceState {
            stokes: &mut stokes,
            path_pdf: &mut path_pdf,
        };
        let mut exit_event1 = ExitEvent {
            exit: &mut exit_ctx,
            hit_point: exit_point,
        };
        let (k2, s2, inside_gem_2, _, _) = apply_partial_fresnel_bounce(
            &bctx1,
            BounceRay {
                k_hat: k1,
                normal: exit_normal,
            },
            PathModeState {
                current_plane_normal: exit_plane_normal,
                inside_gem: true,
                is_extraordinary: true,
            },
            RngDraw {
                rng_seed: seed,
                bounce: 1,
            },
            &mut state1,
            &mut exit_event1,
        );
        if inside_gem_2 {
            continue; // Reflected back into the slab instead of transmitting out -- keep searching.
        }

        // Lateral displacement: perpendicular distance from exit_point to the
        // infinite line through entry_point along incident_dir.
        let to_exit = exit_point - entry_point;
        let along = to_exit.dot(incident_dir);
        let perp = to_exit - along * incident_dir;
        let lateral_displacement = perp.length();

        return (k1, s1, k2, s2, lateral_displacement);
    }
    panic!(
        "no seed in 0..{SEED_SEARCH_LIMIT} entered the extraordinary mode and \
         transmitted cleanly through both slab faces -- test premise violated"
    );
}

/// The decisive correctness check: for a plane-parallel uniaxial slab with a
/// tilted c-axis, the extraordinary ray's exit wave normal `k` (and `S == k` once
/// back in air) must come out parallel to the incident ray within 1e-5 -- the
/// classical "parallel slab" Snell's-law result applied twice to the same `k` (see
/// `refraction.rs`'s design note, rule 4). The exit point must also be laterally
/// displaced by a nonzero amount -- the walk-off did something real, it just
/// didn't change the outgoing direction.
#[test]
fn plane_parallel_uniaxial_slab_extraordinary_ray_exits_parallel_and_displaced() {
    let mut material = GemMaterial::by_name("Zircon")
        .expect("\"Zircon\" is a built-in uniaxial material in GemMaterial::all_materials()");
    assert_eq!(material.crystal_system, CrystalSystem::Tetragonal);
    assert!(
        material.birefringence_delta.abs() > 0.01,
        "test premise: Zircon must be strongly birefringent"
    );
    // c-axis deliberately tilted away from BOTH the slab normal (Y) and the
    // incidence plane (XY) -- genuine 3D walk-off, not a coincidental in-plane one.
    material.c_axis = Vec3::new(0.3, 0.8, 0.5).normalize();

    let incident_origin = Vec3::new(0.0, 5.0, 0.0);
    // ~16.7 degrees off normal incidence -- comfortably sub-critical for n~1.9, and
    // oblique enough that Snell's law genuinely bends k (unlike normal incidence,
    // where k passes straight through regardless of index and the k/S distinction
    // could never show up in the exit direction at all).
    let incident_dir = Vec3::new(0.3, -1.0, 0.0).normalize();

    let (entry_k, entry_s, exit_k, exit_s, lateral_displacement) =
        trace_forced_extraordinary_slab(&material, incident_origin, incident_dir);

    // Sanity: the extraordinary mode's walk-off must have actually fired (entry_k
    // != entry_s) -- otherwise this test would be silently checking the degenerate
    // ordinary-mode-equivalent case instead of what it claims to.
    assert!(
        (entry_k - entry_s).length() > 1e-4,
        "test premise: the extraordinary mode's walk-off should visibly separate k \
         from S at entry (entry_k={entry_k:?}, entry_s={entry_s:?})"
    );

    let cos_parallel_k = exit_k.dot(incident_dir).clamp(-1.0, 1.0);
    let cos_parallel_s = exit_s.dot(incident_dir).clamp(-1.0, 1.0);
    assert!(
        (1.0 - cos_parallel_k).abs() < 1e-5,
        "exit wave normal k must be parallel to the incident ray within 1e-5 \
         (exit_k={exit_k:?}, incident_dir={incident_dir:?}, 1-cos={})",
        1.0 - cos_parallel_k
    );
    // Once back in air, S == k exactly (isotropic medium) -- both must agree.
    assert!(
        (1.0 - cos_parallel_s).abs() < 1e-5,
        "exit Poynting direction S (== k in air) must be parallel to the incident ray \
         within 1e-5 (exit_s={exit_s:?}, incident_dir={incident_dir:?}, 1-cos={})",
        1.0 - cos_parallel_s
    );
    assert!(
        (exit_k - exit_s).length() < 1e-6,
        "S must equal k exactly once back in isotropic air (exit_k={exit_k:?}, \
         exit_s={exit_s:?})"
    );
    assert!(
        lateral_displacement > 1e-4,
        "the exit point must be laterally displaced from the straight-through path \
         by a nonzero amount (the walk-off's real, physical effect) -- got {lateral_displacement}"
    );
}
