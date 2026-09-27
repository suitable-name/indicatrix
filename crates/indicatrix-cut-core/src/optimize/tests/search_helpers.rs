//! Tests for [`super::super::search`]'s `seeded_permutation`, and for
//! [`super::super::SearchStage`]'s code round-trip and
//! [`super::super::inclusive_max_evaluations`].

use super::super::{OptimizeConfig, SearchStage, inclusive_max_evaluations, search};

// --- seeded_permutation ---

#[test]
fn seeded_permutation_is_a_real_permutation() {
    let mut sorted = search::seeded_permutation(10, 12345);
    sorted.sort_unstable();
    assert_eq!(sorted, (0..10).collect::<Vec<_>>());
}

#[test]
fn seeded_permutation_is_deterministic_for_the_same_seed() {
    assert_eq!(
        search::seeded_permutation(20, 42),
        search::seeded_permutation(20, 42)
    );
}

#[test]
fn seeded_permutation_differs_across_seeds_almost_always() {
    assert_ne!(
        search::seeded_permutation(20, 1),
        search::seeded_permutation(20, 2)
    );
}

// --- SearchStage::to_code / from_code ---

#[test]
fn search_stage_code_round_trips_every_variant() {
    for stage in [
        SearchStage::BaselineFull,
        SearchStage::Coordinate,
        SearchStage::Polish,
        SearchStage::FinalFull,
    ] {
        assert_eq!(SearchStage::from_code(stage.to_code()), stage);
    }
}

#[test]
fn search_stage_from_code_defaults_an_unknown_value_to_coordinate() {
    assert_eq!(SearchStage::from_code(255), SearchStage::Coordinate);
}

// --- inclusive_max_evaluations ---

#[test]
fn inclusive_max_evaluations_adds_the_polish_stage_s_default_cap() {
    let config = OptimizeConfig {
        max_evaluations: 200,
        polish_start_step_deg: Some(0.5),
        polish_max_evaluations: None,
        ..OptimizeConfig::default()
    };
    // `run_polish_stage`'s own default: `3 * free_tier_count + 20`.
    assert_eq!(inclusive_max_evaluations(&config, 10), 200 + 3 * 10 + 20);
}

#[test]
fn inclusive_max_evaluations_excludes_polish_when_disabled() {
    let config = OptimizeConfig {
        max_evaluations: 200,
        polish_start_step_deg: None,
        ..OptimizeConfig::default()
    };
    assert_eq!(inclusive_max_evaluations(&config, 10), 200);
}

#[test]
fn inclusive_max_evaluations_honors_an_explicit_polish_cap() {
    let config = OptimizeConfig {
        max_evaluations: 200,
        polish_start_step_deg: Some(0.5),
        polish_max_evaluations: Some(50),
        ..OptimizeConfig::default()
    };
    assert_eq!(inclusive_max_evaluations(&config, 10), 250);
}
