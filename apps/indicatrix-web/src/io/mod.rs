//! File input and output.
//!
//! Input: [`picker`] (File > Open, multi-select) and [`drop`] (drag-and-drop onto
//! the canvas) both end in [`open_files`], which sorts the files by kind
//! (`InputFileKind::classify`) and loads them through [`load`]
//! (designs) or [`hdr`] (environment maps).
//!
//! Output: [`save`] builds every File menu download with the desktop's string
//! APIs and hands it to [`download`].

pub mod download;
pub mod drop;
pub mod hdr;
pub mod load;
pub mod picker;
pub mod save;

use crate::app::{
    Ctx,
    persist::schedule_save,
    push::{MessageKind, push_all, push_settings, show_message},
    solve::auto_solve,
    state::WebApp,
};
use indicatrix_editor::files::{InputFileKind, paired_asc_index};
use load::Opened;

/// One opened or dropped file: its name and its bytes.
pub struct IncomingFile {
    /// The file name as the browser reports it (no directory).
    pub name: String,
    /// The whole file.
    pub bytes: Vec<u8>,
}

/// Why the file called `name` of `size_bytes` (`File.size`) must not be read, if it must not.
///
/// Checked before a picked or dropped file is copied into wasm memory, against
/// `indicatrix_web_core::input`'s limits (64 MiB for an `.hdr` map, 16 MiB for a design).
#[must_use]
pub fn oversize_message(name: &str, size_bytes: f64) -> Option<String> {
    let is_hdr = InputFileKind::from_file_name(name) == Some(InputFileKind::Hdr);
    indicatrix_web_core::input::size_refusal(name, size_bytes, is_hdr)
}

/// Opens a set of files picked or dropped together:
///
/// - `.hdr` maps are checked against the browser caps and kept for rendering;
/// - each file is routed by its content where the name cannot tell
///   (`InputFileKind::classify`): a `.indicatrix` design file is
///   self-contained, an older `.indicatrix.toml` / `.gemcut.toml` sidecar is read with
///   its `.asc`;
/// - of the designs, a `.indicatrix` file wins, then a sidecar (paired with the `.asc`
///   it names, `indicatrix_editor::files::paired_asc_index`, or opened alone when it
///   is self-contained), then a `.asc`, then a `.gem`/`.gcs`; any further design
///   file is named as ignored;
/// - anything else is named as not supported.
///
/// Replacing a design with unsaved changes asks first, in the Save / Discard / Cancel
/// dialog (`crate::editor::unsaved`); the files wait in the dialog's continuation.
pub fn open_files(ctx: &Ctx, files: Vec<IncomingFile>) {
    let mut designs: Vec<(InputFileKind, IncomingFile)> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    for file in files {
        match InputFileKind::classify(&file.name, &file.bytes) {
            Some(InputFileKind::Hdr) => open_hdr(ctx, file),
            Some(kind) => designs.push((kind, file)),
            None => unsupported.push(file.name),
        }
    }
    if !unsupported.is_empty() {
        show_message(
            ctx,
            MessageKind::Warning,
            &format!(
                "Not opened (unsupported file type): {}. Open .indicatrix, .asc, .indicatrix.toml, .gem, .gcs or .hdr files.",
                unsupported.join(", ")
            ),
        );
    }
    if designs.is_empty() {
        return;
    }
    let c = ctx.clone();
    crate::editor::unsaved::confirm_discard(ctx, move || open_designs(&c, designs));
}

