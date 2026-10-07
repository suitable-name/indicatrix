//! Slint glue of the material editor's physics color mode.
//!
//! [`PhysicsState`] (pure, in `physics_state`) owns the recipe, the undo stack and the picked
//! color; this module wires the `PhysicscolorModel` callbacks to it, pushes the derived
//! [`PhysicsView`] into the model after every change, and runs the inverse solver on the
//! worker of `physics_solver` -- the UI thread only edits and renders, it never solves.
//!
//! The row/option models are updated *in place* (`set_row_data`): replacing a `for` model
//! mid-drag would recreate the delegates and cancel the slider drag in progress.

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use indicatrix::optics::chromophore::ChromophoreCatalogue;
use indicatrix_cut_core::material::color::nearest_legacy_rgb;

use super::{
    custom_materials::{
        CUSTOM_KEEP_COLOR_INDEX, absorption_rgb_for_color_index, color_index_for_absorption_rgb,
    },
    physics_solver::{SolveOutcome, SolverWorker},
    physics_state::{
        ModeSwatches, PHYSICS_COLOR_UI, PhysicsState, PhysicsView, Prefill, SolveJob,
        color_section_view,
    },
};
use crate::{
    MainWindow, PhysicsAddOption, PhysicsRow, PhysicsTreatment, PhysicscolorModel, ViewportModel,
    bridge::render_thread::RenderContext,
};

type SharedState = Arc<Mutex<PhysicsState>>;

/// Everything a callback needs; cheap to clone into each closure.
#[derive(Clone)]
struct Ctrl {
    state: SharedState,
    worker: Arc<SolverWorker>,
    ui: slint::Weak<MainWindow>,
    render_ctx: Arc<Mutex<RenderContext>>,
}

fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, PhysicsState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const fn color(rgb: [u8; 3]) -> slint::Color {
    slint::Color::from_rgb_u8(rgb[0], rgb[1], rgb[2])
}

/// `#rrggbb` text of `c` (the picker's hex field).
#[must_use]
pub fn format_hex(c: slint::Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
}

/// Parses `#rgb`/`#rrggbb` (the `#` optional); `None` for anything else.
#[must_use]
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().trim_start_matches('#');
    let digits: Vec<u8> = t
        .chars()
        .map(|c| c.to_digit(16).and_then(|d| u8::try_from(d).ok()))
        .collect::<Option<_>>()?;
    match digits.as_slice() {
        [r, g, b] => Some([r * 17, g * 17, b * 17]),
        [r1, r2, g1, g2, b1, b2] => Some([r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2]),
        _ => None,
    }
}

/// Updates `model` to `rows` without replacing it when only values changed.
fn sync_model<T: Clone + 'static>(
    model: &ModelRc<T>,
    rows: Vec<T>,
    same_item: impl Fn(&T, &T) -> bool,
    differs: impl Fn(&T, &T) -> bool,
) -> Option<ModelRc<T>> {
    let Some(vec) = model.as_any().downcast_ref::<VecModel<T>>() else {
        return Some(ModelRc::new(VecModel::from(rows)));
    };
    let compatible = vec.row_count() == rows.len()
        && rows
            .iter()
            .enumerate()
            .all(|(i, r)| vec.row_data(i).is_some_and(|old| same_item(&old, r)));
    if !compatible {
        vec.set_vec(rows);
        return None;
    }
    for (i, r) in rows.into_iter().enumerate() {
        if vec.row_data(i).is_none_or(|old| differs(&old, &r)) {
            vec.set_row_data(i, r);
        }
    }
    None
}

