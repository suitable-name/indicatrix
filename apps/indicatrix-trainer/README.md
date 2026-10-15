# indicatrix-trainer

Training application for the learned cut models of `indicatrix-ai`: a Slint GUI
(`indicatrix-trainer`) and a headless command-line trainer (`indicatrix-trainer-cli`). Both drive
the same training loop (`src/runner.rs`). The crate is standalone (own `[workspace]`); integrating a
trained model into the desktop app is not part of it.

## Tasks

- **Optimizer**: tier angles of a design are adjusted within the design's own material to lower
  the optical score (windowing, extinction, tilt brilliance, optionally yield).
- **Retarget**: the same, but the stone is scored in a target material drawn from a 13-species
  spectrum (n 1.43 to 2.90) or fixed by name. The source material is the design's own (its
  `.asc` `I` line).

Scores are `ObjectiveWeights::score` figures in `0..=100`, lower is better. Environments are
greedy: a step is only kept when it lowers the score and keeps the stone valid, so an episode's
"improvement" (base score minus final score) is never negative.

## Data

The design library `facet_diagrams.sqlite` is opened read-only. It is found through the
`DIAGRAM_DB_PATH` environment variable, or in the working directory and up to three directories
above it. A missing or unreadable database is an error; there is no substitute data. The only
stand-in is the explicit `--fixture` flag of the CLI (the built-in concave fixture).

Records that do not parse, or that have more than 32 tiers (CLI: `--max-tiers`), are skipped and
logged. The rest is split by library entry id with an FNV-1a hash into train (75 %), validation
(15 %) and test (10 %); the salt is stored in the run statistics as `split_seed`.

## How it learns

The network is the set-attention model of `indicatrix-ai`. Per free tier it predicts how much each
of the four score components changes per degree of that tier: windowing, extinction, brilliance
loss (100 minus tilt brilliance) and yield loss (100 minus yield). The policy direction is composed
from these sensitivities with the weight fractions of the objective, which are part of the
observation, and the environment picks the step length with a line search: it tries the proposed
move at each scale of `--line-search` (default 1, 0.5, 0.25 of the proposal) and keeps the first
candidate that lowers the score, counting one step whatever the number of candidates. Direction
and step size are therefore separate: the model learns directions, the line search finds a length
that crosses the quantised score gate.

### Imitation (`--algo imitation`, the default)

Each round the current network plays `--batch-size` environment steps over the workers, with the
exploration noise `--explore-std` added to its direction. Every `--label-every`-th sample is
labelled before its step. For each free tier one probe candidate (the tier moved by `--label-h`
degrees, default the step limit, forward, or backward when the forward move is refused) measures
the change of all four components at once, and an oracle step built from the measured score
slopes, tried at the line-search scales, records how many points the best step would win. The labelled states are
appended to an aggregate that keeps the newest `--label-capacity` of them (the labels describe the
states the current policy actually visits, so the training set follows the policy), and the network
trains `--epochs` passes over the whole aggregate. The loss is the mean squared error of the
predicted component slopes, plus `--cosine-coeff` times (1 minus the cosine between the composed
direction and the direction of the measured score slopes), plus `--gain-coeff` times a Huber loss
on the predicted oracle gain. Every finite label trains, with no threshold: the score is quantised
(about 0.16 points per sampled ray), a single label is noisy but unbiased, and the regression
averages the noise out. Entries without a label (the yield of a design without a preform volume, a
tier whose probes both failed) are masked. The learning rate decays linearly over
`iterations x epochs x ceil(label-capacity / mini-batch)` updates, an upper bound of what the run
can take: while the aggregate is smaller than its capacity the rate ends above zero.

### Objectives and the shared model

