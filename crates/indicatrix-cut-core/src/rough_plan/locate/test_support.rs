//! Shared helpers for the locate tests: a deterministic noise source, test rigs and mark
//! synthesis. No rendering, small meshes.

use glam::{DVec2, DVec3};

use super::{
    reproject::reproject,
    rig::{Projection, RigProfile, ViewPose},
    shapes::box_mesh,
    trace::{Scene, trace_pixel},
    triangulate::Mark,
};
use crate::rough_plan::shape::RoughMesh;

/// The image size of every test camera, in pixels.
pub const IMAGE: [u32; 2] = [2048, 1536];
/// The focal length of every test camera, in pixels.
pub const FOCAL: f64 = 4000.0;
/// The distance of every test camera from the origin, in mm.
pub const DISTANCE: f64 = 150.0;
/// The refractive index of N-BK7, the glass of the test stones.
pub const N_BK7: f64 = 1.5168;

/// A tiny deterministic generator (a linear congruential generator, high bits), enough for test
/// noise.
pub struct Lcg(u64);

impl Lcg {
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// A uniform number in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1_u64 << 53) as f64
    }

    /// A uniform number in `[low, high)`.
    pub fn uniform(&mut self, low: f64, high: f64) -> f64 {
        (high - low).mul_add(self.unit(), low)
    }

    /// A standard normal number (Box-Muller).
    pub fn gauss(&mut self) -> f64 {
        let first = self.unit().max(1e-12);
        let second = self.unit();
        (-2.0 * first.ln()).sqrt() * (std::f64::consts::TAU * second).cos()
    }
}

/// The pinhole projection of the test cameras.
pub const fn pinhole() -> Projection {
    Projection::Pinhole { focal_px: FOCAL }
}

/// Eight cameras at the corners of a cube around the origin (one per octant), all looking at
/// the origin from [`DISTANCE`] with +Z up.
pub fn octant_views(projection: Projection) -> Vec<ViewPose> {
    let mut views = Vec::new();
    for corner in 0..8_u32 {
        let sign = |bit: u32| if (corner >> bit) & 1 == 1 { 1.0 } else { -1.0 };
        let direction = DVec3::new(sign(0), sign(1), sign(2)).normalize();
        views.push(ViewPose::look_at(
            &format!("octant {corner}"),
            direction * DISTANCE,
            DVec3::ZERO,
            DVec3::Z,
            projection,
            IMAGE,
        ));
    }
    views
}

/// A rig in which the stone has index `stone_n` and the surroundings `surround_n`.
pub fn rig_of(views: Vec<ViewPose>, stone_n: f64, surround_n: f64) -> RigProfile {
    let mut rig = RigProfile::new("test rig", views, stone_n);
    rig.surround_n = surround_n;
    rig
}

/// A cube mesh with the given half edge, centred at the origin.
pub fn cube_mesh(half: f64) -> RoughMesh {
    box_mesh(DVec3::splat(half)).expect("a cube is a valid mesh")
}

/// `pixel` moved by Gaussian noise of `sigma` pixels in each axis.
pub fn noisy(pixel: DVec2, rng: &mut Lcg, sigma: f64) -> DVec2 {
    let noise = DVec2::new(rng.gauss(), rng.gauss());
    pixel + noise * sigma
}

/// `exact` moved by Gaussian noise of `sigma` pixels, redrawn while the noisy pixel would enter
/// the stone through another face (or facet) than the exact one does.
///
/// A click that lands across a face edge sees the point through a different refraction, which no
/// amount of averaging repairs; the tests keep their noise on the right side of the edge so that
/// they measure the noise, not the luck of the draw.
pub fn noisy_on_same_face(
    scene: &Scene<'_>,
    view: usize,
    exact: DVec2,
    rng: &mut Lcg,
    sigma: f64,
) -> DVec2 {
    let face = |pixel: DVec2| {
        trace_pixel(scene, view, pixel, 0)
            .ok()
            .map(|path| path.entry_normal)
    };
    let reference = face(exact);
    let mut pixel = noisy(exact, rng, sigma);
    for _ in 0..50 {
        let same = match (reference, face(pixel)) {
            (Some(wanted), Some(found)) => (wanted - found).length() < 1e-9,
            _ => false,
        };
        if same {
            break;
        }
        pixel = noisy(exact, rng, sigma);
    }
    pixel
}

/// The marks of `target` in every view that has an exact image of it (the refracted ray through
/// the pixel passes within `max_miss_mm` of the target), with `sigma` pixels of noise.
pub fn marks_for(
    scene: &Scene<'_>,
    target: DVec3,
    max_miss_mm: f64,
    rng: &mut Lcg,
    sigma: f64,
) -> Vec<Mark> {
    let mut marks = Vec::new();
    for view in 0..scene.rig.views.len() {
        let Some(image) = reproject(scene, view, target, None) else {
            continue;
        };
        if image.miss_mm <= max_miss_mm {
            let pixel = noisy_on_same_face(scene, view, DVec2::from_array(image.pixel), rng, sigma);
            marks.push(Mark {
                view,
                pixel: pixel.to_array(),
            });
        }
    }
    marks
}
