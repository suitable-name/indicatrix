//! A library design with a frosted tier previews with its frosted planes: the batch resolves
//! them, and the scene a traced preview renders (locally) and ships (to a remote worker)
//! carries them.

use super::engine::{BatchContext, resolve_design};
use crate::{
    bridge::preview_render::{PreviewJob, PreviewView, remote_batch_item},
    gui::{
        batch::test_records::store_design_file,
        library::local::test_gem::{SYNTHETIC_GEM_TIERS, encode_gem},
    },
};
use indicatrix::{geometry::facet_finishes_with_frosted, optics::raytracer::FacetFinish};
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

/// Resolves `entry_id` the way the batch does, traced (`solid` false) or solid.
fn resolve(db: &Mutex<Database>, entry_id: i64, solid: bool) -> super::engine::ResolvedDesign {
    let candidates = [RiPresetCandidate {
        name: "Quartz".to_string(),
        refractive_index: 1.545,
    }];
    let gpu_retired = AtomicBool::new(false);
    let angle_table_entries = Mutex::new(BTreeSet::new());
    let ctx = BatchContext {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        ray_draft: false,
        db,
        material_candidates: &candidates,
        custom_materials: &[],
        preview_size: 8,
        preview_spp: 1,
        solid,
        gpu_retired: &gpu_retired,
        angle_table_entries: &angle_table_entries,
    };
    resolve_design(&ctx, entry_id).expect("the design resolves")
}

#[test]
fn a_library_design_with_a_frosted_tier_resolves_a_non_empty_frosted_plane_set() {
    let (db, entry_id) = library_with_synthetic_gem(true);
    let resolved = resolve(&db, entry_id, false);

    assert!(!resolved.frosted.is_empty(), "the crown is frosted");
    assert!(
        resolved.frosted.len() < resolved.planes.len(),
        "only the frosted tier's planes"
    );
    assert!(
        resolved.frosted.windows(2).all(|pair| pair[0] < pair[1]),
        "ascending indices into the planes"
    );
    assert!(
        resolved
            .frosted
            .iter()
            .all(|&index| (index as usize) < resolved.planes.len())
    );

    // The scene a traced preview renders: the frosted planes are traced Frosted.
    let finishes = facet_finishes_with_frosted(&resolved.planes, false, &resolved.frosted);
    assert_eq!(finishes.len(), resolved.planes.len());
    assert_eq!(
        finishes
            .iter()
            .filter(|finish| **finish == FacetFinish::Frosted)
            .count(),
        resolved.frosted.len()
    );

    // The scene a remote worker traces carries the same set, for both views.
    let job = PreviewJob {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        planes: &resolved.planes,
        tools: &resolved.tools,
        frosted: &resolved.frosted,
        material: &resolved.material,
        size: 8,
        spp: 1,
        max_bounces: 4,
    };
    for view in [PreviewView::Front, PreviewView::Top] {
        let item = remote_batch_item(0, &job, view).expect("the studio scene is sendable");
        assert_eq!(item.scene.frosted_planes, resolved.frosted);
    }
}

#[test]
fn a_polished_library_design_and_the_solid_stand_in_have_no_frosted_planes() {
    let (db, polished_id) = library_with_synthetic_gem(false);
    let polished = resolve(&db, polished_id, false);
    assert_eq!(polished.frosted, [] as [u32; 0]);
    assert_eq!(
        facet_finishes_with_frosted(&polished.planes, false, &polished.frosted),
        [] as [indicatrix::optics::raytracer::FacetFinish; 0]
    );

    // The solid picture is flat shaded: a frosted design resolves none for it either.
    let (db, frosted_id) = library_with_synthetic_gem(true);
    assert_eq!(resolve(&db, frosted_id, true).frosted, [] as [u32; 0]);
}
