//! Shared save/export helpers: a design's custom-catalogue material snapshot, the
//! "not a closed solid" header marker, the cached-solve-reusing paired-save entry
//! point, the source-catalogue-row footnote stamp, and a design's own
//! [`CustomMaterialSnapshot`] for a `.indicatrix` design file, and that file's text.

use crate::{bridge::render_thread::RenderContext, gui::optics::crystal_optics::library_material};
use indicatrix::{
    geometry::{meet_solver::SolvedTier, stone_metrics::ExternalProportions},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    Design, built_in_refractive_index,
    native::{
        CustomMaterialSnapshot, DesignExtras, PairedSave, SaveError, SaveExtras, design_to_file,
        save_paired_extended, save_paired_extended_from_solved,
    },
};
use indicatrix_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex};

/// This design's currently registered custom catalogue materials -- the same list
/// [`crate::gui::editor::view::refresh_design_settings`]'s on-screen "Eff. RI" resolves against
/// (`RenderContext::custom_materials`) -- snapshotted (a cheap `Arc` clone, not a
/// deep copy) for a save/export callback to pass to a `_with` entry point
/// (`Design::to_asc_schedule_with`/`Design::cutting_sheet_with`) or into
/// `SaveExtras::custom_catalogue` (for `save_paired_extended`/
/// `save_paired_extended_from_solved`) so a design on a CUSTOM catalogue material
/// writes/prints that material's own refractive index instead of silently falling
/// back to the legacy schedule RI.
pub(super) fn snapshot_custom_materials(
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Arc<Vec<GemMaterial>> {
    Arc::clone(
        &render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .custom_materials,
    )
}

/// The leading header a confirmed-with-reason write stamps into a written
/// schedule, so the file itself carries a hint of why it needed confirming. A
/// distinct marker from [`indicatrix_formats::asc::mark_reconstructed`]'s own
/// "RECONSTRUCTED" line: that one is specific to an angle-table placeholder
/// reconstruction, a different situation from a design that solves but does
/// not close a real stone.
pub(super) const NOT_CLOSED_SOLID_MARKER: &str = "NOT A CLOSED SOLID";

/// Builds the header line itself, or `None` when `headers` already starts with one
/// (never stamped twice, mirroring `mark_reconstructed`'s own idempotence).
pub(super) fn degenerate_marker_header(headers: &[String], message: &str) -> Option<String> {
    let already_marked = headers
        .first()
        .is_some_and(|h| h.starts_with(NOT_CLOSED_SOLID_MARKER));
    (!already_marked).then(|| format!("{NOT_CLOSED_SOLID_MARKER} -- {message}"))
}

/// A design that solves but does not close a real stone (every mast `0.0`
/// after an angle-table placeholder load is the common way this happens, but a
/// half-built schedule mid-session hits it too) would otherwise export/save as a
/// normal-looking file with no signal beyond a validation banner the cutter may have
/// scrolled past. Checking `design.status()` catches this -- reusing the Edit tab's
/// own validation-banner wording so this asks about exactly the same problem the
/// cutter already saw on screen, never a second, differently-worded description of it.
///
/// Every write call site in this module resolves the design's solve off the UI
/// thread first (via [`resolve_solve_at`]), then decides
/// synchronously with [`decide_write_status`] (a pure decision over the
/// already-resolved solve) whether to prompt with [`ask_write_confirm`] (the
/// in-window dialog, never a blocking native message dialog): `resolve_solve_at` ->
/// `decide_write_status` -> (`Fine`: write immediately) or (`NeedsConfirm`:
/// `ask_write_confirm`, write from its `on_accept`).
///
/// The marker message is always a real finding about the design's geometry. A solve
/// that never finished ([`SolveFailure::Superseded`]) is not one: it aborts the
/// write before this function is reached, so such a message is never stamped into a
/// file header.
///
/// Also returns the `solved` masts themselves (`None` only for a `MissingAnchor`),
/// so a caller that goes on to write the file can pass them straight to
/// [`save_paired_extended_from_solved`] instead of re-solving the same design
/// a second time via `Design::to_asc_schedule` (see [`save_paired_reusing_solve`]).
/// Calls [`save_paired_extended_from_solved`] when `solved` is available and
/// `design` actually has tiers to build masts for, so a caller that already solved
/// once (e.g. [`confirm_status_before_write`], or [`run_autosave_tick`]'s own local
/// solve) never pays for [`save_paired_extended`]'s internal `Design::solve()`
/// a second time. Falls back to [`save_paired_extended`] itself for the two
/// cases `solved` cannot stand in for: `solved` is `None` (the design does not
/// currently solve -- re-solving there is cheap, since `Design::solve` returns
/// `MissingAnchor` before ever running the expensive `solve_meet_points` pass) or
/// `design.tiers` is empty (the tier-less draft path, which never solves at all).
///
/// Every caller populates `extras.custom_catalogue` with its own
/// `RenderContext::custom_materials` snapshot (see `snapshot_custom_materials`), so
/// a design on a CUSTOM catalogue material writes that material's own refractive
/// index to the `.asc` `I` line instead of the legacy schedule RI -- see
/// [`SaveExtras::custom_catalogue`]'s own doc comment.
pub(super) fn save_paired_reusing_solve(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    asc_filename: impl Into<String>,
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
    printed_proportions: Option<&ExternalProportions>,
    extras: &SaveExtras<'_>,
) -> Result<PairedSave, SaveError> {
    match solved {
        Some(solved) if !design.tiers.is_empty() => save_paired_extended_from_solved(
            design,
            solved,
            asc_filename,
            original_asc_text,
            placeholder_note,
            printed_proportions,
            extras,
        ),
        _ => save_paired_extended(
            design,
            asc_filename,
            original_asc_text,
            placeholder_note,
            printed_proportions,
            extras,
        ),
    }
}

/// Stamps (or clears) `footnotes`' own recorded source-catalogue-row marker before a
/// `.asc` is written, so provenance comes from a recorded id, never a
/// title/filename guess. Removes any previous stamp first (idempotent, same
/// reasoning as [`degenerate_marker_header`]) so a design that changes source row
/// (Save creating its very first row) or loses one (the row was deleted) never
/// carries two stamps, or a stale one, across a later save. `gui::library::local::
/// import::save_imported_design` is the reader: a `.asc` re-imported later recovers
/// `source_entry_id` from exactly this line via
/// [`indicatrix_vault::local::parse_source_entry_footnote`] and records it as
/// `diagram_entries.derived_from_entry_id`, turning what would otherwise be a second
/// same-titled row into a recorded version of the original.
///
/// Called from both "Export .asc" and "Save": a bare Export with no design
/// sidecar at all still needs to survive an export-then-reimport round trip, so the
/// plain `.asc` itself has to carry this, not only the design file (which already
/// records everything else about this design, but is never attached to a plain
/// Export).
pub(super) fn stamp_source_entry_footnote(
    footnotes: &mut Vec<String>,
    source_entry_id: Option<i64>,
) {
    footnotes.retain(|f| !f.starts_with(indicatrix_vault::local::SOURCE_ENTRY_FOOTNOTE_PREFIX));
    if let Some(id) = source_entry_id {
        footnotes.push(indicatrix_vault::local::format_source_entry_footnote(id));
    }
}

/// Builds `design`'s [`CustomMaterialSnapshot`] for a save, so a design saved
/// under a custom material does not silently reload as Diamond -- without this,
/// nothing would attach that material's own numbers to the sidecar. `None` when
/// `design.material.name` is unset, or names one of the
/// built-in presets ([`built_in_refractive_index`] resolves it -- nothing to
/// preserve; any build already knows that name), or names a custom material this
/// build's own database has no row for (nothing to snapshot from).
///
/// Reads the custom-materials DATABASE row rather than a resolved `GemMaterial`:
/// the row holds the cutter's own originally typed mean RI/dispersion/
/// birefringence/specific-gravity numbers directly, while re-deriving them from a
/// resolved `GemMaterial`'s dispersion curve would round-trip through
/// `GemMaterial::new_custom`'s own Cauchy fit for no reason when the authored
/// numbers already exist. `crystal_system`/`optical_character` are carried
/// through as the row's own plain-text names (`""` when the row predates those
/// fields) -- [`gem_material_from_custom_snapshot`] re-derives both from the
/// snapshot's `birefringence_delta` sign on load regardless, so these are for a
/// human reading the raw TOML only.
pub(super) fn custom_material_snapshot_for_save(
    design: &Design,
    db: &Arc<Mutex<Database>>,
) -> Option<CustomMaterialSnapshot> {
    let name = design.material.name.as_deref()?;
    if built_in_refractive_index(name).is_some() {
        return None;
    }
    let row = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_custom_materials()
        .ok()?
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(name))?;
    // The snapshot itself (the typed numbers, the dispersion model, both colour payloads) is
    // `LibraryMaterial::snapshot` in `indicatrix-cut-core`, shared with the command line.
    Some(library_material(&row).snapshot())
}

/// The text of `design`'s self-contained `.indicatrix` file: every tier in full plus
/// the custom-material snapshot, history trail, `[meta]` table and attachments
/// `extras` carries, and the printed proportions of the catalogue row the design came
/// from. `draft` records that the design did not solve when it was saved.
///
/// # Errors
///
/// A ready-to-toast message: a `[meta]` value or an attachment the file format refuses
/// (a bad or repeated name, a total over the size limit). Without metadata and
/// attachments the serializer cannot fail for the fields written here.
pub(super) fn design_file_text(
    design: &Design,
    printed_proportions: Option<&ExternalProportions>,
    extras: &DesignExtras<'_>,
    draft: bool,
) -> Result<String, String> {
    let file = design_to_file(design, printed_proportions, extras).with_draft(draft);
    indicatrix_formats::native::design::to_string(&file).map_err(|e| e.to_string())
}