fn push_swatches(model: &PhysicscolorModel<'_>, view: &PhysicsView) {
    let white = [255, 255, 255];
    let set = |s: Option<&ModeSwatches>| -> [slint::Color; 4] {
        s.map_or_else(
            || [color(white); 4],
            |s| {
                [
                    color(s.unpol),
                    color(s.o),
                    color(s.e),
                    color(s.beta.unwrap_or(s.e)),
                ]
            },
        )
    };
    let d = set(view.d65.as_ref());
    let a = set(view.a.as_ref());
    model.set_d65_unpol(d[0]);
    model.set_d65_o(d[1]);
    model.set_d65_e(d[2]);
    model.set_d65_beta(d[3]);
    model.set_a_unpol(a[0]);
    model.set_a_o(a[1]);
    model.set_a_e(a[2]);
    model.set_a_beta(a[3]);
    model.set_has_beta(view.has_beta);
    if let Some((d65, a)) = view.stone {
        model.set_show_stone(true);
        model.set_stone_d65(color(d65));
        model.set_stone_a(color(a));
    } else {
        model.set_show_stone(false);
    }
    model.set_stone_text(view.stone_text.clone().into());
    model.set_fluorescence_text(view.fluorescence_text.clone().into());
    model.set_quench_text(view.quench_text.clone().into());
    model.set_has_glow(view.glow.is_some());
    if let Some([uv365, uv395]) = &view.glow {
        model.set_glow365(color(uv365.srgb));
        model.set_glow365_text(uv365.text.clone().into());
        model.set_glow395(color(uv395.srgb));
        model.set_glow395_text(uv395.text.clone().into());
    }
}

/// Pushes what the dialog's color area shows (toggle, recipe section, Fantasy controls, note).
fn push_section(model: &PhysicscolorModel<'_>, mode_is_physics: bool) {
    let section = color_section_view(PHYSICS_COLOR_UI, mode_is_physics);
    model.set_show_mode_toggle(section.shows_mode_toggle());
    model.set_show_physics_section(section.shows_physics_section());
    model.set_show_fantasy_section(section.shows_fantasy_section());
    model.set_show_recipe_note(section.shows_recipe_note());
}

/// Pushes `view` (and a pending optics `prefill`) into the Slint model.
fn push_view(ui: &MainWindow, view: &PhysicsView, prefill: Option<&Prefill>) {
    let model = ui.global::<PhysicscolorModel>();
    model.set_active(view.active);
    push_section(&model, view.active);
    model.set_dirty(view.dirty);
    model.set_mode_json(view.mode_json.clone().into());
    model.set_host_index(i32::try_from(view.host_index).unwrap_or(0));
    model.set_banner_visible(view.banner.is_some());
    model.set_banner_text(view.banner.clone().unwrap_or_default().into());
    model.set_data_updated(view.data_updated);
    model.set_view_fractions(view.view_fractions);
    model.set_strength(view.strength as f32);
    model.set_reference_path_mm(view.reference_path_mm);
    model.set_path_hint(view.path_hint.clone().into());
    model.set_solving(view.solving);
    model.set_can_undo(view.can_undo);
    model.set_can_redo(view.can_redo);
    model.set_optics_locked(view.active && view.optics_locked);
    model.set_color_change_text(view.color_change_text.clone().into());
    model.set_has_target(view.target.is_some());
    if let Some(t) = view.target {
        model.set_target_color(color(t));
    }
    model.set_match_text(view.match_text.clone().into());
    model.set_match_warn(view.match_warn);
    model.set_note_text(view.note_text.clone().into());
    push_swatches(&model, view);

    let host_names: Vec<SharedString> = view.host_names.iter().map(SharedString::from).collect();
    if model.get_host_names().row_count() != host_names.len() {
        model.set_host_names(ModelRc::new(VecModel::from(host_names)));
    }
    let rows: Vec<PhysicsRow> = view
        .rows
        .iter()
        .map(|r| PhysicsRow {
            id: r.id.clone().into(),
            label: r.label.clone().into(),
            amount_text: r.amount_text.clone().into(),
            unit: r.unit.clone().into(),
            slider: r.slider as f32,
            fraction: r.fraction as f32,
            fraction_text: r.fraction_text.clone().into(),
            locked: r.locked,
            estimated: r.estimated,
            confidence: r.confidence.clone().into(),
            sources: r.sources.clone().into(),
        })
        .collect();
    if let Some(fresh) = sync_model(&model.get_rows(), rows, |a, b| a.id == b.id, |a, b| a != b) {
        model.set_rows(fresh);
    }
    let options: Vec<PhysicsAddOption> = view
        .add_options
        .iter()
        .map(|o| PhysicsAddOption {
            id: o.id.clone().into(),
            label: o.label.clone().into(),
            enabled: o.enabled,
            reason: o.reason.clone().into(),
        })
        .collect();
    if let Some(fresh) = sync_model(
        &model.get_add_options(),
        options,
        |a, b| a.id == b.id,
        |a, b| a != b,
    ) {
        model.set_add_options(fresh);
    }
    let treatments: Vec<PhysicsTreatment> = view
        .treatments
        .iter()
        .map(|t| PhysicsTreatment {
            id: t.id.clone().into(),
            name: t.name.clone().into(),
            conditions: t.conditions.clone().into(),
            active: t.active,
        })
        .collect();
    if let Some(fresh) = sync_model(
        &model.get_treatments(),
        treatments,
        |a, b| a.id == b.id,
        |a, b| a != b,
    ) {
        model.set_treatments(fresh);
    }
    if let Some(p) = prefill {
        model.set_prefill_ri(p.ri);
        model.set_prefill_dispersion(p.dispersion);
        model.set_prefill_birefringence(p.birefringence);
        model.set_prefill_sg(p.specific_gravity);
        model.set_prefill_crystal_system_idx(p.crystal_system_idx);
        model.set_prefill_optical_character_idx(p.optical_character_idx);
        model.set_prefill_biaxial_delta(p.biaxial_delta);
        model.set_prefill_serial(model.get_prefill_serial() + 1);
    }
}

