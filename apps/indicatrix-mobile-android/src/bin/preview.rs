//! The desktop preview: the mobile UI in a desktop window at a phone or tablet size,
//! so it can be exercised without a device.
//!
//! ```text
//! indicatrix-mobile-preview [--size WxH|phone|phone-land|tablet|tablet-land] [--data DIR]
//! ```
//!
//! `--size` defaults to `390x800` (`phone`); `phone-land` is `800x390`, `tablet`
//! `800x1280`, `tablet-land` `1280x800`. `--data` is the app's data directory
//! (default `<temp>/indicatrix-mobile-preview`); settings live in `DIR/files`, shared
//! and printed files in `DIR/cache/share`. `--page library|design|cut|more` opens that
//! destination; `--template N` starts on that template card; `--tab
//! tiers|stone|light|checks` and `--sheet peek|half|full` choose the Design page's tab
//! and sheet height; `--view solid|diagram|render` the viewport mode (a render
//! screenshot waits longer so the picture can converge); `--engine path|ray` chooses its renderer; `--screenshot FILE.png` saves
//! the window shortly after start and quits, `--wait SECONDS` after that many seconds
//! instead. Press Escape for Back.

use std::{path::PathBuf, process::ExitCode, sync::Arc, time::Duration};

use indicatrix_mobile_android::{
    platform_desktop::DesktopPlatform,
    run::{self, RunOptions},
};
use indicatrix_mobile_core::{layout::Destination, platform::AppDirs};
use tracing_subscriber::EnvFilter;

const DEFAULT_SIZE: (f32, f32) = (390.0, 800.0);
const USAGE: &str = "usage: indicatrix-mobile-preview [--size WxH|phone|phone-land|tablet|tablet-land] [--data DIR] [--page library|design|cut|more] [--template N] [--tab tiers|stone|light|checks] [--sheet peek|half|full] [--view solid|diagram|render] [--engine path|ray] [--wait SECONDS] [--screenshot FILE.png]";

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq)]
struct PreviewArgs {
    size: (f32, f32),
    data: PathBuf,
    page: Option<Destination>,
    screenshot: Option<PathBuf>,
    template: Option<usize>,
    tab: Option<i32>,
    sheet: Option<i32>,
    view: Option<i32>,
    engine: Option<u8>,
    wait: Option<Duration>,
}

/// The viewport mode a name selects (0 Solid, 1 Diagram, 2 Render).
fn parse_view(text: &str) -> Result<i32, String> {
    ["solid", "diagram", "render"]
        .iter()
        .position(|name| name.eq_ignore_ascii_case(text))
        .and_then(|index| i32::try_from(index).ok())
        .ok_or_else(|| format!("bad view {text:?}: expected solid, diagram or render"))
}

/// The Design page tab a name selects (0 Tiers, 1 Stone, 2 Light, 3 Checks).
fn parse_tab(text: &str) -> Result<i32, String> {
    ["tiers", "stone", "light", "checks"]
        .iter()
        .position(|name| name.eq_ignore_ascii_case(text))
        .and_then(|index| i32::try_from(index).ok())
        .ok_or_else(|| format!("bad tab {text:?}: expected tiers, stone, light or checks"))
}

/// The sheet detent a name selects (0 peek, 1 half, 2 full).
fn parse_sheet(text: &str) -> Result<i32, String> {
    ["peek", "half", "full"]
        .iter()
        .position(|name| name.eq_ignore_ascii_case(text))
        .and_then(|index| i32::try_from(index).ok())
        .ok_or_else(|| format!("bad sheet {text:?}: expected peek, half or full"))
}

/// A destination from its lower-case label.
fn parse_page(text: &str) -> Result<Destination, String> {
    Destination::ALL
        .into_iter()
        .find(|destination| destination.label().eq_ignore_ascii_case(text))
        .ok_or_else(|| format!("bad page {text:?}: expected library, design, cut or more"))
}

/// A window size from a preset name or `WxH` (logical px, 200 to 4000 per side).
fn parse_size(text: &str) -> Result<(f32, f32), String> {
    match text {
        "phone" => return Ok(DEFAULT_SIZE),
        "phone-land" => return Ok((800.0, 390.0)),
        "tablet" => return Ok((800.0, 1280.0)),
        "tablet-land" => return Ok((1280.0, 800.0)),
        _ => {}
    }
    let (width, height) = text
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("bad size {text:?}: expected WxH or a preset name"))?;
    let parse = |side: &str| {
        side.trim()
            .parse::<u16>()
            .ok()
            .filter(|value| (200..=4000).contains(value))
            .map(f32::from)
            .ok_or_else(|| format!("bad size {text:?}: each side must be 200 to 4000"))
    };
    Ok((parse(width)?, parse(height)?))
}

