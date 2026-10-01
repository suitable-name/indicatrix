//! Parses and validates every WGSL shader with `naga`, the same front end `wgpu`
//! uses at pipeline creation.
//!
//! The build script only concatenates the transport units; nothing validates WGSL
//! before a pipeline is created on a real device. These tests make a shader syntax
//! or type error a `cargo test` failure on any machine, GPU or not. The three
//! generated units are read from `OUT_DIR` exactly as `frame.rs` and
//! `transport_check` include them, so what is validated here is what ships.

use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

/// Every shader the crate compiles, by the name a failure should report.
const SHADERS: &[(&str, &str)] = &[
    (
        "spectral_transport.generated.wgsl",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/spectral_transport.generated.wgsl"
        )),
    ),
    (
        "transport_functions.generated.wgsl",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/transport_functions.generated.wgsl"
        )),
    ),
    (
        "wavefront_transport.generated.wgsl",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/wavefront_transport.generated.wgsl"
        )),
    ),
    (
        "camera_ray.wgsl",
        include_str!("../shaders/camera_ray.wgsl"),
    ),
    (
        "environment.wgsl",
        include_str!("../shaders/environment.wgsl"),
    ),
    ("furnace.wgsl", include_str!("../shaders/furnace.wgsl")),
    (
        "intersect_polyhedron.wgsl",
        include_str!("../shaders/intersect_polyhedron.wgsl"),
    ),
    (
        "layout_echo.wgsl",
        include_str!("../shaders/layout_echo.wgsl"),
    ),
    (
        "phase1_layout_echo.wgsl",
        include_str!("../shaders/phase1_layout_echo.wgsl"),
    ),
    (
        "phase2_layout_echo.wgsl",
        include_str!("../shaders/phase2_layout_echo.wgsl"),
    ),
    (
        "reduce_xyz.wgsl",
        include_str!("../shaders/reduce_xyz.wgsl"),
    ),
    (
        "rng_equivalence.wgsl",
        include_str!("../shaders/rng_equivalence.wgsl"),
    ),
    (
        "self_determinism.wgsl",
        include_str!("../shaders/self_determinism.wgsl"),
    ),
    (
        "shading_normal.wgsl",
        include_str!("../shaders/shading_normal.wgsl"),
    ),
];

fn validate(name: &str, source: &str) -> Result<(), String> {
    let module =
        wgsl::parse_str(source).map_err(|e| format!("{name}: {}", e.emit_to_string(source)))?;
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .map(|_| ())
        .map_err(|e| format!("{name}: {}", e.emit_to_string(source)))
}

#[test]
fn every_shader_parses_and_validates() {
    let failures: Vec<String> = SHADERS
        .iter()
        .filter_map(|(name, source)| validate(name, source).err())
        .collect();
    assert!(
        failures.is_empty(),
        "shader validation failed:
{}",
        failures.join(
            "

"
        )
    );
}

// ---------------------------------------------------------------------------------
// Duplicate-function coverage: `environment.wgsl` (the standalone environment-sampling
// self-test target) is a hand-maintained translation of several functions that ALSO
// live, under the identical name, in the megakernel's shared pieces
// (`transport_physics`/`transport_bounce`) -- e.g. `cie_1931_cmf`, `sample_studio_rig`,
// `studio_dispatch`. Pinning the copies textually equal means a fix applied to one copy
// cannot silently drift from the other (the standalone self-test would keep passing
// against its own stale copy).
// ---------------------------------------------------------------------------------

