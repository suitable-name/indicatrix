//! The fan-out export queue itself: [`ExportQueue`]/[`ExportJob`], the pure
//! scene-transform helpers that build each job's [`bridge::export_thread::SceneSnapshot`],
//! and [`start_next_export_job`]/[`finish_export_queue`] which step through the queue
//! one job at a time -- see this group's own `mod.rs` doc comment for why sequential,
//! and why `Arc<Mutex<_>>>`.

use crate::{
    ExportModel, MainWindow,
    bridge::{
        export_thread::{self, ComputeTarget, SceneSnapshot, TemplateContext},
        render_thread::{RenderContext, load_env_map},
    },
    gui::show_toast,
    settings::{LightingPreset as SavedLightingPreset, LocalComputeTarget, WorkerSettings},
};
use indicatrix::{color::ColorSpace, optics::raytracer::LightingPreset};
use slint::ComponentHandle;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

/// `export_dialog.slint`'s "Compute" pill index (0/1/2, see that property's own doc
/// comment) -> [`ComputeTarget`]. Mirrors `gui::color_space_from_index`'s own
/// int-discriminant convention.
pub(super) const fn compute_target_from_index(index: i32) -> ComputeTarget {
    match index {
        0 => ComputeTarget::LocalOnly,
        1 => ComputeTarget::RemoteOnly,
        _ => ComputeTarget::Both,
    }
}

/// Overrides a freshly captured [`SceneSnapshot`]'s bounce cap with the export
/// dialog's OWN choice, rather than leaving whatever `SceneSnapshot::capture` read from
/// the live viewport's `guard.max_bounces` at capture time.
///
/// This exists as a separate, pure step (not folded into `capture` itself) for two
/// reasons: `capture` takes its lock deliberately short (see that function's own doc
/// comment), so it must not grow a reason to hold it any longer than reading the
/// viewport's own state requires; and `scene.max_bounces` is the SAME field
/// `batch::render_batch`/`GpuSceneRef::max_bounces` (local CPU/GPU) and
/// `remote::scene_state_from_snapshot` (the remote `RenderRequest`) all read from --
/// overriding it here, once, before any fanned-out preset clones the base capture,
/// is what guarantees the base render AND every fanned-out preset render trace at the
/// SAME cap.
#[must_use]
pub(super) const fn apply_export_bounce_cap(
    mut scene: SceneSnapshot,
    max_bounces: u32,
) -> SceneSnapshot {
    scene.max_bounces = max_bounces;
    scene
}

/// Overlays a saved preset's light/camera/HDR-map fields onto a cloned base capture --
/// the export-time counterpart of `gui::render::lighting_presets`'s live-apply
/// callback, and deliberately following the exact same rules:
///
/// - light yaw/pitch/exposure/rig are ALWAYS overlaid -- every preset carries these.
/// - camera pose is overlaid only when the preset carries one (`camera_yaw`/
///   `camera_pitch` both `Some`) -- otherwise the base capture's own camera (whatever
///   the live viewport is currently posed at) is what this preset's render uses. See
///   `settings::model::LightingPreset`'s doc comment for why `None` must mean "leave
///   it alone", not "reset to some default", here exactly as it does when applying a
///   preset live.
/// - the HDR environment map is overlaid only when the preset carries one AND it still
///   loads successfully; a preset pointing at a file that has since moved or been
///   deleted leaves the base capture's own environment untouched (silently, matching
///   `gui::render::lighting_presets`' own "never assign on `Err`" rule) rather than
///   failing this preset's whole render over a missing side asset -- the render itself
///   is still worth producing.
///
/// Material and geometry are never touched here: a preset is a captured VIEW (this
/// type's own doc comment), never a material or cut choice, so every fanned-out render
/// shares the exact material/geometry the base current-view capture already resolved.
#[must_use]
pub(super) fn apply_preset_to_scene(
    mut scene: SceneSnapshot,
    preset: &SavedLightingPreset,
) -> SceneSnapshot {
    scene.light_yaw = preset.light_yaw_deg.to_radians();
    scene.light_pitch = preset.light_pitch_deg.to_radians().clamp(0.15, 1.55);
    scene.exposure = preset.exposure.clamp(0.2, 5.0);
    scene.lighting_preset = LightingPreset::from_label(&preset.lighting_rig);

    if let (Some(yaw), Some(pitch)) = (preset.camera_yaw, preset.camera_pitch) {
        scene.yaw = yaw;
        scene.pitch = pitch.clamp(-1.48, 1.48);
    }

    if let Some(path) = &preset.env_map_path
        && let Ok(map) = load_env_map(path)
    {
        scene.env_map = Some(map);
    }

    scene
}

