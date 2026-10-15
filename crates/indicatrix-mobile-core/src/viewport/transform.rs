//! Where the raster sits in the viewport: the `contain` fit of the image in the view,
//! plus the pan and zoom the user applied on top of it.
//!
//! The Solid view orbits the camera instead of zooming the picture, so it only ever
//! pans; the Diagram view zooms (1x to 6x) and pans the picture itself. Both map a
//! pointer position in view coordinates to a raster pixel (picking) and a raster
//! position back to view coordinates (placing the handles) through the same
//! [`ContainFit`], so what is drawn and what is hit can never disagree.

use indicatrix_solid::preview::view::{ContainFit, contain_fit};

/// The widest zoom of the Diagram view.
pub const MAX_ZOOM: f32 = 6.0;

/// Pan and zoom applied to the raster after it was fitted to the view.
///
/// `zoom` scales the picture about the view's centre, `pan_*` then moves it (logical
/// pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTransform {
    /// Magnification, `1.0` to [`MAX_ZOOM`].
    pub zoom: f32,
    /// Horizontal shift of the picture's centre from the view's centre, in logical px.
    pub pan_x: f32,
    /// Vertical shift of the picture's centre from the view's centre, in logical px.
    pub pan_y: f32,
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl ViewTransform {
    /// No zoom, no pan.
    pub const IDENTITY: Self = Self {
        zoom: 1.0,
        pan_x: 0.0,
        pan_y: 0.0,
    };

    /// This transform moved by `(dx, dy)` logical pixels.
    #[must_use]
    pub fn panned(self, dx: f32, dy: f32) -> Self {
        Self {
            pan_x: self.pan_x + dx,
            pan_y: self.pan_y + dy,
            ..self
        }
    }

    /// This transform zoomed by `factor` (clamped to `1.0..=max_zoom`) while the picture
    /// point under `about` (view coordinates) stays under it.
    #[must_use]
    #[expect(
        clippy::suboptimal_flops,
        reason = "the unfused multiply-then-add is the documented arithmetic; mul_add would change the rounding"
    )]
    pub fn zoomed(self, factor: f32, about: (f32, f32), view: (f32, f32), max_zoom: f32) -> Self {
        if !factor.is_finite() || factor <= 0.0 {
            return self;
        }
        let zoom = (self.zoom * factor).clamp(1.0, max_zoom.max(1.0));
        let ratio = zoom / self.zoom;
        let (cx, cy) = (view.0 * 0.5, view.1 * 0.5);
        Self {
            zoom,
            pan_x: (about.0 - cx) - (about.0 - cx - self.pan_x) * ratio,
            pan_y: (about.1 - cy) - (about.1 - cy - self.pan_y) * ratio,
        }
    }

    /// The same transform with the pan held so that the picture cannot be dragged out of
    /// reach: it may leave the view by at most a quarter of the view beyond what its
    /// own overhang allows.
    #[must_use]
    #[expect(
        clippy::suboptimal_flops,
        reason = "the unfused multiply-then-add is the documented arithmetic; mul_add would change the rounding"
    )]
    pub fn clamped(self, view: (f32, f32), image: (u32, u32)) -> Self {
        let Some(base) = contain_fit(view.0, view.1, image.0, image.1) else {
            return Self::IDENTITY;
        };
        let scale = base.scale * self.zoom;
        let overhang_x = (image.0 as f32 * scale - view.0).max(0.0) * 0.5 + view.0 * 0.25;
        let overhang_y = (image.1 as f32 * scale - view.1).max(0.0) * 0.5 + view.1 * 0.25;
        Self {
            zoom: self.zoom,
            pan_x: self.pan_x.clamp(-overhang_x, overhang_x),
            pan_y: self.pan_y.clamp(-overhang_y, overhang_y),
        }
    }

    /// Where an `image`-sized raster lands in a `view`-sized viewport: the `contain`
    /// fit, zoomed about the view's centre and then panned. `None` for an empty view or
    /// image.
    #[must_use]
    #[expect(
        clippy::suboptimal_flops,
        reason = "the unfused multiply-then-add is the documented arithmetic; mul_add would change the rounding"
    )]
    pub fn fit(self, view: (f32, f32), image: (u32, u32)) -> Option<ContainFit> {
        let base = contain_fit(view.0, view.1, image.0, image.1)?;
        let scale = base.scale * self.zoom;
        let centre = (view.0 * 0.5 + self.pan_x, view.1 * 0.5 + self.pan_y);
        Some(ContainFit {
            offset_x: image.0 as f32 * scale * -0.5 + centre.0,
            offset_y: image.1 as f32 * scale * -0.5 + centre.1,
            scale,
        })
    }
}

