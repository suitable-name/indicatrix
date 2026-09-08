//! File-based import/export for the Edit tab: exporting the edited schedule as a
//! plain `.asc` ([`setup_export_asc_callback`]), and the paired native
//! `.indicatrix.toml` save/open ([`setup_save_native_callback`]/
//! [`setup_open_native_callback`]; legacy `.gemcut.toml` sidecars still open). See
//! this group's own `mod.rs` doc comment.

use super::{state::EditorState, view::refresh_all};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{show_toast, solid_preview::preview_state::SolidPreviewState},
};
use indicatrix_cut_core::{
    FingerprintCheck, History, load_paired, native_path_for_asc, save_paired,
};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, atomic::AtomicU64},
};

/// "Export .asc": writes the edited schedule (never the catalogue's own, unedited
/// one -- see this group's `mod.rs` doc comment) to a user-chosen path via `indicatrix_formats::to_asc_string`.
/// File only, no database write of any kind -- the catalogue stays read-only.
pub(super) fn setup_export_asc_callback(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_export_asc(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // Snapshot the schedule text, then drop the lock before the blocking native
        // file dialog -- same "don't hold a mutex across a blocking dialog call"
        // discipline every other export/import path in this crate follows (see e.g.
        // `gui::library::local::export::setup_export_asc_callback`). `to_asc_schedule`
        // is now fallible (it solves every tier's mast -- see
        // `Design::to_asc_schedule`'s doc comment): a `MissingAnchor` here means the
        // same "add a Scale Reference tier" problem the validation banner already
        // reports, so it is surfaced the same way (a toast) rather than exporting a
        // schedule with fabricated masts.
        let text = {
            let st = state.borrow();
            match st.design.to_asc_schedule() {
                Ok(schedule) => indicatrix_formats::asc::to_asc_string(&schedule),
                Err(missing) => {
                    show_toast(&ui, &format!("Cannot export: {missing}."), "error");
                    return;
                }
            }
        };

        let dest_path = rfd::FileDialog::new()
            .set_file_name("edited_design.asc")
            .add_filter(".asc design", &["asc"])
            .save_file();
        let Some(dest_path) = dest_path else {
            show_toast(&ui, "Export cancelled.", "info");
            return;
        };

        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&dest_path, text) {
            Ok(()) => show_toast(
                &ui,
                &format!("Exported edited schedule to {}", dest_path.display()),
                "success",
            ),
            Err(e) => show_toast(
                &ui,
                &format!("Failed to write {}: {e}", dest_path.display()),
                "error",
            ),
        }
    });
}

/// "Save Native": writes the design's `.indicatrix.toml` sidecar ALONGSIDE a
/// real `.asc` export -- never instead of one. That pairing is the module's own hard
/// rule (see `indicatrix_cut_core::native`'s module doc comment): a design must never exist only
/// in a format its author can be locked out of, so this always writes both files,
/// never the native file alone.
///
/// `indicatrix_cut_core::save_paired` decides whether the `.asc` half can stay byte-identical to
/// `state.original_asc_text` (an untouched design, or one whose only edits were
/// `girdle_diameter_mm`/`material`/`preform` -- none of which round-trip into `.asc`
/// at all) or must be freshly regenerated -- see that function's own doc comment.
/// `state.asc_filename`/`original_asc_text` are `Some` only when this design's
/// schedule came from a real `.asc` on disk (a catalogue attachment via "Load
/// Selected", or a prior Save/Open Native -- see `EditorState::asc_filename`'s own
/// doc comment); `None` (a brand-new "New" design, or the angle-table placeholder
/// reconstruction) always regenerates, exactly like "Export .asc" already does.
///
/// Both fields are refreshed from what was ACTUALLY just written on success, so a
/// second Save right after the first preserves ITS OWN output rather than reopening
/// the preservation question against stale text.
pub(super) fn setup_save_native_callback(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_save_native(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // Snapshot, then drop the borrow before the blocking native file dialog --
        // same discipline `setup_export_asc_callback` follows.
        let (design, default_asc_name, original_asc_text) = {
            let st = state.borrow();
            (
                st.design.clone(),
                st.asc_filename
                    .clone()
                    .unwrap_or_else(|| "edited_design.asc".to_string()),
                st.original_asc_text.clone(),
            )
        };

        let dest_path = rfd::FileDialog::new()
            .set_file_name(&default_asc_name)
            .add_filter(".asc design", &["asc"])
            .save_file();
        let Some(dest_path) = dest_path else {
            show_toast(&ui, "Save cancelled.", "info");
            return;
        };
        let asc_filename = dest_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(default_asc_name);
        let native_path = native_path_for_asc(&dest_path);

        let paired = match save_paired(&design, asc_filename.clone(), original_asc_text.as_deref())
        {
            Ok(paired) => paired,
            Err(e) => {
                show_toast(&ui, &format!("Cannot save: {e}"), "error");
                return;
            }
        };

        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&dest_path, &paired.asc_text) {
            show_toast(
                &ui,
                &format!("Failed to write {}: {e}", dest_path.display()),
                "error",
            );
            return;
        }
        if let Err(e) = std::fs::write(&native_path, &paired.native_toml) {
            show_toast(
                &ui,
                &format!("Failed to write {}: {e}", native_path.display()),
                "error",
            );
            return;
        }

        {
            let mut st = state.borrow_mut();
            st.asc_filename = Some(asc_filename);
            st.original_asc_text = Some(paired.asc_text);
        }

        let asc_note = if paired.asc_preserved {
            "unchanged .asc preserved"
        } else {
            ".asc regenerated from the current schedule"
        };
        show_toast(
            &ui,
            &format!(
                "Saved '{}' and '{}' ({asc_note}).",
                dest_path.display(),
                native_path.display()
            ),
            "success",
        );
    });
}

