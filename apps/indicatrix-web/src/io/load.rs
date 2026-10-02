//! One loader per design file kind, each producing a [`DesignState`] plus the
//! notes the open message should carry. Every loader goes through the same
//! shared-crate entry point the desktop's Open uses:
//!
//! | Kind | Path |
//! |---|---|
//! | `.asc` | `indicatrix_formats::asc::decode_asc_bytes` -> `indicatrix_editor::loading::design_from_asc_text` |
//! | `.indicatrix` | `indicatrix_cut_core::native::design_from_str` (its `[meta]` table and attachments stay with the design) |
//! | `.asc` + older `.indicatrix.toml` sidecar | `decode_asc_bytes` -> `indicatrix_cut_core::native::load_paired` |
//! | older self-contained `.indicatrix.toml` alone | `indicatrix_cut_core::native::load_native_only` |
//! | `.gem` / `.gcs` | `indicatrix_editor::files::convert_foreign_design` -> the `.asc` path |
//!
//! Nothing here panics on bad input: every failure is a readable `Err(String)`.

use crate::app::state::{DesignSource, DesignState, WebApp};
use indicatrix_cut_core::{
    History,
    native::{
        FingerprintCheck, LoadNativeOnlyResult, LoadedDesign, MaterialResolution, TierOverlay,
        design_from_str, gem_material_from_custom_snapshot, load_native_only, load_paired,
        parse_toml_string,
    },
};
use indicatrix_editor::{
    EditorSession,
    files::{InputFileKind, convert_foreign_design},
    loading::design_from_asc_text,
};
use indicatrix_formats::asc::decode_asc_bytes;
use indicatrix_web_core::open_route::asc_name_for_design_file;

/// A loaded design and what the open message should add about it.
pub struct Opened {
    /// The design, ready for `WebApp::replace_design`.
    pub design: DesignState,
    /// Extra sentences (material restored, overlay skipped, converter warnings).
    pub notes: Vec<String>,
    /// Whether a note is something the user must look at (a warning, not success).
    pub needs_attention: bool,
}

/// A session over a freshly loaded design, with the desktop's plain history (the
/// desktop's Open paths build `History::new()` too).
fn session(design: indicatrix_cut_core::Design) -> EditorSession {
    EditorSession::with_history(design, History::new())
}

/// A bare `.asc` file.
///
/// # Errors
///
/// The `.asc` parser's message when the text is not a cutting schedule.
pub fn load_asc(file_name: &str, bytes: &[u8]) -> Result<Opened, String> {
    let text = decode_asc_bytes(bytes);
    let loaded = design_from_asc_text(file_name, &text, None)
        .map_err(|e| format!("\"{file_name}\" is not a readable .asc cutting schedule: {e}"))?;
    let mut design = DesignState::new(session(loaded.design), DesignSource::Asc);
    design.asc_filename = loaded.asc_filename;
    design.original_asc_text = loaded.original_asc_text;
    Ok(Opened {
        design,
        notes: Vec::new(),
        needs_attention: false,
    })
}

/// A `.gem` or `.gcs` design, converted to `.asc` and loaded along the `.asc`
/// path; recorded under `<stem>.asc` so a save never proposes the source name.
///
/// # Errors
///
/// The converter's message, or the `.asc` parser's for the converted text.
pub fn load_converted(
    file_name: &str,
    kind: InputFileKind,
    bytes: &[u8],
) -> Result<Opened, String> {
    let converted = convert_foreign_design(file_name, kind, bytes)
        .map_err(|e| format!("Cannot open \"{file_name}\": {e}"))?;
    let loaded = design_from_asc_text(&converted.asc_file_name, &converted.asc_text, None)
        .map_err(|e| {
            format!("Cannot open \"{file_name}\": the converted schedule does not parse: {e}")
        })?;
    let source = if kind == InputFileKind::Gem {
        DesignSource::Converted(".gem")
    } else {
        DesignSource::Converted(".gcs")
    };
    let mut design = DesignState::new(session(loaded.design), source);
    design.asc_filename = loaded.asc_filename;
    design.original_asc_text = loaded.original_asc_text;
    let needs_attention = !converted.warnings.is_empty();
    let notes = converted
        .warnings
        .into_iter()
        .map(|w| format!("Converter: {w}"))
        .collect();
    Ok(Opened {
        design,
        notes,
        needs_attention,
    })
}