/// One render this export queue still needs to produce: the base current-view render
/// (`preset_label` empty) or one fanned-out preset render (`preset_label` the preset's
/// name, used both for the `{preset}` template variable and the progress label).
pub(super) struct ExportJob {
    pub(super) scene: SceneSnapshot,
    pub(super) preset_label: String,
}

/// Everything shared across every job in one fan-out export, and across the recursive
/// `start_next_export_job` calls that step through [`jobs`](Self::jobs) one at a time
/// -- see this group's own `mod.rs` doc comment for why this lives behind
/// `Arc<Mutex<_>>>` rather than the single-export `Rc<RefCell<Option<ExportHandle>>>>`
/// this replaced.
///
/// # Progress: counting presets, not just samples within one image
///
/// `total` jobs share the export dialog's single progress bar as one combined
/// fraction -- `(current_index + this_job_own_fraction) / total`, computed in
/// `start_next_export_job`'s own `on_progress` closure -- so a five-preset export's bar
/// moves smoothly from 0% to 100% across the whole run. The alternative (each job
/// reporting its own 0%-100%) would snap the bar back to 0% four times, which at a
/// glance reads as the export hanging rather than progressing, or would reach 100%
/// after job 1 and sit there while jobs 2-5 still run.
pub(super) struct ExportQueue {
    pub(super) jobs: VecDeque<ExportJob>,
    pub(super) total: usize,
    /// 0-based index of the job currently running (or about to run) -- also `{preset}`'s
    /// position for progress-label purposes; see [`ExportQueue`]'s own doc comment.
    pub(super) current_index: usize,
    pub(super) completed: Vec<PathBuf>,
    pub(super) failures: Vec<String>,
    /// Set by `on_cancel_export`; checked at the top of `start_next_export_job` so a
    /// cancel stops the WHOLE queue, not just the in-flight job -- see that
    /// function's own doc comment.
    pub(super) cancelled: bool,
    pub(super) current_handle: Option<export_thread::ExportHandle>,

    pub(super) export_dir: PathBuf,
    pub(super) template: String,
    // Fields shared by every job's `TemplateContext` -- see `filename_template`'s own
    // doc comment for what each variable means.
    pub(super) design: String,
    pub(super) designer: String,
    pub(super) shape: String,
    pub(super) ri: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) spp: u32,
    pub(super) bounces: u32,
    pub(super) colorspace: String,

    pub(super) params: export_thread::ExportParams,
    pub(super) color_space: ColorSpace,
    pub(super) compute_target: ComputeTarget,
    pub(super) workers: Vec<WorkerSettings>,
    pub(super) local_compute: LocalComputeTarget,
}

/// `ColorSpace`'s own display label, matching `export_dialog.slint`'s pill text --
/// used only for the `{colorspace}` template variable, so this doesn't need to be a
/// method on `ColorSpace` itself (that type has no UI-facing concerns of its own; every
/// other consumer works with the enum directly).
pub(super) const fn colorspace_label(cs: ColorSpace) -> &'static str {
    match cs {
        ColorSpace::Srgb => "sRGB",
        ColorSpace::DisplayP3 => "Display P3",
        ColorSpace::Rec2020 => "Rec.2020",
        // Not offered by `export_dialog.slint`'s picker (see `bridge::export_thread`'s
        // module doc comment on why a scene-linear space doesn't belong in an 8-bit PNG
        // export) -- included here only so this match stays exhaustive against a future
        // `ColorSpace` variant addition rather than silently compiling with a
        // wildcard arm that could hide one.
        ColorSpace::AcesCg => "ACEScg",
    }
}

