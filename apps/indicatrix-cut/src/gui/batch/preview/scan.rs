//! The once-per-session "designs with no previews yet" library scan -- see this
//! group's own `mod.rs` doc comment.

use crate::MainWindow;
use indicatrix_vault::db::sqlite::Database;
use slint::Weak;
use std::{
    sync::{Mutex, PoisonError},
    thread,
};

/// Every entry id whose cached preview is missing, has either view (front or top)
/// missing, or was rendered with other parameters than the current ones, per `Database::entry_ids_missing_previews`'s
/// fingerprint comparison (`preview_size`/`preview_spp` feed the current
/// fingerprint) -- one blob-free `LEFT JOIN` query rather than a page walk plus a
/// per-row point lookup. The walk this replaced (`search_diagrams_page` plus a `get_preview_images`
/// call per row) loaded both cached 256x256 preview PNGs for every entry in the
/// catalogue just to read `generated_at.is_none()` off each one -- measured at 4.4s and
/// 447MB for one pass over the real ~3,299-entry catalogue. Intended to run off
/// the UI thread (see [`spawn_missing_preview_scan`]); this function does not itself
/// spawn a thread.
///
/// The fingerprint a row is compared with is built from the row's persisted material
/// state (name, stone size and source: `FingerprintMaterial::from_state`), never from a
/// design file -- the scan opens none. A design whose file names a material or a size is
/// therefore first recognised once a batch has resolved it and persisted that state
/// (`gui::batch::catalogue_material`); the vault migration clears the fingerprints of the
/// previews rendered before, so such a design is listed as missing until then.
#[cfg(test)]
fn scan_missing_preview_ids(db: &Mutex<Database>, preview_size: u32, preview_spp: u32) -> Vec<i64> {
    scan_missing_preview_ids_for_engine(
        db,
        preview_size,
        preview_spp,
        indicatrix::renderer::engine::EngineSpec::default(),
    )
}

fn scan_missing_preview_ids_for_engine(
    db: &Mutex<Database>,
    preview_size: u32,
    preview_spp: u32,
    engine: indicatrix::renderer::engine::EngineSpec,
) -> Vec<i64> {
    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    // A frosted design's picture is stored under a fingerprint naming its frosting, which the
    // vault's scan cannot see per row: `frosting_scan` judges each design by its own.
    let outdated = crate::gui::batch::frosting_scan::outdated_ids(
        &guard,
        |state, frosting| {
            crate::bridge::preview_render::cache_fingerprint(
                if engine.is_ray() {
                    crate::bridge::preview_render::CacheKind::RayPreview {
                        size: preview_size,
                        taps: engine.ray.sanitized().aa_taps,
                        fire: engine.ray.fire,
                    }
                } else {
                    crate::bridge::preview_render::CacheKind::Preview {
                        size: preview_size,
                        spp: preview_spp,
                        max_bounces: super::engine::PREVIEW_MAX_BOUNCES,
                    }
                },
                crate::bridge::preview_render::FingerprintMaterial::from_state(state)
                    .with_frosting(frosting),
            )
        },
        |db, fingerprint| db.entry_ids_missing_previews(fingerprint),
    );
    drop(guard);
    outdated
}

