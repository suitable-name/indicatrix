# L1: deterministic CPU chain and engine contract

Status: implemented, source reviewed, not built or tested.

## Delivered

- `crates/indicatrix/src/renderer/engine.rs`: shared `RenderEngine`, `RayOptions`, `EngineSpec`, labels/index conversion, fixed tap choices, bounded targets, and denoise selection. Unsupported tap counts round upward to 1/4/8/16, capped at sixteen. Defaults remain Path-Traced and 8 taps with Fire on.
- `renderer/whitted/mod.rs`: module hub, public API, version/cutoff/worklist constants, and typed UV/fluorescence refusal with the UI sentence.
- `renderer/whitted/aa.rs`: constant centre-first 16-entry base-2/base-3 tap table and absolute tap wrapping.
- `renderer/whitted/fresnel.rs`: f32 unpolarized Fresnel, vector Snell refraction, TIR, fused dot/vector arithmetic, and reflection.
- `renderer/whitted/chain.rs`: deterministic Fresnel chain, exponential absorption, exact CSG side readback, per-facet diffuse frost return, primary-only glare, first-hit reporting, bounded depth/cutoff, and cavity re-entry.
- `renderer/whitted/frame.rs`: one-tap XYZ conversion/white balance and finite-sample interleaved sums. No fields were added to `FrameScene`; analytical metrics and the spectral tracer are untouched.
- `renderer/mod.rs` registers `engine` and `whitted`.
- `simd/exp_poly.rs` widens `exp_lane` to crate-private. `simd/mod.rs` re-exports it crate-privately because the enclosing `exp_poly` module is private. Root approved that necessary accessibility correction; exponential arithmetic is unchanged.

## Contract clarification

The plan's pseudocode can overfill a four-slot stack: an outside leaf can occupy its final slot before the internal continuation is pushed. The implementation reserves that final slot for the internal continuation. When three older leaves are pending, a new outside leaf resolves directly to the environment. This retains deterministic LIFO ordering and prevents a continuation from being lost or an array from overflowing. An unrun regression covers the reservation and fallback.

The preparation lane supplies `WhittedPrep`, `Chain`, `AlphaModel`, `WhittedEnv`, and the agreed test constructor. Frost is detected through `scene.facet_finishes`; the prepared radiance contains plane entries. Tools remain polished unless an explicit matching finish is supplied, consistent with the current scene contract.

## Added regression source

- Engine defaults, index fallback/roundtrips, and every u8 tap-count target.
- f64 Fresnel reference grid, textbook normal incidence, TIR, and Snell's law.
- Brilliant energy conservation with cutoff disabled and a large depth cap.
- Beer-Lambert slab transmission at three thicknesses, accounting for the shared 1e-4 surface restart.
- Concave cavity exit/re-entry side tracking and finite, energy-bounded chain output.
- Bounded-worklist overflow and diffuse frost entry return.
- AA table recomputation, centre-first/bounds/repetition, bit-exact pixel partitions at strides 1/3/7, and sequential absolute-tap accumulation.
- Uniform HDR white-furnace frame, primary-hit/miss behavior, and dispersive per-channel Snell-path recombination under directional HDR lighting.
- Ignored Windows-only 24x16 frame hash printer, awaiting the owner's Gate A value. No hash was invented or pinned.

## Validation and limits

Read the relevant plan sections, camera/hit/finish types, CSG intersection side contract, frame accumulation contract, Fresnel reference, material constructor, and HDR map constructors. Standalone `rustfmt --edition 2024 --config skip_children=true` parsed/formatted owned new files; it emitted only the existing stable-toolchain warnings about nightly formatting options. No builds, Cargo, tests, Clippy, or git commands were run.

All changed Rust files are below 800 lines: engine 138; whitted hub 54; AA 62; Fresnel 53; chain 296; frame 113; chain tests 234; frame tests 255; renderer hub 52; SIMD hub 101; scalar exponential 212. Preparation-lane files are reported separately.

The owner must run the new analytics and full existing regressions after integration. CPU/GPU numerical parity, speed budgets, and image pins remain unmeasured, as requested. No implementation blockers remain in L1.
