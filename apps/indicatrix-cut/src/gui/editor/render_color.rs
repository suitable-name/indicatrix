//! The editor's side of the Live Render toolbar's "Color" control
//! (`gui::render::render_body_color`): reading and editing the open design's body-colour
//! override while the view shows that design with the render material linked to it.
//!
//! A pick is the same edit the Edit tab's color box makes ([`Edit::SetMaterial`] with the
//! design's material and a new `body_color_override`): one undo step, the design is marked
//! changed, and the Edit tab's own controls follow through the usual panel refresh. Unlike
//! the color box it applies only the colour, never a half-typed material or RI in the
//! Design settings form.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use indicatrix_cut_core::{Edit, MaterialSelection};
use indicatrix_editor::lch_color::{self, LchSolve};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{
    state::EditorState,
    view::{refresh_editor_panel_stale, submit_preview_replan},
};
use crate::{
    BodyColorEditorModel, MainWindow, PhysicscolorModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        optics::dialog_color::{self, DialogColor},
        render::render_body_color::{
            ColorOverride, DesignColorPort, setup_render_body_color_callbacks,
        },
        show_toast,
        solid_preview::preview_state::{SolidLastSolved, SolidPreviewState},
    },
};

/// The selection the design would have with `color` as its body-colour override (the
/// L*C*h editor's band form is dropped with it), or `None` when it already has exactly that
/// colour (nothing to edit, so no undo step is added).
fn recoloured(material: &MaterialSelection, color: ColorOverride) -> Option<MaterialSelection> {
    let next = material.clone().with_body_color(color);
    (next != *material).then_some(next)
}

/// Changes the open design's material: `change` maps the current selection to the new one
/// (`None`: nothing to change). One undo step, panel and preview refreshed.
type MaterialEdit =
    Rc<dyn Fn(&dyn Fn(&MaterialSelection) -> Option<MaterialSelection>) -> Result<(), String>>;

/// Registers the toolbar's "Color" control against the open design.
pub(super) fn setup_render_color_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    // `try_borrow`: the control refreshes from Slint `changed` handlers, which can run
    // while a writer elsewhere holds the editor state (a dialog pumping the event loop).
    // It then keeps what it shows and refreshes on the next change.
    let state_read = Rc::clone(state);
    let current = Box::new(move || {
        state_read
            .try_borrow()
            .ok()
            .map(|st| st.design.material.body_color_override)
    });

    let state_edit = Rc::clone(state);
    let render_ctx_edit = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    let edit: MaterialEdit = Rc::new(move |change: &dyn Fn(
        &MaterialSelection,
    ) -> Option<MaterialSelection>|
          -> Result<(), String> {
        let Some(ui) = ui_weak.upgrade() else {
            return Ok(());
        };
        let mut st = state_edit.try_borrow_mut().map_err(|_| {
            "The design is busy right now. Try the color again in a moment.".to_string()
        })?;
        let Some(material) = change(&st.design.material) else {
            return Ok(());
        };
        st.apply(Edit::SetMaterial { material })
            .map_err(|error| error.to_string())?;
        // A material change never moves a tier's own mast.
        refresh_editor_panel_stale(&ui, &render_ctx_edit, &st, &BTreeSet::new());
        submit_preview_replan(
            &ui,
            &render_ctx_edit,
            &preview_state,
            &solid_last_solved,
            &st,
            BTreeSet::new(),
            false,
        );
        Ok(())
    });

    setup_lch_editor(ui, state, render_ctx, Rc::clone(&edit));
    setup_render_body_color_callbacks(
        ui,
        render_ctx,
        DesignColorPort {
            current,
            apply: Box::new(move |color| edit(&|material| recoloured(material, color))),
        },
    );
}