/// [`open_files`]'s second half, once nothing unsaved is at stake: loads the best
/// design among `designs` and installs it.
fn open_designs(ctx: &Ctx, designs: Vec<(InputFileKind, IncomingFile)>) {
    let (result, ignored) = {
        let mut app = ctx.state.borrow_mut();
        load_best(&mut app, designs)
    };
    match result {
        Ok((opened, label)) => commit_opened(ctx, opened, &label, &ignored),
        Err(message) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// Keeps an `.hdr` upload for the renderer, or says why not.
fn open_hdr(ctx: &Ctx, file: IncomingFile) {
    let name = file.name.clone();
    match hdr::accept_hdr(&file.name, file.bytes) {
        Ok(upload) => {
            let summary = format!(
                "Environment map {name} ({} x {}, {:.1} MiB) is ready for rendering.",
                upload.width,
                upload.height,
                upload.bytes.len() as f64 / (1024.0 * 1024.0)
            );
            ctx.state.borrow_mut().hdr = Some(upload);
            if let Some(ui) = ctx.ui.upgrade() {
                push_settings(&ui, &ctx.state.borrow());
            }
            show_message(ctx, MessageKind::Success, &summary);
            // The renderer uploads it to its Workers (and reports the admission).
            crate::render::request_sync(ctx);
        }
        Err(message) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// Picks the design to open among `designs` and loads it. Returns the load
/// result (with the label to name it by) and the names of the design files not
/// opened.
fn load_best(
    app: &mut WebApp,
    mut designs: Vec<(InputFileKind, IncomingFile)>,
) -> (Result<(Opened, String), String>, Vec<String>) {
    let position = |kinds: &[InputFileKind], designs: &[(InputFileKind, IncomingFile)]| {
        designs.iter().position(|(kind, _)| kinds.contains(kind))
    };
    let result = if let Some(at) = position(&[InputFileKind::Design], &designs) {
        let (_, file) = designs.remove(at);
        open_design_file(app, &file)
    } else if let Some(native_at) = position(&[InputFileKind::Sidecar], &designs) {
        let (_, native) = designs.remove(native_at);
        open_sidecar(app, &native, &mut designs)
    } else if let Some(asc_at) = position(&[InputFileKind::Asc], &designs) {
        let (_, asc) = designs.remove(asc_at);
        load::load_asc(&asc.name, &asc.bytes).map(|opened| (opened, asc.name))
    } else if let Some(at) = position(&[InputFileKind::Gem, InputFileKind::Gcs], &designs) {
        let (kind, file) = designs.remove(at);
        load::load_converted(&file.name, kind, &file.bytes).map(|opened| (opened, file.name))
    } else {
        Err("No design file among the opened files.".to_string())
    };
    let ignored = designs.into_iter().map(|(_, file)| file.name).collect();
    (result, ignored)
}

/// A self-contained `.indicatrix` design file.
fn open_design_file(app: &mut WebApp, file: &IncomingFile) -> Result<(Opened, String), String> {
    let text = std::str::from_utf8(&file.bytes)
        .map_err(|_| format!("\"{}\" is not a text (UTF-8) design file.", file.name))?;
    load::load_design_file(app, &file.name, text).map(|opened| (opened, file.name.clone()))
}

/// An older overlay sidecar: with its paired `.asc` when one was opened alongside it
/// (removed from `others`), else on its own (when it is self-contained).
fn open_sidecar(
    app: &mut WebApp,
    native: &IncomingFile,
    others: &mut Vec<(InputFileKind, IncomingFile)>,
) -> Result<(Opened, String), String> {
    let text = String::from_utf8(native.bytes.clone()).map_err(|_| {
        format!(
            "\"{}\" is not a text (UTF-8) .indicatrix.toml sidecar.",
            native.name
        )
    })?;
    let recorded = indicatrix_cut_core::native::parse_toml_string(&text)
        .map_err(|e| {
            format!(
                "\"{}\" is not a valid .indicatrix.toml sidecar: {e}",
                native.name
            )
        })?
        .asc_filename;
    let asc_positions: Vec<usize> = others
        .iter()
        .enumerate()
        .filter(|(_, (kind, _))| *kind == InputFileKind::Asc)
        .map(|(i, _)| i)
        .collect();
    let asc_names: Vec<&str> = asc_positions
        .iter()
        .map(|&i| others[i].1.name.as_str())
        .collect();
    match paired_asc_index(&recorded, &native.name, &asc_names) {
        Some(pick) => {
            let (_, asc) = others.remove(asc_positions[pick]);
            load::load_pair(app, &native.name, &text, &asc.name, &asc.bytes)
                .map(|opened| (opened, format!("{} + {}", asc.name, native.name)))
        }
        None => load::load_native_alone(app, &native.name, &text)
            .map(|opened| (opened, native.name.clone())),
    }
}

/// Installs an opened design, refreshes everything, solves if small, persists,
/// and reports.
fn commit_opened(ctx: &Ctx, opened: Opened, label: &str, ignored: &[String]) {
    let Opened {
        design,
        mut notes,
        needs_attention,
    } = opened;
    ctx.state.borrow_mut().replace_design(design);
    if let Some(ui) = ctx.ui.upgrade() {
        push_all(&ui, &ctx.state.borrow());
    }
    auto_solve(ctx);
    schedule_save(ctx);
    // An opened file may already meet the guided walkthrough's current goal.
    crate::editor::guide::check_progress(ctx);
    if !ignored.is_empty() {
        notes.push(format!(
            "Only one design opens at a time; not opened: {}.",
            ignored.join(", ")
        ));
    }
    let text = if notes.is_empty() {
        format!("Opened {label}.")
    } else {
        format!("Opened {label}. {}", notes.join(" "))
    };
    let kind = if needs_attention || !ignored.is_empty() {
        MessageKind::Warning
    } else {
        MessageKind::Success
    };
    show_message(ctx, kind, &text);
}
