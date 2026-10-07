//! "Snapshot design" / "Compare to snapshot" (the desktop's
//! `retarget_actions::snapshot`): remembers the design and its solved masts, and later
//! shows, tier position by tier position, what the angles, indices and masts have become.
//! The rows are `indicatrix_editor::snapshot`'s (the diff itself is
//! `indicatrix_cut_core::diff_tiers`).
//!
//! The snapshot lives for the page's lifetime only; the desktop's visual before/after
//! compare window has no web counterpart.

use crate::{
    AppWindow, DiffRow, SnapshotModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
        solve::with_solved,
    },
};
use indicatrix_editor::{
    snapshot::{DesignSnapshot, DiffRowView, compare_label, compare_rows},
    view_model::yield_report::design_label_text,
};
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;

thread_local! {
    /// The snapshot taken this session, if any.
    static SNAPSHOT: RefCell<Option<DesignSnapshot>> = const { RefCell::new(None) };
}

/// The current design's label, as the compare header names it.
fn current_label(ctx: &Ctx) -> String {
    design_label_text(
        ctx.state
            .borrow()
            .design
            .as_ref()
            .and_then(|d| d.asc_filename.as_deref()),
    )
}

/// Snapshot design: the design as it stands, with its solved masts (solving first when
/// they are not known).
fn take(ctx: &Ctx) {
    with_solved(ctx, |ctx, solved| {
        let design = ctx
            .state
            .borrow()
            .design
            .as_ref()
            .map(|d| d.session.design.clone());
        let Some(design) = design else {
            return;
        };
        let label = current_label(ctx);
        SNAPSHOT.with(|cell| {
            *cell.borrow_mut() = Some(DesignSnapshot {
                design,
                solved: solved.ok(),
                label: label.clone(),
                from_retarget_apply: false,
            });
        });
        if let Some(ui) = ctx.ui.upgrade() {
            let model = ui.global::<SnapshotModel>();
            model.set_has_snapshot(true);
            model.set_snapshot_label(label.as_str().into());
        }
        show_message(
            ctx,
            MessageKind::Success,
            &format!("Snapshot taken: \"{label}\"."),
        );
    });
}

/// A one-line count of each status, `11 tier position(s): 3 changed, 1 added, ...`.
fn summary(rows: &[DiffRowView]) -> String {
    let count = |label: &str| rows.iter().filter(|r| r.status_label == label).count();
    format!(
        "{} tier position(s): {} changed, {} added, {} removed, {} same.",
        rows.len(),
        count("Changed"),
        count("Added"),
        count("Removed"),
        count("Same")
    )
}

/// Compare to snapshot: diffs the snapshot against the design now and shows the rows.
fn compare(ctx: &Ctx) {
    if SNAPSHOT.with(|cell| cell.borrow().is_none()) {
        show_message(
            ctx,
            MessageKind::Error,
            "No snapshot taken yet -- use Design > Snapshot design first.",
        );
        return;
    }
    with_solved(ctx, |ctx, solved| {
        let Some(snapshot) = SNAPSHOT.with(|cell| cell.borrow().clone()) else {
            return;
        };
        let rows = {
            let app = ctx.state.borrow();
            let Some(design_state) = app.design.as_ref() else {
                return;
            };
            compare_rows(
                &snapshot,
                &design_state.session.design,
                solved.ok().as_deref(),
            )
        };
        let Some(ui) = ctx.ui.upgrade() else {
            return;
        };
        let model = ui.global::<SnapshotModel>();
        model.set_compare_label(compare_label(&snapshot.label, &current_label(ctx)).into());
        model.set_summary(summary(&rows).into());
        model.set_rows(ModelRc::new(VecModel::from(
            rows.iter()
                .map(|row| DiffRow {
                    tier_index: i32::try_from(row.tier_index).unwrap_or(i32::MAX),
                    name: row.name.as_str().into(),
                    old_angle: row.old_angle.as_str().into(),
                    new_angle: row.new_angle.as_str().into(),
                    mast_delta: row.mast_delta.as_str().into(),
                    status_label: row.status_label.into(),
                    status_color: Color::from_rgb_u8(
                        row.status_rgb.0,
                        row.status_rgb.1,
                        row.status_rgb.2,
                    ),
                })
                .collect::<Vec<_>>(),
        )));
        model.set_compare_open(true);
    });
}

/// Registers the snapshot callbacks.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<SnapshotModel>();
    let c = ctx.clone();
    model.on_take(move || take(&c));
    let c = ctx.clone();
    model.on_compare(move || compare(&c));
    let c = ctx.clone();
    model.on_close(move || {
        if let Some(ui) = c.ui.upgrade() {
            ui.global::<SnapshotModel>().set_compare_open(false);
        }
    });
}