Every episode draws the weights of its objective (`--objectives`): `balanced` uses the default
weights, `presets` one of the five presets Balanced, Brilliance, Low Windowing, Low Extinction and
Keep Weight, and `mixed` (the default) a preset with probability 0.7 and otherwise random weights
(windowing, extinction and tilt brilliance between 0.5 and 4, yield zero with probability 0.7 and
between 0.5 and 3 otherwise). The drawn weights are in the observation, so one model covers every
non-tone preset and any custom slider setting; there is no model per objective. The two tone
presets (Lighten Dark Rough, Intensify Pale Rough) are not covered: the environment does not trace
the face-up tone and refuses a tone weight.

`--same-material-frac` (default 0.25) sets the share of retarget episodes whose target is the
stone's own material, which is the optimizer task. One retarget checkpoint can therefore serve the
optimizer inference wrapper as well; the reverse is refused.

### Validation: three drops

Validation (every `--validation-every` iterations and after the last) plays the first
`--validation-designs` designs of the validation split, with retarget targets and objective weights
fixed by the run seed so that consecutive passes are comparable. Each design is played three times
on the same stone, target and weights, and the report gives the score drop (base minus final) of:

- **model**: the network's deterministic direction plus the line search (`val_improvement`);
- **oracle**: the same direction rule on the measured sensitivities plus the line search, the upper
  bound of what this kind of policy can do per step (`val_improvement_oracle`). It does not depend
  on the model, so it is computed once and reused until something it does depend on changes (the
  validation designs, the seed, the step limit, the task or target settings, the objective
  sampling, the line search or the probe size);
- **sampled**: one stochastic episode, the direction plus the exploration noise
  (`val_improvement_sampled`).

The figure to watch is **model / oracle**, printed after each validation and in the summary. The
goal is above 0.8 on unseen designs, reached at one to about two and a half candidate evaluations
per step (`val_evaluations_per_step`). A ratio that stays below 0.5 after 100 rounds does not mean
the optimiser is too slow: it means the observation does not carry the information, and the tier
token needs more features. Alongside, `train_cosine` should pass 0.7 within 50 rounds and
`sens_mse` should fall.

The generalization gap (recent training drop minus validation drop) is only reported when at least
8 designs were validated. The training side comes from episodes that sample their actions and the
validation side from the sampled pass, so both carry the same exploration noise: read the gap as a
trend, and the GUI's overfitting threshold as a rule of thumb. Telemetry files store "no value yet"
as a missing field, never as `0.0`. A resume (`--resume`, or loading weights in the GUI) restarts
the run statistics, which belong to the model, and the imitation aggregate (it is not stored in a
checkpoint, it refills from the first rounds). An empty validation split is reported at start-up.
`--eval-test` does the same on the test split with all three drops; `--eval-fidelity full` uses
the full tilt sweep (about 3 s per design) and skips the oracle pass, which costs a probe per free
tier and step.

### PPO (`--algo ppo`)

The earlier reinforcement-learning loop stays available and shares the model: PPO with generalized
advantage estimation per worker (episode ends are respected: terminated steps do not bootstrap,
time-limit truncations do), a gradient-norm clip, a KL early stop and shuffled mini-batches. It
learns from the scalar step reward only, so it cannot see the per-tier signal that imitation uses;
it is the baseline to compare against. Pinned and flat tiers, which the environment never moves,
and tiers outside their angle envelope, which the projection would override, are masked out of the
sampled action, the log-probability and the entropy bonus. PPO ignores the labels in its loss, so
its default label period is 0 (no probes are paid for); `--label-every` still turns them on to
collect the oracle-gain telemetry. `--lr` and `--entropy-coeff` are PPO flags, `--imitation-lr`,
`--cosine-coeff` and `--gain-coeff` imitation flags; the trainer notes a flag that does not apply
to the chosen algorithm.

### Where the time goes

