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

use std::path::{Path, PathBuf};

use glam::Vec3;
use indicatrix::renderer::gpu::{GpuContext, GpuFrameRenderer, GpuPipelineKind};

mod compare;
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
  gpu_golden_capture compare <dir_a> <dir_b>";

enum CliCommand {
    Capture {
        out_dir: PathBuf,
        pipeline: String,
        date: Option<String>,
    },
    Compare {
        path_a: PathBuf,
        path_b: PathBuf,
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
            let path_a = args
                .next()
                .ok_or_else(|| "missing first argument <a> for compare".to_string())?;
            let path_b = args
                .next()
                .ok_or_else(|| "missing second argument <b> for compare".to_string())?;
            Ok(CliCommand::Compare {
                path_a: PathBuf::from(path_a),
                path_b: PathBuf::from(path_b),
            })
        }
        "help" | "-h" | "--help" => Err(USAGE.to_string()),
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
}

fn get_date_string(date_override: Option<String>) -> String {
    if let Some(d) = date_override {
        return d;
    }
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(dur) => format!("unix-timestamp-{}", dur.as_secs()),
        Err(_) => "unknown-date".to_string(),
    }
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

fn run_capture(out_dir: &Path, pipeline_kind_str: &str, date_arg: Option<String>) {
    match pipeline_kind_str {
        "megakernel" => {}
        "event" => {
            eprintln!(
                "gpu_golden_capture: event wavefront pipeline not available yet (planned for L7)"
            );
            std::process::exit(1);
        }
        other => {
            eprintln!(
                "gpu_golden_capture: unknown pipeline kind '{other}', expected 'megakernel' or 'event'"
            );
            std::process::exit(2);
        }
    }

    let ctx = match GpuContext::acquire() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gpu_golden_capture: cannot acquire GPU context: {e}");
            std::process::exit(2);
        }
    };
    let adapter_info = ctx.adapter.get_info();
    let adapter_label = format!(
        "{} ({:?}, backend={:?})",
        adapter_info.name, adapter_info.device_type, adapter_info.backend
    );
    println!("Adapter: {adapter_label}");

    let mut renderer = match GpuFrameRenderer::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("gpu_golden_capture: cannot build frame renderer: {e}");
            std::process::exit(2);
        }
    };
    renderer.set_pipeline_kind(GpuPipelineKind::Megakernel);

    if let Err(e) = std::fs::create_dir_all(out_dir) {
        eprintln!(
            "gpu_golden_capture: failed to create output directory {}: {e}",
            out_dir.display()
        );
        std::process::exit(1);
    }

    let date = get_date_string(date_arg);
    let shader_hash = fnv1a_64(SHADER_SRC.as_bytes());
    println!("SHADER_SRC FNV-1a 64 = 0x{:016x}", shader_hash);

    let scenes = all_scenes();
    println!(
        "Capturing {} scenes to {}...",
        scenes.len(),
        out_dir.display()
    );

    for scene in &scenes {
        println!(
            "Rendering {} ({}x{}, spp={}, max_bounces={})...",
            scene.name, scene.width, scene.height, scene.spp, scene.max_bounces
        );
        let num_pixels = (scene.width * scene.height) as usize;
        let mut accum = vec![Vec3::ZERO; num_pixels];
        let frame_scene = scene.as_frame_scene();

        if let Err(e) =
            renderer.accumulate(&frame_scene, scene.sample_offset, scene.spp, &mut accum)
        {
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

    let meta_path = out_dir.join("meta.json");
    if let Err(e) = write_meta_json(
        &meta_path,
        &adapter_info,
        &date,
        "megakernel",
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

fn run_compare(path_a: &Path, path_b: &Path) {
    let mut results: Vec<ComparisonResult> = Vec::new();

    if path_a.is_file() && path_b.is_file() {
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

        let buf_a = match compare::read_f32_buffer(path_a, num_pixels) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("gpu_golden_capture: cannot read {}: {e}", path_a.display());
                std::process::exit(1);
            }
        };
        let buf_b = match compare::read_f32_buffer(path_b, num_pixels) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("gpu_golden_capture: cannot read {}: {e}", path_b.display());
                std::process::exit(1);
            }
        };

        results.push(compare_buffers(name, &buf_a, &buf_b, width, height, spp));
    } else if path_a.is_dir() && path_b.is_dir() {
        let scenes = all_scenes();
        for s in &scenes {
            let file_a = path_a.join(format!("{}.f32", s.name));
            let file_b = path_b.join(format!("{}.f32", s.name));

            if !file_a.exists() && !file_b.exists() {
                println!("Scene {} skipped (not found in either directory)", s.name);
                continue;
            }
            if !file_a.exists() {
                eprintln!(
                    "gpu_golden_capture: missing file in <a>: {}",
                    file_a.display()
                );
                std::process::exit(1);
            }
            if !file_b.exists() {
                eprintln!(
                    "gpu_golden_capture: missing file in <b>: {}",
                    file_b.display()
                );
                std::process::exit(1);
            }

            let num_pixels = (s.width * s.height) as usize;
            let buf_a = match compare::read_f32_buffer(&file_a, num_pixels) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("gpu_golden_capture: cannot read {}: {e}", file_a.display());
                    std::process::exit(1);
                }
            };
            let buf_b = match compare::read_f32_buffer(&file_b, num_pixels) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("gpu_golden_capture: cannot read {}: {e}", file_b.display());
                    std::process::exit(1);
                }
            };

            results.push(compare_buffers(
                s.name, &buf_a, &buf_b, s.width, s.height, s.spp,
            ));
        }
    } else {
        eprintln!("gpu_golden_capture: arguments must be either two directories or two .f32 files");
        std::process::exit(1);
    }

    if results.is_empty() {
        eprintln!("gpu_golden_capture: no scenes were compared");
        std::process::exit(1);
    }

    print!("{}", format_comparison_table(&results));

    let mut any_stat = false;
    let mut any_fail = false;

    for r in &results {
        match r.verdict {
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
    } else if any_stat {
        println!("\ngpu_golden_capture: STAT results require human/owner review per §7.2.");
        std::process::exit(1);
    } else {
        println!("\ngpu_golden_capture: all scenes PASSED (EXACT or TOL).");
        std::process::exit(0);
    }
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
        CliCommand::Compare { path_a, path_b } => {
            run_compare(&path_a, &path_b);
        }
    }
}
