//! Locating inclusions from photos taken on a fixed camera rig.
//!
//! An inclusion seen through the rough's surface appears where refraction puts it, not where it
//! is. With the scanned mesh and the camera poses known, every mark the user places on a photo
//! is a ray: it refracts at the mesh surface (Snell's law) and runs on inside the stone, and the
//! inclusion is the point closest to all those interior rays. This module is that pipeline,
//! without any window: the app builds its UI on the API below.
//!
//! # Pieces
//!
//! - [`rig`]: the stored rig profile ([`RigProfile`]: the views, the stone's and the
//!   surroundings' refractive index, the calibration result), the camera model (pixel to ray,
//!   point to pixel) and the rigid mesh-to-rig transform ([`Rigid`]).
//! - [`refract`]: Snell refraction with total internal reflection, and reflection.
//! - [`trace`]: a camera ray into the stone, up to the first exit or through up to two total
//!   internal reflections, against the mesh's BVH ([`RoughMesh::first_hit`](crate::rough_plan::shape::RoughMesh::first_hit)).
//! - [`align`]: the mesh-to-rig alignment from the stone's outline in each photo.
//! - [`triangulate`]: refractive triangulation of a point or a polyline, with the uncertainty
//!   and a leave-one-out outlier flag per view.
//! - [`reproject`]: the verification overlay: where the solved point appears in each photo, and
//!   where its ghost images would.
//! - [`result`]: the point as a closed shell with its margin, ready for stage A's
//!   `add_inclusion_points`.
//! - [`calibrate`]: the beam-splitter-cube calibration (outer edges, then the coated diagonal as
//!   an end-to-end check).
//!
//! # Frames and conventions
//!
//! All distances are millimetres. Pixels are `u` right, `v` down. The MESH frame is the rough
//! frame (the bounding box at the origin, see [`import_mesh`](crate::rough_plan::shape::import_mesh));
//! results (located points) are in it. The RIG frame is the cameras'. [`Rigid`] maps mesh to rig;
//! rays are brought into the mesh frame, so the mesh itself is never moved.
//!
//! For a birefringent stone the profile's stone index is the ordinary index `n_o`, and the user
//! clicks the ordinary image. In immersion the profile's surrounding index is the liquid's; when
//! it equals the stone's the rays do not bend and the result is plain straight-line triangulation.
//!
//! Everything is deterministic: no hashed collections, no randomness, fixed iteration orders.

pub mod align;
pub mod calibrate;
pub mod refract;
pub mod reproject;
pub mod result;
pub mod rig;
pub mod shapes;
mod solve;
pub mod trace;
pub mod triangulate;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_rig;

pub use align::{
    AlignError, AlignOptions, AlignResult, OutlineView, ViewMisfit, align_mesh_to_rig,
};
pub use calibrate::{
    Calibrated, CalibrationError, CalibrationOptions, CalibrationResult, CubeSpec, DiagonalCheck,
    DiagonalLine, EdgeDeviation, EdgeLine, ViewObservations, calibrate_edges, calibrate_rig,
    check_diagonal,
};
pub use refract::{critical_angle_deg, reflect, refract};
pub use reproject::{Ghost, MAX_GHOST_BOUNCES, Reprojection, predict_ghosts, reproject};
pub use result::{DEFAULT_MARGIN_MM, InclusionShell, add_located_point, suggested_margin_mm};
pub use rig::{CameraBasis, Projection, RigError, RigProfile, Rigid, ViewPose};
pub use shapes::{box_mesh, box_points, sphere_shell};
pub use trace::{Leg, Path, Scene, TraceError, trace_pixel, trace_ray};
pub use triangulate::{
    InsideState, Line, LocateError, LocateOptions, Located, LocatedPolyline, Mark, ViewPolyline,
    ViewReport, ViewStatus, closest_point_to_lines, locate_point, locate_polyline,
};
