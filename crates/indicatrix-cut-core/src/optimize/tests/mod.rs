//! Tests for [`super`], split by topic: shared fixtures, the objective score,
//! which tiers/angles are free to move, per-candidate search machinery, the
//! coordinate/polish search stages themselves, and the ignored timing cost
//! probe.

mod fixtures;

mod candidate_search;
mod cost_probe;
mod free_tiers;
mod objective_weights;
mod optimize_design;
mod polish;
mod search_helpers;
