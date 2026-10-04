//! The Tier tab's form: seeding it from the selected tier, the dirty-draft guard, the
//! live critical-angle bar, and Save / Add Tier.
//!
//! Modelled on the desktop's `editor_inspector.slint` (the state machine) and
//! `callbacks/tier_actions/tier_form.rs` (the save). The form's five text / enum fields
//! and the snapshot of what they held when last seeded live in `InspectorModel`, where
//! `form-dirty` is a plain comparison; this module decides WHEN to seed:
//!
//! - a selection change loads the newly selected tier into a clean form, but with a dirty
//!   form only puts up the Keep Draft / Discard Draft choice (`show-dirty-warning`);
//! - after an undo, redo or any edit made elsewhere, a clean form re-seeds from the design
//!   and a dirty one is left alone (forgetting which tier it described when tiers were
//!   added or removed, so a later save cannot land on the wrong row);
//! - a replaced design (New, Open) blanks the form.

use super::{PushCtx, finish, poll::with_current};
use crate::{
    AppWindow, ChipRow, InspectorModel, SolvedInfo,
    app::{
        Ctx,
        push::{MessageKind, show_message},
        state::SolveState,
    },
    editor::edit::{Dirty, set_selection},
    views,
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_editor::{
    loading::{
        TierFormFields, next_free_block_name, non_integral_index_warning, parse_tier_form,
        parse_tier_target, tier_form_error_field,
    },
    session::{EditorSession, tier_nudge_label},
    tier_save::{other_tier_names_excluding, tier_save_edit_with_target, unnaming_blocked_message},
    view_model::{live_margin::angle_live_margin, row_format::first_unresolved_meet_name},
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// Blanks the form to its "Add Tier" state (no tier loaded, no draft, no error).
pub(super) fn clear_form(model: &InspectorModel<'_>) {
    model.set_form_error(SharedString::new());
    model.set_form_error_field(SharedString::new());
    model.set_tier_angle("0.0".into());
    model.set_tier_constraint_kind(0);
    model.set_tier_constraint_text(SharedString::new());
    model.set_tier_name(SharedString::new());
    model.set_tier_indices(SharedString::new());
    snapshot_form(model);
    model.set_loaded_tier_index(-1);
    model.set_show_dirty_warning(false);
    model.set_pending_load_index(-1);
    model.set_angle_margin_level(-1);
    model.set_angle_margin_text(SharedString::new());
    model.set_chips(ModelRc::new(VecModel::<ChipRow>::default()));
    model.set_cheater_text(SharedString::new());
    model.set_note_text(SharedString::new());
    model.set_solved(SolvedInfo::default());
}

/// Records the fields' current values as the clean baseline `form-dirty` compares with.
fn snapshot_form(model: &InspectorModel<'_>) {
    model.set_loaded_angle(model.get_tier_angle());
    model.set_loaded_constraint_kind(model.get_tier_constraint_kind());
    model.set_loaded_constraint_text(model.get_tier_constraint_text());
    model.set_loaded_name(model.get_tier_name());
    model.set_loaded_indices(model.get_tier_indices());
}

/// Everything blank: no design is loaded.
pub(super) fn clear_all(ui: &AppWindow) {
    let model = ui.global::<InspectorModel>();
    clear_form(&model);
    model.set_schedule_rows(ModelRc::new(VecModel::default()));
    model.set_anchor_hint(false);
    model.set_preform_pending(false);
    model.set_yield_pending(false);
}

/// Seeds the form from tier `idx` (the full-precision strings, so a Save with nothing
/// retyped round-trips bit for bit) and makes it the loaded tier.
pub(super) fn apply_load(pcx: &PushCtx<'_>, model: &InspectorModel<'_>, idx: usize) {
    let Some(row) = pcx.rows().get(idx) else {
        return;
    };
    model.set_form_error(SharedString::new());
    model.set_form_error_field(SharedString::new());
    model.set_tier_angle(row.angle_full.as_str().into());
    model.set_tier_constraint_kind(row.constraint_kind);
    model.set_tier_constraint_text(row.constraint_text.as_str().into());
    model.set_tier_name(row.name.as_str().into());
    model.set_tier_indices(row.indices_full.as_str().into());
    snapshot_form(model);
    model.set_loaded_tier_index(i32::try_from(idx).unwrap_or(-1));
    model.set_show_dirty_warning(false);
    model.set_pending_load_index(-1);
    push_live_margin(pcx.design, pcx.n_d, model);
    seed_annotations(pcx, model, idx);
}

/// Seeds the cheater offset and note fields from tier `idx`.
fn seed_annotations(pcx: &PushCtx<'_>, model: &InspectorModel<'_>, idx: usize) {
    model.set_cheater_text(
        pcx.design
            .cheater_offset_deg(idx)
            .map_or_else(String::new, |deg| format!("{deg:.2}"))
            .into(),
    );
    model.set_note_text(
        pcx.design
            .tier_note(idx)
            .map_or_else(String::new, str::to_string)
            .into(),
    );
}

/// Updates the critical-angle bar for the angle currently in the form.
fn push_live_margin(design: &indicatrix_cut_core::Design, n_d: f64, model: &InspectorModel<'_>) {
    if let Some(margin) = angle_live_margin(design, n_d, &model.get_tier_angle()) {
        model.set_angle_margin_text(margin.text.into());
        model.set_angle_margin_level(margin.level);
        model.set_angle_margin_is_estimate(margin.is_estimate);
    } else {
        model.set_angle_margin_text(SharedString::new());
        model.set_angle_margin_level(-1);
        model.set_angle_margin_is_estimate(false);
    }
}

/// The guarded entry point for a selection change (`load_tier_into_form` and
/// `request_new` on the desktop).
fn on_selection_changed(pcx: &PushCtx<'_>, model: &InspectorModel<'_>, selected: Option<usize>) {
    let loaded = model.get_loaded_tier_index();
    let dirty = model.get_form_dirty();
    match selected {
        Some(idx) => {
            let idx_i = i32::try_from(idx).unwrap_or(-1);
            if dirty && idx_i != loaded {
                model.set_pending_load_index(idx_i);
                model.set_show_dirty_warning(true);
            } else {
                apply_load(pcx, model, idx);
            }
        }
        None => {
            if loaded != -1 {
                if dirty {
                    model.set_pending_load_index(-1);
                    model.set_show_dirty_warning(true);
                } else {
                    clear_form(model);
                }
            }
        }
    }
}

/// After an edit made elsewhere or an undo / redo that left the selection alone
/// (`reseed_after_external_change` on the desktop).
fn reseed_after_external_change(pcx: &PushCtx<'_>, model: &InspectorModel<'_>) {
    let loaded = model.get_loaded_tier_index();
    let tier_count_changed = pcx.design.tiers.len() != pcx.previous_tiers;
    let in_range = usize::try_from(loaded)
        .ok()
        .filter(|&i| i < pcx.design.tiers.len());
    if model.get_form_dirty() {
        if tier_count_changed {
            // The draft may now describe a different row: forget which tier it was for
            // rather than risk a save landing on the wrong one.
            model.set_loaded_tier_index(-1);
        }
        if let Some(idx) = in_range.filter(|_| !tier_count_changed) {
            seed_annotations(pcx, model, idx);
        }
        return;
    }
    match in_range {
        Some(idx) => apply_load(pcx, model, idx),
        None if loaded >= 0 => clear_form(model),
        None => {}
    }
}

/// One refresh of the Tier tab.
pub(super) fn push(pcx: &PushCtx<'_>) {
    let model = pcx.ui.global::<InspectorModel>();
    if pcx.replaced {
        clear_form(&model);
    }
    let selected = pcx.selected();
    let generation = pcx.design_state.session.current_generation();
    if selected != pcx.previous_selection {
        on_selection_changed(pcx, &model, selected);
    } else if pcx.previous_generation.is_some_and(|g| g != generation) {
        reseed_after_external_change(pcx, &model);
    }
    push_chips(pcx, &model);
    model.set_anchor_hint(needs_anchor(pcx));
}

/// Whether the current solve failed for lack of a scale anchor (`MissingAnchor`'s
/// "... has no anchor: add a tier with an exact scale value."), which the Tier tab
/// then explains (the desktop's one-time anchor explainer card).
fn needs_anchor(pcx: &PushCtx<'_>) -> bool {
    matches!(
        &pcx.app.solve,
        SolveState::Failed { generation, message }
            if *generation == pcx.design_state.session.current_generation()
                && message.contains("has no anchor")
    )
}

/// The chip row of the loaded tier.
fn push_chips(pcx: &PushCtx<'_>, model: &InspectorModel<'_>) {
    let chips: Vec<ChipRow> = usize::try_from(model.get_loaded_tier_index())
        .ok()
        .and_then(|i| pcx.design.tiers.get(i))
        .map_or_else(Vec::new, |tier| {
            indicatrix_editor::view_model::row_format::index_chip_items(
                &tier.indices,
                &tier.detached,
            )
            .into_iter()
            .map(|chip| ChipRow {
                label: chip.label.into(),
                position: chip.position,
                detached: chip.detached,
            })
            .collect()
        });
    model.set_chips(ModelRc::new(VecModel::from(chips)));
}

/// What a successful save reports.
struct Saved {
    /// Where the tier now sits.
    index: usize,
    /// Whether it was added (rather than modified).
    added: bool,
    label: String,
    warning: Option<String>,
}

/// A failed save: the message and the field it concerns.
struct SaveError {
    message: String,
    field: &'static str,
}

impl SaveError {
    fn new(message: String) -> Self {
        let field = tier_form_error_field(&message);
        Self { message, field }
    }
}

/// The form's fields as the user left them.
struct FormValues {
    index: i32,
    angle: String,
    kind: i32,
    constraint_text: String,
    name: String,
    indices: String,
}

fn read_form(model: &InspectorModel<'_>) -> FormValues {
    FormValues {
        index: model.get_loaded_tier_index(),
        angle: model.get_tier_angle().to_string(),
        kind: model.get_tier_constraint_kind(),
        constraint_text: model.get_tier_constraint_text().to_string(),
        name: model.get_tier_name().to_string(),
        indices: model.get_tier_indices().to_string(),
    }
}

/// Parses the form and applies it as `AddTier` (index < 0, inserted after the selection)
/// or `ModifyTier`, with the desktop's checks: name collisions, an unresolvable meet
/// name, the tier's carried-through import data and detached set, the auto-name of a new
/// unnamed tier.
fn apply_save(
    session: &mut EditorSession,
    form: &FormValues,
    selected: Option<usize>,
) -> Result<Saved, SaveError> {
    let design = &session.design;
    let existing = usize::try_from(form.index)
        .ok()
        .and_then(|i| design.tiers.get(i));
    let (imported_meet, original_notes) = existing
        .map(|t| (t.imported_meet.clone(), t.original_notes.clone()))
        .unwrap_or_default();
    let other_tier_names = other_tier_names_excluding(design, form.index);
    let mut tier = parse_tier_form(TierFormFields {
        angle: &form.angle,
        constraint_kind: form.kind,
        constraint_text: &form.constraint_text,
        name: &form.name,
        indices: &form.indices,
        gear_teeth_abs: design.meta.gear_teeth_abs(),
        imported_meet,
        original_notes,
        other_tier_names: other_tier_names.clone(),
    })
    .map_err(SaveError::new)?;
    let target = parse_tier_target(form.kind, &form.constraint_text).map_err(SaveError::new)?;
    if form.index < 0 && tier.name.is_empty() {
        tier.name = next_free_block_name(tier.angle_deg, &other_tier_names);
    }
    let warning = non_integral_index_warning(&tier.indices);
    // A rename or angle edit must not silently re-link a deliberately detached tier.
    if let Some(current) = existing {
        tier.detached.clone_from(&current.detached);
        if current.angle_deg.is_sign_negative() {
            tier.angle_deg = -tier.angle_deg.abs();
        }
    } else if form.index < 0
        && indicatrix_cut_core::design::labelling::name_indicates_pavilion(&tier.name)
    {
        tier.angle_deg = -tier.angle_deg.abs();
    }
    // An unresolved `MeetNamed` token would degrade silently inside the solver.
    if let MeetConstraint::MeetNamed(names) = &tier.constraint
        && let Some(bad_name) = first_unresolved_meet_name(design, names)
    {
        return Err(SaveError {
            message: format!("No facet named '{bad_name}' -- check the Meets field."),
            field: "constraint",
        });
    }
    // Clearing the name of a tier other tiers meet by name would leave those references
    // dangling.
    if let Some(message) = unnaming_blocked_message(design, form.index, &form.name) {
        return Err(SaveError {
            message,
            field: "name",
        });
    }
    let (index, edit) = tier_save_edit_with_target(design, form.index, tier, target, selected);
    session
        .apply(edit)
        .map_err(|e| SaveError::new(e.to_string()))?;
    let label = session
        .design
        .tiers
        .get(index)
        .map(|tier| tier_nudge_label(tier, index))
        .unwrap_or_default();
    Ok(Saved {
        index,
        added: form.index < 0,
        label,
        warning,
    })
}

/// Add Tier / Save Tier (and Enter in a form field).
fn save_tier(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    let form = read_form(&model);
    let outcome = {
        let mut app = ctx.state.borrow_mut();
        let selected = app.selected_tier;
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        let outcome = apply_save(&mut design_state.session, &form, selected);
        if let Ok(saved) = &outcome
            && saved.added
        {
            set_selection(&mut app, Some(saved.index));
        }
        outcome
    };
    match outcome {
        Ok(saved) => {
            model.set_form_error(SharedString::new());
            model.set_form_error_field(SharedString::new());
            // Optimistic: the values were just committed, so the form reads clean against
            // them; the refresh below re-seeds it from the design.
            snapshot_form(&model);
            finish(
                ctx,
                if saved.added {
                    Dirty::All
                } else {
                    Dirty::one(saved.index)
                },
            );
            if saved.added {
                show_message(ctx, MessageKind::Info, &format!("Added {}", saved.label));
            }
            // Shown last so it is the one left on screen: worth more attention than the
            // bare confirmation.
            if let Some(warning) = saved.warning {
                show_message(ctx, MessageKind::Info, &warning);
            }
        }
        Err(error) => {
            model.set_form_error(error.message.as_str().into());
            model.set_form_error_field(error.field.into());
            show_message(ctx, MessageKind::Error, &error.message);
        }
    }
}

/// Asks the Tier tab to put the keyboard cursor in the field named `target` ("angle" or
/// "constraint"): the form focuses it when it shows (or already shows) and clears the
/// request.
fn request_focus(model: &InspectorModel<'_>, target: &str) {
    model.set_focus_target(target.into());
    model.set_focus_pending(true);
    model.set_focus_serial(model.get_focus_serial().wrapping_add(1));
}

/// New Tier: blanks the form for a new tier unless a draft would be lost, and puts the
/// cursor in the Angle field.
fn new_tier(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    if model.get_form_dirty() && model.get_loaded_tier_index() != -1 {
        model.set_pending_load_index(-1);
        model.set_show_dirty_warning(true);
        return;
    }
    clear_form(&model);
    views::select_tier(ctx, None);
    request_focus(&model, "angle");
}

/// The tier table's "Add Anchor": selects tier `index`, opens the Tier tab on it with
/// "Exact scale value" chosen for Meets (the desktop's `suggested_constraint_kind`), and
/// puts the cursor in the scale value field -- the fix the missing-anchor message asks for.
///
/// A draft in the form is never overwritten: the tab then shows its Keep / Discard choice
/// and the cursor stays where it was.
pub fn open_anchor(ctx: &Ctx, index: usize) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    model.set_tab(0);
    model.set_collapsed(false);
    views::select_tier(ctx, Some(index));
    // The refresh a selection change causes loads the tier into a clean form; run it now
    // (and load explicitly when the tier was already the selected one, which changes
    // nothing for the poll to notice).
    super::refresh(ctx);
    let wanted = i32::try_from(index).unwrap_or(-1);
    if model.get_loaded_tier_index() != wanted && !model.get_form_dirty() {
        with_current(ctx, |pcx| apply_load(pcx, &model, index));
    }
    if model.get_loaded_tier_index() != wanted || model.get_form_dirty() {
        return;
    }
    // Set before the snapshot, like the desktop: the form still reads clean, and typing
    // the scale value is what makes it saveable.
    model.set_tier_constraint_kind(2);
    snapshot_form(&model);
    request_focus(&model, "constraint");
}

/// Keep Draft: the form keeps its draft and the warning goes away.
fn keep_draft(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    model.set_show_dirty_warning(false);
    model.set_pending_load_index(-1);
}

/// Discard Draft: loads the tier the selection moved to (or blanks the form for a new
/// tier).
fn discard_draft(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    let pending = model.get_pending_load_index();
    model.set_show_dirty_warning(false);
    model.set_pending_load_index(-1);
    match usize::try_from(pending) {
        Ok(idx) => {
            with_current(ctx, |pcx| apply_load(pcx, &model, idx));
        }
        Err(_) => clear_form(&model),
    }
}

/// The angle field was edited: refreshes the margin bar.
fn angle_edited(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    let app = ctx.state.borrow();
    if let Some(design_state) = &app.design {
        let design = &design_state.session.design;
        let n_d = design.effective_refractive_index_with(&app.custom_materials);
        push_live_margin(design, n_d, &model);
    }
}

/// Registers the Tier tab's callbacks.
pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<InspectorModel>();
    let c = ctx.clone();
    model.on_save_tier(move || save_tier(&c));
    let c = ctx.clone();
    model.on_new_tier(move || new_tier(&c));
    let c = ctx.clone();
    model.on_keep_draft(move || keep_draft(&c));
    let c = ctx.clone();
    model.on_discard_draft(move || discard_draft(&c));
    let c = ctx.clone();
    model.on_angle_edited(move || angle_edited(&c));
}
