//! The frosting of a library design on the phone: a `.gcs` import stores what the file states
//! beside the `.asc` it converts to, opening and exporting the entry carry it, and a polished
//! file is untouched.

use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta, TierFrosting};
use indicatrix_formats::gcs::{GcsTierFrosting, to_gcs_string, to_gcs_string_with_frosting};

use super::tests::{Fixture, picked};

/// The standard round brilliant, the same design every test here writes as `.gcs`.
fn round_brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// The round brilliant as `.gcs` text with tier 1 frosted as a whole and tier 3 at its facet
/// slots 1 and 3.
fn frosted_gcs() -> Vec<u8> {
    let schedule = round_brilliant()
        .to_asc_schedule()
        .expect("the design exports");
    let frosting = [
        (1, GcsTierFrosting::Whole),
        (3, GcsTierFrosting::Facets(vec![1, 3])),
    ];
    to_gcs_string_with_frosting(&schedule, &frosting)
        .expect("the schedule writes as .gcs")
        .into_bytes()
}

/// Whether `name` has the file extension `extension` (given without the dot), whatever its case.
fn has_extension(name: &str, extension: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

fn design_of_file(bytes: &[u8]) -> Design {
    indicatrix_cut_core::native::design_from_str(std::str::from_utf8(bytes).expect("text"))
        .expect("the design file reads")
        .design
}

/// A frosted `.gcs` keeps its frosting through the library: the import stores it beside the
/// `.asc` it converts to, `load` hands the editor a native design file that states it, and
/// the export carries it too.
#[test]
fn an_imported_gcs_keeps_its_frosting_through_load_and_export() {
    let fx = Fixture::new();
    assert_eq!(
        fx.h().import(vec![picked("rb.gcs", &frosted_gcs())]).added,
        1
    );
    let id = fx.all()[0].id;
    assert_eq!(
        fx.h().lock().diagram_frosting(id).expect("reads").len(),
        2,
        "the library stored the two frosted tiers"
    );

    let loaded = fx.h().load(id).expect("loads");
    assert!(
        has_extension(&loaded.file_name, "isdd"),
        "a frosted entry opens as a native design file: {}",
        loaded.file_name
    );
    let design = design_of_file(&loaded.bytes);
    assert_eq!(design.tier_frosting(1), Some(&TierFrosting::Tier));
    assert!(matches!(
        design.tier_frosting(3),
        Some(TierFrosting::Facets(facets)) if facets.len() == 2
    ));
    assert_eq!(design.tier_frosting.len(), 2);

    let shared = fx.h().export_bytes(id).expect("exports");
    assert_eq!(
        design_of_file(&shared.bytes).tier_frosting.len(),
        2,
        "the export keeps it"
    );
}

/// A polished `.gcs` stores no frosting and loads as the `.asc` it converted to.
#[test]
fn an_imported_gcs_without_frosting_loads_as_its_converted_asc() {
    let fx = Fixture::new();
    let text = to_gcs_string(&round_brilliant().to_asc_schedule().expect("exports"))
        .expect("the schedule writes as .gcs");
    assert_eq!(
        fx.h()
            .import(vec![picked("plain.gcs", text.as_bytes())])
            .added,
        1
    );
    let id = fx.all()[0].id;
    assert_eq!(
        fx.h().lock().diagram_frosting(id).expect("reads"),
        [] as [indicatrix_vault::model::frosting::StoredTierFrosting; 0]
    );
    let stored = fx.h().load(id).expect("loads");
    assert!(
        has_extension(&stored.file_name, "asc"),
        "{}",
        stored.file_name
    );
}