impl Ctrl {
    /// Runs `edit` on the state, pushes the resulting view, and hands any solver job to the
    /// worker -- never blocking on the solve.
    fn edit(
        &self,
        edit: impl FnOnce(&mut PhysicsState, &ChromophoreCatalogue) -> Option<SolveJob>,
    ) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        self.refresh_stone_width();
        let cat = ChromophoreCatalogue::global();
        let (job, view, prefill) = {
            let mut state = lock(&self.state);
            let job = edit(&mut state, cat);
            (job, state.view(cat), state.take_prefill())
        };
        if let Some(job) = job {
            self.worker.submit(job);
        }
        push_view(&ui, &view, prefill.as_ref());
    }

    fn refresh_stone_width(&self) {
        let mm = self
            .render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stone_width_mm;
        lock(&self.state).stone_width_mm = mm;
    }
}

/// The solver finished on the worker: apply it on the UI thread if it is still current.
fn on_solved(ui_weak: &slint::Weak<MainWindow>, state: &SharedState, outcome: SolveOutcome) {
    let state = Arc::clone(state);
    let ui_weak = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let cat = ChromophoreCatalogue::global();
        let (view, prefill) = {
            let mut s = lock(&state);
            // Stale generations are dropped here as the second guard.
            if !s.finish_solve(outcome.generation, outcome.result, cat) {
                return;
            }
            (s.view(cat), s.take_prefill())
        };
        push_view(&ui, &view, prefill.as_ref());
    });
}

fn preset_rgb_for(idx: i32) -> Option<[f32; 3]> {
    (idx != CUSTOM_KEEP_COLOR_INDEX).then(|| absorption_rgb_for_color_index(idx))
}

fn idx_for_rgb(rgb: [f32; 3]) -> i32 {
    color_index_for_absorption_rgb(rgb).unwrap_or(CUSTOM_KEEP_COLOR_INDEX)
}