/// "Open Native": loads a `.indicatrix.toml` (or legacy `.gemcut.toml`) sidecar
/// together with its paired `.asc` via `indicatrix_cut_core::load_paired`. Replaces the whole editor state the same way
/// "New"/"Load Selected" do (fresh `History`, no printed proportions -- a locally
/// opened native file has no catalogue row to verify Deep Solve against, exactly
/// like a brand-new design).
///
/// The native file's own recorded `asc_filename` (a bare file name -- see
/// [`indicatrix_cut_core::NativeDesignFile::asc_filename`]'s own doc comment) is read AFTER
/// parsing the chosen file, since that field is the authoritative pointer to the
/// real paired file once a native file has actually been parsed; this never guesses
/// at a sibling `.asc` name the way [`indicatrix_cut_core::asc_path_for_native`] does for a
/// picker's initial directory (there is no picker here to seed -- the native file's
/// own directory plus its own recorded name is exact).
///
/// A fingerprint mismatch -- the paired `.asc` was re-touched (by `GemCAD`, by hand, or
/// otherwise) since this native file was last saved -- is a real, expected scenario,
/// never silently trusted either way (see `indicatrix_cut_core::native`'s own module doc comment
/// for why): the design still loads (`.asc` stays canonical for geometry), but the
/// toast reports the mismatch and the resulting per-tier overlay status in plain
/// language, using [`FingerprintCheck`]/`TierOverlay`'s own `Display` text, so the
/// user can decide whether to re-save rather than have it silently resolved either
/// way.
pub(super) fn setup_open_native_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_open_native(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let Some(native_path) = rfd::FileDialog::new()
            .add_filter("Indicatrix native design", &["toml"])
            .pick_file()
        else {
            show_toast(&ui, "Open cancelled.", "info");
            return;
        };

        let native_text = match std::fs::read_to_string(&native_path) {
            Ok(text) => text,
            Err(e) => {
                show_toast(
                    &ui,
                    &format!("Failed to read {}: {e}", native_path.display()),
                    "error",
                );
                return;
            }
        };
        let parsed_native = match indicatrix_cut_core::native::parse_toml_string(&native_text) {
            Ok(n) => n,
            Err(e) => {
                show_toast(
                    &ui,
                    &format!(
                        "'{}' is not a valid native design file: {e}",
                        native_path.display()
                    ),
                    "error",
                );
                return;
            }
        };
        let asc_path = native_path.with_file_name(&parsed_native.asc_filename);
        let asc_text = match std::fs::read_to_string(&asc_path) {
            Ok(text) => text,
            Err(e) => {
                show_toast(
                    &ui,
                    &format!(
                        "'{}' names a paired .asc file '{}', but reading '{}' failed: {e}",
                        native_path.display(),
                        parsed_native.asc_filename,
                        asc_path.display()
                    ),
                    "error",
                );
                return;
            }
        };

        match load_paired(&asc_text, &native_text) {
            Ok(loaded) => {
                let fingerprint_note = loaded.fingerprint.to_string();
                let overlay_note = loaded.tier_overlay.to_string();
                let is_mismatch = !matches!(loaded.fingerprint, FingerprintCheck::Match);

                let mut st = state.borrow_mut();
                *st = EditorState {
                    design: loaded.design,
                    history: History::new(),
                    printed_proportions: None,
                    generation: Arc::new(AtomicU64::new(0)),
                    deep_solve: None,
                    optimize: None,
                    pending_optimize: Arc::new(Mutex::new(None)),
                    asc_filename: Some(parsed_native.asc_filename),
                    original_asc_text: Some(asc_text),
                    pending_gear_remap: None,
                    pending_retarget: None,
                    multi_selected: std::collections::BTreeSet::new(),
                };
                refresh_all(&ui, &render_ctx, &preview_state, &solid_last_solved, &st);
                show_toast(
                    &ui,
                    &format!(
                        "Loaded '{}': {fingerprint_note}; {overlay_note}.",
                        native_path.display()
                    ),
                    if is_mismatch { "info" } else { "success" },
                );
            }
            Err(e) => show_toast(&ui, &format!("Cannot open: {e}"), "error"),
        }
    });
}