/// Builds this job's own `TemplateContext` from the queue's fixed, whole-export fields
/// plus this job's own resolved scene (material name, lighting rig actually in effect,
/// camera pose, exposure) and preset label.
fn template_context_for_job(queue: &ExportQueue, job: &ExportJob) -> TemplateContext {
    TemplateContext {
        design: queue.design.clone(),
        designer: queue.designer.clone(),
        shape: queue.shape.clone(),
        material: job.scene.material.name.clone(),
        ri: queue.ri.clone(),
        width: queue.width,
        height: queue.height,
        spp: queue.spp,
        bounces: queue.bounces,
        colorspace: queue.colorspace.clone(),
        preset: job.preset_label.clone(),
        lighting: job.scene.lighting_preset.label().to_string(),
        yaw_deg: job.scene.yaw.to_degrees(),
        pitch_deg: job.scene.pitch.to_degrees(),
        distance: job.scene.distance,
        exposure: job.scene.exposure,
    }
}

/// Finalizes the queue once every job has run (or the queue was cancelled): resumes the
/// live viewport, clears the in-progress preview, and reports one combined outcome
/// message covering every file this export attempted -- rather than the LAST job's own
/// outcome, which would silently hide an earlier preset's failure the moment a later
/// one happened to succeed.
pub(super) fn finish_export_queue(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    queue: &ExportQueue,
) {
    ui.global::<ExportModel>().set_is_exporting(false);
    render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .export_active = false;
    ui.global::<ExportModel>()
        .set_preview_image(slint::Image::default());

    if queue.cancelled {
        ui.global::<ExportModel>().set_has_error(false);
        ui.global::<ExportModel>()
            .set_status_message("Export cancelled.".into());
        return;
    }

    if queue.failures.is_empty() {
        let msg = if queue.completed.len() == 1 {
            format!("Exported to {}", queue.completed[0].display())
        } else {
            format!(
                "Exported {} files to {}",
                queue.completed.len(),
                queue.export_dir.display()
            )
        };
        ui.global::<ExportModel>().set_has_error(false);
        ui.global::<ExportModel>()
            .set_status_message(msg.clone().into());
        show_toast(ui, &msg, "success");
    } else {
        let msg = format!(
            "Exported {} of {} files -- {} failed: {}",
            queue.completed.len(),
            queue.total,
            queue.failures.len(),
            queue.failures.join("; ")
        );
        ui.global::<ExportModel>().set_has_error(true);
        ui.global::<ExportModel>()
            .set_status_message(msg.clone().into());
        show_toast(ui, &msg, "error");
    }
}

/// Pops the next job off `queue` and starts it, or finalizes the queue if none remain
/// (or it was cancelled) -- called once to kick off the whole export, and once more
/// from every job's own `on_done` to advance to the next one. Recursion through a named
/// function (not a closure capturing itself) so `export_thread::spawn_export`'s
/// `on_done` bound (`Fn`, not `FnMut`, `+ Clone`) is trivially satisfiable: each call
/// just builds a fresh pair of closures over `Arc::clone`s of `render_ctx`/`queue`.
pub(super) fn start_next_export_job(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    queue: &Arc<Mutex<ExportQueue>>,
) {
    let job = {
        let mut q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if q.cancelled {
            None
        } else {
            q.jobs.pop_front()
        }
    };

    let Some(job) = job else {
        let q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        finish_export_queue(ui, render_ctx, &q);
        return;
    };

    let (current_index, total, output_path) = {
        let q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ctx = template_context_for_job(&q, &job);
        let output_path = export_thread::resolve_export_path(&q.export_dir, &q.template, &ctx);
        (q.current_index, q.total, output_path)
    };
    set_starting_job_ui_state(ui, current_index, total, &job);

    let (params, color_space, compute_target, workers, local_compute) = {
        let q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            q.params,
            q.color_space,
            q.compute_target,
            q.workers.clone(),
            q.local_compute,
        )
    };
    ui.global::<ExportModel>()
        .set_preview_samples_total(params.samples_per_pixel as i32);

    let queue_progress = queue.clone();
    let render_ctx_done = render_ctx.clone();
    let queue_done = queue.clone();

    let handle = export_thread::spawn_export(
        ui.as_weak(),
        job.scene,
        params,
        color_space,
        output_path,
        compute_target,
        workers,
        local_compute,
        move |ui: &MainWindow, progress: export_thread::ExportProgress| {
            handle_export_job_progress(ui, &queue_progress, progress);
        },
        move |ui: &MainWindow, outcome: export_thread::ExportOutcome| {
            handle_export_job_done(ui, &render_ctx_done, &queue_done, outcome);
        },
    );

    {
        let mut q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        q.current_handle = Some(handle);
    }
}