The environment is CPU work: solving, meshing, measuring and tracing a candidate takes 7 to 25 ms.
The network forward is a few hundred microseconds, so the GPU can only take the batched forward of
a pass and the update, and a run is limited by how many candidate evaluations the cores deliver.
`--workers` therefore defaults to the core count (an environment is cheap, hundreds are fine, the
limit is 1024), and the label probes are scheduled as individual tasks over all cores so that no
core waits for the slowest slot. Read `evals/s` in the progress line. The telemetry has, per
iteration, `rollout_secs` (collecting the rollout, probes included), `label_secs` (the probe phase
of it), `update_secs`, `evaluations_this_round` (step candidates, probes and oracle candidates) and
`workers`; the summary prints the mean seconds per iteration of each phase. If `update_secs` is
small against `rollout_secs`, more cores help and a bigger model costs little; if it is large, the
update is the limit.

## Command line

```
indicatrix-trainer-cli --mode retarget --tier m --device auto --iterations 200 \
    --save-stats run.json --save-weights run.json.ckpt
indicatrix-trainer-cli --help
```

Unknown flags (`--teacher-every` is gone), non-numeric values and out-of-range values are errors
(exit status 2). An explicit `--device gpu` fails when no adapter works; `auto` falls back to the
CPU with a warning.

Before anything else is printed the CLI asks GitHub (2 s at most) whether a newer release exists
and, only then, prints one `info:` line first (`info: Indicatrix 0.7.3 is available (this is
0.7.2): <release page>`). Up to date, offline or any failure prints nothing; nothing is stored or
downloaded. `--no-update-check` or the environment variable `INDICATRIX_NO_UPDATE_CHECK` (any
value) skips the request.

| Flag | Default | Meaning |
|---|---|---|
| `--mode <optimizer\|retarget>` | retarget | Task to train |
| `--tier <s\|m\|l\|xl\|xxl>` | m | Model size |
| `--device <cpu\|gpu\|auto>` | auto | Compute backend |
| `--algo <imitation\|ppo>` | imitation | Training algorithm |
| `--iterations <N>` | 100 | Iterations, each one rollout and one update |
| `--workers <N>` | the core count | Parallel environments, 1 to 1024 |
| `--batch-size <N>` | 256 | Environment steps per iteration, 16 to 4096 |
| `--mini-batch <N>` | 256 imitation, 64 PPO | Mini-batch size of both algorithms, 8 to 1024, not above the batch size |
| `--epochs <N>` | 2 imitation, 4 PPO | Epochs per round (imitation) or per iteration (PPO), 1 to 16 |
| `--validation-designs <N>` | 16 | Held-out designs in the fixed validation subset, 1 to 64 |
| `--validation-every <N>` | 10 | Validate every N iterations (and after the last), 1 to 1000 |
| `--label-every <N>` | 2 imitation, 0 PPO | Every N-th rollout sample is labelled, 0 to 64; 0 is refused with imitation |
| `--label-h <X>` | the step limit | Probe size of the labels, degrees, above 0 |
| `--label-capacity <N>` | 50000 | Labelled states the aggregate keeps, 1 to 1000000 |
| `--explore-std <X>` | 0.3 | Exploration noise on the direction, 0 or more |
| `--line-search <a,b,c>` | 1,0.5,0.25 | Step scales tried in order, each above 0 |
| `--objectives <balanced\|presets\|mixed>` | mixed | How the objective weights of an episode are drawn |
| `--same-material-frac <X>` | 0.25 | Share of retarget episodes aimed at the stone's own material, 0 to 1 |
| `--imitation-lr <X>` | 3e-4 | Imitation peak learning rate, decayed linearly |
| `--cosine-coeff <X>` | 0.5 | Weight of the direction-cosine term, 0 or more |
| `--gain-coeff <X>` | 0.1 | Weight of the gain-regression term, 0 or more |
| `--lr <X>` | PPO built-in | PPO peak learning rate, decayed linearly to 0 |
| `--entropy-coeff <X>` | PPO built-in | PPO entropy bonus, 0 or more |
| `--max-step-deg <X>` | the environment's | Step limit per tier and step, degrees, above 0 |
| `--seed <N>` | 42 | Seed of sampling, shuffling and initialisation |
| `--max-tiers <N>` | 32 | Skip designs with more tiers, 1 to 64 |
| `--no-corpus` | off | Train on the first train design only |
| `--no-spectrum`, `--target-material <NAME>` | off, Diamond | One fixed target material instead of the spectrum |
| `--eval-test`, `--eval-fidelity <fast\|full>` | off, fast | Evaluate the test split at the end |
| `--fixture` | off | Train on the built-in concave fixture |
| `--no-update-check` | off | Do not ask GitHub for a newer release (also `INDICATRIX_NO_UPDATE_CHECK`) |
| `--resume <PATH>` | none | Continue from a checkpoint (mode and tier must match) |
| `--save-stats <PATH>` | training_stats.json | Telemetry output |
| `--save-weights <PATH>` | indicatrix-\<mode\>-\<tier\>-iter\<N\>.json | Checkpoint output |

