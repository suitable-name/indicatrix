//! Converting between the live app's render types and the frozen job file.
//!
//! A job stores the scene as the remote wire's `SceneState` (the one serializable
//! description of a scene the program already has), the HDR map as a file path plus hash,
//! and the compute choices as small serde enums. This module is the only place those are
//! mapped to and from `SceneSnapshot`, `RemoteSelection`, `ColorSpace` and
//! `LocalComputeTarget`, so the mapping cannot drift between the queue and the CLI.

use crate::{
    bridge::{
        export_thread::{
            ComputeTarget, ExportParams, RemoteSelection, SceneSnapshot, scene_state_for_job,
        },
        remote::hdr_asset,
    },
    gui::tilt::video_export::{
        metrics::{MetricCurves, MetricSelection},
        run::VideoExportRequest,
    },
    settings::{ExportTransfer, LocalComputeTarget, WorkerSettings},
};
use indicatrix::{
    color::ColorSpace, geometry::girdle_facet_finishes, renderer::env_map::EnvironmentMap,
};
use indicatrix_net::{
    SceneState,
    messages::{content_hash, hash_hex},
};
use indicatrix_render_jobs::{
    ComputeChoice, FailureKind, JobColorSpace, JobKind, LocalEngines, RenderJobFile, StillJob,
    TiltVideoJob, TransferChoice,
    job::{
        DesignInfo, HdrSource, JOB_FORMAT_VERSION, JobCompute, MetricCurvesData, OverlaySelection,
        new_job_token,
    },
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// The job colour space for `cs`.
///
/// # Errors
///
/// `AcesCg`, which a job cannot hold: it is scene-linear, and 8 bits per channel would
/// band badly. The export dialog never offers it either.
pub fn job_color_space(cs: ColorSpace) -> Result<JobColorSpace, String> {
    match cs {
        ColorSpace::Srgb => Ok(JobColorSpace::Srgb),
        ColorSpace::DisplayP3 => Ok(JobColorSpace::DisplayP3),
        ColorSpace::Rec2020 => Ok(JobColorSpace::Rec2020),
        ColorSpace::AcesCg => Err("ACEScg cannot be saved in an 8-bit picture.".to_string()),
    }
}

/// The renderer's colour space for a job colour space.
pub const fn color_space_of(cs: JobColorSpace) -> ColorSpace {
    match cs {
        JobColorSpace::Srgb => ColorSpace::Srgb,
        JobColorSpace::DisplayP3 => ColorSpace::DisplayP3,
        JobColorSpace::Rec2020 => ColorSpace::Rec2020,
    }
}

/// The app's local compute setting for the engines a command line asked for.
pub const fn local_target_of(engines: LocalEngines) -> LocalComputeTarget {
    match engines {
        LocalEngines::Cpu => LocalComputeTarget::Cpu,
        LocalEngines::CpuGpu => LocalComputeTarget::CpuGpu,
        LocalEngines::Gpu => LocalComputeTarget::Gpu,
    }
}

/// The engines for the app's local compute setting.
pub const fn engines_of(target: LocalComputeTarget) -> LocalEngines {
    match target {
        LocalComputeTarget::Cpu => LocalEngines::Cpu,
        LocalComputeTarget::CpuGpu => LocalEngines::CpuGpu,
        LocalComputeTarget::Gpu => LocalEngines::Gpu,
    }
}

/// The compute choices a remote selection holds, as a job freezes them.
pub const fn job_compute(remote: &RemoteSelection) -> JobCompute {
    JobCompute {
        target: match remote.compute_target {
            ComputeTarget::LocalOnly => ComputeChoice::Local,
            ComputeTarget::RemoteOnly => ComputeChoice::Remote,
            ComputeTarget::Both => ComputeChoice::Both,
        },
        transfer: match remote.transfer {
            ExportTransfer::FullData => TransferChoice::FullData,
            ExportTransfer::FinalPicture => TransferChoice::FinalPicture,
        },
        contribute_local: remote.contribute_local,
    }
}

/// The remote selection for a job's compute choices and the worker to use now.
///
/// The worker is not part of a job (it is machine configuration): the app reads it from
/// its settings when the job runs, the command line from its flags.
pub const fn remote_selection(
    compute: JobCompute,
    worker: Option<WorkerSettings>,
) -> RemoteSelection {
    RemoteSelection {
        compute_target: match compute.target {
            ComputeChoice::Local => ComputeTarget::LocalOnly,
            ComputeChoice::Remote => ComputeTarget::RemoteOnly,
            ComputeChoice::Both => ComputeTarget::Both,
        },
        worker,
        transfer: match compute.transfer {
            TransferChoice::FullData => ExportTransfer::FullData,
            TransferChoice::FinalPicture => ExportTransfer::FinalPicture,
        },
        contribute_local: compute.contribute_local,
    }
}

/// The HDR map file and hash a snapshot is lit by, `None` for the studio rig.
///
/// # Errors
///
/// A map that was not loaded from a file (built in memory) has no path to store.
pub fn hdr_source(snapshot: &SceneSnapshot) -> Result<Option<HdrSource>, String> {
    let Some(map) = &snapshot.env_map else {
        return Ok(None);
    };
    let asset = hdr_asset::asset_for(map).ok_or_else(|| {
        "This lighting uses an HDR map that was not loaded from a file, so it cannot be \
         saved in a job."
            .to_string()
    })?;
    Ok(Some(HdrSource {
        path: asset.path().to_string_lossy().into_owned(),
        sha256_hex: hash_hex(asset.content_hash()),
    }))
}

/// The serializable scene for `snapshot` at `width x height`, at the snapshot's camera.
pub fn scene_from_snapshot(snapshot: &SceneSnapshot, width: u32, height: u32) -> SceneState {
    scene_state_for_job(snapshot, width, height)
}

/// The snapshot a job's scene describes, built field by field so it matches what
/// `SceneSnapshot::capture_finished` made it from.
///
/// `env_map` is the decoded HDR map (see [`load_hdr`]) when the scene is lit by one.
pub fn snapshot_from_scene(
    scene: &SceneState,
    env_map: Option<Arc<EnvironmentMap>>,
) -> SceneSnapshot {
    let active_planes = scene.planes.clone();
    // The same derivation `capture_finished` uses: a frosted girdle is the classification
    // of the planes, and an empty list means every facet is polished.
    let facet_finishes = if scene.girdle_frosted {
        girdle_facet_finishes(&active_planes)
    } else {
        Vec::new()
    };
    SceneSnapshot {
        yaw: scene.yaw,
        pitch: scene.pitch,
        distance: scene.distance,
        light_yaw: scene.light_yaw,
        light_pitch: scene.light_pitch,
        material: scene.material.clone(),
        // A stored UV lamp falls back to the default rig in a build without `physical-color`.
        lighting_preset: crate::gui::optics::offered_lighting::offered(scene.lighting_preset),
        max_bounces: scene.max_bounces,
        exposure: scene.exposure,
        backdrop: scene.backdrop,
        surface_glare: scene.surface_glare,
        active_planes,
        tools: scene.tools.clone(),
        fluorescence: Arc::new(scene.fluorescence.clone()),
        head_shadow_deg: scene.head_shadow_deg,
        facet_finishes,
        env_map,
    }
}

/// Loads the HDR map a job names, checking it is still the file the job was made with.
///
/// `base_dir` is the folder relative paths in the job resolve against. Loading registers
/// the map as a protocol asset, so a remote worker that asks for it can be sent its bytes.
///
/// # Errors
///
/// `FailureKind::Input` with one plain sentence when the file is missing, changed since
/// the job was added, or cannot be decoded.
pub fn load_hdr(
    job: &RenderJobFile,
    base_dir: &Path,
) -> Result<Option<Arc<EnvironmentMap>>, (FailureKind, String)> {
    let (Some(source), Some(path)) = (&job.hdr, job.hdr_path(base_dir)) else {
        return Ok(None);
    };
    let shown = path.display();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err((
                FailureKind::Input,
                format!("The HDR map {shown} is missing."),
            ));
        }
        Err(e) => {
            return Err((
                FailureKind::Input,
                format!("The HDR map {shown} could not be read: {e}"),
            ));
        }
    };
    let changed = || {
        (
            FailureKind::Input,
            format!("The HDR map {shown} changed since the job was added."),
        )
    };
    if hash_hex(&content_hash(&bytes)) != source.sha256_hex {
        return Err(changed());
    }
    drop(bytes);
    let (map, asset) = hdr_asset::load_hdr_file(&path).map_err(|e| {
        (
            FailureKind::Input,
            format!("The HDR map {shown} could not be loaded: {e}"),
        )
    })?;
    // The file could have been replaced between the check above and the load.
    if hash_hex(asset.content_hash()) != source.sha256_hex {
        return Err(changed());
    }
    Ok(Some(map))
}

