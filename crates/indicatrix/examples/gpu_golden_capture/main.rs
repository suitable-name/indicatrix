//! GPU Golden Capture Tool (`gpu_golden_capture`).
//!
//! Captures and compares GPU frame outputs across kernel revisions according to the §7.2
//! golden rule (G-D10).
//!
//! # Usage
//!
//! Capture baseline or after-change golden files:
//! ```text
//! cargo run --profile probe -p indicatrix --features gpu --example gpu_golden_capture -- \
//!     capture --out target/gpu_golden/baseline
//! ```
//!
//! Compare two golden directories:
//! ```text
//! cargo run --profile probe -p indicatrix --features gpu --example gpu_golden_capture -- \
//!     compare target/gpu_golden/baseline target/gpu_golden/after_l5
//! ```
//!
//! `compare` reads both `meta.json` files and exits 2 when the adapter, the scene
//! parameters or (within one pipeline) the shader hash differ; `--allow-mismatch`
//! overrides. NaN or Inf anywhere in either buffer is always a FAIL.

use std::path::{Path, PathBuf};

use glam::Vec3;
use indicatrix::renderer::gpu::{
    GpuContext, GpuFrameRenderer, GpuPipelineKind,
    event_wavefront::{EventWavefrontRenderer, KERNEL_SET_HASH},
};

mod compare;
mod meta;
mod scenes;

use compare::{
    ComparisonResult, Verdict, compare_buffers, format_comparison_table, write_f32_buffer,
};
use scenes::{SceneBundle, all_scenes};

/// Generated WGSL source for the spectral transport megakernel.
const SHADER_SRC: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/spectral_transport.generated.wgsl"
));

/// Fowler-Noll-Vo 1a 64-bit hash.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET_BASIS;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

const USAGE: &str = "Usage:
  gpu_golden_capture capture --out <dir> [--pipeline <megakernel|event>] [--date <str>]
  gpu_golden_capture compare <dir_a> <dir_b> [--allow-mismatch]

compare refuses (exit 2) when the two meta.json files show a different adapter, different
scene parameters, or (within one pipeline) a different shader hash, unless --allow-mismatch.";

enum CliCommand {
    Capture {
        out_dir: PathBuf,
        pipeline: String,
        date: Option<String>,
    },
    Compare {
        path_a: PathBuf,
        path_b: PathBuf,
        allow_mismatch: bool,
    },
}

fn parse_cli(mut args: impl Iterator<Item = String>) -> Result<CliCommand, String> {
    let cmd = args
        .next()
        .ok_or_else(|| "missing command (expected 'capture' or 'compare')".to_string())?;

    match cmd.as_str() {
        "capture" => {
            let mut out_dir: Option<PathBuf> = None;
            let mut pipeline = "megakernel".to_string();
            let mut date: Option<String> = None;

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--out" => {
                        let val = args
                            .next()
                            .ok_or_else(|| "--out requires a directory path".to_string())?;
                        out_dir = Some(PathBuf::from(val));
                    }
                    "--pipeline" => {
                        let val = args.next().ok_or_else(|| {
                            "--pipeline requires an argument ('megakernel' or 'event')".to_string()
                        })?;
                        pipeline = val;
                    }
                    "--date" => {
                        let val = args
                            .next()
                            .ok_or_else(|| "--date requires an argument".to_string())?;
                        date = Some(val);
                    }
                    other => {
                        return Err(format!("unknown option '{other}' for capture"));
                    }
                }
            }

            let out_dir =
                out_dir.ok_or_else(|| "missing required argument '--out <dir>'".to_string())?;
            Ok(CliCommand::Capture {
                out_dir,
                pipeline,
                date,
            })
        }
        "compare" => {
            let mut positional: Vec<String> = Vec::new();
            let mut allow_mismatch = false;
            for arg in args {
                if arg == "--allow-mismatch" {
                    allow_mismatch = true;
                } else if arg.starts_with("--") {
                    return Err(format!("unknown option '{arg}' for compare"));
                } else {
                    positional.push(arg);
                }
            }
            let mut positional = positional.into_iter();
            let path_a = positional
                .next()
                .ok_or_else(|| "missing first argument <a> for compare".to_string())?;
            let path_b = positional
                .next()
                .ok_or_else(|| "missing second argument <b> for compare".to_string())?;
            if positional.next().is_some() {
                return Err("compare takes exactly two paths".to_string());
            }
            Ok(CliCommand::Compare {
                path_a: PathBuf::from(path_a),
                path_b: PathBuf::from(path_b),
                allow_mismatch,
            })
        }
        "help" | "-h" | "--help" => Err(USAGE.to_string()),
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
}

