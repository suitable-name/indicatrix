//! The Slint-free retarget proposal view model, and the `push_*` renderers that
//! turn it into `MainWindow`'s `editor_retarget_*`/`RetargetModel` properties -- see
//! this group's own `mod.rs` doc comment ("Slint-free view-model split").

use crate::{
    MainWindow, RetargetModel, RetargetRowItem,
    gui::editor::retarget::{
        self, CrownShift, RetargetError, RetargetMode, RetargetProposal, RetargetRow,
    },
};
use indicatrix::{geometry::meet_solver::Block, optics::materials::GemMaterial};
use indicatrix_cut_core::{Design, MaterialSelection, ResolvedMaterial, Risk};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

/// Pure, Slint-free view of one [`RetargetRow`] -- see this module's doc comment
/// ("Slint-free view-model split").
pub(super) struct RetargetRowView {
    tier_index: usize,
    block: &'static str,
    name: String,
    old_angle: String,
    new_angle: String,
    margin: String,
    risk_label: &'static str,
    risk_rgb: (u8, u8, u8),
}

/// [`Risk`]'s label plus its RGB badge color -- chosen HERE, once, so the two can
/// never drift apart, matching `Theme.accent-emerald`/`accent-amber`/`accent-ruby`
/// exactly.
const fn risk_label_and_rgb(risk: Risk) -> (&'static str, (u8, u8, u8)) {
    match risk {
        Risk::Safe => ("Safe", (0x10, 0xb9, 0x81)),
        Risk::Marginal => ("Marginal", (0xf5, 0x9e, 0x0b)),
        Risk::Windows => ("Windows", (0xf4, 0x3f, 0x5e)),
    }
}

pub(super) fn row_view(row: &RetargetRow) -> RetargetRowView {
    let block = match row.block {
        Block::Crown => "Crown",
        Block::Pavilion => "Pavilion",
        // Never actually produced by `retarget::build_proposal` (girdle tiers are
        // never listed), but this stays total rather than panicking on a future change.
        Block::Girdle => "Girdle",
    };
    let (risk_label, risk_rgb) = risk_label_and_rgb(row.risk);
    RetargetRowView {
        tier_index: row.tier_index,
        block,
        name: row.name.clone(),
        old_angle: format!("{:.2}\u{b0}", row.old_angle),
        new_angle: format!("{:.2}\u{b0}", row.new_angle),
        margin: format!("{:+.2}\u{b0}", row.margin_deg),
        risk_label,
        risk_rgb,
    }
}

/// Pure, Slint-free view of one [`retarget::build_proposal`] call -- exactly what
/// [`push_retarget_view`] pushes into `MainWindow`'s `editor_retarget_*` properties,
/// computed here so the routing decision (which of `rows`/`notes`/`anchored_errors`/
/// `solve_error` gets populated) is unit-tested directly. The second element of the
/// returned pair is the real [`RetargetProposal`] to stash in
/// `EditorState::pending_retarget`, `None` for either [`RetargetError`] variant.
pub(super) fn retarget_view(
    design: &Design,
    target: &ResolvedMaterial,
    crown: CrownShift,
    mode: RetargetMode,
    // The catalogue's custom materials, so the design's CURRENT refractive index
    // resolves through the same lookup the rest of the editor uses (
    // 53). Without it a design whose material is a custom catalogue entry had its
    // source RI read from the built-in table alone, which silently fell back to a
    // different number -- and every proposed angle is a shift from that number.
    custom_materials: &[GemMaterial],
) -> (RetargetView, Option<RetargetProposal>) {
    match retarget::build_proposal(design, target, crown, mode, custom_materials) {
        Ok(proposal) => {
            let rows = proposal.rows.iter().map(row_view).collect();
            let view = RetargetView {
                rows,
                notes: proposal.notes.clone(),
                anchored_errors: Vec::new(),
                solve_error: String::new(),
            };
            (view, Some(proposal))
        }
        Err(RetargetError::AnchoredTiers(tiers)) => {
            let anchored_errors = tiers
                .iter()
                .map(|(index, name)| format!("#{index} \"{name}\""))
                .collect();
            let view = RetargetView {
                rows: Vec::new(),
                notes: Vec::new(),
                anchored_errors,
                solve_error: String::new(),
            };
            (view, None)
        }
        Err(RetargetError::Solve(err)) => {
            let view = RetargetView {
                rows: Vec::new(),
                notes: Vec::new(),
                anchored_errors: Vec::new(),
                solve_error: err.to_string(),
            };
            (view, None)
        }
    }
}

