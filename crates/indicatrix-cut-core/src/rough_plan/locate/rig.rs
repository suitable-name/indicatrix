//! The camera rig: the profile that is stored, the camera model and the mesh-to-rig transform.
//!
//! Pixels use the image convention: `u` to the right, `v` downward, the origin at the corner
//! of the image. A camera looks along `forward`; its `up` vector says which way is up in the
//! image (it is made perpendicular to `forward`, so it need not be exact). Positions are in
//! millimetres in the rig frame. A pinhole view has a focal length in pixels; a telecentric or
//! macro lens is orthographic, with a scale in pixels per millimetre and no perspective.

use std::fmt;

use glam::{DMat3, DQuat, DVec2, DVec3};
use serde::{Deserialize, Serialize};

use super::calibrate::CalibrationResult;

/// How a view maps the scene onto the image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Projection {
    /// A pinhole camera with this focal length in pixels.
    Pinhole {
        /// The focal length in pixels.
        focal_px: f64,
    },
    /// An orthographic (telecentric) camera with this scale.
    Orthographic {
        /// Pixels per millimetre.
        px_per_mm: f64,
    },
}

impl Projection {
    /// The focal length in pixels (pinhole) or the scale in pixels per millimetre
    /// (orthographic): the one number that sets the image scale.
    #[must_use]
    pub const fn scale_px(self) -> f64 {
        match self {
            Self::Pinhole { focal_px } => focal_px,
            Self::Orthographic { px_per_mm } => px_per_mm,
        }
    }

    /// The same kind of projection with its scale number replaced.
    #[must_use]
    pub const fn with_scale(self, scale: f64) -> Self {
        match self {
            Self::Pinhole { .. } => Self::Pinhole { focal_px: scale },
            Self::Orthographic { .. } => Self::Orthographic { px_per_mm: scale },
        }
    }
}

/// The three unit axes of a camera: image right, image down, and the viewing direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraBasis {
    /// Towards the right of the image.
    pub right: DVec3,
    /// Towards the bottom of the image.
    pub down: DVec3,
    /// The viewing direction.
    pub forward: DVec3,
}

/// One camera of the rig.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewPose {
    /// A label for the user, for example `+X upper`.
    pub name: String,
    /// The camera centre in the rig frame, mm.
    pub position: [f64; 3],
    /// The viewing direction in the rig frame (need not be a unit vector).
    pub forward: [f64; 3],
    /// Which way is up in the image, in the rig frame.
    pub up: [f64; 3],
    /// How the camera maps the scene to pixels.
    pub projection: Projection,
    /// The principal point in pixels (default: the image centre).
    pub principal_point: [f64; 2],
    /// The image size in pixels, width then height.
    pub image_size: [u32; 2],
}

impl ViewPose {
    /// A camera at `position` looking at `target`, with the principal point at the image centre.
    #[must_use]
    pub fn look_at(
        name: &str,
        position: DVec3,
        target: DVec3,
        up: DVec3,
        projection: Projection,
        image_size: [u32; 2],
    ) -> Self {
        Self {
            name: name.to_owned(),
            position: position.to_array(),
            forward: (target - position).normalize().to_array(),
            up: up.to_array(),
            projection,
            principal_point: [
                f64::from(image_size[0]) * 0.5,
                f64::from(image_size[1]) * 0.5,
            ],
            image_size,
        }
    }

    /// The camera centre.
    #[must_use]
    pub const fn position_vec(&self) -> DVec3 {
        DVec3::from_array(self.position)
    }

    /// The camera's axes (`up` is made perpendicular to `forward`).
    #[must_use]
    pub fn basis(&self) -> CameraBasis {
        let forward = DVec3::from_array(self.forward).normalize();
        let raw_up = DVec3::from_array(self.up);
        let flattened = raw_up - forward * raw_up.dot(forward);
        let up = if flattened.length_squared() < 1e-18 {
            forward.any_orthonormal_vector()
        } else {
            flattened.normalize()
        };
        let right = forward.cross(up);
        let down = forward.cross(right);
        CameraBasis {
            right,
            down,
            forward,
        }
    }