fn get_date_string(date_override: Option<String>) -> String {
    date_override.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or_else(
                |_| "unknown-date".to_string(),
                |dur| format!("unix-timestamp-{}", dur.as_secs()),
            )
    })
}

fn write_meta_json(
    path: &Path,
    adapter_info: &wgpu::AdapterInfo,
    date: &str,
    pipeline: &str,
    shader_hash: u64,
    scenes: &[SceneBundle],
) -> std::io::Result<()> {
    let mut scene_json_entries = Vec::new();
    for s in scenes {
        scene_json_entries.push(format!(
            "    {{\n      \"name\": \"{}\",\n      \"file\": \"{}.f32\",\n      \"description\": \"{}\",\n      \"width\": {},\n      \"height\": {},\n      \"spp\": {},\n      \"max_bounces\": {},\n      \"sample_offset\": {},\n      \"material\": \"{}\"\n    }}",
            s.name, s.name, s.description, s.width, s.height, s.spp, s.max_bounces, s.sample_offset, s.material.name
        ));
    }

    let json = format!(
        "{{\n  \"adapter\": {{\n    \"name\": \"{}\",\n    \"vendor\": {},\n    \"device\": {},\n    \"device_type\": \"{:?}\",\n    \"backend\": \"{:?}\",\n    \"driver\": \"{}\",\n    \"driver_info\": \"{}\"\n  }},\n  \"date\": \"{}\",\n  \"pipeline\": \"{}\",\n  \"shader_src_fnv1a64\": \"0x{:016x}\",\n  \"scenes\": [\n{}\n  ]\n}}\n",
        adapter_info.name,
        adapter_info.vendor,
        adapter_info.device,
        adapter_info.device_type,
        adapter_info.backend,
        adapter_info.driver,
        adapter_info.driver_info,
        date,
        pipeline,
        shader_hash,
        scene_json_entries.join(",\n")
    );

    std::fs::write(path, json)
}

/// `true` for the event pipeline, `false` for the megakernel; exits 2 on any other name.
fn parse_pipeline(kind: &str) -> bool {
    match kind {
        "megakernel" => false,
        "event" => true,
        other => {
            eprintln!(
                "gpu_golden_capture: unknown pipeline kind '{other}', expected 'megakernel' or 'event'"
            );
            std::process::exit(2);
        }
    }
}

fn acquire_context() -> GpuContext {
    match GpuContext::acquire() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gpu_golden_capture: cannot acquire GPU context: {e}");
            std::process::exit(2);
        }
    }
}

/// The megakernel renderer. It acquires its own device, while `meta.json` records the
/// adapter of the capture's context; on a multi-GPU machine the two can differ, which would
/// make the baseline mislabelled, so that is warned about.
fn build_megakernel_renderer(adapter_info: &wgpu::AdapterInfo) -> GpuFrameRenderer {
    let mut r = match GpuFrameRenderer::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("gpu_golden_capture: cannot build frame renderer: {e}");
            std::process::exit(2);
        }
    };
    r.set_pipeline_kind(GpuPipelineKind::Megakernel);
    if !r.adapter_label().contains(adapter_info.name.as_str()) {
        eprintln!(
            "gpu_golden_capture: WARNING: the megakernel renderer uses adapter '{}' but meta.json will record '{}'; the capture is mislabelled. Re-run with a single GPU visible.",
            r.adapter_label(),
            adapter_info.name
        );
    }
    r
}

/// The hash `meta.json` records for the capture's pipeline.
fn pipeline_shader_hash(is_event: bool) -> u64 {
    if is_event {
        u64::from_str_radix(KERNEL_SET_HASH, 16)
            .expect("build.rs writes KERNEL_SET_HASH as 16 hex digits")
    } else {
        fnv1a_64(SHADER_SRC.as_bytes())
    }
}

/// The renderer of one capture run: exactly one of the two is set.
struct Renderers {
    megakernel: Option<GpuFrameRenderer>,
    event: Option<EventWavefrontRenderer>,
}

impl Renderers {
    /// Renders `scene` into `accum` on whichever pipeline the run captures.
    fn accumulate(&mut self, scene: &SceneBundle, accum: &mut [Vec3]) -> Result<(), String> {
        let frame_scene = scene.as_frame_scene();
        if let Some(mr) = self.megakernel.as_mut() {
            mr.accumulate(&frame_scene, scene.sample_offset, scene.spp, accum)
                .map_err(|e| e.to_string())
        } else if let Some(er) = self.event.as_mut() {
            pollster::block_on(er.accumulate(&frame_scene, scene.sample_offset, scene.spp, accum))
                .map_err(|e| e.to_string())
        } else {
            unreachable!("a run builds exactly one renderer")
        }
    }
}