/// Every `transport_physics/*.wgsl` piece, by the name `build.rs`'s
/// `TRANSPORT_PHYSICS_PIECES` gives it.
const TRANSPORT_PHYSICS_PIECE_SOURCES: &[(&str, &str)] = &[
    (
        "transport_physics/01_prelude_polarization.wgsl",
        include_str!("../shaders/transport_physics/01_prelude_polarization.wgsl"),
    ),
    (
        "transport_physics/02_biaxial_eigenmodes.wgsl",
        include_str!("../shaders/transport_physics/02_biaxial_eigenmodes.wgsl"),
    ),
    (
        "transport_physics/03_dispersion_absorption_frosted.wgsl",
        include_str!("../shaders/transport_physics/03_dispersion_absorption_frosted.wgsl"),
    ),
    (
        "transport_physics/04_scattering.wgsl",
        include_str!("../shaders/transport_physics/04_scattering.wgsl"),
    ),
    (
        "transport_physics/05_nee_env_sampling.wgsl",
        include_str!("../shaders/transport_physics/05_nee_env_sampling.wgsl"),
    ),
    (
        "transport_physics/06_uniaxial_fullwave_complex.wgsl",
        include_str!("../shaders/transport_physics/06_uniaxial_fullwave_complex.wgsl"),
    ),
    (
        "transport_physics/07_uniaxial_fullwave_solve.wgsl",
        include_str!("../shaders/transport_physics/07_uniaxial_fullwave_solve.wgsl"),
    ),
    (
        "transport_physics/08_exit_splitting.wgsl",
        include_str!("../shaders/transport_physics/08_exit_splitting.wgsl"),
    ),
    (
        "transport_physics/09_lighting_presets.wgsl",
        include_str!("../shaders/transport_physics/09_lighting_presets.wgsl"),
    ),
];

/// Every `transport_bounce/*.wgsl` piece, by the name `build.rs`'s
/// `TRANSPORT_BOUNCE_PIECES` gives it.
const TRANSPORT_BOUNCE_PIECE_SOURCES: &[(&str, &str)] = &[
    (
        "transport_bounce/01_header_and_constants.wgsl",
        include_str!("../shaders/transport_bounce/01_header_and_constants.wgsl"),
    ),
    (
        "transport_bounce/02_scene_bindings.wgsl",
        include_str!("../shaders/transport_bounce/02_scene_bindings.wgsl"),
    ),
    (
        "transport_bounce/03_rng_and_spectral_tables.wgsl",
        include_str!("../shaders/transport_bounce/03_rng_and_spectral_tables.wgsl"),
    ),
    (
        "transport_bounce/04_studio_and_hdr_environment.wgsl",
        include_str!("../shaders/transport_bounce/04_studio_and_hdr_environment.wgsl"),
    ),
    (
        "transport_bounce/05_intersect_and_edge_rounding.wgsl",
        include_str!("../shaders/transport_bounce/05_intersect_and_edge_rounding.wgsl"),
    ),
    (
        "transport_bounce/06_nee_sampling.wgsl",
        include_str!("../shaders/transport_bounce/06_nee_sampling.wgsl"),
    ),
    (
        "transport_bounce/07_camera_and_mode_selection.wgsl",
        include_str!("../shaders/transport_bounce/07_camera_and_mode_selection.wgsl"),
    ),
    (
        "transport_bounce/08_bounce_step.wgsl",
        include_str!("../shaders/transport_bounce/08_bounce_step.wgsl"),
    ),
    (
        "transport_bounce/09_finalize_and_ray_gen.wgsl",
        include_str!("../shaders/transport_bounce/09_finalize_and_ray_gen.wgsl"),
    ),
];

