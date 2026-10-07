//! Following one camera ray into the stone: refraction at the surface, then the straight
//! path inside up to the first exit, or on through up to two total internal reflections.
//!
//! All the geometry here is in the MESH frame (the rough frame, mm). The rig is in its own
//! frame; [`Scene`] carries the rigid transform between them, and rays from the cameras are
//! brought into the mesh frame before they meet the surface.

use glam::{DVec2, DVec3};

use super::{
    refract::{reflect, refract},
    rig::{RigProfile, Rigid},
};
use crate::rough_plan::shape::RoughMesh;

/// The mesh, the rig and the transform that puts the mesh in the rig.
#[derive(Debug, Clone, Copy)]
pub struct Scene<'a> {
    /// The rough, in its own frame.
    pub mesh: &'a RoughMesh,
    /// The cameras and the refractive indices.
    pub rig: &'a RigProfile,
    /// The transform from the mesh frame to the rig frame.
    pub alignment: Rigid,
}

impl<'a> Scene<'a> {
    /// A scene from its three parts.
    #[must_use]
    pub const fn new(mesh: &'a RoughMesh, rig: &'a RigProfile, alignment: Rigid) -> Self {
        Self {
            mesh,
            rig,
            alignment,
        }
    }

    /// The diagonal of the mesh's bounding box in mm, the scale all tolerances hang on.
    #[must_use]
    pub fn mesh_scale(&self) -> f64 {
        let (lo, hi) = self.mesh.bounds();
        (hi - lo).length()
    }

    /// The ray of `pixel` in view `view`, in the mesh frame: origin and unit direction. `None`
    /// when the view does not exist.
    #[must_use]
    pub fn camera_ray(&self, view: usize, pixel: DVec2) -> Option<(DVec3, DVec3)> {
        let pose = self.rig.views.get(view)?;
        let (origin, dir) = pose.pixel_ray(pixel);
        Some((
            self.alignment.to_mesh(origin),
            self.alignment.dir_to_mesh(dir).normalize(),
        ))
    }
}

/// Why a ray could not be followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceError {
    /// The view does not exist.
    NoSuchView,
    /// The ray misses the stone.
    Miss,
    /// The ray starts inside the stone or meets its surface from behind.
    FromInside,
    /// The ray is beyond the critical angle at the surface: total internal reflection at
    /// entry. The mark is invalid for this view.
    TotalInternalReflection,
    /// The ray inside the stone never reaches the surface again (a damaged mesh).
    NoExit,
    /// The ray would leave the stone at this bounce (0-based) instead of being reflected.
    NotReflected(u8),
}

/// One straight stretch of a ray inside the stone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leg {
    /// Where the stretch starts.
    pub from: DVec3,
    /// Where it ends, at the surface.
    pub to: DVec3,
}

impl Leg {
    /// The unit direction of the stretch (zero for an empty one).
    #[must_use]
    pub fn dir(&self) -> DVec3 {
        (self.to - self.from).normalize_or_zero()
    }

    /// The length in mm.
    #[must_use]
    pub fn length(&self) -> f64 {
        (self.to - self.from).length()
    }

    /// The distance from `point` to the nearest point of the stretch.
    #[must_use]
    pub fn distance_to(&self, point: DVec3) -> f64 {
        let along = self.to - self.from;
        let len2 = along.length_squared();
        let t = if len2 > 0.0 {
            ((point - self.from).dot(along) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (point - (self.from + along * t)).length()
    }

    /// How far along the stretch the foot of `point` lies, in mm from its start (negative
    /// before the start, beyond [`length`](Self::length) after the end).
    #[must_use]
    pub fn param_of(&self, point: DVec3) -> f64 {
        (point - self.from).dot(self.dir())
    }
}

/// A camera ray followed into the stone.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// Where the ray meets the surface.
    pub entry: DVec3,
    /// The outward unit normal there.
    pub entry_normal: DVec3,
    /// The angle between the ray and the normal at entry, in degrees.
    pub incidence_deg: f64,
    /// The stretches inside: one for the direct path, one more for every internal reflection.
    pub legs: Vec<Leg>,
}

/// Follows the ray `origin + t dir` (mesh frame, `dir` a unit vector) into the stone.
///
/// It is refracted at the first surface it meets, then runs inside to the first exit, or, for
/// `bounces > 0`, is reflected totally at that many exits and runs on.
///
/// # Errors
///
/// [`TraceError`]: the ray misses the stone, starts inside it, is totally reflected at entry,
/// never exits, or would leave at a bounce that was to be a total reflection.
pub fn trace_ray(
    scene: &Scene<'_>,
    origin: DVec3,
    dir: DVec3,
    bounces: u8,
) -> Result<Path, TraceError> {
    let (n_around, n_stone) = (scene.rig.surround_n, scene.rig.stone_n);
    let eps = 1e-7 * scene.mesh_scale();
    let hit = scene
        .mesh
        .first_hit(origin, dir, eps)
        .ok_or(TraceError::Miss)?;
    if dir.dot(hit.normal) >= 0.0 {
        return Err(TraceError::FromInside);
    }
    let entry = origin + dir * hit.t;
    let mut inside =
        refract(dir, hit.normal, n_around, n_stone).ok_or(TraceError::TotalInternalReflection)?;
    let incidence_deg = (-dir.dot(hit.normal)).clamp(-1.0, 1.0).acos().to_degrees();
    let mut legs = Vec::with_capacity(usize::from(bounces) + 1);
    let mut from = entry;
    for bounce in 0..=bounces {
        let next = scene
            .mesh
            .first_hit(from, inside, eps)
            .ok_or(TraceError::NoExit)?;
        let to = from + inside * next.t;
        legs.push(Leg { from, to });
        if bounce == bounces {
            break;
        }
        if refract(inside, next.normal, n_stone, n_around).is_some() {
            return Err(TraceError::NotReflected(bounce));
        }
        inside = reflect(inside, next.normal);
        from = to;
    }
    Ok(Path {
        entry,
        entry_normal: hit.normal,
        incidence_deg,
        legs,
    })
}

/// [`trace_ray`] for the ray of `pixel` in view `view`.
///
/// # Errors
///
/// [`TraceError::NoSuchView`] for a view that does not exist, else as [`trace_ray`].
pub fn trace_pixel(
    scene: &Scene<'_>,
    view: usize,
    pixel: DVec2,
    bounces: u8,
) -> Result<Path, TraceError> {
    let (origin, dir) = scene
        .camera_ray(view, pixel)
        .ok_or(TraceError::NoSuchView)?;
    trace_ray(scene, origin, dir, bounces)
}
