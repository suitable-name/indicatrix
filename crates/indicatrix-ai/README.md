# indicatrix-ai

Standalone reinforcement-learning environments, models, training code and inference wrappers for gemstone cut optimization and material retargeting. The crate is built independently; it is not currently connected to the desktop application.

## Modules

- `env`: deterministic greedy `OptimizerEnv` and `RetargetEnv`, shared scoring/preservation logic, measured observations and finite-difference teacher labels.
- `model`: pure Rust set-attention network, composed sensitivity policy, mobility-aware action projection, random sampler and schema-versioned checkpoints.
- `training`: Burn network twin, supervised imitation trainer and PPO support.
- `inference`: task-specific optimizer/retarget wrappers and geometry-aware `score_design_at_fidelity` readout.
- `bridge`: compatibility adapter for producing an editor angle plan from a certified retarget result; it refuses a reconstruction that does not match the exact solved geometry.

## Environment behavior

Actions contain one requested angle delta per design tier. Moves are projected against an immutable meet-preservation baseline, with configurable angle scopes/bounds and coordinated mast repairs. A candidate is accepted only when it remains solvable and closed, preserves configured manufacturability constraints, and scores better. A no-light result is not a construction error.

Both environments accept `ObjectiveFidelity::{Fast, Full}`, lighting, an optional `ShapeTarget`, angle-coordinate allow-lists and per-tier bounds. Their observation has `GLOBAL_DIM = 41` and `TIER_DIM = 26` with `OBS_LAYOUT_VERSION = 5`. The seven sensitivity components are windowing, extinction, brilliance loss, yield loss, lighter-tone loss, deeper-tone loss and shape distance. Yield is omitted from normalized optical coefficients when it is unavailable; shape cost remains additive. Tone is scored using the same core path as other optical terms.

`OptimizerEnvConfig::source_material_override` and `RetargetEnvConfig::source_material_override` allow training code to provide measured source material properties. In retargeting, the configured target material still controls scoring. Observations include measured principal-axis absorption summaries at 450/550/650 nm, material path scale combined with measured stone width, scattering, target lighting and geometry descriptors. Zoning and fluorescence are not encoded as separate observation features.

## Models and training

The network predicts per-tier component slopes. It composes a policy direction from those slopes and the seven objective coefficients rather than predicting an independent action. Immobile tiers are marked in the token and masked during action projection. Checkpoints record task, schema and observation layout; inference checks all three and requires an exact optimizer/retarget task match.

Training is driven by the separate `indicatrix-trainer` package. This crate contains no catalogue database integration or desktop UI hookup. The trainer configures objective, fidelity, lighting, shape target and material conditions before constructing environments.

## Inference

`RlOptimizerModel::optimize` returns cut-core's optimizer result after endpoint scoring. `RlRetargetModel::retarget_certified` returns a `CertifiedRetargetResult` containing the proposal, materialized design, exact angles and masts, and original-relative preservation facts. The compatibility `retarget` method also carries the solved angles and masts in its proposal. `score_design_at_fidelity` measures a materialized design without rebuilding or recapturing an environment.

Rollout fidelity and endpoint/report fidelity are separate settings. The package guarantees shared scoring semantics and preservation certification; it makes no empirical claim about training speed or model quality.