    /// The ray through `pixel` in the rig frame: its origin and unit direction. A pinhole ray
    /// starts at the camera centre; an orthographic ray starts on the camera's image plane.
    #[must_use]
    pub fn pixel_ray(&self, pixel: DVec2) -> (DVec3, DVec3) {
        let basis = self.basis();
        let offset = pixel - DVec2::from_array(self.principal_point);
        let lateral = basis.right * offset.x + basis.down * offset.y;
        match self.projection {
            Projection::Pinhole { focal_px } => (
                self.position_vec(),
                (lateral + basis.forward * focal_px).normalize(),
            ),
            Projection::Orthographic { px_per_mm } => {
                (self.position_vec() + lateral / px_per_mm, basis.forward)
            }
        }
    }

    /// The pixel a point in the rig frame lands on in a straight line of sight (no
    /// refraction), or `None` when a pinhole camera has it behind it.
    #[must_use]
    pub fn project(&self, point: DVec3) -> Option<DVec2> {
        let basis = self.basis();
        let rel = point - self.position_vec();
        let centre = DVec2::from_array(self.principal_point);
        let lateral = DVec2::new(rel.dot(basis.right), rel.dot(basis.down));
        match self.projection {
            Projection::Pinhole { focal_px } => {
                let depth = rel.dot(basis.forward);
                if depth > 1e-9 {
                    Some(centre + lateral * (focal_px / depth))
                } else {
                    None
                }
            }
            Projection::Orthographic { px_per_mm } => Some(centre + lateral * px_per_mm),
        }
    }
}

/// A rigid transform from the mesh frame to the rig frame: a rotation (as a rotation vector,
/// axis times angle in radians) and then a translation in mm.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rigid {
    /// The rotation vector.
    pub rotation: [f64; 3],
    /// The translation in mm.
    pub translation: [f64; 3],
}

impl Rigid {
    /// No rotation, no translation.
    pub const IDENTITY: Self = Self {
        rotation: [0.0; 3],
        translation: [0.0; 3],
    };

    /// The transform that rotates by `rotation` and then translates by `translation`.
    #[must_use]
    pub fn from_parts(rotation: DQuat, translation: DVec3) -> Self {
        Self {
            rotation: rotation.normalize().to_scaled_axis().to_array(),
            translation: translation.to_array(),
        }
    }

    /// A coarse starting transform from two mesh axes.
    ///
    /// Mesh axis `z_axis` points along rig +Z, and the part of `x_axis` perpendicular to it along rig +X (right-handed), then `translation` is added.
    /// `None` when the two axes are zero or parallel.
    #[must_use]
    pub fn from_axes(z_axis: DVec3, x_axis: DVec3, translation: DVec3) -> Option<Self> {
        let axis_z = z_axis.try_normalize()?;
        let axis_x = (x_axis - axis_z * x_axis.dot(axis_z)).try_normalize()?;
        let axis_y = axis_z.cross(axis_x);
        let matrix = DMat3::from_cols(axis_x, axis_y, axis_z).transpose();
        Some(Self::from_parts(DQuat::from_mat3(&matrix), translation))
    }

    /// The rotation as a quaternion.
    #[must_use]
    pub fn quat(&self) -> DQuat {
        DQuat::from_scaled_axis(DVec3::from_array(self.rotation))
    }

    /// A point of the mesh in the rig frame.
    #[must_use]
    pub fn to_rig(&self, point: DVec3) -> DVec3 {
        self.quat() * point + DVec3::from_array(self.translation)
    }

    /// A direction of the mesh frame in the rig frame.
    #[must_use]
    pub fn dir_to_rig(&self, dir: DVec3) -> DVec3 {
        self.quat() * dir
    }

    /// A point of the rig in the mesh frame.
    #[must_use]
    pub fn to_mesh(&self, point: DVec3) -> DVec3 {
        self.quat().inverse() * (point - DVec3::from_array(self.translation))
    }