/// Rebuilds the physics state from the selected custom material (every dialog open).
fn open_dialog(ctrl: &Ctrl) {
    let Some(ui) = ctrl.ui.upgrade() else {
        return;
    };
    let vm = ui.global::<ViewportModel>();
    let valid = vm.get_selected_custom_material_valid();
    let (json, rgb, name) = if valid {
        let rgb = vm.get_selected_custom_material_rgb();
        let rgb = [
            rgb.row_data(0).unwrap_or(0.0),
            rgb.row_data(1).unwrap_or(0.0),
            rgb.row_data(2).unwrap_or(0.0),
        ];
        let name = usize::try_from(vm.get_selected_material_index())
            .ok()
            .and_then(|i| vm.get_material_options().row_data(i))
            .unwrap_or_default();
        (
            vm.get_selected_custom_material_color_recipe_json()
                .to_string(),
            rgb,
            name.to_string(),
        )
    } else {
        (String::new(), [0.0; 3], String::new())
    };
    let stone = ctrl
        .render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .stone_width_mm;
    let cat = ChromophoreCatalogue::global();
    let (view, prefill) = {
        let mut state = lock(&ctrl.state);
        state.cancel_solves();
        let generation = state.generation;
        *state = PhysicsState::open(&json, rgb, &name, stone, cat, generation);
        (state.view(cat), None::<Prefill>)
    };
    push_view(&ui, &view, prefill.as_ref());
}

fn set_mode(ctrl: &Ctrl, physics: bool, preset_idx: i32) -> i32 {
    let mut new_idx = preset_idx;
    ctrl.edit(|s, cat| {
        let job = s.set_mode(physics, preset_rgb_for(preset_idx), cat);
        if !physics {
            new_idx = idx_for_rgb(s.mode.fantasy_rgb);
        }
        job
    });
    new_idx
}

fn host_selected(ctrl: &Ctrl, index: i32) {
    ctrl.edit(|s, cat| {
        if let Some(host) = usize::try_from(index).ok().and_then(|i| cat.hosts.get(i)) {
            s.select_host(&host.id, cat);
        }
        None
    });
}

fn pick_solve(ctrl: &Ctrl, c: slint::Color) {
    ctrl.edit(|s, _| {
        s.is_physics()
            .then(|| s.pick([c.red(), c.green(), c.blue()]))
    });
}

/// The fantasy picker: fits the three legacy bands on a thread, then selects the result.
fn fantasy_pick(ctrl: &Ctrl, c: slint::Color) {
    let generation = lock(&ctrl.state).generation;
    let lab = indicatrix::color::body_color::srgb_to_lab([
        f64::from(c.red()) / 255.0,
        f64::from(c.green()) / 255.0,
        f64::from(c.blue()) / 255.0,
    ]);
    let ctrl = ctrl.clone();
    std::thread::spawn(move || {
        let rgb = nearest_legacy_rgb(lab);
        let state = Arc::clone(&ctrl.state);
        let ui_weak = ctrl.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let view = {
                let mut s = lock(&state);
                if s.generation != generation {
                    return;
                }
                s.set_fantasy_pick(rgb);
                s.view(ChromophoreCatalogue::global())
            };
            push_view(&ui, &view, None);
            let model = ui.global::<PhysicscolorModel>();
            model.set_fantasy_idx(idx_for_rgb(rgb));
            model.set_fantasy_serial(model.get_fantasy_serial() + 1);
        });
    });
}

/// The L*C*h editor's "Apply colour" in the material editor dialog: the solved colour's nearest
/// legacy triple becomes the fantasy colour, exactly like a finished [`fantasy_pick`] (the
/// seven-band rows are held by `dialog_color` until "Save"). Runs on the UI thread.
fn fixed_color_picked(ctrl: &Ctrl, rgb: [f32; 3]) {
    let Some(ui) = ctrl.ui.upgrade() else {
        return;
    };
    let view = {
        let mut s = lock(&ctrl.state);
        s.set_fantasy_pick(rgb);
        s.view(ChromophoreCatalogue::global())
    };
    push_view(&ui, &view, None);
    let model = ui.global::<PhysicscolorModel>();
    model.set_fantasy_idx(idx_for_rgb(rgb));
    model.set_fantasy_serial(model.get_fantasy_serial() + 1);
}

