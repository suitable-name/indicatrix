# L2: Core Preparation

Implemented the fixed section 5 preparation contract for Ray-Traced: one or three refractive-index chains, direction-sensitive RGB absorption calibrated at the face-up path, Studio/ASET/HDR environment sampling, primary backdrop, white balance, surface glare and fixed spherical frost quadrature. UV lamps and fluorescence return the shared refusal.

Added the HDR-gated `whitted_gallery` example. It writes each built-in material under Light Tent, Ring Lights and Isotropic Hemisphere, pairing an 8-tap Ray-Traced image with a 256-spp CPU Path-Traced image at 256 × 256, plus an HTML index. Geometry, camera, depth and output encoding are shared.

## Files And Lines

| File | Lines |
|---|---:|
| `crates/indicatrix/src/renderer/whitted/prep.rs` | 156 |
| `crates/indicatrix/src/renderer/whitted/tint.rs` | 51 |
| `crates/indicatrix/src/renderer/whitted/env.rs` | 142 |
| `crates/indicatrix/src/renderer/whitted/frost.rs` | 39 |
| `crates/indicatrix/src/renderer/whitted/tests_prep.rs` | 207 |
| `crates/indicatrix/examples/whitted_gallery.rs` | 86 |
| `crates/indicatrix/Cargo.toml` | 157 |

Line counts are before lead formatting. The existing Cargo file retains CRLF. All new Rust files use LF. No existing tracer, metrics, environment or optical-overlay implementation was edited. No AGENTS.md was found by `rg --files -g AGENTS.md`.

## Additive Contract Correction

`WhittedPrep::into_owned(self) -> WhittedPrep<'static>` and `WhittedEnv::into_owned(self) -> WhittedEnv<'static>` safely support a long-lived preparation cache beside an owned scene. A borrowed HDR map is cloned into an Arc once at detachment; cached taps/chunks perform no clone. This avoids unsafe self-reference without changing the specified `new` constructor or fields.

The spec-power Studio wrapper hardcodes the default head-shadow cone. For default shadows this exact wrapper is used. For an authored override, the existing explicit-shadow wrapper at 550 nm is divided by the preset power at 550 nm to recover its wavelength-independent geometric factor. This preserves the real override without editing the oversized environment module. The independent spectral-integral test covers off, default and wide shadows.

The fixed constructor has no tool-primitive input. Frost preparation therefore covers plane facets, which is also the documented shape of `FrameScene::facet_finishes`. Tool facets remain polished unless a future contract supplies their normals and finishes.

## Added Tests (Unrun)

- Independent 5-nm spectral tint anchor for Ruby, Sapphire, Emerald, Tanzanite and Peridot under D65 and Incandescent; each mode checked separately, with the prescribed biaxial coefficient mean.
- Uniaxial axis and colorless material.
- Studio separation for every non-ASET/non-UV preset and three head-shadow sizes; ASET table against independent spectral integration.
- HDR radiance and detached ownership, backdrop and Incandescent white balance.
- UV/fluorescence declines, uniform-environment frost quadrature, and fire chain-count rules.

L1 covers slab Beer-Lambert transport and the Fire Snell recombination test.

## Verification Limits

Source inspection verified `FrameScene`, `FacetFinish`, `GpuFacetPlane::normal`, `GemMaterial` absorption/index constructors, `Fluorescence` and emitter APIs, `ColorSpace` row-major matrices, the CMF table, Studio light wrappers and shadow behavior, cloneable `EnvironmentMap`, and PNG encoding signatures. The gallery and tests have not been built or run. No builds, cargo commands, tests, Clippy or git commands were run.