/// Pushes a finished solve to the editor: swatches with their captions, the badge, `busy` off.
fn push_solve(model: &BodyColorEditorModel<'_>, solve: &LchSolve) {
    let colours: Vec<slint::Color> = solve
        .swatches
        .iter()
        .map(|rgb| {
            let [r, g, b] = lch_color::swatch_bytes(*rgb);
            slint::Color::from_rgb_u8(r, g, b)
        })
        .collect();
    let labels: Vec<SharedString> = lch_color::swatch_labels(solve.path_mm)
        .into_iter()
        .map(SharedString::from)
        .collect();
    model.set_swatches(ModelRc::new(VecModel::from(colours)));
    model.set_swatch_labels(ModelRc::new(VecModel::from(labels)));
    model.set_badge_text(lch_color::badge_text(solve.delta_e, solve.reachable).into());
    model.set_reachable(solve.reachable);
    model.set_has_solution(true);
    model.set_busy(false);
}

/// Starts one solve for `[L*, C*, h]` (the callback every slider and the picker share).
type LchStart = dyn Fn(&MainWindow, [f64; 3]);

/// The reference-path callback of the colour editor, in millimetres, for the editor's
/// `BodyColorEditorModel.target` (one of the `TARGET_*` constants).
type PathMm = dyn Fn(i32) -> f64;

// `BodyColorEditorModel.target` 0 (anything not below) is the open design's material: Design
// settings, and the Live Render popup while it targets the design.

/// `BodyColorEditorModel.target`: "Apply colour" sets the material editor dialog's colour.
const TARGET_DIALOG: i32 = 1;
/// `BodyColorEditorModel.target`: "Apply colour" sets the Live Render popup's view-only colour
/// (stored as the legacy triple the view setting holds).
const TARGET_VIEW: i32 = 2;

/// Builds the shared "start a solve" callback of the colour editor.
///
/// Each call supersedes the previous one (`latest` ticket, `running` cancel flag), solves on
/// a background thread and, when still the newest, pushes the result into the model and
/// `last`.
fn lch_start_solver(
    latest: Arc<AtomicU64>,
    running: Rc<RefCell<Arc<AtomicBool>>>,
    last: Arc<Mutex<Option<LchSolve>>>,
    path_mm: Rc<PathMm>,
) -> Rc<LchStart> {
    Rc::new(move |ui: &MainWindow, lch: [f64; 3]| {
        let ticket = latest.fetch_add(1, Ordering::SeqCst) + 1;
        let flag = Arc::new(AtomicBool::new(false));
        running
            .replace(Arc::clone(&flag))
            .store(true, Ordering::SeqCst);
        let model = ui.global::<BodyColorEditorModel>();
        let path = path_mm(model.get_target());
        model.set_busy(true);
        let (latest, last, ui_weak) = (Arc::clone(&latest), Arc::clone(&last), ui.as_weak());
        std::thread::spawn(move || {
            let solve = lch_color::solve_lch(lch, path, &flag);
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                if latest.load(Ordering::SeqCst) != ticket {
                    return;
                }
                let model = ui.global::<BodyColorEditorModel>();
                let Some(solve) = solve else {
                    model.set_busy(false);
                    return;
                };
                push_solve(&model, &solve);
                *last.lock().unwrap_or_else(PoisonError::into_inner) = Some(solve);
            });
        });
    })
}

