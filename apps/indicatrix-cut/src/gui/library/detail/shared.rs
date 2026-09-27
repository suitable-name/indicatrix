//! Helpers shared by the local and remote detail-load paths: angle-side
//! classification, the filtered-row-count callback, display formatting, the
//! material guess, and clearing the detail pane.

use crate::{
    AngleItem, DiagramDetailData, LibraryModel, MainWindow,
    gui::editor::material_lookup::material_for_refractive_index,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// The angle magnitude (degrees) at or above which a catalogue schedule row is
/// treated as describing the girdle itself rather than a crown or pavilion facet --
/// a fallback heuristic (see [`sides_from_angle_sequence`]'s own doc
/// comment for why a sign-based classifier cannot work on this data).
const GIRDLE_ANGLE_THRESHOLD_DEG: f64 = 89.5;

/// Parses a catalogue angle-settings row's `angle` text the same way
/// `indicatrix_vault::local::parse_angle_deg` does -- that function is private to the
/// vault crate, so this reimplements its two-line body: strip a trailing `\u{b0}`
/// degree sign, then a plain `f64` parse. `None` for text that still doesn't parse.
pub(super) fn parse_catalogue_angle_deg(angle: &str) -> Option<f64> {
    angle.trim().trim_end_matches('\u{b0}').trim().parse().ok()
}

/// Derives every row's [`AngleItem::side`] (`-1` pavilion, `1` crown, `0` neither) from
/// the ORDER a schedule lists `angles` in, not from the sign of the angle text.
///
/// Measured on the real catalogue: 50,809 of 50,817 stored `angle_settings.angle`
/// values end in `\u{b0}` (so a bare `str::parse` fails on virtually every row unless
/// that's stripped first, reporting `0`/neither side for everything), and every stored pavilion angle is
/// written UNSIGNED -- so even after stripping the degree sign, a sign check
/// (`deg < 0.0` / `deg > 0.0`) still reports every real row as `0`. The Pavilion/Crown
/// filter pills in `cutting_table.slint` were therefore dead for every stored design.
///
/// This crate's own schedule rows (`gui::editor::state::cutting_schedule_rows`) take
/// the side from the SOLVER's per-tier block classification instead, which needs a
/// solved [`indicatrix_cut_core::Design`] and is `pub(super)` to `gui::editor` -- not
/// reachable from this module (which deliberately never names that feature-gated
/// `Design` type in its own signatures, see `resolve_catalogue_planes_for_entry`'s own
/// doc comment) without pulling in the whole editor pipeline just to classify a
/// display-only column. This is the documented fallback in its place: a catalogue
/// schedule reliably lists crown facets first, then the girdle row(s) (~90 degrees),
/// then pavilion facets -- the same order `.asc`/`GemCad` schedules and this crate's own
/// tier table use. So the FIRST row whose angle is at least
/// [`GIRDLE_ANGLE_THRESHOLD_DEG`] marks the crown/pavilion boundary: every row at or
/// past that magnitude (there may be more than one girdle-adjacent facet) reports `0`
/// (the girdle itself, neither side); every row before the boundary is crown (`1`);
/// every row strictly after it is pavilion (`-1`). A row whose angle text doesn't
/// parse at all also reports `0`.
pub(super) fn sides_from_angle_sequence<'a>(angles: impl Iterator<Item = &'a str>) -> Vec<i32> {
    let degrees: Vec<Option<f64>> = angles.map(parse_catalogue_angle_deg).collect();
    let girdle_idx = degrees
        .iter()
        .position(|deg| deg.is_some_and(|d| d >= GIRDLE_ANGLE_THRESHOLD_DEG));
    degrees
        .iter()
        .enumerate()
        .map(|(i, deg)| match deg {
            None => 0,
            Some(d) if *d >= GIRDLE_ANGLE_THRESHOLD_DEG => 0,
            Some(_) => {
                if girdle_idx.is_some_and(|g| i > g) {
                    -1
                } else {
                    1
                }
            }
        })
        .collect()
}

/// Counts `angles` under `mode` (0 = All, 1 = Pavilion, 2 = Crown) by
/// [`AngleItem::side`] -- exactly `cutting_table.slint`'s own `row_shown` predicate
/// (side < 0 pavilion, side > 0 crown), pulled out so [`setup_filtered_row_count_callback`]
/// stays a thin Slint-callback wrapper.
#[must_use]
pub(super) fn count_angles_for_mode(angles: &ModelRc<AngleItem>, mode: i32) -> i32 {
    let count = angles
        .iter()
        .filter(|a| match mode {
            1 => a.side < 0,
            2 => a.side > 0,
            _ => true,
        })
        .count();
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// Registers `LibraryModel::filtered_row_count` -- the per-mode row count
/// `cutting_table.slint`'s "All Steps (N)"/Pavilion/Crown pills need, which cannot
/// be computed in Slint itself (see the comment above that file's row-count
/// pills for why: Slint has no general loop or array `.filter`/`.reduce`, and a
/// `pure function` there cannot even recurse to fake one). Reads
/// `LibraryModel.current_angles` fresh on every call rather than
/// taking it as a second argument -- see [`count_angles_for_mode`]'s doc comment,
/// and `library.slint`'s own doc comment on `filtered_row_count` for why this
/// still reacts correctly to a `current_angles` change.
pub fn setup_filtered_row_count_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_filtered_row_count(move |mode: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return 0;
            };
            let angles = ui.global::<LibraryModel>().get_current_angles();
            count_angles_for_mode(&angles, mode)
        });
}

