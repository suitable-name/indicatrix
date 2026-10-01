//! Mouse-driven direct manipulation of the selected facet: the GUI-free math behind the
//! three drag handles and the Slice tool, and all their wording.
//!
//! # The pick frame
//!
//! Everything here works in the **pick frame**: physical pixels of the solid raster
//! (which is also its pick buffer), origin top-left of the letterboxed raster. A caller
//! converts pointer coordinates into it, and back out to draw handles. The projection
//! ([`projection`]) mirrors the rasterizer's own `project` and `Camera::generate_ray`
//! op for op, and takes the camera and frame size the raster used for that frame, so a
//! handle lands exactly where the facet is drawn.
//!
//! # The three handles
//!
//! A selected facet ([`FacetFrame`]) grows three handles from its centroid
//! ([`handle_layout`]), each dragging the WHOLE tier through the normal edit path:
//!
//! - **Angle**: along `d normal / d theta`; tilts the tier (degrees, snapped to 0.1, or
//!   0.01 with Shift).
//! - **Depth**: along the facet normal; moves the tier in or out (its mast, snapped to
//!   0.01 or 0.001), which pins it with a scale reference.
//! - **Index**: along `d normal / d phi`; turns the tier around the index wheel in whole
//!   teeth.
//!
//! [`drag_value`] turns pointer travel along a handle into a [`DragValue`];
//! `EditorSession::{set_tier_angle, pin_tier_mast, rotate_tier_indices}` apply it as
//! coalescing edits keyed by [`drag_coalesce_key`], so a whole gesture is one undo step.
//! [`dependents`] names the tiers that follow a drag; [`text`] holds the hint and toast
//! wording; [`slice`] turns a dragged screen line into a snapped cutting plane and the
//! provisional tier it becomes.
//!
//! # What both apps share around them
//!
//! - [`target`]: which facet carries the handles, the alignment checks that hide them
//!   rather than land them on the wrong facet, and the hit-test.
//! - [`gesture`]: one drag as pure decisions -- what a press captured, which edit each
//!   throttled pointer value asks for, the live feedback and the closing toast.
//! - [`provisional`]: the Slice tool's provisional tier, edited in place until Keep or
//!   Discard.
//!
//! The pointer <-> pick-frame mapping for a view that letterboxes its raster lives in
//! `indicatrix_solid::preview::view::ContainFit`.

pub mod dependents;
pub mod drag;
pub mod frame;
pub mod gesture;
pub mod handles;
pub mod projection;
pub mod provisional;
pub mod slice;
pub mod target;
pub mod text;

#[cfg(test)]
mod frame_tests;
#[cfg(test)]
mod gesture_tests;
#[cfg(test)]
mod tests;

pub use dependents::{moved_tiers, tiers_meeting};
pub use drag::{DragStart, DragValue, SnapMode, drag_coalesce_key, drag_value, snap_to_step};
pub use frame::FacetFrame;
pub use gesture::{ActiveDrag, AppliedEdit, DragProgress, GestureInputs, Step, drain_value};
pub use handles::{HANDLE_HIT_RADIUS_PX, HandleKind, HandleLayout, handle_layout, hit_test};
pub use projection::{ScreenPoint, ScreenSize, project, unproject};
pub use provisional::{DiscardReason, ProvisionalSlice, SliceLine, Snapshot};
pub use slice::{SliceSide, SnappedFacet, slice_normal, slice_tier, snap_to_gear, tangency_mast};
pub use target::{HandleTarget, target_for};