    /// A direction of the rig in the mesh frame.
    #[must_use]
    pub fn dir_to_mesh(&self, dir: DVec3) -> DVec3 {
        self.quat().inverse() * dir
    }
}

/// What is wrong with a rig profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigError {
    /// The profile has no views.
    NoViews,
    /// A refractive index is not finite or is below 1.
    BadRefractiveIndex,
    /// This view (0-based) has a zero or non-finite direction, scale or image size.
    BadView(usize),
}

impl fmt::Display for RigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NoViews => write!(f, "the rig has no views"),
            Self::BadRefractiveIndex => {
                write!(f, "a refractive index is missing or below 1")
            }
            Self::BadView(i) => write!(
                f,
                "view {} has an unusable direction, scale or image size",
                i + 1
            ),
        }
    }
}

impl std::error::Error for RigError {}

/// The stored description of a camera rig.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RigProfile {
    /// The name the user gave the rig.
    pub name: String,
    /// The cameras, in a fixed order that the marks refer to by index.
    pub views: Vec<ViewPose>,
    /// The refractive index of the stone. For a birefringent stone this is the ordinary
    /// index `n_o`, and the user clicks the ordinary image.
    pub stone_n: f64,
    /// The refractive index around the stone: 1.0 for air, the liquid's index in immersion.
    pub surround_n: f64,
    /// What the beam-splitter calibration measured, when it was run.
    #[serde(default)]
    pub calibration: Option<CalibrationResult>,
}

impl RigProfile {
    /// A rig in air with this stone index and no calibration.
    #[must_use]
    pub fn new(name: &str, views: Vec<ViewPose>, stone_n: f64) -> Self {
        Self {
            name: name.to_owned(),
            views,
            stone_n,
            surround_n: 1.0,
            calibration: None,
        }
    }

    /// The default eight-camera layout around the origin.
    ///
    /// For each of the four directions +X, -X, +Y, -Y one camera `elevation_deg` above the horizontal and one the same
    /// angle below it, so both ends of the stone are seen. All look at the origin from
    /// `distance_mm`, with +Z up.
    #[must_use]
    pub fn side_layout(
        distance_mm: f64,
        elevation_deg: f64,
        projection: Projection,
        image_size: [u32; 2],
    ) -> Vec<ViewPose> {
        let elevation = elevation_deg.to_radians();
        let (sin_el, cos_el) = elevation.sin_cos();
        let sides = [
            ("+X", DVec3::X),
            ("-X", DVec3::NEG_X),
            ("+Y", DVec3::Y),
            ("-Y", DVec3::NEG_Y),
        ];
        let mut views = Vec::with_capacity(8);
        for (label, axis) in sides {
            for (end, sign) in [("upper", 1.0), ("lower", -1.0)] {
                let position = (axis * cos_el + DVec3::Z * (sign * sin_el)) * distance_mm;
                views.push(ViewPose::look_at(
                    &format!("{label} {end}"),
                    position,
                    DVec3::ZERO,
                    DVec3::Z,
                    projection,
                    image_size,
                ));
            }
        }
        views
    }

    /// Checks that the profile can be used.
    ///
    /// # Errors
    ///
    /// [`RigError`] for no views, a refractive index that is not finite or below 1, or a view
    /// with a zero direction, a scale that is not positive, or an empty image.
    pub fn validate(&self) -> Result<(), RigError> {
        if self.views.is_empty() {
            return Err(RigError::NoViews);
        }
        let index_ok = |n: f64| n.is_finite() && n >= 1.0;
        if !(index_ok(self.stone_n) && index_ok(self.surround_n)) {
            return Err(RigError::BadRefractiveIndex);
        }
        for (i, view) in self.views.iter().enumerate() {
            let forward = DVec3::from_array(view.forward);
            let usable = forward.is_finite()
                && forward.length_squared() > 0.0
                && view.position_vec().is_finite()
                && view.projection.scale_px().is_finite()
                && view.projection.scale_px() > 0.0
                && view.image_size.iter().all(|&side| side > 0);
            if !usable {
                return Err(RigError::BadView(i));
            }
        }
        Ok(())
    }
}
