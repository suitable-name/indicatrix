//! File > Export PNG: the dialog (`ui/components/export_dialog.slint`), rendering at
//! the export's own size and samples through the render Workers, and the download.
//!
//! # Pipeline
//!
//! 1. The live scene (`live::live_scene`, the same `SceneSpec` the view renders) at
//!    the export size: the view's aspect with the chosen long edge
//!    (`display::export_size`, at most 4096 px).
//! 2. The live render pauses: the pool gets the export scene (`set_scene`, so stale
//!    live chunks are dropped) and traces it to the chosen samples; the dialog shows
//!    the progress and offers Cancel.
//! 3. The finished sum goes to the pool's picture Worker (`request_picture(Png)`), which runs
//!    the desktop's still export -- `tonemap_accumulation` for the colour space, then
//!    `encode_png_with_icc` (`display::export_png`), after the live view's denoise
//!    when chosen -- off the page's thread. `indicatrix-web-core`'s
//!    `display::tests::web_export_bytes_equal_the_desktop_export_for_a_fixed_sum`
//!    pins the bytes to the desktop's calls.
//! 4. The PNG downloads (`io::download`) under the desktop's default export name
//!    (`display::export_file_name`), and the live render resumes.
//!
//! Denoising an export needs about 100 bytes per pixel in the Worker (the sum, the
//! guides and the denoiser's buffers), so it is offered up to
//! [`MAX_DENOISE_PIXELS`] (2048 x 2048); larger exports render without it.

use super::{live, now_ms};
use crate::{
    AppWindow, RenderModel,
    app::{
        Ctx,
        persist::schedule_save,
        push::{MessageKind, show_message},
        solve::with_solved,
    },
    io::download::{MIME_PNG, download_bytes},
};
use indicatrix_web_core::{
    display::{color_space_label, export_color_space, export_file_name, export_size},
    host::PictureResult,
    protocol::PictureKind,
    render::{Accumulator, clamp_export_spp},
    settings::ExportSettings,
};
use slint::ComponentHandle;
use std::cell::RefCell;

/// The largest export the denoise option is offered for.
pub const MAX_DENOISE_PIXELS: u64 = 2048 * 2048;

/// A running export.
struct ExportJob {
    scene_id: u64,
    spp: u32,
    width: u32,
    height: u32,
    color_space: i32,
    denoise: bool,
    file_name: String,
    started_ms: f64,
    encoding: bool,
}

thread_local! {
    static JOB: RefCell<Option<ExportJob>> = const { RefCell::new(None) };
}

/// Whether an export owns the render pool.
pub(super) fn is_running() -> bool {
    JOB.with(|job| job.borrow().is_some())
}

/// The dialog's current choices, sanitised.
fn read_dialog(ui: &AppWindow) -> ExportSettings {
    let model = ui.global::<RenderModel>();
    ExportSettings {
        long_edge: u32::try_from(model.get_export_long_edge()).unwrap_or(1),
        spp: u32::try_from(model.get_export_spp()).unwrap_or(0),
        color_space: model.get_export_color_space(),
        denoise: model.get_export_denoise(),
    }
}

fn push_dialog(ui: &AppWindow, export: &ExportSettings) {
    let model = ui.global::<RenderModel>();
    model.set_export_long_edge(export.long_edge as i32);
    model.set_export_spp(export.spp as i32);
    model.set_export_color_space(export.color_space);
    model.set_export_denoise(export.denoise);
}

/// The export size for the current view and `export`.
fn planned_size(ctx: &Ctx, export: &ExportSettings) -> (u32, u32) {
    let app = ctx.state.borrow();
    export_size(
        app.view.render_width,
        app.view.render_height,
        export.long_edge,
    )
}

fn push_size_text(ctx: &Ctx, ui: &AppWindow, export: &ExportSettings) {
    let (width, height) = planned_size(ctx, export);
    let mut text = format!("{width} x {height} px, the view's aspect");
    if export.denoise && u64::from(width) * u64::from(height) > MAX_DENOISE_PIXELS {
        text.push_str(" -- too large to denoise (up to 2048 x 2048)");
    }
    ui.global::<RenderModel>().set_export_size_text(text.into());
}

