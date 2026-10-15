//! Screenshot harness for the mobile UI (custom `main`, `harness = false`): rewrites the
//! mobile screenshots in the workspace's `assets/mobile` folder from the desktop preview.
//!
//! It starts the preview binary (`indicatrix-mobile-preview`) once per shot, on a fresh data
//! directory, with the flags that open the wanted page, tab, sheet and viewport mode, and with
//! `--screenshot` pointing at a scratch file. A shot that finishes and wrote a readable PNG is
//! copied into the output folder; anything else leaves the old file alone.
//!
//! ```text
//! cargo ui-screens-mobile
//! cargo ui-screens-mobile -- phone_light tablet   (only shots whose name contains one of these)
//! ```
//!
//! Arguments that do not start with `-` select shots by name substring.
//!
//! Environment variables:
//!
//! - `INDICATRIX_SCREENS_OUT`: the output folder (default `<manifest>/../../assets/mobile`).
//! - `INDICATRIX_SCREENS_SCALE`: the Slint scale factor of the preview window (default 2, so a
//!   390 x 800 phone shot is 780 x 1600 pixels). A `SLINT_SCALE_FACTOR` you set yourself wins.
//! - `INDICATRIX_SCREENS_TIMEOUT`: seconds a normal shot may take (default 40); a Render shot
//!   gets twice that. A preview that is still running then is killed and the shot fails.
//!
//! Only the PNG files named in [`SHOTS`] are written, and only when their shot succeeded. No
//! file is ever deleted or renamed. Every written file is announced as
//! `ui-screens-mobile: wrote assets/mobile/<name>.png (<w> x <h>)`; the report is in
//! `target/ui-screens-mobile/report.md`. The process exits non-zero when a shot failed.
//!
//! - `INDICATRIX_HARNESS_LIBRARY`: a `facet_diagrams.sqlite` to show in the library (the same
//!   variable as the desktop `cargo ui-screens`). It is copied into each shot's data directory
//!   (`files/library/`) and never opened by the harness itself. Without it every shot starts
//!   from an empty library, so `phone_library` and `tablet_library` show the empty state.
//!
//! Every shot starts from a fresh data directory. The design shots open template card 1.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    time::{Duration, Instant},
};

/// One shot: the file stem and the preview flags that produce it.
struct Shot {
    name: &'static str,
    args: &'static [&'static str],
}

/// A phone, as the preview's `phone` preset (logical px).
const PHONE: &str = "390x800";
/// Seconds a library shot waits: a large library takes a while to open and list.
const LIBRARY_WAIT: &str = "20";
/// Seconds a render shot waits so the trace converges (the preview's own default is 8).
const RENDER_WAIT: &str = "40";
/// A tablet in landscape, as the preview's `tablet-land` preset (logical px).
const TABLET: &str = "1280x800";

/// Every shot, in the order it is taken.
const SHOTS: [Shot; 13] = [
    Shot {
        name: "phone_library",
        args: &["--size", PHONE, "--page", "library", "--wait", LIBRARY_WAIT],
    },
    Shot {
        name: "phone_design_solid",
        args: &[
            "--size",
            PHONE,
            "--page",
            "design",
            "--template",
            "1",
            "--view",
            "solid",
            "--sheet",
            "peek",
        ],
    },
    Shot {
        name: "phone_design_diagram",
        args: &[
            "--size",
            PHONE,
            "--page",
            "design",
            "--template",
            "1",
            "--view",
            "diagram",
            "--sheet",
            "peek",
        ],
    },
    Shot {
        name: "phone_design_render",
        args: &[
            "--size",
            PHONE,
            "--page",
            "design",
            "--template",
            "1",
            "--view",
            "render",
            "--wait",
            RENDER_WAIT,
            "--sheet",
            "peek",
        ],
    },
    Shot {
        name: "phone_tiers_sheet",
        args: &[
            "--size",
            PHONE,
            "--page",
            "design",
            "--template",
            "1",
            "--tab",
            "tiers",
            "--sheet",
            "half",
        ],
    },
    Shot {
        name: "phone_stone",
        args: &[
            "--size",
            PHONE,
            "--page",
            "design",
            "--template",
            "1",
            "--tab",
            "stone",
            "--sheet",
            "half",
        ],
    },
    Shot {
        name: "phone_light",
        args: &[
            "--size",
            PHONE,
            "--page",
            "design",
            "--template",
            "1",
            "--tab",
            "light",
            "--sheet",
            "half",
        ],
    },
    Shot {
        name: "phone_checks",
        args: &[
            "--size",
            PHONE,
            "--page",
            "design",
            "--template",
            "1",
            "--tab",
            "checks",
            "--sheet",
            "half",
        ],
    },
    Shot {
        name: "phone_cut",
        args: &["--size", PHONE, "--page", "cut", "--template", "1"],
    },
    Shot {
        name: "phone_more",
        args: &["--size", PHONE, "--page", "more"],
    },
    Shot {
        name: "tablet_library",
        args: &[
            "--size",
            TABLET,
            "--page",
            "library",
            "--wait",
            LIBRARY_WAIT,
        ],
    },
    Shot {
        name: "tablet_design_render",
        args: &[
            "--size",
            TABLET,
            "--page",
            "design",
            "--template",
            "1",
            "--view",
            "render",
            "--wait",
            RENDER_WAIT,
        ],
    },
    Shot {
        name: "tablet_cut",
        args: &["--size", TABLET, "--page", "cut", "--template", "1"],
    },
];