/// Registers a design file's custom-material snapshot (when its material is not a
/// built-in) and returns the note to show, plus whether the material is still
/// unresolved -- the desktop's `open_native_self_contained`/`commit_loaded_native`
/// rule.
fn restore_material(
    app: &mut WebApp,
    design: &mut DesignState,
    resolution: MaterialResolution,
    snapshot: Option<indicatrix_cut_core::native::CustomMaterialSnapshot>,
) -> (Option<String>, bool) {
    let name = design.session.design.material.name.clone();
    if let (Some(snapshot), Some(name)) = (snapshot, name) {
        // The colour travels in the snapshot, so the restored material is the one saved.
        let restored =
            app.register_custom_material(gem_material_from_custom_snapshot(&name, &snapshot));
        match restored {
            Ok(()) => {
                design.custom_material = Some(snapshot);
                (
                    Some(format!(
                        "'{name}' was restored from this file's own saved material data."
                    )),
                    false,
                )
            }
            // A built-in's name: the file's data is not registered (it would replace the
            // built-in everywhere), and the design keeps naming a material this build
            // resolves on its own.
            Err(message) => (Some(message), true),
        }
    } else {
        let unresolved = matches!(resolution, MaterialResolution::Unresolved);
        (unresolved.then(|| resolution.to_string()), unresolved)
    }
}

/// The "written by a newer version" note, when it applies.
fn newer_version_note(newer: bool) -> Option<String> {
    newer.then(|| {
        "This file was written by a newer version of Indicatrix; some settings may not \
         have been understood and could be lost on your next save."
            .to_string()
    })
}

/// A self-contained older `.indicatrix.toml` already parsed by `load_native_only` (an
/// opened file, or this tab's own restored session from an earlier build).
pub fn design_from_native_only(
    app: &mut WebApp,
    loaded: LoadNativeOnlyResult,
    asc_filename: Option<String>,
    source: DesignSource,
) -> (DesignState, Vec<String>, bool) {
    let mut design = DesignState::new(session(loaded.design), source);
    design.asc_filename = asc_filename;
    design.printed_proportions = loaded.printed_proportions;
    let (material_note, unresolved) = restore_material(
        app,
        &mut design,
        loaded.material_resolution,
        loaded.restorable_custom_material,
    );
    let notes: Vec<String> = material_note
        .into_iter()
        .chain(newer_version_note(loaded.written_by_newer_version))
        .collect();
    (design, notes, unresolved || loaded.written_by_newer_version)
}

/// A design file already parsed by `design_from_str` (an opened `.indicatrix` file,
/// or this tab's own restored session). The notes name a material that could not be
/// restored and a file saved as a draft.
pub fn design_from_design_file(
    app: &mut WebApp,
    loaded: LoadedDesign,
    asc_filename: Option<String>,
    source: DesignSource,
) -> (DesignState, Vec<String>, bool) {
    let draft = loaded.draft;
    let mut design = DesignState::new(session(loaded.design), source);
    design.asc_filename = asc_filename;
    design.printed_proportions = loaded.printed_proportions;
    design.metadata = loaded.metadata;
    design.attachments = loaded.attachments;
    let (material_note, unresolved) = restore_material(
        app,
        &mut design,
        loaded.material_resolution,
        loaded.restorable_custom_material,
    );
    let mut notes: Vec<String> = material_note.into_iter().collect();
    if draft {
        notes.push(
            "It was saved as a draft because it did not solve at the time; solve it again \
             before exporting."
                .to_string(),
        );
    }
    (design, notes, unresolved || draft)
}