/// The raster pixel under the view position `(x, y)`, `None` outside the picture.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "px and py are floored and checked to be in 0..image size just above, so they fit u32"
)]
pub fn pixel_at(fit: ContainFit, image: (u32, u32), x: f32, y: f32) -> Option<(u32, u32)> {
    let (px, py) = fit.to_image(x, y);
    let (px, py) = (px.floor(), py.floor());
    if px >= 0.0 && py >= 0.0 && px < image.0 as f32 && py < image.1 as f32 {
        Some((px as u32, py as u32))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: (f32, f32) = (400.0, 300.0);
    const IMAGE: (u32, u32) = (800, 600);

    #[test]
    fn identity_fit_is_the_contain_fit() {
        let fit = ViewTransform::IDENTITY.fit(VIEW, IMAGE).expect("fit");
        assert!((fit.scale - 0.5).abs() < 1e-6);
        assert!(fit.offset_x.abs() < 1e-4 && fit.offset_y.abs() < 1e-4);
    }

    #[test]
    fn pixel_round_trip_through_a_zoomed_fit() {
        let transform = ViewTransform::IDENTITY.zoomed(2.0, (100.0, 80.0), VIEW, MAX_ZOOM);
        let fit = transform.fit(VIEW, IMAGE).expect("fit");
        let (px, py) = fit.to_image(150.0, 120.0);
        let (vx, vy) = fit.to_view(px, py);
        assert!((vx - 150.0).abs() < 1e-3 && (vy - 120.0).abs() < 1e-3);
        assert!(pixel_at(fit, IMAGE, 150.0, 120.0).is_some());
    }

    #[test]
    fn zoom_keeps_the_point_under_the_fingers() {
        let about = (120.0, 90.0);
        let before = ViewTransform::IDENTITY.fit(VIEW, IMAGE).expect("fit");
        let image_point = before.to_image(about.0, about.1);
        let after = ViewTransform::IDENTITY
            .zoomed(3.0, about, VIEW, MAX_ZOOM)
            .fit(VIEW, IMAGE)
            .expect("fit");
        let moved = after.to_view(image_point.0, image_point.1);
        assert!((moved.0 - about.0).abs() < 1e-2 && (moved.1 - about.1).abs() < 1e-2);
    }

    #[test]
    fn zoom_is_clamped() {
        let zoomed = ViewTransform::IDENTITY.zoomed(100.0, (0.0, 0.0), VIEW, MAX_ZOOM);
        assert!((zoomed.zoom - MAX_ZOOM).abs() < 1e-6);
        let out = zoomed.zoomed(0.001, (0.0, 0.0), VIEW, MAX_ZOOM);
        assert!((out.zoom - 1.0).abs() < 1e-6);
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "the unfused multiply-then-add is the documented arithmetic; mul_add would change the rounding"
    )]
    fn pan_cannot_leave_the_view() {
        let far = ViewTransform::IDENTITY.panned(5000.0, -5000.0);
        let held = far.clamped(VIEW, IMAGE);
        assert!(held.pan_x <= VIEW.0 * 0.25 + 1e-3);
        assert!(held.pan_y >= -VIEW.1 * 0.25 - 1e-3);
    }

    #[test]
    fn pixel_outside_the_picture_is_none() {
        let fit = ViewTransform::IDENTITY
            .fit((400.0, 600.0), IMAGE)
            .expect("fit");
        // The picture is letterboxed: the top strip of the view holds no pixel.
        assert!(pixel_at(fit, IMAGE, 200.0, 1.0).is_none());
        assert!(pixel_at(fit, IMAGE, 200.0, 300.0).is_some());
    }
}