/// Everything `build_still_job` freezes for one picture.
pub struct StillJobInputs<'a> {
    /// The finished stone, material and lighting, already carrying the export's bounce cap
    /// and the preset overlay.
    pub scene: &'a SceneSnapshot,
    /// Size, samples per pixel and bounce cap.
    pub params: ExportParams,
    /// The colour space of the PNG.
    pub color_space: ColorSpace,
    /// The compute choices (the remote endpoint is read when the job runs).
    pub remote: &'a RemoteSelection,
    /// Where the PNG goes.
    pub output: &'a Path,
    /// The one-line name shown in the job list.
    pub label: String,
    /// Where the design came from.
    pub design: DesignInfo,
    /// The lighting preset this picture was added with, empty for the current view.
    pub preset_label: String,
}

/// Freezes one still picture as a job.
///
/// # Errors
///
/// An HDR map with no file, a colour space a job cannot hold, or a value the job rules
/// refuse.
pub fn build_still_job(inputs: &StillJobInputs<'_>, now: i64) -> Result<RenderJobFile, String> {
    let color_space = job_color_space(inputs.color_space)?;
    let hdr = hdr_source(inputs.scene)?;
    let mut scene = scene_from_snapshot(inputs.scene, inputs.params.width, inputs.params.height);
    scene.max_bounces = inputs.params.max_bounces;
    let job = RenderJobFile {
        format: JOB_FORMAT_VERSION,
        token: new_job_token(),
        label: inputs.label.clone(),
        created_at: now,
        design: inputs.design.clone(),
        scene,
        hdr,
        compute: job_compute(inputs.remote),
        output: inputs.output.to_string_lossy().into_owned(),
        kind: JobKind::Still(StillJob {
            samples_per_pixel: inputs.params.samples_per_pixel,
            color_space,
            preset_label: inputs.preset_label.clone(),
        }),
    };
    job.validate().map_err(|e| e.to_string())?;
    Ok(job)
}

