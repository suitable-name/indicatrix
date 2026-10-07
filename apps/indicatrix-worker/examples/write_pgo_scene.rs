//! Writes a small, deterministic [`SceneState`] to a JSON file -- for scripts that need
//! a real `--scene` argument for `indicatrix-worker render` without hand-authoring the
//! wire format themselves.
//!
//! `scripts/pgo-bolt-build.sh`'s BOLT training stage is the one caller today: BOLT
//! profiles `render`'s deterministic, headless workload (see that script's own header
//! comment on why `render`, not `serve`/`join`, is what gets a scriptable BOLT profile),
//! and `render` always needs a real `--scene` file on disk.
//!
//! Same construction as [`capture_frame_payloads`]'s `scene` helper: the GUI's default
//! camera/light pose, a round brilliant diamond, studio lighting -- just serialized to
//! disk instead of traced in-process.
//!
//! [`capture_frame_payloads`]: https://docs.rs/indicatrix-worker (see that example's source)
//!
//! ```text
//! cargo run --release -p indicatrix-worker --features worker --example write_pgo_scene -- <output.json>
//! ```

use std::{env, error::Error, fs::File, io::BufWriter};

use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{BACKDROP_GREY, LightingPreset},
    },
};
use indicatrix_net::{SceneState, scene::SceneEnvironment};

/// Writes the fixed training scene to the path given as the sole command-line argument.
///
/// # Errors
///
/// Returns an error if no output path was given, the file can't be created, or
/// serialization fails.
fn main() -> Result<(), Box<dyn Error>> {
    let out = env::args()
        .nth(1)
        .ok_or("usage: write_pgo_scene <output.json>")?;

    let scene = SceneState {
        width: 640,
        height: 360,
        yaw: 0.60,
        pitch: 0.45,
        distance: 2.4,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 12,
        lighting_preset: LightingPreset::RingLights,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: BACKDROP_GREY,
        environment: SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
    };

    let file = File::create(&out)?;
    serde_json::to_writer_pretty(BufWriter::new(file), &scene)?;
    println!("wrote {out}");
    Ok(())
}