/// The settings of one run.
struct Config {
    out: PathBuf,
    scratch: PathBuf,
    scale: String,
    timeout: Duration,
    library: Option<PathBuf>,
}

/// The outcome of one shot, for the report.
struct Outcome {
    name: &'static str,
    result: Result<(u32, u32), String>,
}

fn main() -> ExitCode {
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let selected: Vec<&Shot> = SHOTS
        .iter()
        .filter(|shot| filters.is_empty() || filters.iter().any(|f| shot.name.contains(f.as_str())))
        .collect();
    if selected.is_empty() {
        eprintln!("ui-screens-mobile: no shot matches {filters:?}");
        return ExitCode::FAILURE;
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = std::env::var_os("INDICATRIX_SCREENS_OUT")
        .map_or_else(|| manifest.join("../../assets/mobile"), PathBuf::from);
    let scratch = manifest.join("../../target/ui-screens-mobile");
    let seconds = std::env::var("INDICATRIX_SCREENS_TIMEOUT")
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(40);
    let config = Config {
        out,
        scratch,
        scale: std::env::var("INDICATRIX_SCREENS_SCALE").unwrap_or_else(|_| "2".to_owned()),
        timeout: Duration::from_secs(seconds),
        library: std::env::var_os("INDICATRIX_HARNESS_LIBRARY").map(PathBuf::from),
    };
    if let Err(error) = fs::create_dir_all(&config.scratch) {
        eprintln!(
            "ui-screens-mobile: cannot create {}: {error}",
            config.scratch.display()
        );
        return ExitCode::FAILURE;
    }

    let mut outcomes = Vec::new();
    for shot in selected {
        let result = take(shot, &config);
        match &result {
            Ok((width, height)) => println!(
                "ui-screens-mobile: wrote assets/mobile/{}.png ({width} x {height})",
                shot.name
            ),
            Err(reason) => eprintln!("ui-screens-mobile: {} FAILED: {reason}", shot.name),
        }
        outcomes.push(Outcome {
            name: shot.name,
            result,
        });
    }

    write_report(&config, &outcomes);
    if outcomes.iter().any(|outcome| outcome.result.is_err()) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Runs the preview for `shot`, checks the picture and copies it into the output folder.
/// Returns the pixel size of the written file.
fn take(shot: &Shot, config: &Config) -> Result<(u32, u32), String> {
    let data = std::env::temp_dir().join(format!(
        "indicatrix-mobile-screens-{}-{}",
        std::process::id(),
        shot.name
    ));
    // A fresh data directory per shot: no settings, no autosave, and either an empty library
    // or a copy of the one named by `INDICATRIX_HARNESS_LIBRARY`.
    let _ = fs::remove_dir_all(&data);
    if let Some(library) = &config.library {
        let folder = data.join("files").join("library");
        fs::create_dir_all(&folder)
            .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
        fs::copy(library, folder.join("facet_diagrams.sqlite"))
            .map_err(|e| format!("cannot copy the library {}: {e}", library.display()))?;
    }
    let raw = config.scratch.join(format!("{}.png", shot.name));
    let log = config.scratch.join(format!("{}.log", shot.name));
    let _ = fs::remove_file(&raw);

    let is_render = shot
        .args
        .windows(2)
        .any(|pair| pair == ["--view", "render"]);
    let timeout = if is_render {
        config.timeout * 2
    } else {
        config.timeout
    };
    let stderr = fs::File::create(&log).map_err(|e| format!("cannot create the log: {e}"))?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_indicatrix-mobile-preview"));
    command
        .args(shot.args)
        .arg("--data")
        .arg(&data)
        .arg("--screenshot")
        .arg(&raw)
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    if std::env::var_os("SLINT_SCALE_FACTOR").is_none() {
        command.env("SLINT_SCALE_FACTOR", &config.scale);
    }
    let outcome = run_with_timeout(&mut command, timeout);
    let _ = fs::remove_dir_all(&data);
    outcome?;

    let (width, height) = image::image_dimensions(&raw)
        .map_err(|e| format!("no readable screenshot (see {}): {e}", log.display()))?;
    fs::create_dir_all(&config.out)
        .map_err(|e| format!("cannot create {}: {e}", config.out.display()))?;
    let target = config.out.join(format!("{}.png", shot.name));
    fs::copy(&raw, &target).map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    Ok((width, height))
}

/// Starts `command` and waits for it; kills it after `timeout`.
fn run_with_timeout(command: &mut Command, timeout: Duration) -> Result<(), String> {
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start the preview: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("the preview exited with {status}")),
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => return Err(format!("cannot wait for the preview: {error}")),
        }
    }
}

/// Writes `target/ui-screens-mobile/report.md`.
fn write_report(config: &Config, outcomes: &[Outcome]) {
    let mut text = String::from("# ui-screens-mobile report\n\n");
    let failed = outcomes.iter().filter(|o| o.result.is_err()).count();
    let _ = writeln!(
        text,
        "{} shots, {} written, {failed} failed. Scale factor {}. Output: {}\n",
        outcomes.len(),
        outcomes.len() - failed,
        config.scale,
        config.out.display()
    );
    for outcome in outcomes {
        match &outcome.result {
            Ok((width, height)) => {
                let _ = writeln!(text, "- {}: ok ({width} x {height})", outcome.name);
            }
            Err(reason) => {
                let _ = writeln!(text, "- {}: FAILED, {reason}", outcome.name);
            }
        }
    }
    if let Err(error) = fs::write(config.scratch.join("report.md"), text) {
        eprintln!("ui-screens-mobile: cannot write the report: {error}");
    }
}
