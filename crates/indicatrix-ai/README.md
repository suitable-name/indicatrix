# indicatrix-ai

Reinforcement-learning environments, a set-attention policy network, PPO training and inference
wrappers for gemstone cut optimization and material retargeting in the Indicatrix suite.

The crate has its own `[workspace]` table: it is built and tested on its own
(`cargo test --manifest-path crates/indicatrix-ai/Cargo.toml`), not as part of the root workspace.

## What is in it

| Module | Contents |
|---|---|
| `env` | `OptimizerEnv` (improve a design in its own material) and `RetargetEnv` (adapt it to a target material). One shared episode core, one observation layout (`OBS_LAYOUT_VERSION`). |
| `model` | `GemstoneSetAttentionNetwork` (pure Rust, deterministic) with sensitivity, value and gain heads, `GaussianPolicy` with the composed mean (`compose_mean`), the kinematic action projection, `Prng` (SplitMix64) and the schema-versioned `Checkpoint` file. |
| `training` | `BurnSetAttention` (the Burn twin of the network) and `BurnTrainer`: imitation on finite-difference labels (the primary algorithm) and PPO (kept), with attention/log-prob/entropy masking of padded tiers, global gradient clipping and an LR schedule. `RolloutBuffer` does per-worker GAE. |
| `inference` | `RlOptimizerModel` and `RlRetargetModel` run a trained network greedily and report the result. |
| `bridge` | `EditorRetargetAdapter` merges a retarget result into the editor's `RetargetPlan`. |

## Conventions

- Angles are degrees and **signed**: crown tiers positive, pavilion tiers negative. A tier never
  changes sign (the sign bit decides crown or pavilion in the meet solver).
- Scores are `ObjectiveWeights::score` figures, `0..=100`, **lower is better**.
- Actions are one angle delta per tier of the design, in design order. A wrong-length action is
  rejected by the environment (`StepRejection::WrongActionLength`), never reinterpreted.
- Everything is deterministic. The environments have no randomness; exploration noise comes from
  the training sampler with an explicit seed.

## The environments

Both are greedy: a candidate replaces the current design only when it solves into a closed
stone, keeps the girdle (at least `min_girdle_ratio` of the base band), does not add vanishing or
undersized facets, and scores strictly lower. The state therefore never gets worse; a rejected
proposal leaves it untouched and earns a negative reward.

- The design is scored in the material it was authored for (named preset and overrides, else the
  `.asc` refractive index), not in diamond. `RetargetEnv` scores in the target material.
- Imported `.asc` designs pin every tier to a mast. With `vary_anchored` (the default) those tiers
  turn about their measured hinge, as in the editor's Optimize. A design with no movable tier is
  an `EnvError::NoMovableTiers`, not a silent no-op training run.
- Constructors return `Result<_, EnvError>`; nothing trains on an unsolvable, open or unscorable
  base design.
- Observation: a global token (14 features) and one token per tier (10 features), every feature
  measured from the current design. `GemstoneObservation::flatten(max_tiers)` is exactly the
  global token followed by the tier tokens and zero padding; `OBS_LAYOUT_VERSION` changes whenever
  the meaning or order of a feature changes, and checkpoints record it.

## The model

`GemstoneSetAttentionNetwork` embeds the global token and one token per tier, runs pre-norm
multi-head attention blocks and a final `LayerNorm`, and reads three kinds of heads:

- `sens_head`: per tier, the predicted slope of each objective component per degree of that tier
  (windowing, extinction, brilliance loss, yield loss; `N_COMPONENTS` = 4), raw outputs.
- `critic_head` and `gain_head` on the pooled vector (context token plus the mean of the tier
  tokens): the value baseline `V(s)` and the predicted score drop of the oracle step at the state.
- Three raw log-std values (crown, pavilion, other), state-independent.

The policy mean is **composed**, not predicted. The global token carries the objective's weight
fractions (entries 15 to 18); per tier `g_k = sum_c fraction[c] * sens[k][c]` and
`mean_k = -MEAN_BOUND * g_k / (max_k |g_k| + SLOPE_EPS)` with `MEAN_BOUND = atanh(0.95)`, in
pre-squash units. After the `tanh` of the step the steepest tier moves 95 % of the step limit and
every other tier in proportion, so one model serves every objective preset and any custom slider
setting; there is no model per objective. The two tone presets are not covered (the environment
refuses a tone weight). `compose_mean` is the pure function, and the Burn twin composes on tensors
with the same operation order. `forward_full` also returns the sensitivities and the gain.

## Checkpoints

`Checkpoint::save` writes compact JSON to `<path>.tmp` and renames it over `path`. A checkpoint
records the schema version (3: sensitivity and gain heads, composed mean, observation layout 4),
the task (`TrainingMode::Optimizer` or `Retarget`), the model tier, the observation layout version,
`max_tiers`, the iteration and the seed. `Checkpoint::load` refuses another schema or layout
version and any network whose shapes are inconsistent, so a damaged file is an error, not a panic
in `forward`. Files of schema 2 and older are refused with `UnsupportedSchema`.

## How to train

Training is done by the `indicatrix-trainer` app (`apps/indicatrix-trainer`): a Slint GUI and a
command-line binary that drive `BurnTrainer` over designs from the catalogue database and write
checkpoints. Imitation on finite-difference labels (`BurnTrainer::train_imitation_epochs`, module
`training::imitation`) is the primary algorithm; PPO (`train_batch`, `train_rollout_epochs`) is
kept and shares the model. See that app's README for the options.

## How to run a trained model

```rust
use indicatrix_ai::inference::{InferenceConfig, RlOptimizerModel};

let model = RlOptimizerModel::load(checkpoint_path, InferenceConfig::optimizer())?;
let result = model.optimize(&design, ObjectiveWeights::default())?;
// `result` is cut-core's OptimizeResult: apply it with `apply_optimize_result`.
```

- `RlOptimizerModel::optimize` returns cut-core's `OptimizeResult`. Its before/after figures are
  measured at `Full` fidelity (`InferenceConfig::report_fidelity`), and a change is proposed only
  when it measures strictly better there; otherwise `candidates` and `changes` are empty.
- `RlRetargetModel::retarget` returns the angles of the tiers the policy changed, in the target
  material, starting from the design it is given.
- `EditorRetargetAdapter::build_plan_with_rl` starts the retarget policy from the editor's
  critical-angle shift baseline and returns a `RetargetPlan` whose rows keep their baseline angle
  unless the policy changed the tier.
- Every fallible step returns an `InferenceError` (bad checkpoint, wrong task, wrong layout,
  unsolvable design, no movable tier). Nothing falls back to a made-up answer.

## Known limits

- Rollouts score candidates at `Fast` fidelity (one table-up pose, no tilt information); the
  `Full` fidelity report is measured only for the start and the end of an inference run.
- The score ignores the yield and face-up tone terms of `ObjectiveWeights` while the policy acts
  (inference reports the yield). A tone weight is not supported.
- The retarget policy anchors tiers about the hinges of the baseline stone, the editor about
  those of the original stone, so masts computed on apply can differ slightly from the scored ones.
- The batched training pass pads to `max_tiers` (at most 64) and masks the padding; the pure
  network used for inference takes any tier count.
- The trained model is not wired into the desktop app.