/// A preset swatch (or a template's color) was chosen: returns the swatch to select.
///
/// Only with the physics editor hidden and a physics recipe in force does this do anything:
/// the choice replaces the recipe's color with that fixed color (the existing switch-to-Fantasy
/// path, [`PhysicsState::choose_fixed_color`], which keeps the recipe in the saved JSON). In
/// every other case -- the editor is shown, or the material is plainly Fantasy -- the swatch is
/// just the dialog's own selection and this returns it unchanged.
fn color_preset_chosen(ctrl: &Ctrl, idx: i32) -> i32 {
    // A preset swatch replaces the L*C*h editor's seven-band colour.
    super::dialog_color::preset_chosen(preset_rgb_for(idx));
    let replaces_recipe =
        color_section_view(PHYSICS_COLOR_UI, lock(&ctrl.state).is_physics()).shows_recipe_note();
    if !replaces_recipe {
        return idx;
    }
    let mut shown = idx;
    ctrl.edit(|s, _| {
        s.choose_fixed_color(preset_rgb_for(idx));
        if idx == CUSTOM_KEEP_COLOR_INDEX {
            // "Keep" may have been seeded from the recipe: show the swatch that matches it.
            shown = idx_for_rgb(s.mode.fantasy_rgb);
        }
        None
    });
    shown
}

/// Applies the physics recipe held in `ctx.pending_color_choice`; returns the material's name.
///
/// This is the "keep the physics recipe" answer for an opened file whose top-level color an
/// older build edited: the recipe's material replaces the edited one and the material renders
/// from its recipe again. `None` when nothing was pending.
pub(in crate::gui) fn keep_pending_recipe(ctx: &mut RenderContext) -> Option<String> {
    let (name, recipe_material, recipe_glow) = ctx.pending_color_choice.take()?;
    let materials = Arc::make_mut(&mut ctx.custom_materials);
    if let Some(pos) = materials
        .iter()
        .position(|m| m.name.eq_ignore_ascii_case(&name))
    {
        materials[pos] = recipe_material;
    }
    ctx.set_custom_material_physics(&name, true);
    ctx.set_custom_material_fluorescence(&name, recipe_glow);
    ctx.dirty = true;
    Some(name)
}

fn color_conflict_resolved(ctrl: &Ctrl, keep_recipe: bool) {
    let Some(ui) = ctrl.ui.upgrade() else {
        return;
    };
    ui.global::<PhysicscolorModel>()
        .set_color_conflict_name(SharedString::new());
    let mut ctx = ctrl
        .render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if keep_recipe {
        keep_pending_recipe(&mut ctx);
    } else {
        // "Use the edited color": that color is already in use, so only the held recipe goes.
        ctx.pending_color_choice = None;
    }
}

/// Registers every `PhysicscolorModel` callback.
pub(in crate::gui) fn setup_physics_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state: SharedState = Arc::new(Mutex::new(PhysicsState::open(
        "",
        [0.0; 3],
        "",
        0.0,
        ChromophoreCatalogue::global(),
        0,
    )));
    let worker = {
        let state = Arc::clone(&state);
        let ui_weak = ui.as_weak();
        Arc::new(SolverWorker::spawn(move |outcome| {
            on_solved(&ui_weak, &state, outcome);
        }))
    };
    let ctrl = Ctrl {
        state,
        worker,
        ui: ui.as_weak(),
        render_ctx: Arc::clone(render_ctx),
    };
    let m = ui.global::<PhysicscolorModel>();
    // Whether this build shows the physics editor (the `physical-color` feature). Every
    // callback below is registered either way: the Fantasy picker's hex helpers and the save
    // path's `mode_json` depend on them, and skipping them would save a material's recipe away.
    m.set_ui_enabled(PHYSICS_COLOR_UI);

    let c = ctrl.clone();
    m.on_dialog_opened(move || open_dialog(&c));
    let c = ctrl.clone();
    m.on_set_mode(move |physics, idx| set_mode(&c, physics, idx));
    let c = ctrl.clone();
    m.on_host_selected(move |i| host_selected(&c, i));
    register_recipe_callbacks(&m, &ctrl);
    register_pick_callbacks(&m, &ctrl);
}

