//! Extra training stages (BLD-1): the paths the production apps run that the first five
//! stages in `stages.rs` never reached -- the guide prepass at production frame sizes, the
//! denoiser's path-signature branch, the full tilt sweep entry points the apps call, and a
//! real-sized HDR environment map for the importance sampler.
//!
//! Split from `stages.rs` to keep both files under the house 800-line limit. Every stage
//! is deterministic (fixed camera, fixed hash-derived noise, no clock or RNG input) and
//! bounded to a few seconds at `PGO_SCALE=1`.
//!
//! Deliberately NOT here (needs another crate, so an `indicatrix` example cannot link it
//! without a dependency cycle or a new dependency): the `indicatrix-net` payload codecs
//! (shuffle + zstd/lz4), the `indicatrix-solid` rasterizer and mesh cache, the rough
//! planner and the vault search.

use glam::Vec3;
use indicatrix::{
    color::metrics::{
        SweepProgress, evaluate_all_axes_profiles_geom, evaluate_all_axes_profiles_stepped_geom,
    },
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, hash_u32},
    },
    renderer::{
        denoise::{AtrousDenoiser, AtrousParams, GBuffers},
        env_map::EnvironmentMap,
        guide_pass::{GuideBuffers, generate_guide_buffers_geom_with_index},
        tonemap::tonemap_to_rgba,
    },
};
use std::time::Instant;

/// Reference refractive index handed to the guide prepass, as the apps do for a typical
/// coloured corundum/garnet-range stone (`DispersionModel::n_d`); any value above 1.0 turns
/// the path-signature walk on, which is the branch this training exists to reach.
pub const TRAINING_N_D: f32 = 1.76;

/// Scales `base` by `scale` like `stages::scaled`, but private to this file's size maths.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "scale is a small positive PGO_SCALE factor; the rounded product is far below u32::MAX and saturating is fine"
)]
fn scaled_dim(base: u32, scale: f32, min: u32) -> u32 {
    ((base as f32 * scale).round() as u32).max(min)
}

/// A deterministic, real-sized equirectangular HDR panorama: sky gradient, dim ground, one
/// very bright sun disc and two softboxes over a lightly hashed texture.
///
/// A production map has hundreds of thousands of texels with a dynamic range of several
/// orders of magnitude; the 8x4 gradient this replaces made the importance sampler's CDF
/// search and the bilinear lookup train on a map where every branch is trivially the same.
/// Uses only `f32` arithmetic and `hash_u32`, so it is bit-reproducible and needs no file.
pub fn procedural_hdr_map(width: usize, height: usize) -> EnvironmentMap {
    let mut pixels: Vec<[f32; 3]> = Vec::with_capacity(width * height);
    let sun = (width as f32 * 0.3, height as f32 * 0.3);
    let sun_r2 = (height as f32 * 0.05).powi(2);
    for y in 0..height {
        let v = y as f32 / height as f32;
        for x in 0..width {
            let mut rgb = if v < 0.5 {
                let t = v * 2.0;
                [
                    0.75f32.mul_add(t, 0.25),
                    0.5f32.mul_add(t, 0.45),
                    0.05f32.mul_add(-t, 0.9),
                ]
            } else {
                [0.15, 0.12, 0.10]
            };
            let (dx, dy) = (x as f32 - sun.0, y as f32 - sun.1);
            if f32::mul_add(dy, dy, dx * dx) < sun_r2 {
                rgb = [60.0, 55.0, 45.0];
            }
            let u = x as f32 / width as f32;
            if (0.6..0.7).contains(&u) && (0.2..0.35).contains(&v) {
                rgb = [6.0, 6.0, 6.0];
            }
            if (0.1..0.18).contains(&u) && (0.55..0.7).contains(&v) {
                rgb = [2.0, 1.6, 1.2];
            }
            let (col_u32, row_u32) = (
                u32::try_from(x).expect("a training pixel column fits u32"),
                u32::try_from(y).expect("a training pixel row fits u32"),
            );
            let cell = hash_u32(((row_u32 >> 4) << 16) ^ (col_u32 >> 4));
            let tex = 0.1f32.mul_add((cell & 0xff) as f32 / 255.0, 0.9);
            pixels.push([rgb[0] * tex, rgb[1] * tex, rgb[2] * tex]);
        }
    }
    EnvironmentMap::from_rgb(width, height, pixels).expect("training HDR map is well-formed")
}

/// Sums one guide set into a checksum so the prepass cannot be optimised away.
fn guide_checksum(g: &GuideBuffers) -> f32 {
    let sig: u32 = g.path_sig.iter().fold(0, |a, &s| a.wrapping_add(s));
    let depth: f32 = g.depth.iter().filter(|d| **d < 1.0e5).sum();
    let hits = g.facet_id.iter().filter(|f| **f >= 0).count();
    (hits as f32).mul_add(1e-6, depth.mul_add(1e-6, (sig & 0xffff) as f32 * 1e-3))
}

