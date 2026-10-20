# indicatrix-trainer

Standalone Slint GUI and headless CLI for training the two `indicatrix-ai` policies. Training is
not wired into the editor, web app, mobile app, or worker services. A trained checkpoint must be
integrated separately. The GUI and CLI share the trainer loop and model format.

## Tasks and data

The two tasks have distinct checkpoint identities: **Optimizer** changes tier angles while
scoring in the sampled source material; **Retarget** scores in a target material and applies the
retarget preservation policy. Retarget runs may include same-material episodes, but those are a
robustness mixture and do not certify cross-material capability. Loaders reject a checkpoint
whose mode, tier, or observation layout does not match.

Scores are lower-is-better and combine optical quality, measured yield when available, optional
tone goals, and sampled shape context. Seven objective presets are trainable: Balanced,
Brilliance, Low Windowing, Low Extinction, Keep Weight, Lighten Dark Rough, and Intensify Pale
Rough. The two tone tasks use catalog materials with real absorption; episode-seeded material,
lighting, and shape context are recorded in evaluation case artifacts.

The CLI reads the design library read-only through `DIAGRAM_DB_PATH`, or searches the working
directory and up to three parents for `facet_diagrams.sqlite`. Invalid or oversized designs are
reported as skips. Records sharing an entry ID or normalized geometry are connected into one
group before train/validation/test assignment, preventing attachments or duplicate cuts from
crossing splits. The split seed is recorded. `--fixture` is a smoke test that deliberately shares
the same fixture across splits; it is not evidence of generalization and cannot produce a
best-validation checkpoint.

## Training behavior

The model predicts seven sensitivity components per tier: windowing, extinction, brilliance loss,
yield loss, lighter-tone loss, deeper-tone loss, and shape distance. Unavailable labels are masked.
The shared observation includes the seven normalized objective coefficients and source/target
material, lighting, geometry, task, and fidelity context. Observation widths are exported by the
AI crate; the trainer does not duplicate their numeric dimensions.

With `--algo imitation`, labelled states are appended to a bounded aggregate and optimized by
mini-batches. The default `--updates-per-round` bounds work per update; `0` retains full-aggregate
epoch behavior. A deterministic bounded preparation path streams oversized full epochs in
windows instead of silently reducing requested training work. The imitation update intentionally
trains on the previous rollout while the next one is collected. With `--algo ppo`, the policy
trains on scalar rollout rewards. `--label-every` can still request teacher probes for diagnostics
in PPO mode, but those labels do not enter PPO's loss and add environment work.

Environment slots are capped by rollout size and are scheduled on one persistent bounded Rayon
pool. Inner line-search work stays serial, so slot work does not multiply native threads. The
banner and telemetry report configured workers, effective slots, actual pool width, backend, and
debug-build status. Timings are host phase measurements, not GPU utilization: they include rollout
forward, data packing, update bodies, and weight export, with asynchronous device work potentially
crossing phase boundaries. Component support counts show how many selected labels train each of
the seven heads.

Training and validation fidelity are separate CLI settings. Validation uses a fixed cohort;
`--objective-sweep` evaluates each selected design under all seven named objectives. Coverage
reports show missing objectives and spectrum materials rather than implying that a small random
cohort covered every material. `--classical-baseline N` optionally runs the public coordinate
optimizer with a separate nominal per-design candidate budget (a candidate pair can exceed it),
task-appropriate retarget preservation,
and the same material, objective, shape, and lighting context. Its result and time are recorded
separately in the evaluation sidecar.

## Progress, artifacts, and checkpoints

The CLI emits a phase heartbeat while initialization, rollout/update, or validation is still
running. By default it atomically saves stats and a numbered checkpoint every 10 iterations, and
also writes a recovery save before each validation. `--save-every 0` disables the regular cadence;
pre-validation and final saves remain. Periodic checkpoint names use the directory and stem of
`--save-weights`. The exact requested checkpoint path receives the final weights. Stats are saved
before final test evaluation, so a test failure does not erase the run output. A sibling
`<stats-stem>-evaluations.json` stores per-design validation and test contexts, scores, failures,
and optional comparator measurements.

`--resume PATH` warm-starts weights and carries the checkpoint iteration counter; it is not an
exact training resume. Mode and tier are checked, but optimizer state, replay aggregate, RNG state,
and reward normalizer are freshly initialized. Checkpoints use the AI crate's strict schema v4 and
observation layout.

## Native CUDA (Linux CLI)

