//! Opening design files: [`open_picked`] sorts the picked files by kind, loads the best
//! design among them and installs it as the current design.
//!
//! A port of the web app's `io` module (`open_files`, `load_best` and the per-kind
//! loaders), every one of which goes through the same shared-crate entry point the
//! desktop's Open uses:
//!
//! | Kind | Path |
//! |---|---|
//! | `.asc` | `decode_asc_bytes` -> `design_from_asc_text` |
//! | `.isdd` (or a legacy `.indicatrix`) | `design_from_str` (its `[meta]` table and attachments stay with the document) |
//! | `.gem` / `.gcs` | `convert_foreign_design` -> the `.asc` path |
//!
//! Nothing here panics on bad input: every failure is a readable [`OpenError`], and a
//! failed open leaves the current design untouched.

use std::fmt;

use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{
    Design, History,
    native::{
        CustomMaterialSnapshot, LoadedDesign, MaterialResolution, design_from_str,
        gem_material_from_custom_snapshot,
    },
};
use indicatrix_editor::{
    EditorSession,
    files::{InputFileKind, convert_foreign_design},
    loading::design_from_asc_text,
};
use indicatrix_formats::asc::decode_asc_bytes;
use indicatrix_web_core::{
    custom_material::{builtin_name_conflict, builtin_name_message},
    input::MAX_DESIGN_FILE_BYTES,
    open_route::asc_name_for_design_file,
};

use super::{DocumentOrigin, DocumentState, display_title, install, unix_now};
use crate::{
    app_state::AppState,
    platform::PickedFile,
    settings::{RecentItem, RecentKind},
};

/// Why nothing was opened. The message is a sentence the UI shows as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenError {
    /// What went wrong, for the user.
    pub message: String,
}

impl OpenError {
    /// An error with this message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OpenError {}

/// What opening did, for the toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenReport {
    /// What was opened: the file name.
    pub label: String,
    /// Extra sentences: a material restored from the file, converter warnings, a draft.
    pub notes: Vec<String>,
    /// Whether a note is something the user must look at (a warning, not good news).
    pub needs_attention: bool,
    /// Design files among the picked ones that were not opened (only one design opens
    /// at a time).
    pub ignored: Vec<String>,
    /// Files that were not read at all, each with the reason (too large, not a design).
    pub skipped: Vec<String>,
}

impl OpenReport {
    /// The text for a snackbar: "Opened X." followed by the notes, the ignored files and
    /// the skipped ones.
    #[must_use]
    pub fn message(&self) -> String {
        let mut text = format!("Opened {}.", self.label);
        for note in &self.notes {
            text.push(' ');
            text.push_str(note);
        }
        if !self.ignored.is_empty() {
            text.push_str(" Only one design opens at a time; not opened: ");
            text.push_str(&self.ignored.join(", "));
            text.push('.');
        }
        for reason in &self.skipped {
            text.push(' ');
            text.push_str(reason);
        }
        text
    }

    /// Whether the toast should read as a warning.
    #[must_use]
    pub const fn is_warning(&self) -> bool {
        self.needs_attention || !self.ignored.is_empty() || !self.skipped.is_empty()
    }
}

/// A design that loaded and waits to be installed.
pub(super) struct Loaded {
    /// A session over the design with a plain history.
    pub(super) session: EditorSession,
    /// The document state that goes with it.
    pub(super) document: DocumentState,
    /// A custom material restored from the file, to register.
    pub(super) restored: Option<GemMaterial>,
    /// Sentences for the open message.
    pub(super) notes: Vec<String>,
    /// Whether a note needs the user's attention.
    pub(super) attention: bool,
}

/// Opens the best design among `files` and makes it the current design.
///
/// A `.isdd` (or legacy `.indicatrix`) file wins, then a `.asc`, then a `.gem` or `.gcs`. The current
/// design is replaced even when it has unsaved changes; the caller asks first.
///
/// Files over the design-file size cap and files that are no design are named in
/// [`OpenReport::skipped`]; further design files in [`OpenReport::ignored`].
///
/// # Errors
/// [`OpenError`] when no file could be opened: nothing usable was picked, or the best
/// design is damaged or from a newer Indicatrix than this build reads. The current design
/// is unchanged then.
pub fn open_picked(state: &mut AppState, files: Vec<PickedFile>) -> Result<OpenReport, OpenError> {
    open_files(state, files, true)
}

