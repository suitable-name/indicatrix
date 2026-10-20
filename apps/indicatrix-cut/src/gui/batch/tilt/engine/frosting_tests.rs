//! A library design with a frosted tier is swept frosted: the batch resolves its frosted planes
//! (as the preview batch does), the local sweep sees the finishes they make, and the curves it
//! stores differ from the same stone swept polished.

use super::{BatchContext, ResolvedDesign, resolve_design, sweep::compute_tilt_curves_locally};
use crate::gui::{
    batch::test_records::store_design_file,
    library::local::test_gem::{SYNTHETIC_GEM_TIERS, encode_gem},
};
use indicatrix::{color::metrics::FinishedStone, optics::raytracer::FacetFinish};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{frosting::StoredTierFrosting, material_match::RiPresetCandidate},
};
use std::{
    collections::BTreeSet,
    sync::{Mutex, atomic::AtomicBool},
};

/// A database with the synthetic `.gem` design, its crown (tier 2, eight facets) frosted when
/// `frosted`, and the entry id.
fn library_with_synthetic_gem(frosted: bool) -> (Mutex<Database>, i64) {
    let db = Database::new(Some(":memory:")).expect("in-memory database opens");
    let entry_id = store_design_file(
        &db,
        "Synthetic.GEM",
        encode_gem(SYNTHETIC_GEM_TIERS, 96, "Synthetic"),
    );
    if frosted {
        db.set_diagram_frosting(entry_id, &[StoredTierFrosting::whole(2)])
            .expect("the frosting stores");
    }
    (Mutex::new(db), entry_id)
}

/// Resolves `entry_id` the way the tilt batch does.
fn resolve(db: &Mutex<Database>, entry_id: i64) -> ResolvedDesign {
    let candidates = [RiPresetCandidate {
        name: "Quartz".to_string(),
        refractive_index: 1.545,
    }];
    let angle_table_entries = Mutex::new(BTreeSet::new());
    let ctx = BatchContext {
        db,
        material_candidates: &candidates,
        custom_materials: &[],
        angle_table_entries: &angle_table_entries,
    };
    resolve_design(&ctx, entry_id).expect("the design resolves")
}

/// The stored frosting of a library design becomes the frosted planes of its resolved stone,
/// and those planes become frosted facet finishes; a polished design resolves none.
#[test]
fn a_frosted_library_design_resolves_frosted_finishes_and_a_polished_one_none() {
    let (db, entry_id) = library_with_synthetic_gem(true);
    let resolved = resolve(&db, entry_id);
    assert!(!resolved.frosted.is_empty(), "the crown is frosted");
    assert!(
        resolved.frosted.len() < resolved.planes.len(),
        "only the frosted tier's planes"
    );
    let finishes = resolved.finishes();
    assert_eq!(finishes.len(), resolved.planes.len());
    assert_eq!(
        finishes
            .iter()
            .filter(|finish| **finish == FacetFinish::Frosted)
            .count(),
        resolved.frosted.len()
    );

    let (db, polished_id) = library_with_synthetic_gem(false);
    let polished = resolve(&db, polished_id);
    assert_eq!(polished.frosted, [] as [u32; 0]);
    assert!(
        polished.finishes().is_empty(),
        "no frosting is the empty finish list of the polished sweep"
    );
}

/// The batch's local sweep of a frosted design differs from the polished sweep of the same
/// stone, in every axis; this is what makes the `frosting=` part of the stored fingerprint a
/// true dependency of the curves.
#[test]
fn the_local_sweep_of_a_frosted_design_differs_from_its_polished_sweep() {
    let (db, entry_id) = library_with_synthetic_gem(true);
    let resolved = resolve(&db, entry_id);
    let cancel = AtomicBool::new(false);
    let finishes = resolved.finishes();

    let polished = compute_tilt_curves_locally(
        FinishedStone::polished(resolved.stone()),
        &resolved.material,
        &cancel,
    )
    .expect("the polished sweep completes");
    let frosted = compute_tilt_curves_locally(
        FinishedStone::new(resolved.stone(), &finishes),
        &resolved.material,
        &cancel,
    )
    .expect("the frosted sweep completes");

    for (axis, (p, f)) in polished.axes.iter().zip(&frosted.axes).enumerate() {
        assert!(
            p.brilliance_pct != f.brilliance_pct
                || p.extinction_pct != f.extinction_pct
                || p.windowing_pct != f.windowing_pct,
            "axis {axis}: the frosted curves equal the polished ones"
        );
    }
}