/// Renders one scene and writes its `.f32` file; exits 1 on any failure.
fn capture_scene(scene: &SceneBundle, out_dir: &Path, renderers: &mut Renderers) {
    println!(
        "Rendering {} ({}x{}, spp={}, max_bounces={})...",
        scene.name, scene.width, scene.height, scene.spp, scene.max_bounces
    );
    let num_pixels = (scene.width * scene.height) as usize;
    let mut accum = vec![Vec3::ZERO; num_pixels];

    if let Err(e) = renderers.accumulate(scene, &mut accum) {
        eprintln!(
            "gpu_golden_capture: error accumulating scene {}: {e}",
            scene.name
        );
        std::process::exit(1);
    }

    let out_f32_path = out_dir.join(format!("{}.f32", scene.name));
    if let Err(e) = write_f32_buffer(&out_f32_path, &accum) {
        eprintln!(
            "gpu_golden_capture: failed to write {}: {e}",
            out_f32_path.display()
        );
        std::process::exit(1);
    }
}

fn run_capture(out_dir: &Path, pipeline_kind_str: &str, date_arg: Option<String>) {
    let is_event = parse_pipeline(pipeline_kind_str);
    let ctx = acquire_context();
    let adapter_info = ctx.adapter.get_info();
    println!(
        "Adapter: {} ({:?}, backend={:?})",
        adapter_info.name, adapter_info.device_type, adapter_info.backend
    );

    let mut renderers = Renderers {
        megakernel: (!is_event).then(|| build_megakernel_renderer(&adapter_info)),
        event: is_event.then(|| EventWavefrontRenderer::new(ctx)),
    };

    if let Err(e) = std::fs::create_dir_all(out_dir) {
        eprintln!(
            "gpu_golden_capture: failed to create output directory {}: {e}",
            out_dir.display()
        );
        std::process::exit(1);
    }

    let date = get_date_string(date_arg);
    let shader_hash = pipeline_shader_hash(is_event);
    println!(
        "Shader hash ({}): 0x{:016x}",
        if is_event {
            "KERNEL_SET_HASH"
        } else {
            "SHADER_SRC FNV-1a 64"
        },
        shader_hash
    );

    let scenes = all_scenes();
    println!(
        "Capturing {} scenes to {} using pipeline '{}'...",
        scenes.len(),
        out_dir.display(),
        pipeline_kind_str
    );

    for scene in &scenes {
        capture_scene(scene, out_dir, &mut renderers);
    }

    let meta_path = out_dir.join("meta.json");
    if let Err(e) = write_meta_json(
        &meta_path,
        &adapter_info,
        &date,
        pipeline_kind_str,
        shader_hash,
        &scenes,
    ) {
        eprintln!(
            "gpu_golden_capture: failed to write metadata {}: {e}",
            meta_path.display()
        );
        std::process::exit(1);
    }

    println!("Capture completed successfully.");
}

fn check_meta(path_a: &Path, path_b: &Path, allow_mismatch: bool) {
    let (dir_a, dir_b) = if path_a.is_dir() && path_b.is_dir() {
        (path_a, path_b)
    } else {
        // Two bare .f32 files: look next to them.
        match (path_a.parent(), path_b.parent()) {
            (Some(a), Some(b)) => (a, b),
            _ => return,
        }
    };
    let problems = match (meta::read_meta(dir_a), meta::read_meta(dir_b)) {
        (Ok(a), Ok(b)) => {
            println!(
                "a: pipeline={} shader hash={} adapter={}
b: pipeline={} shader hash={} adapter={}",
                a.pipeline, a.shader_hash, a.adapter, b.pipeline, b.shader_hash, b.adapter
            );
            meta::mismatches(&a, &b)
        }
        (a, b) => [a.err(), b.err()].into_iter().flatten().collect(),
    };
    if problems.is_empty() {
        return;
    }
    eprintln!(
        "
!!! gpu_golden_capture: THE TWO CAPTURES ARE NOT COMPARABLE !!!"
    );
    for p in &problems {
        eprintln!("  - {p}");
    }
    if allow_mismatch {
        eprintln!(
            "  (continuing because --allow-mismatch was given)
"
        );
    } else {
        eprintln!("  Refusing to compare. Pass --allow-mismatch to compare anyway.");
        std::process::exit(2);
    }
}

/// Reads `path` as a buffer of `num_pixels` pixels; exits 1 when it cannot be read.
fn read_buffer_or_exit(path: &Path, num_pixels: usize) -> Vec<Vec3> {
    match compare::read_f32_buffer(path, num_pixels) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("gpu_golden_capture: cannot read {}: {e}", path.display());
            std::process::exit(1);
        }
    }
}