/// Stores the dialog's (sanitised) choices in the settings.
fn store_dialog(ctx: &Ctx, ui: &AppWindow) -> ExportSettings {
    let export = {
        let mut app = ctx.state.borrow_mut();
        let mut settings = app.settings.clone();
        settings.export = read_dialog(ui);
        app.settings = settings.sanitized();
        app.settings.export.clone()
    };
    push_dialog(ui, &export);
    push_size_text(ctx, ui, &export);
    export
}

/// File > Export PNG.
pub fn open_export_dialog(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    if ctx.state.borrow().design.is_none() {
        show_message(
            ctx,
            MessageKind::Info,
            "Open or create a design to export a render.",
        );
        return;
    }
    let export = ctx.state.borrow().settings.export.clone();
    push_dialog(&ui, &export);
    push_size_text(ctx, &ui, &export);
    let model = ui.global::<RenderModel>();
    if !is_running() {
        model.set_export_status(slint::SharedString::new());
        model.set_export_progress(0.0);
    }
    model.set_export_open(true);
}

fn set_status(ctx: &Ctx, text: &str, progress: f32) {
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<RenderModel>();
        model.set_export_status(text.into());
        model.set_export_progress(progress);
    }
}

fn set_running(ctx: &Ctx, running: bool) {
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<RenderModel>().set_export_running(running);
    }
}

/// "Export": solves first when needed, then starts.
fn start(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    if is_running() {
        return;
    }
    let export = store_dialog(ctx, &ui);
    schedule_save(ctx);
    let (width, height) = planned_size(ctx, &export);
    if export.denoise && u64::from(width) * u64::from(height) > MAX_DENOISE_PIXELS {
        set_status(
            ctx,
            "Denoise is available up to 2048 x 2048 pixels: lower the long edge or turn denoise off.",
            0.0,
        );
        return;
    }
    set_status(ctx, "Preparing...", 0.0);
    with_solved(ctx, move |ctx, solved| match solved {
        Ok(_) => begin(ctx, &export),
        Err(message) => set_status(ctx, &format!("Cannot export: {message}"), 0.0),
    });
}

/// Starts rendering the export scene.
fn begin(ctx: &Ctx, export: &ExportSettings) {
    let pool = match live::ensure_pool(ctx) {
        Ok(pool) => pool,
        Err(error) => {
            set_status(ctx, &error, 0.0);
            return;
        }
    };
    let live_spec = match live::live_scene(ctx, &pool) {
        Ok(spec) => spec,
        Err(reason) => {
            set_status(ctx, &format!("Cannot export: {reason}"), 0.0);
            return;
        }
    };
    let (width, height) = planned_size(ctx, export);
    let spp = clamp_export_spp(export.spp);
    let unix_seconds = (js_sys::Date::now() / 1000.0).max(0.0) as u64;
    let file_name = export_file_name(&live_spec.material.name, width, height, spp, unix_seconds);
    let spec = indicatrix_web_core::scene::SceneSpec {
        width,
        height,
        ..live_spec
    };
    // The job exists before `set_scene`, so its progress is routed here.
    JOB.with(|job| {
        *job.borrow_mut() = Some(ExportJob {
            scene_id: u64::MAX,
            spp,
            width,
            height,
            color_space: export.color_space,
            denoise: export.denoise,
            file_name,
            started_ms: now_ms(),
            encoding: false,
        });
    });
    live::forget_scene();
    let render = pool.render();
    render.set_scene(spec);
    let scene_id = render.scene_id();
    JOB.with(|job| {
        if let Some(job) = job.borrow_mut().as_mut() {
            job.scene_id = scene_id;
        }
    });
    render.start(spp);
    set_running(ctx, true);
    set_status(ctx, &format!("Rendering 0/{spp} spp..."), 0.0);
}