/// Resets the export panel's progress/preview UI for the job about to start. Split out
/// of `start_next_export_job` purely to keep that function under clippy's
/// function-length lint.
fn set_starting_job_ui_state(ui: &MainWindow, current_index: usize, total: usize, job: &ExportJob) {
    ui.global::<ExportModel>()
        .set_preset_index(current_index as i32);
    ui.global::<ExportModel>().set_preset_total(total as i32);
    ui.global::<ExportModel>()
        .set_current_preset_label(if job.preset_label.is_empty() {
            "Current view".into()
        } else {
            job.preset_label.clone().into()
        });
    ui.global::<ExportModel>()
        .set_progress(current_index as f32 / total as f32);
    ui.global::<ExportModel>().set_preview_samples_done(0);
    ui.global::<ExportModel>()
        .set_preview_image(slint::Image::default());
}

/// `export_thread::spawn_export`'s `on_progress` callback body for
/// `start_next_export_job`. Split out purely to keep that function under clippy's
/// function-length lint.
fn handle_export_job_progress(
    ui: &MainWindow,
    queue: &Arc<Mutex<ExportQueue>>,
    progress: export_thread::ExportProgress,
) {
    let (current_index, total) = {
        let q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (q.current_index, q.total)
    };
    let combined = (current_index as f32 + progress.fraction) / total as f32;
    ui.global::<ExportModel>().set_progress(combined);
    ui.global::<ExportModel>()
        .set_preview_samples_done(progress.samples_done as i32);
    ui.global::<ExportModel>()
        .set_preview_samples_total(progress.samples_total as i32);
    if let Some(buf) = progress.preview {
        ui.global::<ExportModel>()
            .set_preview_image(slint::Image::from_rgba8(buf));
    }
    if let Some(note) = progress.note {
        show_toast(ui, &note, "info");
    }
}