/// Compares two bare `.f32` files at the default scene size.
fn compare_files(path_a: &Path, path_b: &Path) -> Vec<ComparisonResult> {
    let name = path_a
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("custom");
    let (width, height, spp) = (
        scenes::DEFAULT_WIDTH,
        scenes::DEFAULT_HEIGHT,
        scenes::DEFAULT_SPP,
    );
    let num_pixels = (width * height) as usize;
    let buf_a = read_buffer_or_exit(path_a, num_pixels);
    let buf_b = read_buffer_or_exit(path_b, num_pixels);
    vec![compare_buffers(name, &buf_a, &buf_b, width, height, spp)]
}

/// Exits 1 when `file` (a scene's file in capture `label`) does not exist.
fn require_file(file: &Path, label: &str) {
    if !file.exists() {
        eprintln!(
            "gpu_golden_capture: missing file in {label}: {}",
            file.display()
        );
        std::process::exit(1);
    }
}

/// Compares every scene's file of two capture directories.
fn compare_directories(path_a: &Path, path_b: &Path) -> Vec<ComparisonResult> {
    let mut results = Vec::new();
    for s in &all_scenes() {
        let file_a = path_a.join(format!("{}.f32", s.name));
        let file_b = path_b.join(format!("{}.f32", s.name));

        if !file_a.exists() && !file_b.exists() {
            println!("Scene {} skipped (not found in either directory)", s.name);
            continue;
        }
        require_file(&file_a, "<a>");
        require_file(&file_b, "<b>");

        let num_pixels = (s.width * s.height) as usize;
        let buf_a = read_buffer_or_exit(&file_a, num_pixels);
        let buf_b = read_buffer_or_exit(&file_b, num_pixels);
        results.push(compare_buffers(
            s.name, &buf_a, &buf_b, s.width, s.height, s.spp,
        ));
    }
    results
}

/// Prints the table and every verdict beyond a clean pass, then exits: 1 for a FAIL or a STAT
/// result awaiting review, 0 when every scene is EXACT or TOL.
fn report_and_exit(results: &[ComparisonResult]) -> ! {
    print!("{}", format_comparison_table(results));

    let mut any_stat = false;
    let mut any_fail = false;

    for r in results.iter().filter(|r| !r.verdict.is_pass()) {
        match r.verdict {
            // Filtered out above.
            Verdict::Exact | Verdict::Tol => {}
            Verdict::StatPass => {
                any_stat = true;
                println!(
                    "STAT (Pass): {} had {} pixels with |z| > 3.0, cluster sizes: {:?}",
                    r.scene_name, r.stat_over_3_sigma, r.stat_cluster_sizes
                );
            }
            Verdict::Fail => {
                any_fail = true;
                println!(
                    "FAIL: {} had {} differing pixels (max abs: {:e}, max rel: {:e}), {} pixels with |z| > 3.0, clusters: {:?}",
                    r.scene_name,
                    r.differing_pixels,
                    r.max_abs_diff,
                    r.max_rel_diff,
                    r.stat_over_3_sigma,
                    r.stat_cluster_sizes
                );
            }
        }
    }

    if any_fail {
        eprintln!("\ngpu_golden_capture: comparison FAILED.");
        std::process::exit(1);
    }
    if any_stat {
        println!("\ngpu_golden_capture: STAT results require human/owner review per §7.2.");
        std::process::exit(1);
    }
    println!("\ngpu_golden_capture: all scenes PASSED (EXACT or TOL).");
    std::process::exit(0)
}

fn run_compare(path_a: &Path, path_b: &Path, allow_mismatch: bool) {
    check_meta(path_a, path_b, allow_mismatch);

    let results = if path_a.is_file() && path_b.is_file() {
        compare_files(path_a, path_b)
    } else if path_a.is_dir() && path_b.is_dir() {
        compare_directories(path_a, path_b)
    } else {
        eprintln!("gpu_golden_capture: arguments must be either two directories or two .f32 files");
        std::process::exit(1);
    };

    if results.is_empty() {
        eprintln!("gpu_golden_capture: no scenes were compared");
        std::process::exit(1);
    }

    report_and_exit(&results);
}

fn main() {
    let options = match parse_cli(std::env::args().skip(1)) {
        Ok(cmd) => cmd,
        Err(msg) => {
            eprintln!("gpu_golden_capture: {msg}");
            std::process::exit(2);
        }
    };

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::io::stderr)
        .init();

    match options {
        CliCommand::Capture {
            out_dir,
            pipeline,
            date,
        } => {
            run_capture(&out_dir, &pipeline, date);
        }
        CliCommand::Compare {
            path_a,
            path_b,
            allow_mismatch,
        } => {
            run_compare(&path_a, &path_b, allow_mismatch);
        }
    }
}