/// A pass of the export merged.
pub(super) fn on_progress(ctx: &Ctx, acc: &Accumulator) {
    let Some((spp, started, encoding)) = JOB.with(|job| {
        job.borrow()
            .as_ref()
            .map(|j| (j.spp, j.started_ms, j.encoding))
    }) else {
        return;
    };
    if encoding {
        return;
    }
    // The pool traces nothing but the export's scene while the export runs (the pool only
    // reports its current scene's passes), but it restarts that scene under a new id when
    // it replaces a crashed Worker: follow the id, so the restarted render still finishes.
    JOB.with(|job| {
        if let Some(job) = job.borrow_mut().as_mut() {
            job.scene_id = acc.scene_id();
        }
    });
    let samples = acc.sample_count();
    let seconds = (now_ms() - started) / 1000.0;
    set_status(
        ctx,
        &format!("Rendering {samples}/{spp} spp \u{b7} {seconds:.0} s"),
        samples as f32 / spp.max(1) as f32,
    );
    let done = crate::workers::existing().is_some_and(|p| p.render().is_done());
    if done {
        encode(ctx);
    }
}

/// The export's samples are all in: tone map, (denoise,) encode in a Worker.
fn encode(ctx: &Ctx) {
    let Some(pool) = crate::workers::existing() else {
        return;
    };
    let Some((color_space, denoise)) = JOB.with(|job| {
        let mut job = job.borrow_mut();
        let job = job.as_mut()?;
        job.encoding = true;
        Some((job.color_space, job.denoise))
    }) else {
        return;
    };
    let kind = PictureKind::Png {
        color_space,
        denoise,
    };
    match pool.render().request_picture(kind) {
        Ok(_) => set_status(
            ctx,
            if denoise {
                "Denoising and encoding the PNG..."
            } else {
                "Encoding the PNG..."
            },
            1.0,
        ),
        Err(error) => fail(ctx, &error),
    }
}

/// The Worker's PNG.
pub(super) fn on_picture(ctx: &Ctx, result: PictureResult) {
    let job = JOB.with(|job| {
        let mut slot = job.borrow_mut();
        if slot.as_ref().is_some_and(|j| j.scene_id == result.scene_id) {
            slot.take()
        } else {
            None
        }
    });
    let Some(job) = job else {
        return;
    };
    let bytes = match result.bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            fail(ctx, &error);
            return;
        }
    };
    let seconds = (now_ms() - job.started_ms) / 1000.0;
    finish_export(ctx);
    match download_bytes(&job.file_name, MIME_PNG, &bytes) {
        Ok(()) => {
            show_message(
                ctx,
                MessageKind::Success,
                &format!(
                    "Exported {} ({} x {}, {} spp, {}) in {seconds:.0} s.",
                    job.file_name,
                    job.width,
                    job.height,
                    job.spp,
                    color_space_label(export_color_space(job.color_space))
                ),
            );
            if let Some(ui) = ctx.ui.upgrade() {
                ui.global::<RenderModel>().set_export_open(false);
            }
        }
        Err(error) => set_status(ctx, &format!("The download failed: {error}"), 1.0),
    }
}

/// Ends the export (done, failed or cancelled) and resumes the live render.
fn finish_export(ctx: &Ctx) {
    JOB.with(|job| *job.borrow_mut() = None);
    set_running(ctx, false);
    live::forget_scene();
    live::request_sync(ctx);
}

fn fail(ctx: &Ctx, error: &str) {
    finish_export(ctx);
    set_status(ctx, &format!("Export failed: {error}"), 0.0);
}

/// A render Worker crashed: an export in progress cannot finish.
pub(super) fn on_worker_failure(ctx: &Ctx) {
    if is_running() {
        fail(
            ctx,
            "a render worker stopped; restart the workers and export again",
        );
    }
}

fn cancel(ctx: &Ctx) {
    if is_running() {
        finish_export(ctx);
        set_status(ctx, "Cancelled.", 0.0);
    }
}

pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<RenderModel>();
    let c = ctx.clone();
    model.on_export_edited(move || {
        if let Some(ui) = c.ui.upgrade() {
            store_dialog(&c, &ui);
        }
    });
    let c = ctx.clone();
    model.on_export_start(move || start(&c));
    let c = ctx.clone();
    model.on_export_cancel(move || cancel(&c));
    let c = ctx.clone();
    model.on_export_close(move || {
        cancel(&c);
        if let Some(ui) = c.ui.upgrade() {
            ui.global::<RenderModel>().set_export_open(false);
        }
        schedule_save(&c);
    });
}
