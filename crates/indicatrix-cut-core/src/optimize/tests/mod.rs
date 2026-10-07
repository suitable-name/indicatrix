//! Tests for [`super`], split by topic: shared fixtures, the objective score,
//! which tiers/angles are free to move, per-candidate search machinery, the
//! coordinate/polish search stages themselves, the anchored-tier geometry and angle
//! bounds, the girdle guard and candidate pool, the named presets, the extended
//! request/result end to end, and the ignored timing cost probe.

mod fixtures;

mod anchored;
mod candidate_search;
mod cost_probe;
mod driven;
mod free_tiers;
mod guard_pool;
mod multistart;
mod objective_weights;
mod optimize_design;
mod polish;
mod presets;
mod search_helpers;
mod search_results;
mod shape_target;
mod tone;
