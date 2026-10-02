//! Tests for native save/export round-tripping, the degenerate-marker header,
//! plain-English load-outcome text, cached-solve reuse, and autosave's write round trip.
//! The `.indicatrix` design file's own tests (names, autosave naming, atomic write)
//! are in [`super::design_tests`].

use super::{
    export::{ScheduleFormat, file_name_for_format, schedule_file_text},
    open_commit::plain_load_outcome_text,
    open_picker::read_foreign_design,
    save_helpers::{NOT_CLOSED_SOLID_MARKER, degenerate_marker_header},
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    ConstraintTier, Design, FingerprintCheck, FreshDesignSpec, MaterialSelection, PreformSpec,
    TierOverlay, load_paired, save_paired,
};

/// Material name/RI override, gear, symmetry and mirror all round-trip through
/// the exact pair of functions
/// [`setup_save_native_callback`]/[`setup_open_native_callback`] call
/// (`indicatrix_cut_core::save_paired`/`load_paired`) -- verified here directly rather
/// than trusted, since this crate's own wiring exercises gear/symmetry/mirror
/// persistence only through the editor's own "New Design"/design-settings forms,
/// not through a dedicated round-trip test. `gear`/`symmetry`/
/// `mirror` round-trip through the paired `.asc`'s own header (already
/// exercised, indirectly, by every existing "Open" test in
/// `indicatrix_cut_core::native`); `material`/`refractive_index_override` round-trip
/// through the design file's `[material]` table (already
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
            body_colour_override: None,
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
        original_notes: None,
        detached: Vec::new(),
    });

    let saved =
        save_paired(&design, "roundtrip.asc", None, None, None).expect("a fresh design must save");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");

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

// --- degenerate_marker_header ---

#[test]
fn degenerate_marker_header_stamps_the_reason_when_absent() {
    let headers: Vec<String> = vec!["GemCad 5.0".to_string()];
    let header = degenerate_marker_header(&headers, "Degenerate: only 2 distinct vertices.")
        .expect("no existing marker -- must stamp one");
    assert!(header.starts_with(NOT_CLOSED_SOLID_MARKER));
    assert!(header.contains("Degenerate: only 2 distinct vertices."));
}

#[test]
fn degenerate_marker_header_never_stamps_twice() {
    let headers = vec![format!("{NOT_CLOSED_SOLID_MARKER} -- already noted")];
    assert!(degenerate_marker_header(&headers, "a different message").is_none());
}

// --- plain_load_outcome_text ---

#[test]
fn a_clean_match_and_applied_overlay_reads_as_restored() {
    let text = plain_load_outcome_text(&FingerprintCheck::Match, &TierOverlay::Applied);
    assert_eq!(text, "Your saved meet constraints were restored.");
}

#[test]
fn a_tier_count_mismatch_names_both_counts_even_on_a_clean_fingerprint() {
    let text = plain_load_outcome_text(
        &FingerprintCheck::Match,
        &TierOverlay::SkippedTierCountMismatch {
            native_tiers: 5,
            asc_tiers: 6,
        },
    );
    assert!(text.contains('5') && text.contains('6'), "{text}");
    assert!(
        !text.contains("fingerprint"),
        "must read in plain English, not the crate's own diagnostic vocabulary: {text}"
    );
}

#[test]
fn a_fingerprint_mismatch_says_the_asc_changed_and_constraints_were_not_restored() {
    let text = plain_load_outcome_text(
        &FingerprintCheck::Mismatch {
            expected_sha256: "aaaa".to_string(),
            found_sha256: "bbbb".to_string(),
        },
        &TierOverlay::SkippedFingerprintMismatch,
    );
    assert!(
        text.contains("changed since this sidecar was saved"),
        "{text}"
    );
    assert!(text.contains("not restored"), "{text}");
    assert!(
        !text.contains("sha256"),
        "must not leak the technical hash text: {text}"
    );
}

#[test]
fn applying_despite_a_mismatch_says_it_was_at_the_cutters_own_request() {
    let text = plain_load_outcome_text(
        &FingerprintCheck::Mismatch {
            expected_sha256: "aaaa".to_string(),
            found_sha256: "bbbb".to_string(),
        },
        &TierOverlay::AppliedDespiteMismatch,
    );
    assert!(text.contains("at your request"), "{text}");
}

#[test]
fn a_draft_overlay_on_a_clean_match_names_the_placeholder_masts() {
    let text = plain_load_outcome_text(&FingerprintCheck::Match, &TierOverlay::AppliedFromDraft);
    assert!(text.contains("placeholders"), "{text}");
}