CUDA is an explicit opt-in backend and does not participate in `--device auto`; Auto keeps its
existing WGPU-then-CPU behavior. `--device gpu` and `--device wgpu` select the existing WGPU
backend. Native CUDA explicitly targets CUDA device index 0 and requires the trainer's `cuda` Cargo feature, a matching NVIDIA driver, and
the CUDA toolkit/development environment used by the pinned `cudarc 0.19.10` and
`cubecl-cuda 0.11` dependencies. Ensure `nvcc` is on `PATH`; set `CUDA_PATH` when the toolkit is
installed in a nonstandard location. `cudarc` detects `nvcc --version` and has its own fallback
when `nvcc` is unavailable, so install the toolkit rather than relying on that fallback. The
CubeCL CUDA backend defaults to its bundled LLVM compiler path, which needs a C++ compiler at build
time but not a system LLVM installation. The CUDA toolkit and libdevice must remain available at
runtime: CubeCL searches `CUDA_PATH`, `/usr/local/cuda`, `/opt/cuda`, `/usr/lib/cuda`, and `/usr`
for `nvvm/libdevice/libdevice.10.bc`. Install the toolkit for both build and runtime. CUDA is used
only by neural updates and batched rollout forwards; environment solving, geometry, label
generation, and evaluation remain CPU work, so high GPU utilization is not guaranteed. The reported
device name, CLI banner, telemetry, tier changes, and update/rollout trainers retain the actual CUDA
backend label. Checkpoints remain backend-independent. The existing GUI
switch only selects between CPU and WGPU; CUDA is treated as GPU by the switch, so toggling from
CUDA moves to CPU.

From `apps/indicatrix-trainer`, build the standalone CLI with CUDA enabled on a Linux CUDA host:

```sh
cargo build --release --features cuda --bin indicatrix-trainer-cli
```

For the long retarget imitation run, add explicit CUDA selection and preserve periodic artifacts:

```sh
./target/release/indicatrix-trainer-cli --algo imitation --mode retarget --tier m --device cuda \
  --iterations 300 --workers 64 --batch-size 4096 --mini-batch 256 --epochs 2 \
  --objectives mixed --max-tiers 64 --validation-designs 16 --validation-every 10 \
  --eval-test --save-every 2 --save-stats retarget_m_imitation.json \
  --save-weights retarget_m_imitation.ckpt.json
```

To use PPO on the same retarget task, change the algorithm and choose whether to pay for teacher
diagnostic labels; those labels do not enter PPO's loss:

```sh
./target/release/indicatrix-trainer-cli --algo ppo --mode retarget --tier m --device cuda \
  --iterations 300 --workers 64 --batch-size 4096 --mini-batch 256 --epochs 2 \
  --label-every 2 --objectives mixed --max-tiers 64 --validation-designs 16 \
  --validation-every 10 --eval-test --save-every 2 \
  --save-stats retarget_m_ppo.json --save-weights retarget_m_ppo.ckpt.json
```

For optimizer imitation training, select the optimizer mode and keep separate task artifacts:

```sh
./target/release/indicatrix-trainer-cli --algo imitation --mode optimizer --tier m --device cuda \
  --iterations 300 --workers 64 --batch-size 4096 --mini-batch 256 --epochs 2 \
  --objectives mixed --max-tiers 64 --validation-designs 16 --validation-every 10 \
  --eval-test --save-every 2 --save-stats optimizer_m_imitation.json \
  --save-weights optimizer_m_imitation.ckpt.json
```

An explicit CUDA selection fails with a feature/runtime/device error instead of silently choosing
WGPU or CPU. The checkpoint format is backend-independent; a weights warm-start can use another
backend if that backend is selected and available.

Example for the CLI options used in a long retarget run:

```text
indicatrix-trainer-cli --algo ppo --mode retarget --tier m --iterations 300 --workers 64 \
  --batch-size 4096 --mini-batch 256 --epochs 2 --label-every 2 --objectives mixed \
  --max-tiers 64 --validation-designs 16 --validation-every 10 --eval-test \
  --save-every 2 --save-stats retarget_m_ppo.json --save-weights retarget_m_ppo.ckpt.json
```

Training quality should be assessed from fixed validation and final test cohorts, objective and
material coverage, per-case outcomes, component support, and the chosen endpoint fidelity. This
README sets no numeric success threshold; quality depends on the corpus, task mixture, fidelity,
and measured evaluation coverage.
