//! "Export Script...": the unfinished jobs written to a new folder as a PowerShell script,
//! a shell script or both, with one job file per job, so another computer can render them
//! with `indicatrix-cli`.
//!
//! The layout (`render-jobs-YYYY-MM-DD-HHMM/`, with `jobs/`, `jobs/assets/` and the
//! scripts) is built by the pure [`build_bundle`], which is tested without a window or a
//! disk. The picker, the HDR copies and the writes are the thin part around it; the writes run
//! on a background thread, so a large HDR map never freezes the window.

use super::{controller::unix_now, convert::engines_of, wiring::Deps};
use crate::{
    MainWindow, RenderJobsModel,
    bridge::{
        export_thread::filename_template::civil_from_unix_seconds, render_thread::RenderContext,
    },
    gui::{
        pickers::{PickerKind, PickerRequest, pick},
        show_toast,
    },
};
use indicatrix_net::messages::{content_hash, hash_hex};
use indicatrix_render_jobs::{
    JobKind, LocalEngines, RenderJobFile, codec,
    paths::{job_file_name, reserve_unique_file, reserve_unique_folder},
    script::{
        ASSETS_DIR, JOBS_DIR, RENDERS_DIR, RemoteArgs, ScriptCommand, ScriptFlavor, ScriptJob,
        ScriptSettings, script_bytes,
    },
    state::JobState,
};
use slint::ComponentHandle;
use std::path::{Path, PathBuf};

/// What a script folder is built with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleOptions {
    /// The scripts to write (each flavor once).
    pub flavors: Vec<ScriptFlavor>,
    /// The engines of the rendering computer, written at the top of the scripts.
    pub engines: LocalEngines,
    /// The remote worker the scripts use, or `None` to render on one computer.
    pub remote: Option<RemoteArgs>,
    /// Put the finished pictures and videos in `renders/` beside the script.
    pub outputs_beside: bool,
    /// The comment line with the export date and version.
    pub header_note: String,
}

/// The files of one script folder, with paths relative to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    /// The job files and the scripts, as bytes to write.
    pub files: Vec<(PathBuf, Vec<u8>)>,
    /// The HDR maps to copy: `(source file, destination, content hash in hex)`. One entry
    /// per hash, however many jobs use the map.
    pub hdr_copies: Vec<(PathBuf, PathBuf, String)>,
}

/// `render-jobs-YYYY-MM-DD-HHMM` for `(year, month, day, hour, minute)`.
pub fn bundle_folder_name(now: (i64, u32, u32, u32, u32)) -> String {
    let (year, month, day, hour, minute) = now;
    format!("render-jobs-{year:04}-{month:02}-{day:02}-{hour:02}{minute:02}")
}