/// [`open_picked`], optionally without the "recent" entry for the file (a design opened
/// from the library records a library entry instead).
pub(super) fn open_files(
    state: &mut AppState,
    files: Vec<PickedFile>,
    record_recent: bool,
) -> Result<OpenReport, OpenError> {
    let mut designs: Vec<(InputFileKind, PickedFile)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for file in files {
        if file.bytes.len() as u64 > MAX_DESIGN_FILE_BYTES {
            skipped.push(oversize_message(&file));
            continue;
        }
        match InputFileKind::classify(&file.display_name, &file.bytes) {
            Some(InputFileKind::Hdr) => skipped.push(format!(
                "\"{}\" is an environment map, not a design.",
                file.display_name
            )),
            Some(kind) => designs.push((kind, file)),
            None => skipped.push(format!(
                "\"{}\" is not a file type Indicatrix opens (.isdd, .indicatrix, .asc, .gem, .gcs).",
                file.display_name
            )),
        }
    }
    if designs.is_empty() {
        let message = if skipped.is_empty() {
            "No design file was selected.".to_owned()
        } else {
            skipped.join(" ")
        };
        return Err(OpenError::new(message));
    }
    let (loaded, label, ignored) = load_best(designs).map_err(OpenError::new)?;
    let Loaded {
        session,
        document,
        restored,
        notes,
        attention,
    } = loaded;
    let recent_name = document.display_name.clone().filter(|_| record_recent);
    install(state, session, document, restored);
    if let Some(display_name) = recent_name {
        let title = display_title(state);
        state.update_settings(|settings| {
            settings.push_recent(RecentItem {
                title,
                opened_at_unix: unix_now(),
                kind: RecentKind::Document { display_name },
            });
        });
    }
    Ok(OpenReport {
        label,
        notes,
        needs_attention: attention,
        ignored,
        skipped,
    })
}

/// The sentence for a file over [`MAX_DESIGN_FILE_BYTES`].
fn oversize_message(file: &PickedFile) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    format!(
        "\"{}\" is {:.1} MiB; design files are limited to {} MiB, so it was not opened.",
        file.display_name,
        file.bytes.len() as f64 / MIB,
        MAX_DESIGN_FILE_BYTES / (1024 * 1024)
    )
}

/// Picks the design to open among `designs` and loads it. Returns it with the label to
/// name it by and the names of the design files not opened.
fn load_best(
    mut designs: Vec<(InputFileKind, PickedFile)>,
) -> Result<(Loaded, String, Vec<String>), String> {
    let position = |kinds: &[InputFileKind], designs: &[(InputFileKind, PickedFile)]| {
        designs.iter().position(|(kind, _)| kinds.contains(kind))
    };
    let (loaded, label) = if let Some(at) = position(&[InputFileKind::Design], &designs) {
        let (_, file) = designs.remove(at);
        let loaded = load_design_file(&file)?;
        (loaded, file.display_name)
    } else if let Some(at) = position(&[InputFileKind::Asc], &designs) {
        let (_, asc) = designs.remove(at);
        let loaded = load_asc(&asc)?;
        (loaded, asc.display_name)
    } else if let Some(at) = position(&[InputFileKind::Gem, InputFileKind::Gcs], &designs) {
        let (kind, file) = designs.remove(at);
        let loaded = load_converted(&file, kind)?;
        (loaded, file.display_name)
    } else {
        return Err("No design file among the selected files.".to_owned());
    };
    let ignored = designs
        .into_iter()
        .map(|(_, file)| file.display_name)
        .collect();
    Ok((loaded, label, ignored))
}

/// A session over a freshly loaded design, with the desktop's plain history (the
/// desktop's Open paths build `History::new()` too).
fn session_of(design: Design) -> EditorSession {
    EditorSession::with_history(design, History::new())
}

/// The document state of a design read from the file called `display_name`.
fn document_for(display_name: &str, asc_filename: Option<String>) -> DocumentState {
    DocumentState {
        origin: DocumentOrigin::Document {
            display_name: display_name.to_owned(),
        },
        display_name: Some(display_name.to_owned()),
        asc_filename,
        ..DocumentState::default()
    }
}

/// The file's text, or a sentence saying it is not UTF-8 text.
fn utf8_text<'a>(file: &'a PickedFile, what: &str) -> Result<&'a str, String> {
    std::str::from_utf8(&file.bytes)
        .map_err(|_| format!("\"{}\" is not a text (UTF-8) {what}.", file.display_name))
}

/// A bare `.asc` file.
fn load_asc(file: &PickedFile) -> Result<Loaded, String> {
    let text = decode_asc_bytes(&file.bytes);
    let loaded = design_from_asc_text(&file.display_name, &text, None).map_err(|error| {
        format!(
            "\"{}\" is not a readable .asc cutting schedule: {error}",
            file.display_name
        )
    })?;
    let mut document = document_for(&file.display_name, loaded.asc_filename);
    document.original_asc_text = loaded.original_asc_text;
    Ok(Loaded {
        session: session_of(loaded.design),
        document,
        restored: None,
        notes: Vec::new(),
        attention: false,
    })
}