/// Every `transport_functions/*.wgsl` piece, by the name `build.rs`'s
/// `TRANSPORT_FUNCTIONS_PIECES` gives it.
const TRANSPORT_FUNCTIONS_PIECE_SOURCES: &[(&str, &str)] = &[
    (
        "transport_functions/01_polarization_fresnel_tir.wgsl",
        include_str!("../shaders/transport_functions/01_polarization_fresnel_tir.wgsl"),
    ),
    (
        "transport_functions/02_dispersion_absorption_uniaxial_index.wgsl",
        include_str!("../shaders/transport_functions/02_dispersion_absorption_uniaxial_index.wgsl"),
    ),
    (
        "transport_functions/03_frosted_and_scattering.wgsl",
        include_str!("../shaders/transport_functions/03_frosted_and_scattering.wgsl"),
    ),
    (
        "transport_functions/04_biaxial_and_scatter_extinguish.wgsl",
        include_str!("../shaders/transport_functions/04_biaxial_and_scatter_extinguish.wgsl"),
    ),
    (
        "transport_functions/05_uniaxial_fullwave_and_exit_splitting.wgsl",
        include_str!("../shaders/transport_functions/05_uniaxial_fullwave_and_exit_splitting.wgsl"),
    ),
    (
        "transport_functions/06_assigned_mode_and_balance.wgsl",
        include_str!("../shaders/transport_functions/06_assigned_mode_and_balance.wgsl"),
    ),
    (
        "transport_functions/07_env_distribution_sampling.wgsl",
        include_str!("../shaders/transport_functions/07_env_distribution_sampling.wgsl"),
    ),
    (
        "transport_functions/08_nee_and_intersect.wgsl",
        include_str!("../shaders/transport_functions/08_nee_and_intersect.wgsl"),
    ),
];

const ENVIRONMENT_WGSL_SOURCE: &str = include_str!("../shaders/environment.wgsl");
const ENVIRONMENT_WGSL_ENTRY: (&str, &str) = ("environment.wgsl", ENVIRONMENT_WGSL_SOURCE);