/// The recipe-editing callbacks: amounts, fractions, view, locks, items, strength, treatments
/// and path.
fn register_recipe_callbacks(m: &PhysicscolorModel<'_>, ctrl: &Ctrl) {
    let c = ctrl.clone();
    m.on_amount_changed(move |id, pos| {
        c.edit(|s, cat| {
            s.set_amount_pos(&id, f64::from(pos), cat);
            None
        });
    });
    let c = ctrl.clone();
    m.on_amount_released(move || {
        c.edit(|s, _| {
            s.release();
            None
        });
    });
    let c = ctrl.clone();
    m.on_fraction_changed(move |id, f| {
        c.edit(|s, cat| {
            s.set_fraction(&id, f64::from(f), cat);
            None
        });
    });
    let c = ctrl.clone();
    m.on_view_changed(move |fractions| {
        c.edit(|s, _| {
            s.view_fractions = fractions;
            None
        });
    });
    let c = ctrl.clone();
    m.on_lock_toggled(move |id| {
        c.edit(|s, _| {
            s.toggle_lock(&id);
            None
        });
    });
    let c = ctrl.clone();
    m.on_remove_item(move |id| {
        c.edit(|s, cat| {
            s.remove_item(&id, cat);
            None
        });
    });
    let c = ctrl.clone();
    m.on_add_item(move |id| {
        c.edit(|s, cat| {
            s.add_item(&id, cat);
            None
        });
    });
    let c = ctrl.clone();
    m.on_strength_changed(move |v| {
        c.edit(|s, cat| {
            s.set_strength(f64::from(v), cat);
            None
        });
    });
    let c = ctrl.clone();
    m.on_strength_released(move || {
        c.edit(|s, _| {
            s.release();
            None
        });
    });
    let c = ctrl.clone();
    m.on_treatment_toggled(move |id, on| {
        c.edit(|s, cat| {
            s.toggle_treatment(&id, on, cat);
            None
        });
    });
    let c = ctrl.clone();
    m.on_path_changed(move |mm| {
        c.edit(|s, cat| {
            s.set_path(mm, cat);
            None
        });
    });
    let c = ctrl.clone();
    m.on_path_released(move || {
        c.edit(|s, _| {
            s.release();
            None
        });
    });
}