/// A `.gem` or `.gcs` design, converted to `.asc` and loaded along the `.asc` path;
/// recorded under `<stem>.asc` so a save never proposes the source name.
fn load_converted(file: &PickedFile, kind: InputFileKind) -> Result<Loaded, String> {
    let name = &file.display_name;
    let converted = convert_foreign_design(name, kind, &file.bytes)
        .map_err(|error| format!("Cannot open \"{name}\": {error}"))?;
    let loaded = design_from_asc_text(&converted.asc_file_name, &converted.asc_text, None)
        .map_err(|error| {
            format!("Cannot open \"{name}\": the converted schedule does not parse: {error}")
        })?;
    let mut document = document_for(name, loaded.asc_filename);
    document.original_asc_text = loaded.original_asc_text;
    let extension = if kind == InputFileKind::Gem {
        ".gem"
    } else {
        ".gcs"
    };
    let mut notes = vec![format!(
        "Converted from a {extension} file; check the result before cutting."
    )];
    let attention = !converted.warnings.is_empty();
    notes.extend(
        converted
            .warnings
            .into_iter()
            .map(|warning| format!("Converter: {warning}")),
    );
    Ok(Loaded {
        session: session_of(loaded.design),
        document,
        restored: None,
        notes,
        attention,
    })
}

/// What restoring a file's custom material came to.
struct MaterialRestore {
    /// The material to register.
    material: Option<GemMaterial>,
    /// The snapshot to keep for the next save.
    snapshot: Option<CustomMaterialSnapshot>,
    /// The sentence to tell the user.
    note: Option<String>,
    /// Whether the design still names a material this build cannot resolve.
    unresolved: bool,
}

/// Decides what to do with the custom-material snapshot a file carried (when its
/// material is not a built-in): the desktop's `open_native_self_contained` rule.
fn restore_material(
    name: Option<&str>,
    resolution: MaterialResolution,
    snapshot: Option<CustomMaterialSnapshot>,
) -> MaterialRestore {
    if let (Some(snapshot), Some(name)) = (snapshot, name) {
        // A built-in's name: the file's data is not registered (it would replace the
        // built-in everywhere), and the design keeps naming a material this build
        // resolves on its own.
        if let Some(builtin) = builtin_name_conflict(name) {
            return MaterialRestore {
                material: None,
                snapshot: None,
                note: Some(builtin_name_message(name, &builtin)),
                unresolved: true,
            };
        }
        return MaterialRestore {
            material: Some(gem_material_from_custom_snapshot(name, &snapshot)),
            snapshot: Some(snapshot),
            note: Some(format!(
                "'{name}' was restored from this file's own saved material data."
            )),
            unresolved: false,
        };
    }
    let unresolved = matches!(resolution, MaterialResolution::Unresolved);
    MaterialRestore {
        material: None,
        snapshot: None,
        note: unresolved.then(|| resolution.to_string()),
        unresolved,
    }
}

/// A design file already parsed by `design_from_str` (an opened `.isdd` file, or
/// the autosave). The notes name a material that could not be restored and a file saved
/// as a draft.
pub(super) fn from_design_file(
    loaded: LoadedDesign,
    asc_filename: Option<String>,
    display_name: &str,
) -> Loaded {
    let draft = loaded.draft;
    let restore = restore_material(
        loaded.design.material.name.as_deref(),
        loaded.material_resolution,
        loaded.restorable_custom_material,
    );
    let mut document = document_for(display_name, asc_filename);
    document.printed_proportions = loaded.printed_proportions;
    document.metadata = loaded.metadata;
    document.attachments = loaded.attachments;
    document.custom_material = restore.snapshot;
    let mut notes: Vec<String> = restore.note.into_iter().collect();
    if draft {
        notes.push(
            "It was saved as a draft because it did not solve at the time; solve it again \
             before exporting."
                .to_owned(),
        );
    }
    Loaded {
        session: session_of(loaded.design),
        document,
        restored: restore.material,
        notes,
        attention: restore.unresolved || draft,
    }
}

/// A self-contained `.isdd` (or legacy `.indicatrix`) design file.
fn load_design_file(file: &PickedFile) -> Result<Loaded, String> {
    let text = utf8_text(file, "design file")?;
    let loaded = design_from_str(text)
        .map_err(|error| format!("Cannot open \"{}\": {error}", file.display_name))?;
    Ok(from_design_file(
        loaded,
        Some(asc_name_for_design_file(&file.display_name)),
        &file.display_name,
    ))
}
