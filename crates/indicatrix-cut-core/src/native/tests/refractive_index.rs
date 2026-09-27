//! `.asc`'s single, lossy RI slot versus the sidecar's own authored/override
//! refractive-index fields.

use super::fixtures::{fully_anchored_design, simple_design};
use crate::{
    design::Design,
    native::{load_paired, save_paired},
};

/// `.asc` has exactly one RI slot and export writes the EFFECTIVE value there
/// (see `Design::effective_refractive_index`'s own doc comment). Re-importing
/// that `.asc` alone would silently overwrite the AUTHORED (`meta.refractive_index`)
/// figure. The native sidecar's own `authored_refractive_index` must survive a
/// real save/load round trip and restore the true authored value instead.
#[test]
fn authored_refractive_index_round_trips_through_save_and_load_even_when_the_asc_i_line_differs() {
    let mut design = simple_design();
    // `simple_design` pins an override equal to the fixture's own authored RI
    // (see that function's own doc comment) specifically so an UNRELATED test
    // doesn't have to think about this divergence -- removed here so this test
    // can exercise it on purpose.
    design.material.refractive_index_override = None;
    let authored = design.meta.refractive_index;
    assert_ne!(
        design.effective_refractive_index(),
        authored,
        "fixture must actually exercise the authored/effective divergence"
    );

    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");

    // Importing the paired `.asc` ALONE (no native sidecar at all) derives the
    // EFFECTIVE RI back as `meta.refractive_index` -- proving this fixture
    // really does exercise the lossy round trip `authored_refractive_index`
    // exists to fix, not one that happens to already agree.
    let schedule = indicatrix_formats::asc::parse_asc(&saved.asc_text).expect("must parse");
    let asc_only = Design::from_asc_schedule(design.preform, &schedule);
    assert_ne!(asc_only.meta.refractive_index, authored);

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(
        loaded.design.meta.refractive_index, authored,
        "the AUTHORED RI must survive, not the effective one a plain .asc \
         re-import would derive"
    );
}

/// `MaterialTable::refractive_index_override` must round-trip through a real
/// save/load pair exactly like every other `material` field already does.
#[test]
fn refractive_index_override_round_trips_through_save_and_load() {
    // `fully_anchored_design`, not `simple_design`: this test needs the design to
    // actually SOLVE (`save_paired` calls `to_asc_schedule`), and
    // `simple_design`'s own tier-1 `MeetExisting` override removes the Crown
    // block's only anchor.
    let mut design = fully_anchored_design();
    design.material.refractive_index_override = Some(1.90);
    let saved = save_paired(
        &design,
        "design.asc",
        Some(super::fixtures::FULLY_CANONICAL_ASC),
        None,
        None,
    )
    .expect("must save");

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load");
    assert_eq!(loaded.design.material.refractive_index_override, Some(1.90));
}