/// The pick/solve, fantasy picker, data-update, undo/redo, template, conflict and hex callbacks.
fn register_pick_callbacks(m: &PhysicscolorModel<'_>, ctrl: &Ctrl) {
    let c = ctrl.clone();
    m.on_pick_solve(move |col| pick_solve(&c, col));
    let c = ctrl.clone();
    m.on_fantasy_pick(move |col| fantasy_pick(&c, col));
    let c = ctrl.clone();
    m.on_fixed_color_picked(move |r, g, b| fixed_color_picked(&c, [r, g, b]));
    let c = ctrl.clone();
    m.on_update_data(move || {
        c.edit(|s, cat| {
            s.update_data(cat);
            None
        });
    });
    let c = ctrl.clone();
    m.on_undo(move || {
        c.edit(|s, _| {
            s.undo();
            None
        });
    });
    let c = ctrl.clone();
    m.on_redo(move || {
        c.edit(|s, _| {
            s.redo();
            None
        });
    });
    let c = ctrl.clone();
    m.on_template_selected(move |name| {
        c.edit(|s, cat| {
            // Never with the editor hidden: a new host would replace the recipe it cannot show.
            if PHYSICS_COLOR_UI
                && s.is_physics()
                && let Some(host) = cat.host_for_material(&name)
            {
                s.select_host(&host.id, cat);
            }
            None
        });
    });
    let c = ctrl.clone();
    m.on_color_preset_chosen(move |idx| color_preset_chosen(&c, idx));
    let c = ctrl.clone();
    m.on_color_conflict_resolved(move |keep| color_conflict_resolved(&c, keep));
    m.on_format_hex(|c| format_hex(c).into());
    m.on_parse_hex(|text, fallback| parse_hex(&text).map_or(fallback, color));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn physics_ruby_without_bands() -> indicatrix::optics::materials::GemMaterial {
        // A pure-host recipe resolves to NO bands: the flag must not be inferred from them.
        indicatrix::optics::materials::GemMaterial::new_custom(
            "Pure Host",
            1.76,
            0.018,
            -0.008,
            [0.0; 3],
        )
    }

    fn pure_host_context() -> RenderContext {
        RenderContext {
            custom_materials: Arc::new(vec![physics_ruby_without_bands()]),
            material_name: "Pure Host".to_string(),
            ..RenderContext::default()
        }
    }

    #[test]
    fn the_render_context_derives_the_flag_from_the_explicit_list() {
        let mut ctx = pure_host_context();
        assert!(!ctx.physics_color(), "not flagged yet");
        ctx.set_custom_material_physics("pure host", true);
        assert!(ctx.physics_color(), "case-insensitive, and no bands needed");
        // The editor's override (what the design traces) wins over the selected name.
        ctx.material_override = Some(indicatrix::optics::materials::GemMaterial::diamond());
        assert!(!ctx.physics_color());
        ctx.material_override = None;
        ctx.set_custom_material_physics("Pure Host", false);
        assert!(!ctx.physics_color());
    }

    /// Export and the remote scene read the same flag: a zero-band physics material takes the
    /// 7 mm default stone width, the same material unflagged keeps scale 1.
    #[test]
    fn export_and_remote_snapshots_apply_the_default_width_to_a_physics_material() {
        use crate::bridge::export_thread::SceneSnapshot;
        let capture = |physics: bool| {
            let mut ctx = pure_host_context();
            ctx.set_custom_material_physics("Pure Host", physics);
            SceneSnapshot::capture(&Mutex::new(ctx)).expect("resolves")
        };
        let plain = capture(false);
        let physics = capture(true);
        assert!((plain.material.absorption_path_scale - 1.0).abs() < f32::EPSILON);
        assert!((physics.material.absorption_path_scale - 1.0).abs() > 1e-3);
    }

    /// Without the physics editor an opened file's "edited by an older version" question is
    /// answered "keep the recipe" on the spot: the recipe's material replaces the edited one and
    /// renders as physics again.
    #[test]
    fn keeping_the_pending_recipe_restores_the_recipe_material() {
        use indicatrix::optics::{fluorescence::Fluorescence, materials::GemMaterial};
        let edited = GemMaterial::new_custom("Pure Host", 1.76, 0.018, -0.008, [0.9, 0.1, 0.1]);
        let from_recipe = GemMaterial::new_custom("Pure Host", 1.76, 0.018, -0.008, [0.0; 3]);
        assert_ne!(edited.absorption, from_recipe.absorption);
        let mut ctx = RenderContext {
            custom_materials: Arc::new(vec![edited]),
            material_name: "Pure Host".to_string(),
            dirty: false,
            pending_color_choice: Some((
                "Pure Host".to_string(),
                from_recipe.clone(),
                Fluorescence::new(Vec::new()),
            )),
            ..RenderContext::default()
        };
        assert!(
            !ctx.physics_color(),
            "the edited color is in use until answered"
        );

        assert_eq!(keep_pending_recipe(&mut ctx).as_deref(), Some("Pure Host"));
        assert!(ctx.pending_color_choice.is_none());
        assert_eq!(ctx.custom_materials[0].absorption, from_recipe.absorption);
        assert!(ctx.physics_color());
        assert!(ctx.dirty, "the viewport re-renders with the recipe");

        assert_eq!(keep_pending_recipe(&mut ctx), None, "nothing left to apply");
    }

    #[test]
    fn hex_formats_and_parses() {
        assert_eq!(
            format_hex(slint::Color::from_rgb_u8(255, 0, 128)),
            "#ff0080"
        );
        assert_eq!(parse_hex("#ff0080"), Some([255, 0, 128]));
        assert_eq!(parse_hex("FF0080"), Some([255, 0, 128]));
        assert_eq!(parse_hex(" #f08 "), Some([255, 0, 136]));
        assert_eq!(parse_hex("#ff00"), None);
        assert_eq!(parse_hex("#gg0000"), None);
        assert_eq!(parse_hex(""), None);
    }
}