/// The Design settings colour editor (the combo's "Custom..." entry): Tone / Saturation / Hue
/// sliders and the hue-saturation-brightness picker feed ONE solver ([`lch_color::solve_lch`])
/// that runs on a background thread for the design's reference path; a newer request cancels
/// and supersedes an older one that is still running. Nothing reaches the design until
/// "Apply colour", which stores the last finished solve as ONE undoable edit through `edit`.
///
/// No `changed` handler here touches the editor state: the callbacks only read it with
/// `try_borrow` (the design's girdle and material) and write Slint properties, so the
/// re-entrancy rule of `slint-changed-handler-reentrancy` is not at risk.
fn setup_lch_editor(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    edit: MaterialEdit,
) {
    let model = ui.global::<BodyColorEditorModel>();
    let latest = Arc::new(AtomicU64::new(0));
    let running: Rc<RefCell<Arc<AtomicBool>>> = Rc::new(RefCell::new(Arc::default()));
    let last: Arc<Mutex<Option<LchSolve>>> = Arc::default();

    // The material editor dialog colours a material whose stone width is the render's; every
    // other target colours the open design, whose girdle diameter it is.
    let state_path = Rc::clone(state);
    let ctx_path = Arc::clone(render_ctx);
    let path_mm: Rc<PathMm> = Rc::new(move |target: i32| {
        if target == TARGET_DIALOG {
            let width = RenderContext::lock(&ctx_path).stone_width_mm;
            return lch_color::reference_path_mm(Some(f64::from(width)));
        }
        let girdle = state_path
            .try_borrow()
            .ok()
            .and_then(|st| st.design.girdle_diameter_mm);
        lch_color::reference_path_mm(girdle)
    });

    let start = lch_start_solver(latest, running, Arc::clone(&last), Rc::clone(&path_mm));

    register_lch_open(ui, state, render_ctx, &start, &last, &path_mm);

    let (start_edit, ui_weak) = (Rc::clone(&start), ui.as_weak());
    model.on_lch_edited(move |tone, chroma, hue| {
        if let Some(ui) = ui_weak.upgrade() {
            start_edit(&ui, [f64::from(tone), f64::from(chroma), f64::from(hue)]);
        }
    });

    let (start_pick, ui_weak) = (start, ui.as_weak());
    model.on_picker_picked(move |picked| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let [l, c, h] = lch_color::lch_from_srgb([
            f64::from(picked.red()) / 255.0,
            f64::from(picked.green()) / 255.0,
            f64::from(picked.blue()) / 255.0,
        ]);
        let model = ui.global::<BodyColorEditorModel>();
        model.set_tone(l as f32);
        model.set_chroma(c as f32);
        model.set_hue(h as f32);
        start_pick(&ui, [l, c, h]);
    });

    register_lch_apply(ui, last, edit);
}

/// Registers "Apply colour": stores the last finished solve through the target's own path.
fn register_lch_apply(ui: &MainWindow, last: Arc<Mutex<Option<LchSolve>>>, edit: MaterialEdit) {
    let model = ui.global::<BodyColorEditorModel>();
    let ui_weak = ui.as_weak();
    model.on_apply_requested(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let solve = last.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let Some(solve) = solve else {
            return;
        };
        let [r, g, b] = solve.triple;
        match ui.global::<BodyColorEditorModel>().get_target() {
            TARGET_DIALOG => {
                // The bands wait in the dialog until Save; the triple goes through the same
                // path as the old picker's result (fantasy colour, preset selection).
                dialog_color::set(DialogColor {
                    bands: (!solve.bands.is_empty()).then(|| solve.bands.clone()),
                    triple: Some(solve.triple),
                });
                ui.global::<PhysicscolorModel>()
                    .invoke_fixed_color_picked(r, g, b);
            }
            // A view-only colour is the legacy triple the view setting stores.
            TARGET_VIEW => ui
                .global::<ViewportModel>()
                .invoke_render_color_custom_solved(r, g, b),
            _ => {
                if let Err(message) =
                    edit(&|material| lch_color::recoloured_with_solve(material, &solve))
                {
                    show_toast(&ui, &message, "error");
                }
            }
        }
    });
}

