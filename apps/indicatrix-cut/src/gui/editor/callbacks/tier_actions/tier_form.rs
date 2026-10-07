//! The tier inspector's Save Tier form: parsing, target/name bookkeeping, and the
//! form's own error reporting.

use std::{
    cell::{RefCell, RefMut},
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::TierTarget;
// The error-field classification and the non-integral-index warning moved to
// `indicatrix_editor::loading` (shared with the web inspector) -- see
// `report_tier_form_error`'s doc comment for what the field strings mean. Re-exported at
// their old path (`super::tests` exercises the classification directly).
pub(super) use indicatrix_editor::loading::tier_form_error_field;
use indicatrix_editor::{
    loading::{non_integral_index_warning, parse_tier_form_with_relation},
    slider_ranges::{
        AnglePreset, AngleSide, SliderSpecData, angle_presets_for, angle_spec_data,
        format_angle_text, margin_preview_text, stepped_angle_text,
    },
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{nudge::tier_nudge_label, tier_generation::next_free_block_name};
use crate::{
    EditorModel, MainWindow, SliderModel, SliderPreset, SliderSpec,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            guide, loading,
            relation_ui::{
                is_relation_text, plan_tier_save, relation_placeholder_angle_deg,
                save_error_text_and_field, with_followers,
            },
            state::{EditorState, first_unresolved_meet_name},
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// "Add Tier" / "Save Tier": parses the form and applies it through
/// `EditorState::apply` as `AddTier` (index `< 0`, "new tier" mode -- appended at the
/// end) or `ModifyTier` (an existing row's index).
/// Every OTHER tier's own name token (`ConstraintTier::names()`, split on `/`),
/// with the tier at `excluded_index` left out. Feeds `loading::TierFormFields::
/// other_tier_names` so `parse_tier_form` can reject name collisions. `excluded_index
/// < 0` (a brand-new tier) excludes nothing, since there is no existing row to exempt.
fn other_tier_names_excluding(st: &EditorState, excluded_index: i32) -> Vec<String> {
    indicatrix_editor::tier_save::other_tier_names_excluding(&st.design, excluded_index)
}

/// [`setup_save_tier_callback`]'s successful-`AddTier` label, looked up while
/// `st` is still borrowed (before the `drop(st)` [`select_and_announce_added_
/// tier`] needs so it can re-borrow `EditorModel` freely).
fn added_tier_label(st: &EditorState, dirty_index: usize) -> Option<String> {
    st.design
        .tiers
        .get(dirty_index)
        .map(|tier| tier_nudge_label(tier, dirty_index))
}

/// Selects and reveals the tier `setup_save_tier_callback`'s `AddTier` branch
/// just added, then names it in a toast. Setting `EditorModel.selected_tier_index`
/// is enough to reveal: `editor_view.slint`'s `changed tracked_selected_tier_index`
/// re-seeds the inspector form AND calls `tier_table.focus_row`, which scrolls the
/// new row into view.
fn select_and_announce_added_tier(ui: &MainWindow, dirty_index: usize, label: Option<String>) {
    ui.global::<EditorModel>()
        .set_selected_tier_index(dirty_index as i32);
    if let Some(label) = label {
        show_toast(ui, &format!("Added {label}"), "info");
    }
}

/// [`setup_save_tier_callback`]'s target-parsing step: parses
/// `constraint_kind`/`constraint_text` (the SAME two fields
/// [`loading::parse_tier_form`] already validated for its own 0/1/2 kinds)
/// via [`loading::parse_tier_target`] into the [`TierTarget`] the form's
/// Meets combo authored, if any -- kinds `3`/`4`/`5` ("cut to depth"/"girdle
/// thickness"/"table width"); `Ok(None)` for every other kind, since
/// `parse_tier_form` already turned those into a plain `ConstraintTier` with
/// no target at all.
///
/// Reports any parse error exactly the way a `parse_tier_form` failure would
/// (`report_tier_form_error`, classified via `tier_form_error_field`) and
/// returns `Err(())` -- the caller reads that as "already reported, apply
/// nothing" and bails out. `Result<Option<TierTarget>, ()>` rather than the
/// more obvious `Option<Option<TierTarget>>` purely to dodge clippy's
/// `option_option` lint; the `()` carries no information beyond "stop."
/// Split out of `setup_save_tier_callback` purely to keep that function
/// under clippy's `too_many_lines` lint.
fn parse_tier_target_reporting(
    ui: &MainWindow,
    constraint_kind: i32,
    constraint_text: &str,
) -> Result<Option<TierTarget>, ()> {
    loading::parse_tier_target(constraint_kind, constraint_text).map_err(|e| {
        report_tier_form_error(ui, &e, tier_form_error_field(&e));
    })
}

/// The table row currently selected -- a new tier is inserted right after it. The edit
/// itself (including the depth/girdle-thickness/table-width target and, when the Angle
/// field holds a relation, the relation) is built by `relation_ui::plan_tier_save` from
/// `indicatrix_editor::tier_save::tier_save_edit_with_target`.
fn selected_row(ui: &MainWindow) -> Option<usize> {
    usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()).ok()
}

/// [`setup_save_tier_callback`]'s preamble: the current row's carried-through
/// `imported_meet`/`original_notes` (for an existing tier), the gear tooth
/// count, and every OTHER tier's own name tokens -- everything [`loading::
/// parse_tier_form`] needs beyond the form's own five text/enum fields. Split
/// out purely to keep that function under clippy's function-length lint.
struct SaveTierFormContext {
    imported_meet: Option<MeetConstraint>,
    original_notes: Option<String>,
    gear_teeth_abs: u32,
    other_tier_names: Vec<String>,
}

/// Builds [`SaveTierFormContext`] from a short-lived immutable borrow of
/// `state` -- dropped before this returns, so the caller's later
/// `state.borrow_mut()` never races it.
fn save_tier_form_context(state: &Rc<RefCell<EditorState>>, index: i32) -> SaveTierFormContext {
    let st = state.borrow();
    // An existing row keeps its own `imported_meet` and the `.asc` file's
    // original `G` note across this save -- looked up here, before the
    // caller's mutable borrow, so editing an imported tier's name/angle/indices
    // never silently drops what the file claimed it meets, nor the note a
    // cutter reads while cutting it.
    let (imported_meet, original_notes) = (index >= 0)
        .then(|| {
            let tier = st.design.tiers.get(usize::try_from(index).ok()?)?;
            Some((tier.imported_meet.clone(), tier.original_notes.clone()))
        })
        .flatten()
        .unwrap_or_default();
    SaveTierFormContext {
        imported_meet,
        original_notes,
        gear_teeth_abs: st.design.meta.gear_teeth_abs(),
        other_tier_names: other_tier_names_excluding(&st, index),
    }
}

/// [`apply_tier_save_success`]'s outcome fields, bundled purely to keep that
/// function under clippy's too-many-arguments lint.
struct TierSaveOutcome {
    /// The form's own `index` argument: negative for a fresh `AddTier`, the
    /// tier's own index for a `ModifyTier`.
    index: i32,
    /// The saved tier's actual index in `st.design.tiers` after the edit applied.
    dirty_index: usize,
    /// Every row the save changed: the saved tier and the tiers that follow it through
    /// a relation (`relation_ui::with_followers`).
    dirty: BTreeSet<usize>,
    /// [`non_integral_index_warning`]'s verdict for the saved indices, if any.
    non_integral_warning: Option<String>,
}

/// [`setup_save_tier_callback`]'s success arm: clears the form's stale error state,
/// replans the preview for `outcome.dirty_index`, reveals a freshly added row, and
/// surfaces the non-integral-index warning (if any). Split out purely to keep that
/// function under clippy's function-length lint.
///
/// Takes `st` by value (not `&mut EditorState`) so the `AddTier` branch can `drop`
/// it before calling back into Slint through `select_and_announce_added_tier` --
/// exactly the same borrow-scope this body had inlined, just made explicit at the
/// call boundary.
fn apply_tier_save_success(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    st: RefMut<'_, EditorState>,
    outcome: TierSaveOutcome,
) {
    let TierSaveOutcome {
        index,
        dirty_index,
        dirty,
        non_integral_warning,
    } = outcome;
    // A successful save means the form is valid again, so this clears whatever the
    // last save's parse/validation error left behind.
    clear_tier_form_error(ui);
    // `AddTier` changes the tier count, so the alignment
    // check falls back to a full solve regardless of
    // `dirty`; for `ModifyTier` the saved tier and the tiers following it through a
    // relation are exactly what changed.
    refresh_editor_panel_stale(ui, render_ctx, &st, &dirty);
    // The guide's tier steps complete on the saved tier itself (name, angle,
    // indices) -- checked here explicitly as well as inside the stale refresh.
    guide::check_progress(ui, &st);
    submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        &st,
        dirty,
        false,
    );
    // Selects and reveals the row just added -- an `AddTier`-only branch, since a
    // `ModifyTier` save is already on the row it edited. See
    // [`select_and_announce_added_tier`]'s own doc comment for why setting
    // `selected_tier_index` alone is enough to reveal it too.
    if index < 0 {
        let label = added_tier_label(&st, dirty_index);
        drop(st);
        select_and_announce_added_tier(ui, dirty_index, label);
    }
    // The non-integral-index warning is shown last (after the "Added <label>" toast
    // above, when this was a new tier) so it is the one left on screen -- the single
    // toast slot keeps only the most recent call, and a possibly-unintentional
    // fractional index is more worth a cutter's attention than a bare confirmation
    // that the save succeeded.
    if let Some(warning) = non_integral_warning {
        show_toast(ui, &warning, "info");
    }
}

/// Preserves an existing tier's detached indices and block side (pavilion vs crown)
/// across a tier form save, or infers pavilion side from name for a new tier.
fn preserve_saved_tier_side_and_detached(
    tier: &mut indicatrix_cut_core::ConstraintTier,
    design: &indicatrix_cut_core::Design,
    index: i32,
) {
    if let Some(current) = usize::try_from(index)
        .ok()
        .and_then(|i| design.tiers.get(i))
    {
        tier.detached.clone_from(&current.detached);
        if current.angle_deg.is_sign_negative() {
            tier.angle_deg = -tier.angle_deg.abs();
        }
    } else if index < 0
        && indicatrix_cut_core::design::labelling::name_indicates_pavilion(&tier.name)
    {
        tier.angle_deg = -tier.angle_deg.abs();
    }
}

/// Settles the side of the tier a Tier form save describes
/// ([`preserve_saved_tier_side_and_detached`]) and THEN names a new tier that was left
/// unnamed.
///
/// A brand-new tier saved with a blank Name field would otherwise stay unnamed and
/// un-meetable (`ConstraintTier::names()` returns nothing for an empty name), so it is
/// named here: `G<n>` at 90 degrees, else `P<n>` or `C<n>` by the sign of its angle,
/// matching what Duplicate does for its copies. The name is read off the side the tier
/// ends up on, never off the angle the form carried in: a tier saved with a relation to a
/// pavilion tier (`=P1-2`) is a pavilion tier and is named `P<n>`, and a tier typed as a
/// crown angle is `C<n>`, whichever way the form arrived at the number. Only for a fresh
/// `AddTier` (`index < 0`): an existing tier's name was either already set or the cutter
/// just deliberately blanked it, neither of which this should override.
fn settle_saved_tier(
    tier: &mut indicatrix_cut_core::ConstraintTier,
    design: &indicatrix_cut_core::Design,
    index: i32,
    other_tier_names: &[String],
) {
    preserve_saved_tier_side_and_detached(tier, design, index);
    if index < 0 && tier.name.is_empty() {
        tier.name = next_free_block_name(tier.angle_deg, other_tier_names);
    }
}

/// The handles [`save_tier_now`] needs besides the form itself.
struct SaveServices<'a> {
    state: &'a Rc<RefCell<EditorState>>,
    render_ctx: &'a Arc<Mutex<RenderContext>>,
    preview_state: &'a Arc<SolidPreviewState>,
    solid_last_solved: &'a SolidLastSolved,
}