pub(super) struct RetargetView {
    pub(super) rows: Vec<RetargetRowView>,
    pub(super) notes: Vec<String>,
    pub(super) anchored_errors: Vec<String>,
    pub(super) solve_error: String,
}

/// Pushes [`RetargetView`] into `MainWindow`'s `editor_retarget_rows`/`_notes`/
/// `_anchored_errors`/`_solve_error` -- the only place a [`RetargetRowView`] becomes
/// a real `RetargetRowItem`.
pub(super) fn push_retarget_view(ui: &MainWindow, view: RetargetView) {
    let rows: Vec<RetargetRowItem> = view
        .rows
        .into_iter()
        .map(|r| RetargetRowItem {
            tier_index: r.tier_index as i32,
            block: r.block.into(),
            name: r.name.into(),
            old_angle: r.old_angle.into(),
            new_angle: r.new_angle.into(),
            margin: r.margin.into(),
            risk_label: r.risk_label.into(),
            risk_color: Color::from_rgb_u8(r.risk_rgb.0, r.risk_rgb.1, r.risk_rgb.2),
        })
        .collect();
    ui.global::<RetargetModel>()
        .set_rows(ModelRc::new(VecModel::from(rows)));
    ui.global::<RetargetModel>()
        .set_notes(ModelRc::new(VecModel::from(
            view.notes
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    ui.global::<RetargetModel>()
        .set_anchored_tier_errors(ModelRc::new(VecModel::from(
            view.anchored_errors
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    ui.global::<RetargetModel>()
        .set_solve_error(view.solve_error.into());
}

/// Pushes the target material readout -- fixed for the lifetime of one dialog
/// session, but re-pushed on every rebuild anyway since it costs nothing. Also clears
/// `RetargetModel.target_material_error` (see [`push_target_error`]): reaching this
/// function at all means `material::resolve_target_selection` just succeeded, so any
/// earlier parse error no longer applies.
pub(super) fn push_target_readout(
    ui: &MainWindow,
    selection: &MaterialSelection,
    target: &ResolvedMaterial,
) {
    ui.global::<RetargetModel>()
        .set_target_material_name(super::material::target_display_name(selection).into());
    ui.global::<RetargetModel>()
        .set_target_material_ri(format!("{:.4}", target.n_d).into());
    ui.global::<RetargetModel>()
        .set_target_critical_angle(format!("{:.2}\u{b0}", target.critical_angle_deg).into());
    ui.global::<RetargetModel>()
        .set_target_material_error("".into());
}

/// `ri_override_text` failed to parse -- pushes `message` into
/// `RetargetModel.target_material_error` (shown above the proposal table,
/// `retarget_dialog.slint`) and clears `rows`/`notes` so nothing on screen implies a
/// proposal was actually built against this text. The readout above the error
/// (`target_material_name`/`_ri`/`_critical_angle`) is deliberately left as-is --
/// whatever the last valid target was -- rather than reset, matching the error
/// banner's own "showing the last valid target" wording.
pub(super) fn push_target_error(ui: &MainWindow, message: &str) {
    ui.global::<RetargetModel>()
        .set_target_material_error(message.into());
    push_retarget_view(
        ui,
        RetargetView {
            rows: Vec::new(),
            notes: Vec::new(),
            anchored_errors: Vec::new(),
            solve_error: String::new(),
        },
    );
}