// --- Group 1, cached-solve reuse ---

use super::{
    atomic_write::{WriteGate, temp_sibling, write_synced},
    autosave::write_autosave,
    confirm::{StatusDecision, decide_write_status},
    open_commit::ReplaceGuard,
    solve::{SolveFailure, SolveLane, classify_solve_result, solve_matches_design},
};
use crate::gui::editor::{solve_service::SolveOutcome, state::EditorState};
use std::time::{Duration, Instant};

/// The same round-brilliant fixture `cut_sheet.rs`'s own tests use: 8 tiers,
/// all `ScaleReference`, always solves and closes.
fn round_brilliant_design() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        indicatrix_cut_core::ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

#[test]
fn solve_matches_design_accepts_a_cache_whose_tier_count_still_matches() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("standard round brilliant solves");
    assert_eq!(solved.len(), design.tiers.len());
    assert!(solve_matches_design(Some(&solved), &design).is_some());
}

#[test]
fn solve_matches_design_rejects_a_cache_whose_tier_count_no_longer_matches() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("standard round brilliant solves");
    // One tier short of `design.tiers.len()` -- the shape an edit that added or
    // removed a tier since this solve was cached would leave behind.
    assert!(solve_matches_design(Some(&solved[..solved.len() - 1]), &design).is_none());
}

#[test]
fn solve_matches_design_rejects_no_cache_at_all() {
    let design = round_brilliant_design();
    assert!(solve_matches_design(None, &design).is_none());
}

// --- Group 2, the write-confirm decision ---

#[test]
fn decide_write_status_is_fine_for_a_closed_solid() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("standard round brilliant solves");
    assert!(matches!(
        decide_write_status(&design, Ok(solved.as_slice())),
        StatusDecision::Fine
    ));
}

#[test]
fn decide_write_status_needs_confirm_when_the_design_does_not_solve() {
    let design = round_brilliant_design();
    match decide_write_status(&design, Err("no scale-reference tier")) {
        StatusDecision::NeedsConfirm(message) => {
            assert_eq!(message, "no scale-reference tier");
        }
        StatusDecision::Fine => panic!("a solve error must always need confirmation"),
    }
}

// The picker test hook's own take/set mechanics live in `gui::pickers`
// along with the picker itself -- see that module's own test suite
// (`pick_test_hook_is_consumed_exactly_once`) for the equivalent coverage.

// --- Group 4, autosave write round trip ---

#[test]
fn write_autosave_round_trips_its_own_toml_text() {
    let dir = native_io_temp_dir("autosave_round_trip");
    let path = dir.join("design.autosave.toml");

    write_autosave(&path, "design = \"round trip\"\n").expect("write must succeed");
    let read_back = std::fs::read_to_string(&path).expect("must read back what was written");
    assert_eq!(read_back, "design = \"round trip\"\n");

    // The stage-then-rename discipline: no leftover temp sibling once the write has
    // completed -- only the target itself remains in the directory.
    assert_eq!(dir_entry_names(&dir), ["design.autosave.toml"]);

    let _ = std::fs::remove_dir_all(&dir);
}

/// The file names in `dir`, sorted.
fn dir_entry_names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read temp dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// --- Save durability: unique temp names and the single-writer gate ---

#[test]
fn temp_siblings_of_one_target_are_distinct_and_stay_beside_it() {
    let target = std::path::Path::new("some_dir").join("design.asc");
    let first = temp_sibling(&target);
    let second = temp_sibling(&target);
    assert_ne!(first, second, "two writers must never share a staging file");
    assert_eq!(first.parent(), target.parent());
    assert_eq!(second.parent(), target.parent());
    assert!(
        first
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("design.asc.")),
        "{first:?}"
    );
}

