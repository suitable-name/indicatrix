//! The solved design's resolved tools and frosted planes for the live view, each cached for
//! one generation (resolving them measures the finished stone, which `sync` would otherwise
//! redo on every settings change).

use crate::app::{Ctx, state::SolveState};
use indicatrix::geometry::tool::ToolPrimitive;
use std::cell::RefCell;

thread_local! {
    /// The resolved tools of the solved design at one generation (resolving them measures the
    /// finished flat stone, which `sync` would otherwise redo on every settings change).
    static TOOLS: RefCell<Option<(u64, Vec<ToolPrimitive>)>> = const { RefCell::new(None) };
    /// The frosted planes of the solved design at one generation, for the same reason.
    static FROSTED: RefCell<Option<(u64, Vec<u32>)>> = const { RefCell::new(None) };
}

/// The frosted planes of the solved design (`Design::frosted_planes_from_solved`), or none: a
/// design without frosting, and a generation that has no solve yet (`live_planes` has already
/// answered "Solving..." then, so no scene is built from this).
pub(super) fn live_frosted(ctx: &Ctx) -> Vec<u32> {
    let app = ctx.state.borrow();
    let Some(state) = &app.design else {
        return Vec::new();
    };
    if !state.session.design.has_frosting() {
        return Vec::new();
    }
    let current = state.session.current_generation();
    let SolveState::Solved {
        generation, tiers, ..
    } = &app.solve
    else {
        return Vec::new();
    };
    if *generation != current {
        return Vec::new();
    }
    if let Some(frosted) = FROSTED.with(|cache| {
        cache
            .borrow()
            .as_ref()
            .filter(|(cached, _)| *cached == current)
            .map(|(_, frosted)| frosted.clone())
    }) {
        return frosted;
    }
    let frosted = state.session.design.frosted_planes_from_solved(tiers);
    FROSTED.with(|cache| *cache.borrow_mut() = Some((current, frosted.clone())));
    frosted
}

/// The concave tools of the solved design, resolved against its flat stone exactly as the
/// desktop's scoring and display paths do (`Design::tools_for_scoring`: an unresolvable
/// concave tier is read as "no tools", the flat stone). Empty for a design without concave
/// tiers, and while the current generation has no solve -- `live_planes` has already
/// answered "Solving..." then, so no scene is built from this.
pub(super) fn live_tools(ctx: &Ctx) -> Vec<ToolPrimitive> {
    let app = ctx.state.borrow();
    let Some(state) = &app.design else {
        return Vec::new();
    };
    if state.session.design.concave_tiers.is_empty() {
        return Vec::new();
    }
    let current = state.session.current_generation();
    let SolveState::Solved {
        generation, tiers, ..
    } = &app.solve
    else {
        return Vec::new();
    };
    if *generation != current {
        return Vec::new();
    }
    if let Some(tools) = TOOLS.with(|cache| {
        cache
            .borrow()
            .as_ref()
            .filter(|(cached, _)| *cached == current)
            .map(|(_, tools)| tools.clone())
    }) {
        return tools;
    }
    let design = &state.session.design;
    let tools = design.tools_for_scoring(&design.planes_from_solved(tiers));
    TOOLS.with(|cache| *cache.borrow_mut() = Some((current, tools.clone())));
    tools
}
