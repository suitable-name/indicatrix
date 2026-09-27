//! Tests for native save/export round-tripping, the degenerate-marker header,
//! plain-English load-outcome text, cached-solve reuse, and autosave's write round trip.

use super::{
    open_commit::plain_load_outcome_text,
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
/// exercised, indirectly, by every existing "Open Native" test in
/// `indicatrix_cut_core::native`); `material`/`refractive_index_override` round-trip
/// through the native sidecar's `[material]` table (already
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
    atomic_write::temp_sibling,
    autosave::write_autosave,
    confirm::{StatusDecision, decide_write_status},
    solve::solve_matches_design,
};

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
    let path = std::env::temp_dir().join(format!(
        "indicatrix_cut_write_autosave_test_{}.toml",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    write_autosave(&path, "design = \"round trip\"\n").expect("write must succeed");
    let read_back = std::fs::read_to_string(&path).expect("must read back what was written");
    assert_eq!(read_back, "design = \"round trip\"\n");

    // The stage-then-rename discipline: no leftover `.tmp` sibling
    // once the write has completed.
    assert!(!temp_sibling(&path).is_file());

    let _ = std::fs::remove_file(&path);
}