#[test]
fn two_writers_staging_one_target_keep_their_own_contents() {
    let dir = native_io_temp_dir("two_writers");
    let target = dir.join("design.asc");
    let first = temp_sibling(&target);
    let second = temp_sibling(&target);

    write_synced(&first, "first writer").expect("stage first");
    write_synced(&second, "second writer").expect("stage second");
    // Neither staging write touched the other's file.
    assert_eq!(
        std::fs::read_to_string(&first).expect("read"),
        "first writer"
    );
    assert_eq!(
        std::fs::read_to_string(&second).expect("read"),
        "second writer"
    );

    std::fs::rename(&first, &target).expect("publish first");
    std::fs::rename(&second, &target).expect("publish second");
    assert_eq!(
        std::fs::read_to_string(&target).expect("read"),
        "second writer"
    );
    assert_eq!(dir_entry_names(&dir), ["design.asc"]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_gate_parks_a_second_save_until_the_first_reports_in() {
    let now = Instant::now();
    let mut gate = WriteGate::new();
    assert_eq!(gate.admit("first", now), Some("first"));
    assert_eq!(
        gate.admit("second", now),
        None,
        "a save arriving mid-write must wait, not race"
    );
    assert_eq!(
        gate.release(now),
        Some("second"),
        "the parked save runs after the first"
    );
    assert_eq!(gate.release(now), None, "nothing left parked");
    assert_eq!(
        gate.admit("third", now),
        Some("third"),
        "the gate is idle again"
    );
}

#[test]
fn write_gate_keeps_only_the_newest_parked_save() {
    let now = Instant::now();
    let mut gate = WriteGate::new();
    assert_eq!(gate.admit("first", now), Some("first"));
    assert_eq!(gate.admit("second", now), None);
    assert_eq!(gate.admit("third", now), None);
    assert_eq!(
        gate.release(now),
        Some("third"),
        "the second describes a design the third already supersedes"
    );
}

#[test]
fn write_gate_stops_waiting_for_a_writer_that_never_reported() {
    let start = Instant::now();
    let mut gate = WriteGate::new();
    assert_eq!(gate.admit("first", start), Some("first"));
    // A writer thread that panicked never calls `release`; a much later save must
    // not stay parked forever.
    let much_later = start + Duration::from_secs(3600);
    assert_eq!(gate.admit("second", much_later), Some("second"));
}

// --- Write-time solves: typed outcome and one request at a time ---

#[test]
fn a_superseded_result_is_never_read_as_a_geometry_problem() {
    let superseded = classify_solve_result(true, SolveOutcome::Solved(Ok(Vec::new())));
    assert!(
        matches!(superseded, Err(SolveFailure::Superseded)),
        "a displaced result must not reach the not-a-closed-solid prompt or the file header"
    );
}

#[test]
fn a_current_result_passes_through_and_a_foreign_kind_is_a_failure() {
    let current = classify_solve_result(false, SolveOutcome::Solved(Ok(Vec::new())));
    assert!(matches!(current, Ok(ref solved) if solved.is_empty()));
    let foreign = classify_solve_result(
        false,
        SolveOutcome::Verified(Err(
            indicatrix::geometry::meet_solver::SolveError::Cancelled,
        )),
    );
    assert!(matches!(foreign, Err(SolveFailure::Failed(_))));
}

#[test]
fn the_solve_lane_runs_one_request_at_a_time_in_order() {
    let mut lane = SolveLane::default();
    lane.enqueue(1);
    lane.enqueue(2);
    lane.enqueue(3);
    assert_eq!(lane.start_next(), Some(1));
    assert_eq!(
        lane.start_next(),
        None,
        "a second request must not reach the worker while the first computes"
    );
    assert!(lane.finish(1));
    assert_eq!(lane.start_next(), Some(2));
    assert!(lane.finish(2));
    assert_eq!(lane.start_next(), Some(3));
}

#[test]
fn a_displaced_request_keeps_its_place_at_the_front_of_the_lane() {
    let mut lane = SolveLane::default();
    lane.enqueue(1);
    lane.enqueue(2);
    assert_eq!(lane.start_next(), Some(1));
    assert!(lane.finish(1));
    lane.requeue_front(1);
    assert_eq!(
        lane.start_next(),
        Some(1),
        "the re-issue goes before request 2"
    );
}

#[test]
fn finishing_a_key_that_is_not_running_changes_nothing() {
    let mut lane = SolveLane::default();
    lane.enqueue(7);
    assert_eq!(lane.start_next(), Some(7));
    assert!(!lane.finish(8));
    assert_eq!(lane.start_next(), None, "7 is still on the worker");
}

// --- Open: edits made after the dirty check ---

#[test]
fn the_replace_guard_asks_only_when_edits_landed_after_the_decision() {
    let state = EditorState::fresh();
    let guard = ReplaceGuard::capture(&state, true);
    let decided_at = state.current_generation();
    assert!(
        guard.discards_new_edits(decided_at + 1, true),
        "an edit landed while the picker was open"
    );
    assert!(
        !guard.discards_new_edits(decided_at, true),
        "a Discard that resumed the open leaves the design dirty by choice"
    );
    assert!(
        !guard.discards_new_edits(decided_at + 3, false),
        "edits undone back to the saved state lose nothing"
    );
}

// --- Catalogue write-back's panic guard (see `catalogue::write_back_to_catalogue`'s
// own doc comment: `local::import_asc` + `apply_measured_metadata` now runs inside
// `catch_file_panic`, exactly like a `.asc` import's per-file loop, instead of
// panicking straight through Save's background thread with no toast and no
// completion callback ever firing) ---

/// The ordinary (non-panicking) path through the now-panic-guarded parse-and-measure
/// step: a real solved design, written out via `save_paired` the same way Save
/// itself does, must still land as a brand-new catalogue row with its angle settings
/// and both attachments (the `.asc` plus its design file) intact -- proving the
/// `catch_file_panic` wrapper added around this step changed nothing about the
/// ordinary success path. The panic arm itself is not forced here, for the same
/// reason `catch_file_panic`'s own unit test in `gui::library::local::import::tests`
/// doesn't try to make `indicatrix`'s geometry code panic on demand: that mechanism
/// (`std::panic::catch_unwind` around a deliberately panicking closure) is already
/// covered directly there, and reused verbatim by `write_back_to_catalogue`.
#[test]
fn write_back_to_catalogue_still_creates_a_new_row_through_its_panic_guard() {
    let design = round_brilliant_design();
    design.solve().expect("standard round brilliant solves");
    let paired = save_paired(&design, "write_back_guard_test.asc", None, None, None)
        .expect("a closed, solved design must save without a draft fallback");
    assert!(
        paired.draft_reason.is_none(),
        "the round-brilliant fixture must not fall back to a draft save"
    );

    let db_path = std::env::temp_dir().join(format!(
        "indicatrix_cut_write_back_panic_guard_test_{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&db_path);
    let db = std::sync::Arc::new(std::sync::Mutex::new(
        indicatrix_vault::db::sqlite::Database::new(Some(
            db_path.to_str().expect("temp path is valid UTF-8"),
        ))
        .expect("create fresh temp test db"),
    ));

    let (id, outcome) = super::catalogue::write_back_to_catalogue(
        &db,
        None,
        "write_back_guard_test.asc",
        &paired.asc_text,
        "write_back_guard_test.indicatrix.toml",
        &paired.native_toml,
    )
    .expect("an ordinary, non-panicking save must still succeed through the panic guard");
    assert!(
        matches!(outcome, super::catalogue::CatalogueWriteBack::NewRow),
        "a design with no known source_entry_id must insert a brand-new row"
    );

    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let full = conn
        .get_diagram_full(id)
        .expect("query must succeed")
        .expect("the row just written must read back");
    assert!(
        !full.angle_settings.is_empty(),
        "the measured design's angle settings must have been saved"
    );
    assert_eq!(
        full.attached_files.len(),
        2,
        "both the .asc and its design file must be attached"
    );
    drop(conn);

    let _ = std::fs::remove_file(&db_path);
}

/// Save must not overwrite a catalogue design that already owns the
/// `local://<file>` url: the write-back declines, reports the owner and leaves its
/// title and attachments untouched.
#[test]
fn write_back_to_catalogue_declines_to_overwrite_a_design_that_owns_the_url() {
    let design = round_brilliant_design();
    design.solve().expect("standard round brilliant solves");
    let paired = save_paired(&design, "collide_test.asc", None, None, None).expect("saves");
    let db_path = std::env::temp_dir().join(format!(
        "indicatrix_cut_write_back_collision_test_{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&db_path);
    let db = std::sync::Arc::new(std::sync::Mutex::new(
        indicatrix_vault::db::sqlite::Database::new(Some(
            db_path.to_str().expect("temp path is valid UTF-8"),
        ))
        .expect("create fresh temp test db"),
    ));
    let owner_id = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .save_design(
            &indicatrix_vault::model::entry::FacetingDiagramEntry {
                title: "Imported Original".to_string(),
                url: "local://collide_test.asc".to_string(),
                design_id: String::new(),
            },
            &indicatrix_vault::model::detail::FacetingDiagramDetail {
                attached_files: vec![indicatrix_vault::model::file::AttachedFile {
                    name: "collide_test.asc".to_string(),
                    url: String::new(),
                    content: b"original bytes".to_vec(),
                }],
                ..Default::default()
            },
            indicatrix_vault::local::LOCAL_SOURCE_ID,
        )
        .expect("seed the owning design");

    let (id, outcome) = super::catalogue::write_back_to_catalogue(
        &db,
        None,
        "collide_test.asc",
        &paired.asc_text,
        "collide_test.indicatrix.toml",
        &paired.native_toml,
    )
    .expect("a collision is an outcome, not an error");
    assert_eq!(id, owner_id);
    assert!(
        matches!(
            outcome,
            super::catalogue::CatalogueWriteBack::UrlCollision { existing_id, ref title }
                if existing_id == owner_id && title == "Imported Original"
        ),
        "the owner of the url must be reported, not overwritten"
    );

    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let full = conn
        .get_diagram_full(owner_id)
        .expect("query must succeed")
        .expect("the owning row must still exist");
    assert_eq!(full.title, "Imported Original");
    assert_eq!(full.attached_files.len(), 1);
    assert_eq!(full.attached_files[0].content, b"original bytes");
    assert_eq!(conn.get_total_count().expect("count"), 1);
    drop(conn);

    let _ = std::fs::remove_file(&db_path);
}

// --- .gcs export and .gem/.gcs open ---

/// The standard round brilliant's cutting instructions, from the first New
/// Design template, as `Design::to_asc_schedule` builds them for the exports.
fn standard_round_brilliant_schedule() -> indicatrix_formats::asc::AscSchedule {
    let spec = indicatrix_cut_core::templates::TEMPLATES
        .first()
        .expect("TEMPLATES is non-empty");
    assert_eq!(spec.name, "Standard Round Brilliant");
    let design = Design::new(
        PreformSpec::cylinder(spec.gear_teeth.unsigned_abs() as usize, 1.5, 1.0, 1.5),
        spec.schedule_meta(),
        spec.tiers(),
    );
    design
        .to_asc_schedule()
        .expect("the standard round brilliant solves")
}

/// "Export as Gem Cut Studio (.gcs)..." writes text the `.gcs` reader parses
/// back with every tier of the schedule the `.asc` export would have written.
#[test]
fn gcs_export_of_the_standard_round_brilliant_parses_back_with_the_same_tier_count() {
    let schedule = standard_round_brilliant_schedule();
    let text =
        schedule_file_text(&schedule, ScheduleFormat::Gcs).expect("the .gcs writer accepts it");
    let parsed = indicatrix_formats::gcs::parse_gcs_bytes(text.as_bytes())
        .expect("the exported .gcs parses back");
    assert_eq!(parsed.tiers.len(), schedule.tiers.len());
    let asc =
        schedule_file_text(&schedule, ScheduleFormat::Asc).expect("the .asc writer accepts it");
    assert!(asc.starts_with("GemCad"), "{asc}");
}

/// The `.gcs` export's save dialog proposes the design's own name with a `.gcs`
/// extension.
#[test]
fn export_file_name_takes_the_format_extension() {
    assert_eq!(
        file_name_for_format("round.asc", ScheduleFormat::Gcs),
        "round.gcs"
    );
    assert_eq!(
        file_name_for_format("edited_design.asc", ScheduleFormat::Asc),
        "edited_design.asc"
    );
    assert_eq!(
        file_name_for_format("plain", ScheduleFormat::Gcs),
        "plain.gcs"
    );
}

/// A temp directory unique to this test process and `label`.
fn native_io_temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix_cut_native_io_{label}_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Open... converts a picked `.gcs` to `.asc` cutting instructions recorded under
/// `<stem>.asc`, which is what the editor's bare-`.asc` path then loads.
#[test]
fn open_reads_a_gcs_file_as_asc_cutting_instructions() {
    let schedule = standard_round_brilliant_schedule();
    let gcs = indicatrix_formats::gcs::to_gcs_string(&schedule).expect("writes as .gcs");
    let dir = native_io_temp_dir("open_gcs");
    let path = dir.join("round.gcs");
    std::fs::write(&path, gcs).expect("write round.gcs");

    let picked = read_foreign_design(&path).expect("a written .gcs opens");
    assert_eq!(picked.source_path, path);
    assert_eq!(picked.asc_file_name, "round.asc");
    let loaded = crate::gui::editor::loading::design_from_asc_text(
        &picked.asc_file_name,
        &picked.asc_text,
        None,
    )
    .expect("the converted .asc loads as a design");
    assert_eq!(loaded.design.tiers.len(), schedule.tiers.len());
    assert_eq!(loaded.asc_filename.as_deref(), Some("round.asc"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A corrupt `.gem` picked in Open... is a readable error, never a panic.
#[test]
fn open_rejects_a_corrupt_gem_with_the_readers_message() {
    let dir = native_io_temp_dir("open_bad_gem");
    let path = dir.join("broken.gem");
    std::fs::write(&path, [0_u8; 5]).expect("write broken.gem");

    let Err(message) = read_foreign_design(&path) else {
        panic!("a 5-byte .gem must not open");
    };
    assert!(message.contains("not a readable .gem file"), "{message}");

    let _ = std::fs::remove_dir_all(&dir);
}