/// Where a finished picture or video goes beside the scripts: `../renders/<name>` from
/// the job's folder, made unique within the bundle.
fn beside_output(output: &str, is_video: bool, taken: &mut Vec<PathBuf>) -> String {
    let name = Path::new(output).file_name().map_or_else(
        || "output".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let candidate = Path::new("..").join(RENDERS_DIR).join(name);
    let unique = if is_video {
        reserve_unique_folder(&candidate, taken, &|_| false)
    } else {
        reserve_unique_file(&candidate, taken, &|_| false)
    };
    taken.push(unique.clone());
    unique.to_string_lossy().replace('\\', "/")
}

/// Builds the script folder for `jobs` (in queue order).
///
/// - Each job file is named `NN-label.job.json` and holds the job with its paths made
///   relative to the `jobs` folder: an HDR map becomes `assets/<hash>.hdr` (one copy per
///   hash), and with `outputs_beside` the output becomes `../renders/<name>`.
/// - The scripts list the jobs in order.
///
/// # Errors
///
/// No jobs, a job that cannot be written, or a value a script cannot hold.
pub fn build_bundle(jobs: Vec<RenderJobFile>, options: &BundleOptions) -> Result<Bundle, String> {
    if jobs.is_empty() {
        return Err("There are no unfinished jobs to export.".to_string());
    }
    let total = jobs.len();
    let mut files = Vec::new();
    let mut hdr_copies: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    let mut script_jobs = Vec::with_capacity(total);
    let mut taken_outputs = Vec::new();
    for (index, mut job) in jobs.into_iter().enumerate() {
        let file_name = job_file_name(index + 1, total, &job.label);
        if let Some(hdr) = job.hdr.as_mut() {
            let sha = hdr.sha256_hex.clone();
            if !hdr_copies.iter().any(|(_, _, other)| *other == sha) {
                hdr_copies.push((
                    PathBuf::from(&hdr.path),
                    Path::new(JOBS_DIR)
                        .join(ASSETS_DIR)
                        .join(format!("{sha}.hdr")),
                    sha.clone(),
                ));
            }
            // Relative to the job file, which is inside `jobs`.
            hdr.path = format!("{ASSETS_DIR}/{sha}.hdr");
        }
        let is_video = matches!(job.kind, JobKind::TiltVideo(_));
        if options.outputs_beside {
            job.output = beside_output(&job.output, is_video, &mut taken_outputs);
        }
        let text = codec::to_text(&job).map_err(|error| error.to_string())?;
        script_jobs.push(ScriptJob {
            command: if is_video {
                ScriptCommand::TiltVideo
            } else {
                ScriptCommand::Render
            },
            job_file: format!("{JOBS_DIR}/{file_name}"),
            label: job.label.clone(),
        });
        files.push((Path::new(JOBS_DIR).join(&file_name), text.into_bytes()));
    }
    let settings = ScriptSettings {
        engines: options.engines,
        remote: options.remote.clone(),
        header_note: options.header_note.clone(),
    };
    for flavor in &options.flavors {
        let bytes =
            script_bytes(*flavor, &script_jobs, &settings).map_err(|error| error.to_string())?;
        files.push((PathBuf::from(flavor.file_name()), bytes));
    }
    Ok(Bundle { files, hdr_copies })
}

/// Writes `bundle` into `folder`, checking each copied HDR map is still the file the job
/// was made with.
fn write_all(folder: &Path, bundle: &Bundle) -> Result<(), String> {
    let write = |target: &Path, bytes: &[u8]| -> Result<(), String> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
        }
        std::fs::write(target, bytes)
            .map_err(|error| format!("Could not write {}: {error}", target.display()))
    };
    for (source, destination, sha) in &bundle.hdr_copies {
        let bytes = std::fs::read(source).map_err(|error| {
            format!(
                "The HDR map {} could not be read: {error}",
                source.display()
            )
        })?;
        if hash_hex(&content_hash(&bytes)) != *sha {
            return Err(format!(
                "The HDR map {} changed since the job was added.",
                source.display()
            ));
        }
        write(&folder.join(destination), &bytes)?;
    }
    for (relative, bytes) in &bundle.files {
        write(&folder.join(relative), bytes)?;
    }
    Ok(())
}

/// Builds the script folder inside `base` and writes it. Returns the new folder and how many
/// jobs it holds. A failure leaves nothing behind.
pub(super) fn write_bundle(
    base: &Path,
    jobs: Vec<RenderJobFile>,
    options: &BundleOptions,
    now: i64,
) -> Result<(PathBuf, usize), String> {
    let count = jobs.len();
    let bundle = build_bundle(jobs, options)?;
    let (year, month, day, hour, minute, _) = civil_from_unix_seconds(now);
    let folder = reserve_unique_folder(
        &base.join(bundle_folder_name((year, month, day, hour, minute))),
        &[],
        &|path| path.exists(),
    );
    let written = write_all(&folder, &bundle);
    if written.is_err() {
        // Only the folder this export just made.
        let _ = std::fs::remove_dir_all(&folder);
    }
    written.map(|()| (folder, count))
}