/// Freezes one tilt video as a job. The output is the frame folder, which is not created.
///
/// The metric curves are stored only when an overlay is selected.
///
/// # Errors
///
/// An HDR map with no file, a colour space a job cannot hold, or a value the job rules
/// refuse.
pub fn build_video_job(
    request: &VideoExportRequest,
    label: String,
    design: DesignInfo,
    video_name: String,
    now: i64,
) -> Result<RenderJobFile, String> {
    let color_space = job_color_space(request.color_space)?;
    let hdr = hdr_source(&request.scene)?;
    let scene = scene_from_snapshot(&request.scene, request.width, request.height);
    let total_frames = u32::try_from(request.total_frames)
        .map_err(|_| "The tilt video has too many frames.".to_string())?;
    let overlay = OverlaySelection {
        brilliance: request.selection.brilliance,
        windowing: request.selection.windowing,
        extinction: request.selection.extinction,
        tilt_brilliance: request.selection.tilt_brilliance,
        angle: request.selection.angle,
    };
    let curves = (request.selection.count() > 0).then(|| MetricCurvesData {
        brilliance: request.curves.brilliance.clone(),
        windowing: request.curves.windowing.clone(),
        extinction: request.curves.extinction.clone(),
    });
    let job = RenderJobFile {
        format: JOB_FORMAT_VERSION,
        token: new_job_token(),
        label,
        created_at: now,
        design,
        scene,
        hdr,
        compute: job_compute(&request.remote),
        output: request.out_dir.to_string_lossy().into_owned(),
        kind: JobKind::TiltVideo(TiltVideoJob {
            axis_index: u32::try_from(request.axis_index).unwrap_or(0),
            start_deg: request.start_deg,
            end_deg: request.end_deg,
            step_deg: request.step_deg,
            total_frames,
            fps: request.fps,
            samples_per_pixel: request.samples_per_pixel,
            color_space,
            overlay,
            curves,
            keep_frames: request.keep_frames,
            video_name,
        }),
    };
    job.validate().map_err(|e| e.to_string())?;
    Ok(job)
}

/// The frame loop's request for a tilt video job.
///
/// `scene` is the job's scene rebuilt by [`snapshot_from_scene`]; `out_dir` is the
/// resolved frame folder; `remote` and `local_compute` are the machine's choices now.
pub fn video_request_from_job(
    job: &RenderJobFile,
    video: &TiltVideoJob,
    scene: SceneSnapshot,
    out_dir: PathBuf,
    remote: RemoteSelection,
    local_compute: LocalComputeTarget,
) -> VideoExportRequest {
    let selection = MetricSelection {
        brilliance: video.overlay.brilliance,
        windowing: video.overlay.windowing,
        extinction: video.overlay.extinction,
        tilt_brilliance: video.overlay.tilt_brilliance,
        angle: video.overlay.angle,
    };
    // The curves are read only for a selected overlay, and a job with one always holds them.
    let curves = video.curves.as_ref().map_or_else(
        || MetricCurves {
            brilliance: Vec::new(),
            windowing: Vec::new(),
            extinction: Vec::new(),
        },
        |curves| MetricCurves {
            brilliance: curves.brilliance.clone(),
            windowing: curves.windowing.clone(),
            extinction: curves.extinction.clone(),
        },
    );
    VideoExportRequest {
        scene,
        axis_index: video.axis_index as usize,
        start_deg: video.start_deg,
        end_deg: video.end_deg,
        step_deg: video.step_deg,
        total_frames: video.total_frames as usize,
        fps: video.fps,
        width: job.scene.width,
        height: job.scene.height,
        samples_per_pixel: video.samples_per_pixel,
        color_space: color_space_of(video.color_space),
        out_dir,
        out_name: video.video_name.clone(),
        selection,
        curves,
        keep_frames: video.keep_frames,
        remote,
        local_compute,
    }
}