/// Deletes `// ...` line comments (this codebase's WGSL has no `/* */` block comments
/// and no string literals containing `//`, checked), so a comment-only edit to one copy
/// of a duplicated function never fails the identity check below.
fn strip_line_comments(src: &str) -> String {
    src.lines()
        .map(|line| line.find("//").map_or(line, |i| &line[..i]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Lowercases WGSL hex-literal digits (`0x2545F491u` vs `0x2545f491u`) so the same
/// physics constant written with different literal case in two copies still compares
/// equal; every other character (including identifiers, which WGSL treats as
/// case-sensitive) is left untouched.
fn lowercase_hex_literals(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '0' && matches!(chars.peek(), Some('x' | 'X')) {
            chars.next();
            out.push('0');
            out.push('x');
            while let Some(&d) = chars.peek() {
                if d.is_ascii_hexdigit() {
                    out.push(d.to_ascii_lowercase());
                    chars.next();
                } else {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Comment-stripped, hex-lower-cased, trailing-whitespace-trimmed form of a `{ ... }`
/// function body, so two copies that differ only in a comment, a literal's hex case,
/// or trailing whitespace still compare equal -- see [`strip_line_comments`] and
/// [`lowercase_hex_literals`] for what is (and, by omission, is not) normalised away.
///
/// There is deliberately NO alias table here: `environment.wgsl` is compiled standalone
/// (Phase 1's self-contained test target) and cannot reuse the megakernel pieces'
/// symbols, so it carries its own copies of e.g. `asymmetric_gaussian` and
/// `rgb_to_spectral_radiance` -- under the SAME names as
/// `transport_physics/05_nee_env_sampling.wgsl`, precisely so that
/// [`same_named_functions_agree_everywhere_they_are_defined`] compares the two bodies
/// and a formula change to either copy fails the test until it is mirrored. (Until
/// 2026-09-28 the `environment.wgsl` copies were `hdr_`-prefixed and an alias table
/// papered over the different callee name in `hdr_env_radiance_at`, which meant the
/// helpers themselves were never compared.)
fn normalize_wgsl_body(body: &str) -> String {
    let stripped = strip_line_comments(body);
    let lowered = lowercase_hex_literals(&stripped);
    lowered
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Extracts every top-level `fn NAME(...) { body }` from `src`, as `(name, normalized
/// body)` pairs. Only matches a `fn` keyword at column 0 (true of every function this
/// crate's WGSL pieces define -- checked), and finds the body's opening brace as the
/// first `{` after the argument list's parentheses balance back to zero, then the
/// matching closing brace by counting `{`/`}` from there. Comments are stripped FIRST
/// (many of this codebase's header comments quote a `path::{a, b}`-style Rust import
/// list, whose braces would otherwise desync the brace count) -- see
/// [`strip_line_comments`]. WGSL has no block comments and no string literals
/// containing braces in this codebase, and no nested `fn`, so a plain brace count over
/// the comment-stripped text cannot be confused by either.
fn extract_top_level_fns(src: &str) -> Vec<(String, String)> {
    let src = strip_line_comments(src);
    let padded = format!("\n{src}");
    let bytes = padded.as_bytes();
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(off) = padded[pos..].find("\nfn ") {
        let start = pos + off + 1;
        let name_start = start + 3;
        let Some(paren_rel) = padded[name_start..].find('(') else {
            break;
        };
        let name = padded[name_start..name_start + paren_rel]
            .trim()
            .to_string();
        let mut i = name_start + paren_rel;
        let mut paren_depth = 0i32;
        loop {
            match bytes[i] {
                b'(' => paren_depth += 1,
                b')' => {
                    paren_depth -= 1;
                    if paren_depth == 0 {
                        i += 1;
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let Some(brace_rel) = padded[i..].find('{') else {
            break;
        };
        let body_start = i + brace_rel;
        let mut j = body_start;
        let mut brace_depth = 0i32;
        loop {
            match bytes[j] {
                b'{' => brace_depth += 1,
                b'}' => {
                    brace_depth -= 1;
                    if brace_depth == 0 {
                        j += 1;
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        out.push((name, normalize_wgsl_body(&padded[body_start..j])));
        pos = j;
    }
    out
}

/// Every top-level function defined across `transport_physics/*`, `transport_bounce/*`,
/// `transport_functions/*`, and `environment.wgsl`, as `(function name, source label,
/// normalized body)` -- the input population [`same_named_functions_agree_everywhere_
/// they_are_defined`] groups by name.
fn all_known_fn_definitions() -> Vec<(String, &'static str, String)> {
    let mut all = Vec::new();
    for &(label, src) in TRANSPORT_PHYSICS_PIECE_SOURCES
        .iter()
        .chain(TRANSPORT_BOUNCE_PIECE_SOURCES)
        .chain(TRANSPORT_FUNCTIONS_PIECE_SOURCES)
        .chain(std::iter::once(&ENVIRONMENT_WGSL_ENTRY))
    {
        for (name, body) in extract_top_level_fns(src) {
            all.push((name, label, body));
        }
    }
    all
}

/// Pins every function NAME that is defined more than once across
/// `transport_physics/*`, `transport_bounce/*`, `transport_functions/*`, and
/// `environment.wgsl` to have the IDENTICAL body (modulo comments/hex-case/trailing
/// whitespace) everywhere it appears -- e.g. `environment.wgsl`'s own
/// `sample_studio_rig`/`studio_dispatch`/`cie_1931_cmf`/... against the megakernel
/// pieces' copies of the same functions. See this file's header comment for why
/// `environment.wgsl` is in this set at all (it duplicates rather than shares).
///
/// A name defined exactly once (most of them -- these pieces are split by
/// responsibility precisely so most functions have ONE home) is not compared against
/// anything and cannot fail this test; the canary assertion at the end guards against
/// the extraction itself silently matching nothing.
#[test]
fn same_named_functions_agree_everywhere_they_are_defined() {
    let mut by_name: std::collections::BTreeMap<String, Vec<(&'static str, String)>> =
        std::collections::BTreeMap::new();
    for (name, label, body) in all_known_fn_definitions() {
        by_name.entry(name).or_default().push((label, body));
    }

    let mut duplicated_names = 0usize;
    let mut failures = Vec::new();
    for (name, defs) in &by_name {
        if defs.len() < 2 {
            continue;
        }
        duplicated_names += 1;
        let (first_label, first_body) = &defs[0];
        for (label, body) in &defs[1..] {
            if body != first_body {
                failures.push(format!(
                    "`{name}` differs between {first_label} and {label} (after \
                     stripping comments, lower-casing hex literals, and trimming \
                     trailing whitespace)"
                ));
            }
        }
    }

    assert!(
        duplicated_names >= 15,
        "expected at least 15 function names to be defined in more than one of \
         transport_physics/*, transport_bounce/*, transport_functions/*, and \
         environment.wgsl (got {duplicated_names}) -- if this legitimately dropped, \
         update the number; if it is 0, `extract_top_level_fns` most likely stopped \
         matching anything and this test is now vacuous"
    );
    assert!(
        failures.is_empty(),
        "duplicated WGSL functions drifted apart:
{}",
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------------
// Stream-salt coverage: `rng_check`'s GPU/CPU equivalence self-test (`renderer::gpu::rng_check`)
// needs a live GPU adapter, so it never runs under plain `cargo test`, and it only
// hashes against 9 of the 13 real per-bounce stream salts -- DISTANCE_SAMPLE_STREAM,
// PHASE_DIR_U/V_STREAM, NEE_ENV_DIR_U/V_STREAM, and FROSTED_NEE_ENV_DIR_U/V_STREAM are
// not among them. This is a text-level stand-in that needs no adapter: parse each of
// those 7 constants' WGSL literal out of the shared shader source and compare it to the
// real CPU constant in `optics::raytracer::sampling`.
// ---------------------------------------------------------------------------------

/// Parses `const NAME: u32 = 0xHEXu;` (or a decimal literal) for `name` out of `src`.
fn wgsl_u32_const(src: &str, name: &str) -> Option<u32> {
    let needle = format!("const {name}:");
    let start = src.find(&needle)? + needle.len();
    let eq = src[start..].find('=')? + start + 1;
    let end = src[eq..].find(';')? + eq;
    let literal = src[eq..end].trim().trim_end_matches('u').trim();
    literal
        .strip_prefix("0x")
        .or_else(|| literal.strip_prefix("0X"))
        .map_or_else(
            || literal.parse().ok(),
            |hex| u32::from_str_radix(hex, 16).ok(),
        )
}

#[test]
fn distance_and_nee_stream_salts_match_between_cpu_and_wgsl() {
    use crate::optics::raytracer::sampling::{
        DISTANCE_SAMPLE_STREAM, FROSTED_NEE_ENV_DIR_U_STREAM, FROSTED_NEE_ENV_DIR_V_STREAM,
        NEE_ENV_DIR_U_STREAM, NEE_ENV_DIR_V_STREAM, PHASE_DIR_U_STREAM, PHASE_DIR_V_STREAM,
    };

    // All 9 `transport_physics` pieces concatenated: these 7 constants are declared in
    // `01_prelude_polarization.wgsl` and `04_scattering.wgsl`, but concatenating every
    // piece means this test does not need updating if a future edit moves one.
    let physics_concat: String = TRANSPORT_PHYSICS_PIECE_SOURCES
        .iter()
        .map(|&(_, src)| src)
        .collect();

    let cases: &[(&str, u32)] = &[
        ("DISTANCE_SAMPLE_STREAM", DISTANCE_SAMPLE_STREAM),
        ("PHASE_DIR_U_STREAM", PHASE_DIR_U_STREAM),
        ("PHASE_DIR_V_STREAM", PHASE_DIR_V_STREAM),
        ("NEE_ENV_DIR_U_STREAM", NEE_ENV_DIR_U_STREAM),
        ("NEE_ENV_DIR_V_STREAM", NEE_ENV_DIR_V_STREAM),
        ("FROSTED_NEE_ENV_DIR_U_STREAM", FROSTED_NEE_ENV_DIR_U_STREAM),
        ("FROSTED_NEE_ENV_DIR_V_STREAM", FROSTED_NEE_ENV_DIR_V_STREAM),
    ];
    for &(name, cpu_value) in cases {
        let wgsl_value = wgsl_u32_const(&physics_concat, name).unwrap_or_else(|| {
            panic!("{name}: not found as a `const {name}: u32 = ...;` in transport_physics/*.wgsl")
        });
        assert_eq!(
            wgsl_value, cpu_value,
            "{name}: WGSL literal 0x{wgsl_value:08x} disagrees with CPU \
             optics::raytracer::sampling::{name} (0x{cpu_value:08x})"
        );
    }
}