/// Runs [`scan_missing_preview_ids`] on its own worker thread and reports the result
/// back to `on_result` on the UI event loop: `gui::mod` calls this once per session,
/// shortly after the initial diagram list loads, and shows a confirmation prompt only
/// if the returned list is non-empty.
pub(super) fn spawn_missing_preview_scan(
    ui_weak: Weak<MainWindow>,
    db: std::sync::Arc<Mutex<Database>>,
    preview_size: u32,
    preview_spp: u32,
    engine: indicatrix::renderer::engine::EngineSpec,
    on_result: impl FnOnce(&MainWindow, Vec<i64>) + Send + 'static,
) {
    #[cfg(feature = "ui-harness")]
    let pending = crate::harness_support::begin_startup_preview_scan();
    thread::spawn(move || {
        let ids = scan_missing_preview_ids_for_engine(&db, preview_size, preview_spp, engine);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            on_result(&ui, ids);
            #[cfg(feature = "ui-harness")]
            pending.store(false, std::sync::atomic::Ordering::Release);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bridge::preview_render::{CacheKind, FingerprintMaterial, cache_fingerprint},
        gui::batch::{
            preview::engine::{
                BatchContext, PREVIEW_MAX_BOUNCES, preview_fingerprint, resolve_design,
            },
            test_records::{design_file_text, store_design_file},
        },
    };
    use indicatrix::{optics::materials::GemMaterial, render_setup};
    use indicatrix_cut_core::material::MaterialSelection;
    use indicatrix_vault::model::preview::{PreviewMaterialSource, PreviewMaterialState};
    use std::{collections::BTreeSet, sync::atomic::AtomicBool};

    const SIZE: u32 = 160;
    const SPP: u32 = 4;

    /// A design whose file names Sapphire at a 9 mm girdle diameter.
    fn sapphire_design(db: &Mutex<Database>) -> i64 {
        let selection = MaterialSelection {
            name: Some("Sapphire".to_string()),
            ..MaterialSelection::none()
        };
        let text = design_file_text(selection, Some(9.0), None);
        let guard = db.lock().expect("the database lock is not poisoned");
        store_design_file(&guard, "sapphire.isdd", text.into_bytes())
    }

    /// A design whose file names a material and a girdle diameter is resolved in them,
    /// its previews are stored under a fingerprint that names them, and the scan then finds
    /// it current. The fingerprint such a design had before the size and the source were
    /// tracked reads as stale, once.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one sequential fixture-and-assert scenario; splitting would scatter the shared setup"
    )]
    fn a_design_files_material_and_size_are_part_of_the_stored_fingerprint() {
        let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
        let entry_id = sapphire_design(&db);
        let gpu_retired = AtomicBool::new(false);
        let angle_table_entries = Mutex::new(BTreeSet::new());
        let ctx = BatchContext {
            engine: indicatrix::renderer::engine::EngineSpec::default(),
            ray_draft: false,
            db: &db,
            material_candidates: &[],
            custom_materials: &[],
            preview_size: SIZE,
            preview_spp: SPP,
            solid: false,
            gpu_retired: &gpu_retired,
            angle_table_entries: &angle_table_entries,
        };

        let resolved = resolve_design(&ctx, entry_id).expect("the design resolves");
        assert_eq!(
            resolved.material,
            render_setup::material_for_stone(
                GemMaterial::by_name("Sapphire").expect("a built-in"),
                9.0,
                &resolved.planes
            ),
            "drawn in the file's own material at the file's own size"
        );
        let state = db
            .lock()
            .expect("the database lock is not poisoned")
            .get_preview_material_state(entry_id)
            .expect("the state reads");
        assert_eq!(
            state,
            PreviewMaterialState {
                name: Some("Sapphire".to_string()),
                girdle_mm: Some(9.0),
                source: PreviewMaterialSource::DesignFile,
            }
        );
        assert_eq!(scan_missing_preview_ids(&db, SIZE, SPP), vec![entry_id]);

        let updated_at = db
            .lock()
            .expect("the database lock is not poisoned")
            .entry_updated_at(entry_id)
            .expect("the stamp reads");
        let store = |fingerprint: &str| {
            db.lock()
                .expect("the database lock is not poisoned")
                .save_preview_images(
                    entry_id,
                    Some(b"front".as_slice()),
                    Some(b"top".as_slice()),
                    1,
                    fingerprint,
                    updated_at,
                )
                .expect("the images save")
        };

        // Stored the way a finished batch stores them: current.
        assert!(store(&preview_fingerprint(&ctx, &state, 0)));
        assert_eq!(scan_missing_preview_ids(&db, SIZE, SPP), Vec::<i64>::new());

        let ray = indicatrix::renderer::engine::EngineSpec {
            engine: indicatrix::renderer::engine::RenderEngine::RayTraced,
            ..Default::default()
        };
        assert_eq!(
            scan_missing_preview_ids_for_engine(&db, SIZE, SPP, ray),
            vec![entry_id]
        );
        let ray_fingerprint = cache_fingerprint(
            CacheKind::RayPreview {
                size: SIZE,
                taps: 8,
                fire: true,
            },
            FingerprintMaterial::from_state(&state),
        );
        assert!(store(&ray_fingerprint));
        assert_eq!(
            scan_missing_preview_ids_for_engine(&db, SIZE, SPP, ray),
            [] as [i64; 0]
        );
        assert_eq!(scan_missing_preview_ids(&db, SIZE, SPP), vec![entry_id]);
        let draft = cache_fingerprint(
            CacheKind::RayDraft {
                size: SIZE,
                taps: 8,
                fire: true,
            },
            FingerprintMaterial::from_state(&state),
        );
        assert!(store(&draft));
        assert_eq!(
            scan_missing_preview_ids_for_engine(&db, SIZE, SPP, ray),
            vec![entry_id]
        );

        // Stored the way the same design was stored before the size and the source were
        // tracked: only the name in the fingerprint, so it is rendered again.
        let legacy = cache_fingerprint(
            CacheKind::Preview {
                size: SIZE,
                spp: SPP,
                max_bounces: PREVIEW_MAX_BOUNCES,
            },
            FingerprintMaterial::picked(Some("Sapphire")),
        );
        assert!(store(&legacy));
        assert_eq!(scan_missing_preview_ids(&db, SIZE, SPP), vec![entry_id]);
    }
}