/// The six fields `EditorModel.save_tier` hands over, as the Save action received them.
struct TierFormInput<'a> {
    /// The tier's table row, or negative for a new tier.
    index: i32,
    /// The Angle field: a number, arithmetic (`41.5+0.3`) or a relation (`=C1-4`).
    angle: &'a str,
    constraint_kind: i32,
    constraint_text: &'a str,
    name: &'a str,
    indices: &'a str,
}

/// Parses the form into its tier and, when the Angle field holds a relation (`=...`), the
/// relation text after the `=`. Any other Angle text goes through
/// [`loading::parse_tier_form`] exactly as before. `Err` is the message to show.
fn parse_save_form(
    st: &Rc<RefCell<EditorState>>,
    form: &TierFormInput<'_>,
    context: SaveTierFormContext,
) -> Result<(indicatrix_cut_core::ConstraintTier, Option<String>), String> {
    let SaveTierFormContext {
        imported_meet,
        original_notes,
        gear_teeth_abs,
        other_tier_names,
    } = context;
    let fields = loading::TierFormFields {
        angle: form.angle,
        constraint_kind: form.constraint_kind,
        constraint_text: form.constraint_text,
        name: form.name,
        indices: form.indices,
        gear_teeth_abs,
        imported_meet,
        original_notes,
        other_tier_names,
    };
    if is_relation_text(form.angle) {
        // The relation, not the form, decides the angle: the tier carries its current
        // angle (a new tier: the one its relation would give) until the session applies it.
        let placeholder =
            relation_placeholder_angle_deg(&st.borrow().design, form.index, form.angle, form.name);
        parse_tier_form_with_relation(fields, placeholder)
    } else {
        loading::parse_tier_form(fields).map(|tier| (tier, None))
    }
}

