//! Edit operations on a [`crate::design::Design`], with undo/redo.
//!
//! Undo is business logic, not UI plumbing -- it needs to be correct and testable
//! without a window, which is the single biggest reason this crate exists separately
//! from `apps/indicatrix-cut` rather than living in the editor's own event
//! handlers. The model here is a classic command/inverse pair: applying an [`Edit`]
//! to a [`crate::design::Design`] returns the exact [`Edit`] that would undo it, so
//! [`History`] never needs a second "how do I reverse this" implementation, a
//! snapshot of the whole design, or any cloning bigger than the one
//! tier/preform/constraint actually touched.
//!
//! [`Edit::SetConstraint`] exists alongside `ModifyTier` (which already covers
//! "replace the whole tier, constraint included") purely as the ergonomic
//! single-field version an editor UI wants for "change what this facet meets"
//! without re-typing angle/name/indices.
//!
//! # Module layout
//!
//! [`edit_type`] is [`Edit`]/[`EditError`] themselves; [`apply`] is
//! [`crate::design::Design::apply_edit`], the one command/inverse primitive;
//! [`history`] is [`History`], the undo/redo stack built on top of it.

mod apply;
mod edit_type;
mod history;
#[cfg(test)]
mod tests;

pub use edit_type::{Edit, EditError, RemapRounding};
pub use history::History;