/// Registers the colour editor's "open" callback: seeds the sliders from the design's stored
/// colour (the editor's own default for a design with the material's colour), forgets the
/// previous solve and solves.
fn register_lch_open(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    start: &Rc<LchStart>,
    last: &Arc<Mutex<Option<LchSolve>>>,
    path_mm: &Rc<PathMm>,
) {
    let model = ui.global::<BodyColorEditorModel>();
    let (state_open, start_open, last_open) =
        (Rc::clone(state), Rc::clone(start), Arc::clone(last));
    let ctx_open = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    let path_open = Rc::clone(path_mm);
    model.on_open_requested(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let model = ui.global::<BodyColorEditorModel>();
        *last_open.lock().unwrap_or_else(PoisonError::into_inner) = None;
        model.set_has_solution(false);
        let target = model.get_target();
        let path = path_open(target);
        let seeded = match target {
            TARGET_DIALOG => lch_color::lch_from_material(
                &dialog_color::as_selection(&dialog_color::current()),
                path,
            ),
            TARGET_VIEW => {
                let view = RenderContext::lock(&ctx_open).view_body_color;
                lch_color::lch_from_material(&MaterialSelection::none().with_body_color(view), path)
            }
            _ => state_open
                .try_borrow()
                .ok()
                .and_then(|st| lch_color::lch_from_material(&st.design.material, path)),
        };
        let [l, c, h] = seeded.unwrap_or_else(|| {
            [
                f64::from(model.get_tone()),
                f64::from(model.get_chroma()),
                f64::from(model.get_hue()),
            ]
        });
        model.set_tone(l as f32);
        model.set_chroma(c as f32);
        model.set_hue(h as f32);
        start_open(&ui, [l, c, h]);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const YELLOW: [f32; 3] = [0.2, 0.4, 2.8];

    fn sapphire() -> MaterialSelection {
        MaterialSelection {
            name: Some("Sapphire".to_string()),
            specific_gravity_override: Some(4.0),
            refractive_index_override: Some(1.77),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        }
    }

    #[test]
    fn a_new_colour_changes_only_the_colour() {
        let before = sapphire();
        let after = recoloured(&before, Some(YELLOW)).expect("a different colour is an edit");
        assert_eq!(after.body_color_override, Some(YELLOW));
        assert_eq!(after.name, before.name);
        assert_eq!(after.specific_gravity_override, Some(4.0));
        assert_eq!(after.refractive_index_override, Some(1.77));
    }

    #[test]
    fn material_default_clears_the_colour() {
        let before = sapphire().with_body_color(Some(YELLOW));
        let after = recoloured(&before, None).expect("clearing a colour is an edit");
        assert_eq!(after.body_color_override, None);
    }

    /// The editor's copy of the physics colour editor's reference-path convention.
    #[test]
    fn the_lch_reference_path_matches_the_physics_editors() {
        use crate::gui::optics::physics_state::default_reference_path_mm;
        for width in [0.0f32, 6.0, 9.5] {
            let ours = lch_color::reference_path_mm(Some(f64::from(width)));
            let theirs = f64::from(default_reference_path_mm(width));
            assert!((ours - theirs).abs() < 1e-4, "{width}: {ours} vs {theirs}");
        }
        assert!(
            (lch_color::reference_path_mm(None) - f64::from(default_reference_path_mm(0.0))).abs()
                < 1e-9
        );
    }

    #[test]
    fn a_toolbar_pick_drops_the_lch_editors_bands() {
        let banded = sapphire().with_body_color_bands(
            Some(YELLOW),
            Some(vec![[460.0, 45.0, 0.25]]),
            Some(2.0),
        );
        let after = recoloured(&banded, Some(YELLOW)).expect("dropping the bands is an edit");
        assert_eq!(after.body_color_override, Some(YELLOW));
        assert_eq!(after.body_color_bands_override, None);
        assert_eq!(after.absorption_path_scale_override, None);
        let cleared = recoloured(&banded, None).expect("clearing is an edit");
        assert_eq!(cleared.body_color_bands_override, None);
    }

    #[test]
    fn the_colour_it_already_has_is_not_an_edit() {
        assert!(recoloured(&sapphire(), None).is_none());
        assert!(recoloured(&sapphire().with_body_color(Some(YELLOW)), Some(YELLOW)).is_none());
    }
}