/// The Tier form's Save action once its six fields are in: parses them
/// ([`parse_save_form`]), applies the resulting edit through the session and refreshes the
/// preview and panel on success. A refusal -- a bad field, a relation that cannot hold, a
/// direct angle edit of a tier that follows a relation -- is shown under the field it
/// belongs to ([`report_tier_form_error`]) and changes nothing.
fn save_tier_now(ui: &MainWindow, services: &SaveServices<'_>, form: &TierFormInput<'_>) {
    let SaveServices {
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    } = services;
    let index = form.index;
    let context = save_tier_form_context(state, index);
    let other_tier_names = context.other_tier_names.clone();
    let (mut tier, relation_text) = match parse_save_form(state, form, context) {
        Ok(parsed) => parsed,
        Err(e) => {
            report_tier_form_error(ui, &e, tier_form_error_field(&e));
            return;
        }
    };
    // The form's Meets combo also carries three target kinds that
    // `loading::parse_tier_form` above only turns into a `ScaleReference(0.0)`
    // placeholder -- see `parse_tier_target_reporting`'s own doc comment.
    let Ok(target) = parse_tier_target_reporting(ui, form.constraint_kind, form.constraint_text)
    else {
        return;
    };
    // Captured before `tier` is moved into the edit below -- see
    // `non_integral_index_warning`'s own doc comment for why this warns rather than
    // rejects.
    let non_integral_warning = non_integral_index_warning(&tier.indices);
    let mut st = state.borrow_mut();
    // The tier's side first, then (for a new tier left unnamed) its name -- see
    // `settle_saved_tier`.
    settle_saved_tier(&mut tier, &st.design, index, &other_tier_names);
    // A `MeetNamed` token that resolves to nothing today would
    // otherwise degrade silently inside the solver (`meet_solver`'s
    // own doc comment: "an unresolved token is dropped") -- caught
    // here, before it is ever applied, with the offending name named.
    if let MeetConstraint::MeetNamed(names) = &tier.constraint
        && let Some(bad_name) = first_unresolved_meet_name(&st.design, names)
    {
        report_tier_form_error(
            ui,
            &format!("No facet named '{bad_name}' -- check the Meets field."),
            "constraint",
        );
        return;
    }
    // Blanking the name of a tier other tiers meet by name would leave
    // their references dangling: refuse, at the Name field.
    if let Some(message) =
        indicatrix_editor::tier_save::unnaming_blocked_message(&st.design, index, form.name)
    {
        report_tier_form_error(ui, &message, "name");
        return;
    }
    // The edit: the tier as the form describes it and, for `=...`, its relation in the same
    // undo step. The session then keeps every relation true and refuses what cannot be.
    let applied = plan_tier_save(
        &st.design,
        index,
        tier,
        relation_text.as_deref(),
        target,
        selected_row(ui),
    )
    .and_then(|(dirty_index, edit)| st.try_apply(edit).map(|_| dirty_index));
    match applied {
        Ok(dirty_index) => {
            // Followers (tiers whose angle follows the saved one) moved with it.
            let dirty = with_followers(&st.design, [dirty_index]);
            apply_tier_save_success(
                ui,
                render_ctx,
                preview_state,
                solid_last_solved,
                st,
                TierSaveOutcome {
                    index,
                    dirty_index,
                    dirty,
                    non_integral_warning,
                },
            );
        }
        Err(error) => {
            let (message, field) = save_error_text_and_field(&error);
            report_tier_form_error(ui, &message, field);
        }
    }
}