/// The jobs that are not Done or Cancelled, in queue order.
fn collect_unfinished(deps: &Deps) -> Result<Vec<RenderJobFile>, String> {
    let db = deps
        .db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let metas = db
        .list_render_jobs()
        .map_err(|error| format!("The render job list could not be read: {error}"))?;
    let mut jobs = Vec::new();
    for meta in metas
        .iter()
        .filter(|meta| JobState::parse(&meta.state).is_none_or(|state| !state.is_terminal()))
    {
        let row = db
            .get_render_job(meta.job_id)
            .map_err(|error| format!("The job \"{}\" could not be read: {error}", meta.label))?
            .ok_or_else(|| format!("The job \"{}\" is gone.", meta.label))?;
        let job = codec::from_text(&row.snapshot)
            .map_err(|error| format!("The job \"{}\" could not be read: {error}", meta.label))?;
        jobs.push(job);
    }
    drop(db);
    Ok(jobs)
}

/// "Choose Folder and Export": asks for the folder, then writes the script folder in the
/// background.
pub(super) fn export_script(ui: &MainWindow, deps: &Deps) {
    let jobs = match collect_unfinished(deps) {
        Ok(jobs) => jobs,
        Err(message) => {
            show_toast(ui, &message, "error");
            return;
        }
    };
    if jobs.is_empty() {
        show_toast(ui, "There are no unfinished jobs to export.", "info");
        return;
    }
    let model = ui.global::<RenderJobsModel>();
    let flavors = match model.get_script_flavor_index() {
        1 => vec![ScriptFlavor::PowerShell],
        2 => vec![ScriptFlavor::Sh],
        _ => vec![ScriptFlavor::PowerShell, ScriptFlavor::Sh],
    };
    let settings = deps.settings_store.snapshot().settings;
    let remote = if model.get_script_target_index() == 1 {
        let Some(worker) = settings.remote_worker() else {
            show_toast(ui, "No remote worker is set up in Settings.", "error");
            return;
        };
        Some(RemoteArgs {
            address: worker.address,
            cert_dir: worker.cert_dir,
        })
    } else {
        None
    };
    let now = unix_now();
    let (year, month, day, hour, minute, _) = civil_from_unix_seconds(now);
    let options = BundleOptions {
        flavors,
        engines: engines_of(RenderContext::lock(&deps.render_ctx).local_compute_target),
        remote,
        outputs_beside: model.get_script_outputs_beside(),
        header_note: format!(
            "Exported {year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC by Indicatrix Cut {}.",
            env!("CARGO_PKG_VERSION")
        ),
    };
    let starting_dir =
        (!settings.export_directory.is_empty()).then(|| PathBuf::from(&settings.export_directory));
    pick(
        ui,
        PickerRequest {
            kind: PickerKind::PickFolder,
            title: Some("Choose where to put the script folder".to_string()),
            filters: Vec::new(),
            default_file_name: None,
            starting_dir,
        },
        move |ui, dir| {
            // Closing the picker changes nothing.
            let Some(dir) = dir else {
                return;
            };
            let ui_weak = ui.as_weak();
            let spawned = std::thread::Builder::new()
                .name("render-job-script".to_string())
                .spawn(move || {
                    let result = write_bundle(&dir, jobs, &options, now);
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| finish(&ui, result));
                });
            if let Err(error) = spawned {
                show_toast(
                    ui,
                    &format!("The script could not be written: {error}"),
                    "error",
                );
            }
        },
    );
}

/// Back on the UI thread: say what was written, and close the section.
fn finish(ui: &MainWindow, result: Result<(PathBuf, usize), String>) {
    match result {
        Ok((folder, count)) => {
            ui.global::<RenderJobsModel>().set_script_open(false);
            show_toast(
                ui,
                &format!(
                    "Exported {count} {} and the run-render-jobs script to {}.",
                    if count == 1 { "job" } else { "jobs" },
                    folder.display()
                ),
                "success",
            );
        }
        Err(message) => show_toast(ui, &message, "error"),
    }
}