/// `export_thread::spawn_export`'s `on_done` callback body for
/// `start_next_export_job`: records the finished job's outcome, then recurses into the
/// next one (see `start_next_export_job`'s own doc comment on this recursion). Split
/// out purely to keep that function under clippy's function-length lint.
fn handle_export_job_done(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    queue: &Arc<Mutex<ExportQueue>>,
    outcome: export_thread::ExportOutcome,
) {
    {
        let mut q = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        q.current_handle = None;
        match outcome {
            export_thread::ExportOutcome::Completed(path) => q.completed.push(path),
            export_thread::ExportOutcome::Cancelled => q.cancelled = true,
            export_thread::ExportOutcome::Failed(err) => q.failures.push(err),
        }
        q.current_index += 1;
    }
    start_next_export_job(ui, render_ctx, queue);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The export's bounce cap must come from the export dialog, not silently
    /// inherit whatever the live viewport happens to be set to -- the exact gap the
    /// top-level task asked to close. Captures a snapshot from a `RenderContext` at one
    /// bounce count (standing in for "whatever the viewport is currently set to") and
    /// confirms `apply_export_bounce_cap` overrides it to a DIFFERENT value rather than
    /// leaving the captured one in place.
    #[test]
    fn apply_export_bounce_cap_overrides_the_snapshot_not_the_viewport_default() {
        let viewport_ctx = Mutex::new(RenderContext {
            max_bounces: 12, // the viewport's own setting, left untouched by this test
            ..Default::default()
        });
        let captured = SceneSnapshot::capture(&viewport_ctx);
        assert_eq!(
            captured.max_bounces, 12,
            "capture must still read the viewport's own setting by itself"
        );

        let exported = apply_export_bounce_cap(captured, 64);
        assert_eq!(
            exported.max_bounces, 64,
            "the export dialog's own bounce cap must win over the captured viewport value"
        );
    }

    /// Compute-target mapping's own seam guard -- see `compute_target_from_index`'s doc
    /// comment for why this mirrors `gui::color_space_from_index`'s convention, which
    /// has its own equivalent test in `gui::mod`.
    #[test]
    fn compute_target_from_index_maps_all_three_pill_values() {
        assert_eq!(compute_target_from_index(0), ComputeTarget::LocalOnly);
        assert_eq!(compute_target_from_index(1), ComputeTarget::RemoteOnly);
        assert_eq!(compute_target_from_index(2), ComputeTarget::Both);
    }

    #[test]
    fn compute_target_from_index_falls_back_to_both_for_unknown_values() {
        assert_eq!(compute_target_from_index(-1), ComputeTarget::Both);
        assert_eq!(compute_target_from_index(99), ComputeTarget::Both);
    }

    #[test]
    fn colorspace_label_covers_every_variant() {
        assert_eq!(colorspace_label(ColorSpace::Srgb), "sRGB");
        assert_eq!(colorspace_label(ColorSpace::DisplayP3), "Display P3");
        assert_eq!(colorspace_label(ColorSpace::Rec2020), "Rec.2020");
        assert_eq!(colorspace_label(ColorSpace::AcesCg), "ACEScg");
    }

    /// A preset with no camera pose must leave the base capture's own camera pose
    /// untouched -- the export-time mirror of `gui::render::lighting_presets`'
    /// `setup_apply_lighting_preset_callback`'s live-apply behaviour, and the whole
    /// reason `camera_yaw`/`camera_pitch` are `Option` in the first place (see
    /// `settings::model::LightingPreset`'s own doc comment).
    #[test]
    fn apply_preset_to_scene_leaves_camera_untouched_when_the_preset_carries_none() {
        let base = SceneSnapshot::capture(&Mutex::new(RenderContext {
            yaw: 1.23,
            pitch: 0.44,
            ..Default::default()
        }));
        let preset = SavedLightingPreset {
            name: "Mood Only".to_string(),
            built_in: false,
            light_yaw_deg: 10.0,
            light_pitch_deg: 20.0,
            exposure: 0.8,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.0,
            camera_yaw: None,
            camera_pitch: None,
            env_map_path: None,
            export_usable: true,
        };
        let overlaid = apply_preset_to_scene(base.clone(), &preset);
        assert_eq!(overlaid.yaw, base.yaw);
        assert_eq!(overlaid.pitch, base.pitch);
        assert_eq!(overlaid.light_yaw, preset.light_yaw_deg.to_radians());
    }

    /// A preset that DOES carry a camera pose must overlay it -- confirms the
    /// `camera_yaw`/`camera_pitch` "`None` means leave it alone" contract (this
    /// function's own doc comment) actually reaches the export path, not just the
    /// live-apply one.
    #[test]
    fn apply_preset_to_scene_overlays_camera_when_the_preset_carries_one() {
        let base = SceneSnapshot::capture(&Mutex::new(RenderContext {
            yaw: 1.23,
            pitch: 0.44,
            ..Default::default()
        }));
        let preset = SavedLightingPreset {
            name: "Full Shot".to_string(),
            built_in: false,
            light_yaw_deg: 10.0,
            light_pitch_deg: 20.0,
            exposure: 0.8,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.0,
            camera_yaw: Some(0.75),
            camera_pitch: Some(0.3),
            env_map_path: None,
            export_usable: true,
        };
        let overlaid = apply_preset_to_scene(base, &preset);
        assert_eq!(overlaid.yaw, 0.75);
        assert_eq!(overlaid.pitch, 0.3);
    }
}