/// The UI's [`SliderSpec`] for the plain data `indicatrix_editor::slider_ranges` worked out:
/// numbers become `f32`, a missing band becomes an empty one (`band_to <= band_from`).
/// `pub(super)` since the Preform tab's sliders map their data the same way.
pub(super) fn slider_spec_from(data: SliderSpecData) -> SliderSpec {
    let (band_from, band_to) = data.band.unwrap_or((0.0, 0.0));
    SliderSpec {
        visible: data.visible,
        usable: data.usable,
        side: data.side.into(),
        value: data.value as f32,
        minimum: data.range.min as f32,
        maximum: data.range.max as f32,
        step: data.range.step as f32,
        fine_step: data.range.fine_step as f32,
        mark: data.range.mark as f32,
        band_from: band_from as f32,
        band_to: band_to as f32,
        note: data.note.into(),
        value_text: data.value_text.into(),
    }
}

/// The "Typical" menu's rows for the UI.
fn preset_rows(presets: Vec<AnglePreset>) -> ModelRc<SliderPreset> {
    let rows: Vec<SliderPreset> = presets
        .into_iter()
        .map(|preset| SliderPreset {
            label: preset.label.into(),
            value: preset.value_deg as f32,
            value_text: preset.value_text.into(),
            reason: preset.reason.into(),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// Answers the Tier form's slider (`SliderModel`'s angle callbacks). All four are pure
/// functions of their arguments -- the design's refractive index arrives as text, so the
/// answers follow a change of material without this code touching the editor state -- and
/// the work is done in `indicatrix_editor::slider_ranges`:
///
/// - `angle_spec`: whether the form has a slider, its range, steps, marked band and value;
/// - `angle_presets`: the "Typical" menu;
/// - `angle_text`: the number the slider writes into the Angle field;
/// - `stepped_angle_text`: the field's text after an Up or Down key press (empty when the
///   field holds a calculation or a relation, which the key leaves alone);
/// - `margin_preview_text`: the Angle text as the live margin bar should read it.
///
/// The slider only fills the Angle field in; the tier is saved by "Save Tier" as ever.
fn setup_angle_slider_callbacks(ui: &MainWindow) {
    let model = ui.global::<SliderModel>();
    model.on_angle_spec(|kind, text, name, ri_text| {
        slider_spec_from(angle_spec_data(&kind, &text, &name, &ri_text))
    });
    model.on_angle_presets(|side, ri_text, material| {
        preset_rows(angle_presets_for(&side, &ri_text, &material))
    });
    model.on_angle_text(|value, current| format_angle_text(f64::from(value), &current).into());
    model.on_stepped_angle_text(|current, delta| {
        stepped_angle_text(&current, f64::from(delta))
            .unwrap_or_default()
            .into()
    });
    model.on_margin_preview_text(|side, text| {
        margin_preview_text(AngleSide::from_name(&side), &text).into()
    });
}

/// The Tier form's Save action: hands the form to [`save_tier_now`], which parses it,
/// applies the resulting edit through the session and refreshes the preview and panel on
/// success.
pub(in crate::gui::editor) fn setup_save_tier_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    // The concave form's three callbacks are wired from here: this is the one existing
    // call site that already receives every argument they need.
    super::concave_tier::setup_concave_tier_callbacks(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    // So are the Angle field's slider callbacks (they need no state at all).
    setup_angle_slider_callbacks(ui);
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_save_tier(
        move |index: i32,
              angle: SharedString,
              constraint_kind: i32,
              constraint_text: SharedString,
              name: SharedString,
              indices: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            save_tier_now(
                &ui,
                &SaveServices {
                    state: &state,
                    render_ctx: &render_ctx,
                    preview_state: &preview_state,
                    solid_last_solved: &solid_last_solved,
                },
                &TierFormInput {
                    index,
                    angle: &angle,
                    constraint_kind,
                    constraint_text: &constraint_text,
                    name: &name,
                    indices: &indices,
                },
            );
        },
    );
}

/// Clears the inline error state a failed save left behind -- what a successful save of
/// either form (flat or concave) does, so a marker never outlives the failure it reports.
pub(super) fn clear_tier_form_error(ui: &MainWindow) {
    let model = ui.global::<EditorModel>();
    model.set_tier_form_error("".into());
    model.set_tier_form_error_field("".into());
}

/// Shows one tier-form failure in both places a cutter looks: inline under the
/// field that caused it (`EditorModel.tier_form_error`, which the inspector renders
/// in red) and as a toast. Factored out because all three failure branches of
/// [`setup_save_tier_callback`] must agree on the wording -- an inline message that
/// disagrees with the toast is worse than either alone.
///
/// `field` is one of `"angle"`/`"name"`/
/// `"indices"`/`"constraint"`, or `""` for a general/unclassified error -- see
/// `EditorModel.tier_form_error_field`'s own doc comment (`ui/models/editor.slint`)
/// for the exact contract `editor_inspector.slint`
/// reads this against to put a red border on the SPECIFIC offending control,
/// not only the shared message under the whole form.
pub(super) fn report_tier_form_error(ui: &MainWindow, message: &str, field: &str) {
    let model = ui.global::<EditorModel>();
    model.set_tier_form_error(message.into());
    model.set_tier_form_error_field(field.into());
    show_toast(ui, message, "error");
}

#[cfg(test)]
mod side_tests;
#[cfg(test)]
mod slider_tests;