The mini-batch size may not exceed the batch size. `--resume` restores the iteration counter and
warns when the checkpoint's `max_tiers` or seed differ.

The recommended run (one model for every non-tone preset and both tasks; `--workers` can be left
out to use all cores):

```
indicatrix-trainer-cli --algo imitation --mode retarget --tier m --iterations 300 --workers 16 \
    --batch-size 2048 --mini-batch 256 --epochs 2 --label-every 2 --objectives mixed \
    --validation-designs 16 --validation-every 10 --eval-test --save-stats retarget_m_imitation1.json
```

Watch `val_improvement` against `val_improvement_oracle` (the ratio above 0.8 is the goal),
`train_cosine` above 0.7 within 50 rounds, a falling `sens_mse`, and `val_evaluations_per_step`
between 1 and 2.5.

## Checkpoints

Schema-versioned JSON (`Checkpoint`, schema 3): mode, tier, observation layout version, iteration,
seed and the network. A file of an older schema (written before the sensitivity heads) is refused.
Written to a temporary file and renamed. `--resume` and the GUI loader refuse a different mode,
tier or observation layout. The default name is `indicatrix-<mode>-<tier>-iter<N>.json`. Adam
moments are not stored; they restart after a load or a backend switch.

## Telemetry

`TrainingRunStats` (JSON, RFC 3339 timestamps) holds the metrics history, recent rewards, log and
final optics. The run record names the algorithm (`algo`), the label period (`label_every`), the
objective sampling (`objectives`) and the exploration noise (`explore_std`). Each metrics point
adds `imitation_loss`, `sens_mse`, `train_cosine`, `gain_loss`, `labelled_samples` (aggregate
size), `labels_this_round`, `oracle_gain_mean`, `val_improvement_oracle`,
`val_evaluations_per_step` and the utilisation figures of "Where the time goes". The GUI imports
the file with "Import Run"; a file written by a different major version is refused. Files from
before these fields load with them at their defaults, and the removed teacher fields
(`teacher_cosine`, `teacher_loss`, `teacher_every`) are ignored when present.

## GUI worker protocol

`src/trainer_worker.rs` runs the loop on a thread, with the default algorithm (imitation). The UI
sends `TrainerCommand`s and receives `WorkerEvent`s: complete `ProgressSnapshot`s, log lines,
device-switch results and a death notice (panics are caught). Pause takes effect between
environment steps. Send `Shutdown` (after clearing the shared `is_active` flag) and join the
thread on exit. The window's loss readout ("Loss: Policy / Value") shows PPO figures and stays at
zero in an imitation run; the imitation loss, the direction cosine, the label counts and the
evaluation rate appear in the console log and in the exported telemetry, and each validation logs
the three drops.

The GUI asks once, in its own dialog, whether it may check GitHub for a newer release ("Check
Daily" / "Don't Check"); no request is made before the answer is yes, and the answer is kept in
`settings.json`. With the check on it asks at startup when a day has passed, on a background
thread, and shows a banner ("View Release", "Skip This Version", "Later") when a newer release
exists. The "Application" section of the setup column holds the switch and "Check Now". Nothing is
downloaded or installed; "View Release" only opens the release page in the browser.
