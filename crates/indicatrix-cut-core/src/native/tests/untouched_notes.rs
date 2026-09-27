//! An untouched import's own `G`-field notes survive a solve/export round
//! trip verbatim.

use crate::{design::Design, preform::PreformSpec};

/// `attached_files` id 4210 ("pc46019.asc") -- "PC 46.019 For Fun" by Michiko
/// Huyhn. Its real notes (`"Cut to mast depth X."`, `"Set girdle width."`) survive
/// a round trip through the native file format. Copied here from
/// `indicatrix_formats::asc`'s own (private, `#[cfg(test)]`-only) fixture of the
/// same name -- that fixture cannot be reused across the crate boundary.
const ASC_FOR_FUN: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 1 y\n\
I 1.54\n\
H PC 46.019  For Fun\n\
H by Michiko Huyhn\n\
a -44.864054 0.53791082 84 n P1 12 G Cut to mast depth X.\n\
a -90.000000 0.78956831 36 12 84 n G1 60 n G1 G Set stone size.\n\
a 54.575729 0.70935195 12 n C1 84 G Set girdle width.\n\
a 0.000000 0.44755829 96 n U\n\
F Also USFG Newsletter Sep 2013, Facets Jan 2014\n";

/// `attached_files` id 4430 ("pc42060.asc") -- "PC 42.060 Large Texas Star" by
/// Charles `McCoy`. Same reuse rationale as [`ASC_FOR_FUN`]; exercises the bare
/// `"TCP"` instruction and a note this crate doesn't classify at all (`"Make table
/// large enough to show all of the star"`).
const ASC_LARGE_TEXAS_STAR: &str = "GemCad 5.0\n\
g 80 0.0\n\
y 5 y\n\
I 1.61\n\
H PC 42.060  Large Texas Star\n\
H by Charles McCoy\n\
a -40.000000 0.54589773 76 n 1 68 60 52 44 36 28 20 12 4 G TCP\n\
a 40.000000 1.11585176 4 n A 12 20 28 36 44 52 60 68 76 G Establish girdle thickness\n\
a 0.000000 0.72641642 80 n T G Make table large enough to show all of the star\n\
F Leave #4 frosted\n";

/// Imports `asc_text` untouched, solves and re-exports it, and asserts every tier's
/// `G`-field text came back exactly as the file stated it, verbatim across the
/// round trip.
fn assert_untouched_import_preserves_notes_on_export(asc_text: &str) {
    let schedule = indicatrix_formats::asc::parse_asc(asc_text).expect("fixture must parse");
    let design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    let solved = design
        .solve()
        .expect("every tier is import-pinned to a ScaleReference, so this always solves");
    let exported = design.to_asc_schedule_from_solved(&solved);

    assert_eq!(exported.tiers.len(), schedule.tiers.len());
    for (original, exported_tier) in schedule.tiers.iter().zip(&exported.tiers) {
        assert_eq!(
            exported_tier.notes, original.notes,
            "tier {:?}: an untouched import's G-field text must survive export verbatim",
            original.name
        );
    }
}

#[test]
fn untouched_import_preserves_g_field_notes_on_export_for_fun() {
    assert_untouched_import_preserves_notes_on_export(ASC_FOR_FUN);
}

#[test]
fn untouched_import_preserves_g_field_notes_on_export_large_texas_star() {
    assert_untouched_import_preserves_notes_on_export(ASC_LARGE_TEXAS_STAR);
}
