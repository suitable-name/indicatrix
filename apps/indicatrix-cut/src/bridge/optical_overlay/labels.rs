//! Facet names registered by the solid worker, bound to the exact geometry they describe.
use super::snapshot::OpticalSnapshot;
use crate::bridge::render_thread::{PlanesOwner, RenderContext};
use indicatrix::geometry::{GpuFacetPlane, tool::ToolPrimitive};
use std::{cell::RefCell, sync::Arc};

/// One registration: the names, and the owner and `Arc`s of the geometry they describe.
struct LabelRecord {
    owner: PlanesOwner,
    planes: Arc<Vec<GpuFacetPlane>>,
    tools: Arc<Vec<ToolPrimitive>>,
    labels: Arc<Vec<String>>,
}
thread_local! {
    static LABELS: RefCell<Option<LabelRecord>> = const { RefCell::new(None) };
}
/// Register facet labels with the exact geometry they describe, on the UI thread.
pub fn facet_labels(ctx: &RenderContext, editor_generation: u64, labels: Arc<Vec<String>>) {
    if ctx.planes_owner
        != (PlanesOwner::Editor {
            generation: editor_generation,
        })
    {
        return;
    }
    LABELS.with(|l| {
        *l.borrow_mut() = Some(LabelRecord {
            owner: ctx.planes_owner,
            planes: ctx.active_planes.clone(),
            tools: ctx.active_tools.clone(),
            labels,
        });
    });
}
/// The registered names, if they describe the context's current geometry.
pub(super) fn captured_labels(ctx: &RenderContext) -> Option<Arc<Vec<String>>> {
    LABELS.with(|l| {
        let record = l.borrow();
        let record = record.as_ref()?;
        (record.owner == ctx.planes_owner
            && Arc::ptr_eq(&record.planes, &ctx.active_planes)
            && Arc::ptr_eq(&record.tools, &ctx.active_tools))
        .then(|| record.labels.clone())
    })
}
/// Whether two captures carry the very same names (by identity).
pub(super) fn same_labels(a: Option<&Arc<Vec<String>>>, b: Option<&Arc<Vec<String>>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}
pub(super) fn label_for(scene: &OpticalSnapshot, id: usize) -> Option<String> {
    scene.labels.as_ref()?.get(id).cloned()
}