#[cfg(test)]
mod tests {
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{
        ConstraintTier, Design, FreshDesignSpec, MaterialSelection, PreformSpec, load_paired,
        save_paired,
    };

    /// Material name/RI override, gear, symmetry and mirror all round-trip through
    /// the exact pair of functions
    /// [`setup_save_native_callback`]/[`setup_open_native_callback`] call
    /// (`indicatrix_cut_core::save_paired`/`load_paired`) -- verified here directly rather
    /// than trusted, since this crate's own wiring exercises gear/symmetry/mirror
    /// persistence only through the editor's own "New Design"/design-settings forms,
    /// not through a dedicated round-trip test. `gear`/`symmetry`/
    /// `mirror` round-trip through the paired `.asc`'s own header (already
    /// exercised, indirectly, by every existing "Open Native" test in
    /// `indicatrix_cut_core::native`); `material`/`refractive_index_override` round-trip
    /// through the native sidecar's `[material]` table (already
    /// unit-tested in `indicatrix_cut_core::native` directly) -- this test's own
    /// value is confirming the ONE combination this app actually writes (a
    /// design with all four set together, via the same `save_paired`/
    /// `load_paired` this module's own callbacks call) survives intact.
    #[test]
    fn gear_symmetry_mirror_and_material_all_round_trip_through_save_and_open() {
        let spec = FreshDesignSpec {
            gear_teeth: 80,
            symmetry_order: 5,
            mirror: false,
            material: MaterialSelection {
                name: Some("Quartz".to_string()),
                specific_gravity_override: Some(2.65),
                refractive_index_override: Some(1.55),
            },
            preform: PreformSpec::cylinder(80, 1.4, 1.0, 1.3),
        };
        let mut design = Design::fresh_from_spec(spec);
        // A schedule with zero tiers exports (and re-solves) fine, but
        // `indicatrix_formats::asc::parse_asc` refuses to parse an `.asc` with no
        // facet ('a') records at all -- one real, anchored tier is what a
        // saved design would actually look like.
        design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 16.0, 32.0, 48.0, 64.0],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            detached: Vec::new(),
        });

        let saved = save_paired(&design, "roundtrip.asc", None).expect("a fresh design must save");
        let loaded = load_paired(&saved.asc_text, &saved.native_toml).expect("must load back");

        assert_eq!(loaded.design.meta.gear_teeth, 80);
        assert_eq!(loaded.design.meta.symmetry_order, 5);
        assert!(!loaded.design.meta.mirror);
        assert_eq!(loaded.design.material.name.as_deref(), Some("Quartz"));
        assert_eq!(loaded.design.material.specific_gravity_override, Some(2.65));
        assert_eq!(loaded.design.material.refractive_index_override, Some(1.55));
        // The effective RI actually written to `.asc`'s `I` line -- confirms
        // the override, not just the raw field, made the round trip in a way
        // that would show up in the exported schedule too.
        assert!((loaded.design.effective_refractive_index() - 1.55).abs() < 1e-9);
    }
}
