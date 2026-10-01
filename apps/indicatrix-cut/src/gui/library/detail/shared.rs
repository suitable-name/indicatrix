//! Helpers shared by the local and remote detail-load paths: angle-side
//! classification, the filtered-row-count callback, display formatting, the
//! material guess, and clearing the detail pane.

use crate::{
    AngleItem, DiagramDetailData, LibraryModel, MainWindow,
    gui::editor::material_lookup::material_for_refractive_index,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// The angle (degrees) a catalogue schedule row must equal, within
/// [`GIRDLE_ANGLE_TOLERANCE_DEG`], to be read as the girdle itself rather than a
/// steep crown or pavilion facet. Deliberately an equality test, not a "steep
/// enough" threshold: measured on the real catalogue, angles cluster tightly at
/// exactly `90.00\u{b0}` (7,972 rows) with every other near-90 facet angle below
/// `88.73\u{b0}` and nothing in between -- so an 89.6\u{b0} row is a real (if steep)
/// facet, not a mislabelled girdle, and a "`>= 89.5`"-style threshold would wrongly
/// have swallowed it.
const GIRDLE_ANGLE_DEG: f64 = 90.0;

/// See [`GIRDLE_ANGLE_DEG`]'s own doc comment for why this is a tight equality
/// tolerance rather than a broad "steep angle" threshold.
const GIRDLE_ANGLE_TOLERANCE_DEG: f64 = 0.01;

/// Parses a catalogue angle-settings row's `angle` text the same way
/// `indicatrix_vault::local::parse_angle_deg` does -- that function is private to the
/// vault crate, so this reimplements its body: strip a trailing degree sign, then a
/// plain `f64` parse. Strips both the normal `\u{b0}` sign and `\u{fffd}` (the Unicode
/// replacement character) -- measured on the real catalogue, 6 of the 50,817 stored
/// `angle_settings.angle` values store a mangled degree sign as `\u{fffd}` instead of
/// `\u{b0}` (detail 3282's `P`/`G`/`C` rows). `None` for text that still doesn't parse.
pub(super) fn parse_catalogue_angle_deg(angle: &str) -> Option<f64> {
    angle
        .trim()
        .trim_end_matches(['\u{b0}', '\u{fffd}'])
        .trim()
        .parse()
        .ok()
}

/// Case-insensitive, trim-insensitive equality against a lowercase `target`.
fn eq_ignore_case_trim(text: &str, target: &str) -> bool {
    text.trim().eq_ignore_ascii_case(target)
}

/// Matches a table facet label: `T`, `t`, `Table`, `table`, any of those with a
/// trailing `.`, case-insensitively (also catches the all-caps `TABLE.` seen in the
/// real catalogue, which a case-sensitive `^[Tt](able)?\.?$` would miss).
fn is_table_facet(facet: &str) -> bool {
    let f = facet.trim();
    let core = f.strip_suffix('.').unwrap_or(f);
    core.eq_ignore_ascii_case("t") || core.eq_ignore_ascii_case("table")
}

/// A row is the table row when its index column says so, or its facet label does --
/// the two conventions seen in the real catalogue (e.g. detail 16's `T(0,'Table')` row
/// sets both).
fn is_table_row(facet: &str, index_val: &str) -> bool {
    eq_ignore_case_trim(index_val, "table") || is_table_facet(facet)
}

/// Matches a pavilion-prefixed facet label: `P`/`p`, optionally `F`/`f`, then a digit
/// (`P1`, `PF1`, `pf3`, `P2(G)` -- only the prefix has to match).
fn is_pavilion_prefixed(facet: &str) -> bool {
    let mut chars = facet.trim().chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.eq_ignore_ascii_case(&'p') {
        return false;
    }
    let mut next = chars.next();
    if next.is_some_and(|c| c.eq_ignore_ascii_case(&'f')) {
        next = chars.next();
    }
    next.is_some_and(|c| c.is_ascii_digit())
}

/// Matches a crown-prefixed facet label: `C`/`c` then a digit (`C1`, `C1A`).
fn is_crown_prefixed(facet: &str) -> bool {
    let mut chars = facet.trim().chars();
    chars.next().is_some_and(|c| c.eq_ignore_ascii_case(&'c'))
        && chars.next().is_some_and(|c| c.is_ascii_digit())
}

/// Matches `1G`, `2G`, ... -- digits with a single trailing `G`/`g`, the catalogue's
/// own way of tagging a facet as girdle-adjacent without its angle equalling
/// [`GIRDLE_ANGLE_DEG`]. Checked before [`is_digits_with_optional_trailing_letter`]
/// so `1G` reports girdle rather than being swallowed by that more general rule.
fn is_digit_with_trailing_girdle_letter(facet: &str) -> bool {
    let f = facet.trim();
    match f.chars().next_back() {
        Some(last) if last.eq_ignore_ascii_case(&'g') => {
            let digits = &f[..f.len() - last.len_utf8()];
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// Matches a facet label starting with `G`/`g` (`G`, `G1`, `G2`, `GIRDLE`).
fn starts_with_girdle_letter(facet: &str) -> bool {
    facet
        .trim()
        .chars()
        .next()
        .is_some_and(|c| c.eq_ignore_ascii_case(&'g'))
}

/// Matches pure digits with an optional single trailing letter (`1`, `21`, `1A`,
/// `6B`) -- the catalogue's numbered-pavilion-facet convention.
fn is_digits_with_optional_trailing_letter(facet: &str) -> bool {
    let f = facet.trim();
    let digits = match f.chars().next_back() {
        Some(c) if c.is_ascii_alphabetic() => &f[..f.len() - c.len_utf8()],
        _ => f,
    };
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Matches a single lettered facet label other than `G`/`T` (`A`, `b`, `F`) -- the
/// catalogue's lettered-crown-facet convention.
fn is_single_crown_letter(facet: &str) -> bool {
    let f = facet.trim();
    let mut chars = f.chars();
    let Some(c) = chars.next() else {
        return false;
    };
    chars.next().is_none()
        && c.is_ascii_alphabetic()
        && !c.eq_ignore_ascii_case(&'g')
        && !c.eq_ignore_ascii_case(&'t')
}

/// Classifies one row purely from its own label (`facet`/parsed `angle`/`index_val`),
/// with no knowledge of any other row in the schedule. `None` means the label alone
/// isn't enough -- [`sides_from_rows`] falls back to [`positional_fallback`] for those.
///
/// Order matters -- checked in this sequence: (a) girdle by angle or index text; (b)
/// table by index text or facet; (c) pavilion-prefixed, then crown-prefixed, then
/// digit+trailing-`G` (must come before the more general digit-plus-letter rule so
/// `1G` reports girdle, not pavilion), then girdle-prefixed, then plain
/// digit(+letter) as pavilion, then a lone crown letter.
fn label_side(facet: &str, angle_deg: Option<f64>, index_val: &str) -> Option<i32> {
    if angle_deg.is_some_and(|d| (d - GIRDLE_ANGLE_DEG).abs() < GIRDLE_ANGLE_TOLERANCE_DEG)
        || eq_ignore_case_trim(index_val, "girdle")
    {
        return Some(0);
    }
    if is_table_row(facet, index_val) {
        return Some(1);
    }
    if is_pavilion_prefixed(facet) {
        return Some(-1);
    }
    if is_crown_prefixed(facet) {
        return Some(1);
    }
    if is_digit_with_trailing_girdle_letter(facet) {
        return Some(0);
    }
    if starts_with_girdle_letter(facet) {
        return Some(0);
    }
    if is_digits_with_optional_trailing_letter(facet) {
        return Some(-1);
    }
    if is_single_crown_letter(facet) {
        return Some(1);
    }
    None
}

/// The positional fallback for a row [`label_side`] couldn't classify (an
/// empty/unrecognised label): anchors on the schedule's own girdle row(s) --
/// `girdle_positions`, every index [`label_side`] reported `0` for -- and, when
/// present, `table_position` -- the FIRST row [`is_table_row`] matched.
///
/// A row strictly inside the girdle block (`girdle_positions.first()..=last()`)
/// reports `0` -- it sits among the girdle rows themselves. Outside that block, the
/// table row settles which side is crown: whichever side of the girdle block the
/// table sits on (before or after) is crown (`1`), the other is pavilion (`-1`) --
/// this is what fixes designs where the girdle is listed FIRST (1,403 of 3,021 real
/// designs) and everything else, including real crown facets, would otherwise be
/// misread as pavilion by a naive before/after-girdle rule.
///
/// With no table row to anchor on, this falls back to the old before/after-girdle
/// rule (before = crown, after = pavilion) but ONLY when the girdle sits in the
/// MIDDLE of the row list (not the first or last row overall) -- a girdle that's
/// itself first or last gives no reliable "before" or "after" side to trust, so that
/// case (and a schedule with no girdle row at all) reports `0` rather than guessing.
const fn positional_fallback(
    index: usize,
    girdle_positions: &[usize],
    table_position: Option<usize>,
    row_count: usize,
) -> i32 {
    let (Some(&first), Some(&last)) = (girdle_positions.first(), girdle_positions.last()) else {
        return 0;
    };
    if index >= first && index <= last {
        return 0;
    }
    let row_after_girdle = index > last;
    match table_position {
        Some(table_idx) if table_idx < first || table_idx > last => {
            let table_after_girdle = table_idx > last;
            if row_after_girdle == table_after_girdle {
                1
            } else {
                -1
            }
        }
        Some(_) => 0,
        None => {
            let girdle_is_first_or_last = first == 0 || last == row_count - 1;
            if girdle_is_first_or_last {
                0
            } else if row_after_girdle {
                -1
            } else {
                1
            }
        }
    }
}

/// Derives every row's [`AngleItem::side`] (`-1` pavilion, `1` crown, `0` girdle/
/// unclassified) from each row's own `facet`/`angle`/`index_val` label text, falling
/// back to schedule POSITION only when the label itself doesn't say (see
/// [`label_side`] and [`positional_fallback`]).
///
/// A purely positional rule (crown before the girdle row, pavilion after) does not
/// work on this data: measured on the real catalogue's 3,021 designs with angle rows,
/// the girdle row sits FIRST in 1,403 of them and in the MIDDLE in 1,401 -- so
/// "before the girdle" is crown for barely more than half of all designs. This reads
/// each row's own label first (facet conventions measured across all 50,817 rows:
/// pure digits with an optional trailing letter, or a `P`/`PF` prefix, are pavilion;
/// a single letter, a `C` prefix, or the table row are crown; `G`-prefixed or
/// an angle equal to [`GIRDLE_ANGLE_DEG`] is the girdle itself) and only falls back
/// to row position for the minority of rows an empty or unrecognised label leaves
/// undecided -- see [`positional_fallback`] for that fallback's own rule. An
/// unclassified row (no label match, no usable positional anchor) reports `0`, the
/// same as the girdle: shown under "All", excluded from both the Pavilion and Crown
/// filters -- never mis-shown as the wrong side.
pub(super) fn sides_from_rows<'a>(
    rows: impl Iterator<Item = (&'a str, &'a str, &'a str)>,
) -> Vec<i32> {
    let rows: Vec<(&str, &str, &str)> = rows.collect();
    let provisional: Vec<Option<i32>> = rows
        .iter()
        .map(|&(facet, angle, index_val)| {
            label_side(facet, parse_catalogue_angle_deg(angle), index_val)
        })
        .collect();

    let girdle_positions: Vec<usize> = provisional
        .iter()
        .enumerate()
        .filter(|&(_, side)| *side == Some(0))
        .map(|(i, _)| i)
        .collect();
    let table_position =
        rows.iter()
            .zip(provisional.iter())
            .position(|(&(facet, _, index_val), side)| {
                *side == Some(1) && is_table_row(facet, index_val)
            });

    let row_count = rows.len();
    provisional
        .iter()
        .enumerate()
        .map(|(i, side)| {
            side.unwrap_or_else(|| {
                positional_fallback(i, &girdle_positions, table_position, row_count)
            })
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