/// A self-contained `.indicatrix` design file.
///
/// # Errors
///
/// The codec's message: not a design file, damaged, or written by a newer Indicatrix
/// than this build reads (the message says to update).
pub fn load_design_file(app: &mut WebApp, file_name: &str, text: &str) -> Result<Opened, String> {
    let loaded = design_from_str(text).map_err(|e| format!("Cannot open \"{file_name}\": {e}"))?;
    let (design, notes, needs_attention) = design_from_design_file(
        app,
        loaded,
        Some(asc_name_for_design_file(file_name)),
        DesignSource::DesignFile,
    );
    Ok(Opened {
        design,
        notes,
        needs_attention,
    })
}

/// An older `.indicatrix.toml` sidecar opened on its own.
///
/// # Errors
///
/// The TOML parser's message; or, for an ordinary paired-mode sidecar (which
/// cannot be rebuilt without its `.asc`), a message naming the `.asc` to open with it.
pub fn load_native_alone(app: &mut WebApp, file_name: &str, text: &str) -> Result<Opened, String> {
    let recorded = parse_toml_string(text)
        .map_err(|e| format!("\"{file_name}\" is not a valid .indicatrix.toml sidecar: {e}"))?
        .asc_filename;
    match load_native_only(text) {
        Ok(loaded) => {
            let (design, notes, needs_attention) =
                design_from_native_only(app, loaded, Some(recorded), DesignSource::DesignFile);
            Ok(Opened {
                design,
                notes,
                needs_attention,
            })
        }
        Err(indicatrix_cut_core::native::LoadNativeOnlyError::NotSelfContained) => Err(format!(
            "\"{file_name}\" is paired with the .asc file \"{recorded}\" -- open both files \
             together (select them both, or drop them together)."
        )),
        Err(e) => Err(format!("Cannot open \"{file_name}\": {e}")),
    }
}

/// An older `.indicatrix.toml` sidecar with its `.asc`. A fingerprint mismatch (the
/// `.asc` changed since the sidecar was saved) keeps the `.asc`'s geometry and
/// skips the sidecar's per-tier overlay -- the desktop's "Load .asc only" answer;
/// the dialog offering "Apply anyway" arrives with the editor panels.
///
/// # Errors
///
/// Either file's parse error.
pub fn load_pair(
    app: &mut WebApp,
    native_name: &str,
    native_text: &str,
    asc_name: &str,
    asc_bytes: &[u8],
) -> Result<Opened, String> {
    let asc_text = decode_asc_bytes(asc_bytes).into_owned();
    let loaded = load_paired(&asc_text, native_text, false)
        .map_err(|e| format!("Cannot open \"{native_name}\" with \"{asc_name}\": {e}"))?;
    let mut design = DesignState::new(session(loaded.design), DesignSource::OlderPair);
    design.asc_filename = Some(asc_name.to_string());
    design.original_asc_text = Some(asc_text);
    design.printed_proportions = loaded.printed_proportions;
    let (material_note, unresolved) = restore_material(
        app,
        &mut design,
        loaded.material_resolution,
        loaded.restorable_custom_material,
    );
    let overlay_note = match loaded.tier_overlay {
        TierOverlay::SkippedFingerprintMismatch => Some(
            "The .asc changed since the sidecar was saved, so the sidecar's per-tier \
             settings (meets, detached facets, offsets) were not applied; the design is the \
             .asc as it is now."
                .to_string(),
        ),
        TierOverlay::SkippedTierCountMismatch {
            native_tiers,
            asc_tiers,
        } => Some(format!(
            "The sidecar describes {native_tiers} tiers but the .asc has {asc_tiers}; \
             its per-tier settings were not applied."
        )),
        _ => None,
    };
    let mismatch = !matches!(loaded.fingerprint, FingerprintCheck::Match);
    let notes: Vec<String> = overlay_note
        .into_iter()
        .chain(material_note)
        .chain(newer_version_note(loaded.written_by_newer_version))
        .collect();
    let needs_attention = mismatch || unresolved || loaded.written_by_newer_version;
    Ok(Opened {
        design,
        notes,
        needs_attention,
    })
}