/// Parses the arguments after the program name.
fn parse_args(
    args: impl IntoIterator<Item = String>,
    default_data: PathBuf,
) -> Result<PreviewArgs, String> {
    let mut parsed = PreviewArgs {
        size: DEFAULT_SIZE,
        data: default_data,
        page: None,
        screenshot: None,
        template: None,
        tab: None,
        sheet: None,
        view: None,
        engine: None,
        wait: None,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => {
                let value = args.next().ok_or("--size needs a value")?;
                parsed.size = parse_size(&value)?;
            }
            "--data" => {
                let value = args.next().ok_or("--data needs a directory")?;
                parsed.data = PathBuf::from(value);
            }
            "--page" => {
                let value = args.next().ok_or("--page needs a destination")?;
                parsed.page = Some(parse_page(&value)?);
            }
            "--template" => {
                let value = args.next().ok_or("--template needs a number")?;
                let index = value
                    .parse::<usize>()
                    .map_err(|_| format!("bad template {value:?}: expected a card number"))?;
                parsed.template = Some(index);
            }
            "--tab" => {
                let value = args.next().ok_or("--tab needs a tab name")?;
                parsed.tab = Some(parse_tab(&value)?);
            }
            "--sheet" => {
                let value = args.next().ok_or("--sheet needs peek, half or full")?;
                parsed.sheet = Some(parse_sheet(&value)?);
            }
            "--engine" => {
                let value = args.next().ok_or("--engine needs path or ray")?;
                parsed.engine = Some(match value.as_str() {
                    "path" => 0,
                    "ray" => 1,
                    _ => return Err("--engine needs path or ray".to_owned()),
                });
            }
            "--view" => {
                let value = args.next().ok_or("--view needs solid, diagram or render")?;
                parsed.view = Some(parse_view(&value)?);
            }
            "--wait" => {
                let value = args.next().ok_or("--wait needs a number of seconds")?;
                let seconds = value
                    .parse::<u64>()
                    .ok()
                    .filter(|seconds| (1..=600).contains(seconds))
                    .ok_or_else(|| format!("bad wait {value:?}: expected 1 to 600 seconds"))?;
                parsed.wait = Some(Duration::from_secs(seconds));
            }
            "--screenshot" => {
                let value = args.next().ok_or("--screenshot needs a file")?;
                parsed.screenshot = Some(PathBuf::from(value));
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(parsed)
}

fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let default_data = std::env::temp_dir().join("indicatrix-mobile-preview");
    let args = match parse_args(std::env::args().skip(1), default_data) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let dirs = AppDirs {
        files: args.data.join("files"),
        cache: args.data.join("cache"),
    };
    tracing::info!(size = ?args.size, data = %args.data.display(), "starting the mobile preview");
    let options = RunOptions {
        initial_size: Some(args.size),
        destination: args.page,
        screenshot: args.screenshot,
        template: args.template,
        tab: args.tab,
        sheet: args.sheet,
        view: args.view,
        engine: args.engine,
        screenshot_delay: args.wait,
    };
    let platform: Arc<dyn indicatrix_mobile_core::platform::Platform> =
        Arc::new(DesktopPlatform::new(dirs));
    match run::run_with(&platform, &options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("the preview failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn defaults_to_a_phone() {
        let parsed = parse_args(args(&[]), PathBuf::from("d")).unwrap();
        assert_eq!(parsed.size, (390.0, 800.0));
        assert_eq!(parsed.data, PathBuf::from("d"));
    }

    #[test]
    fn presets_and_explicit_sizes() {
        assert_eq!(parse_size("phone-land").unwrap(), (800.0, 390.0));
        assert_eq!(parse_size("tablet").unwrap(), (800.0, 1280.0));
        assert_eq!(parse_size("tablet-land").unwrap(), (1280.0, 800.0));
        assert_eq!(parse_size("412x915").unwrap(), (412.0, 915.0));
        assert_eq!(parse_size("600X960").unwrap(), (600.0, 960.0));
    }

    #[test]
    fn bad_sizes_are_rejected() {
        for bad in ["", "390", "axb", "10x10", "390x", "x800", "99999x800"] {
            assert!(parse_size(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn flags_are_parsed() {
        let parsed = parse_args(
            args(&["--size", "tablet", "--data", "some/dir"]),
            PathBuf::from("d"),
        )
        .unwrap();
        assert_eq!(parsed.size, (800.0, 1280.0));
        assert_eq!(parsed.data, PathBuf::from("some/dir"));
    }

    #[test]
    fn page_and_screenshot_flags_are_parsed() {
        let parsed = parse_args(
            args(&["--page", "cut", "--screenshot", "shot.png"]),
            PathBuf::from("d"),
        )
        .unwrap();
        assert_eq!(parsed.page, Some(Destination::Cut));
        assert_eq!(parsed.screenshot, Some(PathBuf::from("shot.png")));
        assert!(parse_page("Library").is_ok());
        assert!(parse_page("nowhere").is_err());
    }

    #[test]
    fn tab_and_sheet_flags_are_parsed() {
        let parsed = parse_args(
            args(&["--tab", "Light", "--sheet", "full", "--template", "1"]),
            PathBuf::from("d"),
        )
        .unwrap();
        assert_eq!(parsed.tab, Some(2));
        assert_eq!(parsed.sheet, Some(2));
        assert_eq!(parsed.template, Some(1));
        assert_eq!(parse_tab("tiers").unwrap(), 0);
        assert_eq!(parse_tab("checks").unwrap(), 3);
        assert_eq!(parse_sheet("peek").unwrap(), 0);
        assert_eq!(parse_sheet("half").unwrap(), 1);
        assert!(parse_tab("nowhere").is_err());
        assert!(parse_sheet("huge").is_err());
        assert!(parse_args(args(&["--tab"]), PathBuf::new()).is_err());
        assert!(parse_args(args(&["--sheet", "x"]), PathBuf::new()).is_err());
    }

    #[test]
    fn unknown_or_incomplete_flags_are_rejected() {
        assert!(parse_args(args(&["--bogus"]), PathBuf::new()).is_err());
        assert!(parse_args(args(&["--size"]), PathBuf::new()).is_err());
        assert!(parse_args(args(&["--data"]), PathBuf::new()).is_err());
    }
}
