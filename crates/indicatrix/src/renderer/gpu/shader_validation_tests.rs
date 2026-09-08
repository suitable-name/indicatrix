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
    ("raytracer.wgsl", include_str!("../shaders/raytracer.wgsl")),
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