/// Stage `guide`: the primary-ray guide prepass with the path signature on
/// (`n_d = TRAINING_N_D`) at 800x600 and 1080p -- the two sizes the apps' display denoiser
/// and the export path run it at (82 ms at 1080p per `guide_pass.rs`) -- through the
/// `_geom` entry the apps call.
pub fn train_guide(scale: f32) -> f32 {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.6, 0.45, 2.4, 42.0);
    let mut checksum = 0.0f32;
    for (w, h) in [(800u32, 600u32), (1920, 1080)] {
        let (w, h) = (scaled_dim(w, scale, 64), scaled_dim(h, scale, 48));
        let t0 = Instant::now();
        let reps = 3;
        for _ in 0..reps {
            let g = generate_guide_buffers_geom_with_index(
                w,
                h,
                &camera,
                StoneGeometry::planes_only(&planes),
                TRAINING_N_D,
            );
            checksum += guide_checksum(&g);
        }
        println!("  guide: {w}x{h} x{reps}: {:.2?}", t0.elapsed());
    }
    checksum
}

/// Stage `denoise_sig`: the A-Trous denoiser with a real path signature (`Some(..)` from
/// the guide prepass) at 800x600 -- the frame the apps' `frame_denoise` runs, which the
/// `stages::train_tracer` frames (path signature on every second preset) only reach at
/// 256x192. Colour is hash-noised per facet so the colour, depth, normal, facet and
/// signature edge-stops all see both matches and mismatches.
pub fn train_denoise_signature(scale: f32) -> f32 {
    let (w, h) = (scaled_dim(800, scale, 64), scaled_dim(600, scale, 48));
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.6, 0.45, 2.4, 42.0);
    let guides = generate_guide_buffers_geom_with_index(
        w,
        h,
        &camera,
        StoneGeometry::planes_only(&planes),
        TRAINING_N_D,
    );
    let color: Vec<Vec3> = guides
        .facet_id
        .iter()
        .enumerate()
        .map(|(i, &f)| {
            let base = 0.05f32.mul_add((f & 7) as f32, 0.2);
            let noise = (hash_u32(u32::try_from(i).expect("a training pixel index fits u32"))
                & 0xffff) as f32
                / 65535.0
                - 0.5;
            Vec3::new(
                base * (1.0 + noise),
                base * 0.5f32.mul_add(-noise, 1.0),
                base * 0.25f32.mul_add(noise, 1.0),
            )
        })
        .collect();
    let t0 = Instant::now();
    let mut denoiser = AtrousDenoiser::new();
    let mut filtered = Vec::new();
    let mut checksum = 0.0f32;
    for with_sig in [true, false, true] {
        let g = GBuffers {
            color: &color,
            depth: &guides.depth,
            normal: &guides.normal,
            facet_id: &guides.facet_id,
            path_sig: with_sig.then_some(guides.path_sig.as_slice()),
            width: w as usize,
            height: h as usize,
            spp: 16,
        };
        denoiser.denoise_into(&g, &AtrousParams::default(), &mut filtered);
        let rgba = tonemap_to_rgba(&filtered, 1.0);
        let hdr: f32 = filtered.iter().map(|v| v.x + v.y + v.z).sum();
        let srgb: f32 = rgba.iter().map(|&b| f32::from(b)).sum();
        checksum = srgb.mul_add(1e-6, hdr.mul_add(1e-6, checksum));
    }
    println!("  denoise_sig: {w}x{h} x3: {:.2?}", t0.elapsed());
    checksum
}

/// Stage `tilt_sweep`: the whole-stone sweep entry points the apps call
/// (`evaluate_all_axes_profiles_geom`, parallel, and the `_stepped_geom` variant the batch
/// tilt engine uses), once each -- 724 raytrace evaluations apiece, spread over all cores.
/// `stages::train_tilt` covers the per-azimuth inner function; this covers the sweep
/// wrapper, the batch scheduler and the progress hook around it.
pub fn train_tilt_sweep() -> f32 {
    let planes = StandardGemCuts::standard_round_brilliant();
    let geom = StoneGeometry::planes_only(&planes);
    let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let mut checksum = 0.0f32;

    let t0 = Instant::now();
    let diamond = GemMaterial::diamond();
    for axis in evaluate_all_axes_profiles_geom(geom, &diamond, env) {
        checksum = f32::mul_add(axis.brilliance.iter().sum::<f32>(), 1e-4, checksum);
        checksum = f32::mul_add(axis.windowing.iter().sum::<f32>(), 1e-4, checksum);
    }
    println!("  tilt_sweep: parallel Diamond: {:.2?}", t0.elapsed());

    let t0 = Instant::now();
    let alexandrite = indicatrix::render_setup::material_for_stone(
        GemMaterial::by_name("Alexandrite").expect("built-in material"),
        0.0,
        &[],
    );
    let mut steps = 0usize;
    let stepped = evaluate_all_axes_profiles_stepped_geom(
        geom,
        &alexandrite,
        env,
        &mut |_: SweepProgress| {
            steps += 1;
            true
        },
    );
    for axis in stepped.expect("the hook never stops the sweep") {
        checksum = f32::mul_add(axis.extinction.iter().sum::<f32>(), 1e-4, checksum);
    }
    println!(
        "  tilt_sweep: stepped Alexandrite ({steps} steps): {:.2?}",
        t0.elapsed()
    );
    checksum
}
