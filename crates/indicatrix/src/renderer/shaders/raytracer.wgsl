// QUARANTINED -- dead scaffolding, not a working GPU renderer. Never compiled (`fmod`
// isn't a WGSL builtin; WGSL uses `%`), but `cargo check` never parses it: it only
// reaches `include_str!` via `IndicatrixRaytracerPipeline::new` (renderer/pipeline.rs),
// which is never instantiated (`gpu` feature is `default = []`).
//
// Transcribed from an early design doc, not current `optics::raytracer`, and carries
// physics bugs since fixed there:
//   - symmetric-Gaussian CIE 1931 CMF fit (see `color::cie1931::cie_1931_cmf` for the
//     corrected piecewise-asymmetric fit)
//   - no interior-ray exit-facet handling
//   - Fresnel applied twice on the same interface
//   - inverted Beer-Lambert absorption
//   - no spectral MIS weighting
//
// Do not patch this file in place (e.g. `fmod` -> `%`) -- it would parse but still be
// wrong. Any real GPU port must be a fresh translation of current `optics::raytracer`,
// validated against it with a CPU/GPU equivalence harness. See also
// `renderer/buffers.rs`'s `DispersionParams` doc comment for a related layout bug.
