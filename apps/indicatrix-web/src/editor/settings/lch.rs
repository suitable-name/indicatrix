//! The Design settings colour editor's callbacks (the desktop's
//! `gui::editor::render_color::setup_lch_editor`): Tone / Saturation / Hue sliders and the
//! hue-saturation-brightness picker feed one solve (`indicatrix_editor::lch_color`), and
//! "Apply colour" stores the last finished solve as ONE undoable `SetMaterial`.
//!
//! A solve is a 2187-point grid plus a short Levenberg-Marquardt polish in pure `f64`, so it
//! runs in the analysis Worker (`SolveRequest::BodyColor`) when a slider is let go, when the
//! picker returns a colour and when the editor opens; the newest request wins (an older answer
//! is dropped by its epoch). While a tilt sweep holds that Worker, or when no Worker is
//! available or it fails, the solve runs on the UI thread instead, as it did before the
//! request existed.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::atomic::AtomicBool,
};

use crate::{
    AppWindow, DesignSettingsModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
    editor::inspector::finish_no_tier,
    metrics::{hud, tilt},
};
use indicatrix_cut_core::Edit;
use indicatrix_editor::lch_color::{
    LchSolve, badge_text, lch_from_material, lch_from_srgb, recoloured_with_solve,
    reference_path_mm, solve_lch, swatch_bytes, swatch_labels,
};
use indicatrix_web_core::solve::{BodyColorParams, SolveRequest, SolveResponse};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

thread_local! {
    /// Bumped by every solve request; an answer of an older one is ignored.
    static EPOCH: Cell<u64> = const { Cell::new(0) };
}

type LastSolve = Rc<RefCell<Option<LchSolve>>>;

/// The reference path (mm) of the open design: its girdle's, or the default.
fn design_path_mm(ctx: &Ctx) -> f64 {
    let girdle = ctx.state.try_borrow().ok().and_then(|app| {
        app.design
            .as_ref()
            .and_then(|design| design.session.design.girdle_diameter_mm)
    });
    reference_path_mm(girdle)
}

/// Sets the sliders to `lch` and solves it.
fn set_sliders_and_solve(ctx: &Ctx, last: &LastSolve, lch: [f64; 3]) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    model.set_lch_tone(lch[0] as f32);
    model.set_lch_chroma(lch[1] as f32);
    model.set_lch_hue(lch[2] as f32);
    solve_and_push(ctx, last, lch);
}

/// Solves `lch` for the design's path and shows swatches and badge; remembers the solve for
/// Apply. The solve runs in the analysis Worker; the newest request wins.
fn solve_and_push(ctx: &Ctx, last: &LastSolve, lch: [f64; 3]) {
    let path_mm = design_path_mm(ctx);
    let epoch = EPOCH.with(|e| {
        e.set(e.get() + 1);
        e.get()
    });
    // A tilt sweep holds the analysis Worker (a request would end it): solve here then.
    let client = if tilt::is_running() {
        None
    } else {
        crate::workers::pool().and_then(|pool| pool.analysis()).ok()
    };
    let Some(client) = client else {
        solve_inline(ctx, last, lch, path_mm);
        return;
    };
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<DesignSettingsModel>()
            .set_lch_badge_text("Matching...".into());
    }
    let future = client.solve(
        String::new(),
        SolveRequest::BodyColor {
            params: BodyColorParams { lch, path_mm },
        },
    );
    let (ctx, last) = (ctx.clone(), Rc::clone(last));
    wasm_bindgen_futures::spawn_local(async move {
        let result = future.await;
        if EPOCH.with(Cell::get) != epoch {
            return;
        }
        match result {
            Ok(SolveResponse::BodyColor(data)) => push_solve(&ctx, &last, data.into_solve()),
            // Superseded by a sweep, a failed Worker or anything else: the editor still works.
            _ => solve_inline(&ctx, &last, lch, path_mm),
        }
        // The metrics HUD may have been superseded by this job: it asks again now.
        hud::resume(&ctx);
    });
}

/// The solve on the UI thread (no Worker to use).
fn solve_inline(ctx: &Ctx, last: &LastSolve, lch: [f64; 3], path_mm: f64) {
    if let Some(solve) = solve_lch(lch, path_mm, &AtomicBool::new(false)) {
        push_solve(ctx, last, solve);
    }
}

/// Shows a finished solve (swatches and badge) and keeps it for Apply.
fn push_solve(ctx: &Ctx, last: &LastSolve, solve: LchSolve) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    let colours: Vec<slint::Color> = solve
        .swatches
        .iter()
        .map(|rgb| {
            let [r, g, b] = swatch_bytes(*rgb);
            slint::Color::from_rgb_u8(r, g, b)
        })
        .collect();
    let labels: Vec<SharedString> = swatch_labels(solve.path_mm)
        .into_iter()
        .map(SharedString::from)
        .collect();
    model.set_lch_swatches(ModelRc::new(VecModel::from(colours)));
    model.set_lch_swatch_labels(ModelRc::new(VecModel::from(labels)));
    model.set_lch_badge_text(badge_text(solve.delta_e, solve.reachable).into());
    model.set_lch_reachable(solve.reachable);
    model.set_lch_has_solution(true);
    *last.borrow_mut() = Some(solve);
}

/// The editor opened: seed the sliders from the design's colour (the sliders' own default
/// for a design with the material's colour) and solve.
fn open(ctx: &Ctx, last: &LastSolve) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    *last.borrow_mut() = None;
    model.set_lch_has_solution(false);
    let path = design_path_mm(ctx);
    let seeded = ctx.state.try_borrow().ok().and_then(|app| {
        app.design
            .as_ref()
            .and_then(|design| lch_from_material(&design.session.design.material, path))
    });
    let lch = seeded.unwrap_or([
        f64::from(model.get_lch_tone()),
        f64::from(model.get_lch_chroma()),
        f64::from(model.get_lch_hue()),
    ]);
    set_sliders_and_solve(ctx, last, lch);
}

/// Apply colour: the last solve as ONE `SetMaterial` that changes only the colour.
fn apply(ctx: &Ctx, last: &LastSolve) {
    let Some(solve) = last.borrow().clone() else {
        return;
    };
    let result = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        let Some(material) = recoloured_with_solve(&design_state.session.design.material, &solve)
        else {
            return;
        };
        design_state
            .session
            .apply(Edit::SetMaterial { material })
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    match result {
        Ok(()) => finish_no_tier(ctx),
        Err(message) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// Registers the editor's callbacks.
pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<DesignSettingsModel>();
    let last: LastSolve = Rc::default();
    let (c, l) = (ctx.clone(), Rc::clone(&last));
    model.on_lch_open(move || open(&c, &l));
    let (c, l) = (ctx.clone(), Rc::clone(&last));
    model.on_lch_edited(move |tone, chroma, hue| {
        solve_and_push(&c, &l, [f64::from(tone), f64::from(chroma), f64::from(hue)]);
    });
    let (c, l) = (ctx.clone(), Rc::clone(&last));
    model.on_lch_picked(move |picked| {
        let lch = lch_from_srgb(
            [picked.red(), picked.green(), picked.blue()].map(|v| f64::from(v) / 255.0),
        );
        set_sliders_and_solve(&c, &l, lch);
    });
    let c = ctx.clone();
    model.on_lch_apply(move || apply(&c, &last));
}