/// Clears the detail pane's display fields and drops the current row selection.
///
/// Shared by [`crate::gui::library::local::organize`]'s delete handler (the original
/// "the selected design is gone" case) and
/// [`crate::gui::library::search::apply_diagram_list_to_ui`] (a search/
/// filter refresh that drops the previously selected row off the new result list must
/// not leave `LibraryModel.selected_entry_id` pointing at a design no longer even
/// visible in the list it came from). Deliberately leaves `RenderContext`/the 3D
/// viewport untouched: unlike an actual delete, the design itself hasn't changed here,
/// only whether the LIST currently shows it, so there is nothing wrong with the
/// viewport going on tracing it.
pub fn clear_current_detail_display(ui: &MainWindow) {
    ui.global::<LibraryModel>()
        .set_current_detail(DiagramDetailData {
            id: -1,
            title: SharedString::default(),
            url: SharedString::default(),
            designer: SharedString::default(),
            shape: SharedString::default(),
            gear: SharedString::default(),
            facets: SharedString::default(),
            lw_ratio: SharedString::default(),
            ri: SharedString::default(),
            material_guess: SharedString::default(),
            volume: SharedString::default(),
            competition: SharedString::default(),
            image_name: SharedString::default(),
            has_image: false,
            is_local: false,
            hw_ratio: SharedString::default(),
            cw_ratio: SharedString::default(),
            pw_ratio: SharedString::default(),
            symmetry_order: SharedString::default(),
            mirror_symmetry: false,
        });
    ui.global::<LibraryModel>().set_selected_entry_id(-1);
    ui.global::<LibraryModel>()
        .set_current_angles(ModelRc::new(VecModel::from(Vec::new())));
    ui.global::<LibraryModel>()
        .set_current_files(ModelRc::new(VecModel::from(Vec::new())));
}

/// Formats a stored proportion string to 3 decimal places for the detail header's
/// metric chips -- DISPLAY ONLY. What actually lives in the database is never touched
/// by this (see `indicatrix_vault::db::sqlite::Database::update_diagram_metadata`,
/// which always round-trips a proportion's full, unrounded text -- the user's own
/// instruction is "store everything, discard nothing, don't recalculate").
///
/// Parses first: only a value that parses cleanly as a finite `f64` gets reformatted.
/// Anything that doesn't parse -- a scraped legacy string in some other format, or a
/// future value this app didn't write -- passes through completely untouched, so it
/// can never be coerced into a misleading "0.000". Checked against the real
/// ~3,187-design catalogue (`facet_diagrams.sqlite`): every non-null `lw_ratio`/
/// `hw_ratio`/`cw_ratio`/`pw_ratio`/`volume` value there is already a plain
/// REAL-affinity number with no exceptions, but the model type is `Option<String>` and
/// nothing guarantees that stays true forever, so this stays defensive rather than
/// assuming it.
fn format_proportion(raw: &str) -> String {
    match raw.trim().parse::<f64>() {
        Ok(n) if n.is_finite() => format!("{n:.3}"),
        _ => raw.to_string(),
    }
}

/// [`format_proportion`] over an `Option<&str>`, collapsing `None` to `""` -- the same
/// "empty string hides the chip" convention every other optional field on
/// `DiagramDetailData` already uses (see `detail_header.slint`'s `if root.detail.xxx
/// != "":` chips).
pub(super) fn format_optional_proportion(raw: Option<&str>) -> String {
    raw.map(format_proportion).unwrap_or_default()
}

/// Trims `text` and turns a blank result into `None` -- the metadata editor's
/// convention for "the user cleared this field", matching
/// `Database::rename_diagram_entry`'s own trim. `None` is what
/// [`indicatrix_vault::model::metadata_update::MetadataUpdate`] stores as a field's new
/// value in that case, not an empty-string sentinel -- consistent with every other
/// optional column this app writes.
pub(super) fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// [`DiagramDetailData::material_guess`]'s own computation -- a catalogue row
/// records only a refractive index string (`ri_text`, `None`/unparseable
/// reads as "nothing to guess"), so this is the only honest way to name the
/// stone for the card ("Inferred material shown as a guess, never as a fact").
/// Formatted
/// identically to the editor's own material-guess badge ("Sapphire? (from RI
/// 1.76)") so the two never disagree about wording; `""` when nothing built
/// in is close enough within [`MATERIAL_MATCH_TOLERANCE`] -- hides the chip
/// entirely rather than showing an empty/misleading one.
pub(super) fn catalogue_material_guess(ri_text: Option<&str>) -> String {
    let Some(n_d) = ri_text.and_then(|s| s.trim().parse::<f64>().ok()) else {
        return String::new();
    };
    material_for_refractive_index(n_d).map_or_else(String::new, |(name, _)| {
        format!("{name}? (from RI {n_d:.2})")
    })
}
