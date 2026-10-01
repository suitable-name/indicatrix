//! Selecting tiers in the table, in step with the Solid and Diagram views.
//!
//! `WebApp::selected_tier` is the one selection all three share, and
//! `EditorSession::multi_selected` the Ctrl / Shift group (whose facets the views outline).
//! Like the desktop:
//!
//! - a plain click, the arrow keys, a click on a manufacturability warning or on a facet
//!   select ONE tier and empty the group ([`crate::views::select_tier`]);
//! - Ctrl+click toggles a row in the group and leaves the plain selection where it is;
//! - Shift+click replaces the group with every row between the selection (the anchor) and
//!   the clicked row -- with no selection yet, just the clicked row.

use super::table;
use crate::{TierTableModel, app::Ctx, views};
use std::collections::BTreeSet;

/// Selects tier `index` (the plain selection); a negative index clears it.
pub fn select(ctx: &Ctx, index: i32) {
    views::select_tier(ctx, usize::try_from(index).ok());
    table::sync(ctx);
}

/// Ctrl+click / Select-mode click: toggles row `index` in the group.
fn toggle_multi(ctx: &Ctx, index: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    {
        let mut app = ctx.state.borrow_mut();
        let Some(design) = app.design.as_mut() else {
            return;
        };
        if index >= design.session.design.tiers.len() {
            return;
        }
        if !design.session.multi_selected.remove(&index) {
            design.session.multi_selected.insert(index);
        }
    }
    views::request_refresh(ctx);
    table::sync(ctx);
}

/// Shift+click: the group becomes every row between the anchor and `index`.
fn select_range(ctx: &Ctx, index: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    {
        let mut app = ctx.state.borrow_mut();
        let anchor = app.selected_tier.unwrap_or(index);
        let Some(design) = app.design.as_mut() else {
            return;
        };
        let (low, high) = if anchor <= index {
            (anchor, index)
        } else {
            (index, anchor)
        };
        let tier_count = design.session.design.tiers.len();
        design.session.multi_selected = (low..=high).filter(|&i| i < tier_count).collect();
    }
    views::request_refresh(ctx);
    table::sync(ctx);
}

/// The batch bar's Clear: empties the group.
fn clear_multi(ctx: &Ctx) {
    {
        let mut app = ctx.state.borrow_mut();
        let Some(design) = app.design.as_mut() else {
            return;
        };
        if design.session.multi_selected.is_empty() {
            return;
        }
        design.session.multi_selected = BTreeSet::new();
    }
    views::request_refresh(ctx);
    table::sync(ctx);
}

/// Wires the selection callbacks of `TierTableModel`.
pub fn wire(model: &TierTableModel<'_>, ctx: &Ctx) {
    let c = ctx.clone();
    model.on_select_tier(move |index| select(&c, index));
    let c = ctx.clone();
    model.on_toggle_multi(move |index| toggle_multi(&c, index));
    let c = ctx.clone();
    model.on_select_range(move |index| select_range(&c, index));
    let c = ctx.clone();
    model.on_clear_multi(move || clear_multi(&c));
    let c = ctx.clone();
    model.on_warning_clicked(move |tier| select(&c, tier));
}
