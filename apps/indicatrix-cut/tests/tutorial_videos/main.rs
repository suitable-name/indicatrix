//! Tutorial video harness for `indicatrix-cut` (custom `main`, `harness = false`): plays every
//! built-in tutorial in the real application and records it as a video.
//!
//! It drives the real `MainWindow` with all its Rust wiring on Slint's testing backend with the
//! software renderer, inside a throw-away profile (the same isolation as the `ui_harness` and
//! `ui_screens` targets), starts each tutorial from the tutorial browser and finishes it step by
//! step the way a learner would. A drawn mouse cursor glides to what each step is about, a ripple
//! shows the click, long background work (solves, renders) is sped up or cut. One scenario is one
//! tutorial (`video:<tutorial-id>`); the app is relaunched fresh for each.
//!
//! Run (ffmpeg must be on `PATH`, or named by `UIPROBE_FFMPEG`):
//!
//! ```text
//! cargo tutorial-videos
//! cargo tutorial-videos -- welcome worked   (only tutorials whose id contains one of these)
//! INDICATRIX_HARNESS_LIBRARY=C:\path\to\facet_diagrams.sqlite cargo tutorial-videos
//! cargo tutorial-videos --features gpu      (faster live renders)
//! ```
//!
//! Arguments that do not start with `-` select tutorials by id substring (the harness filters
//! scenarios by name). The run needs Slint debug info (the alias's environment sets it).
//!
//! Environment variables:
//!
//! - `INDICATRIX_VIDEOS_OUT`: the output folder (default `<manifest>/../../videos`, resolved
//!   before the profile moves the working directory).
//! - `INDICATRIX_VIDEOS_4K`: keep the 3840 x 2160 capture instead of encoding 1920 x 1080.
//! - `INDICATRIX_VIDEOS_FPS`: the frame rate, 24 to 60 (default 30).
//! - `INDICATRIX_VIDEOS_ALLOW_INJECT`: rejected; recordings require visible user input.
//! - `INDICATRIX_HARNESS_LIBRARY`: a `facet_diagrams.sqlite` to copy into the profile (it is
//!   never opened itself). Without it a small fixture library is used.
//! - `UIPROBE_JOBS=serial`: one tutorial at a time; `UIPROBE_FFMPEG`: the ffmpeg executable.
//!
//! Per tutorial: `<id>.mp4` (1920 x 1080, 30 fps, H.264), `<id>.srt` (one cue group per step)
//! and `<id>.chapters.txt` (a `YouTube` chapter list). Each is announced as
//! `tutorial-videos: wrote videos/<id>.mp4 (<mm:ss>, <n> frames)`. A tutorial whose run hits a
//! step the recorder cannot show, a skipped step, a timeout, a panic, an error toast or private
//! text writes no files at all (the old ones stay) and reports a finding `video` in
//! `target/tutorial-videos/report.md`. The video is written as `<id>.mp4.part` and renamed when
//! it is complete.
//!
//! Privacy: no frame may show a path with the user's name; every step's visible labels, values
//! and placeholders are scanned for the user's name and for profile path fragments, and a hit
//! abandons the video.
//!
//! Look: the software renderer draws with the system fonts and draws no window frame, so a video
//! shows the window content only. The window is 1920 x 1080 logical pixels at scale 2.0.

mod compare_capture;
mod config;
mod director;
mod event_extras;
mod pacing;
mod perform_actions;
mod perform_inputs;
mod perform_routes;
mod pure_targets;
mod routes;
mod scripts;
mod stage;
mod startup;
#[path = "../ui_harness/support/mod.rs"]
mod support;
mod targets;

use config::Config;
use indicatrix_editor::guide::GuideCatalog;
use std::{cell::RefCell, process::ExitCode, rc::Rc};
use support::{Ctx, HarnessApp, SettingsSeed, profile::print_shutdown_summary};
use uiprobe::{
    Harness, Jobs,
    backend::Renderer,
    profile::Profile as HarnessProfile,
    report::{Finding, Format},
    take_panics,
};

/// The app, shared by the scenario closures and the teardown (one UI thread).
type SharedApp = Rc<RefCell<HarnessApp>>;

fn main() -> ExitCode {
    // Read before the profile moves the working directory and redirects the user variables.
    let config = Rc::new(Config::from_env());
    let harness = Harness::new()
        .profile(
            HarnessProfile::isolated()
                .name_prefix("indicatrix-tutorial-videos")
                .guard_paths(|| {
                    vec![
                        indicatrix_cut::harness_support::settings_path(),
                        indicatrix_cut::harness_support::database_path(),
                    ]
                }),
        )
        .renderer(Renderer::Software)
        // Worker processes while cores are idle (each its own profile); the tutorials are
        // independent. `UIPROBE_JOBS=serial` runs them one after another.
        .jobs(Jobs::Auto)
        .report_dir(uiprobe::target_dir!().join("tutorial-videos"))
        .formats([Format::Markdown]);

    let app: SharedApp = Rc::new(RefCell::new(HarnessApp::empty()));
    let mut harness = harness;
    let ids: Vec<String> = GuideCatalog::new()
        .all()
        .iter()
        .map(|guide| guide.id.clone())
        .collect();
    for id in ids {
        harness = register(harness, &app, &config, id);
    }
    let teardown_app = Rc::clone(&app);
    harness
        .teardown(move || {
            teardown_app.borrow_mut().shutdown();
            print_shutdown_summary();
        })
        .run()
}

/// Registers one tutorial as a scenario.
fn register(harness: Harness, app: &SharedApp, config: &Rc<Config>, id: String) -> Harness {
    let app = Rc::clone(app);
    let config = Rc::clone(config);
    let name = format!("video:{id}");
    harness.scenario(name, move |cx| {
        let mut guard = app.borrow_mut();
        let _ = take_panics();
        // The toast readers serve the crate's own end-of-scenario `check_health`.
        let reader = Rc::clone(&app);
        cx.on_error_notice("MainWindow", move || {
            reader.try_borrow().ok().and_then(|a| a.error_notice())
        });
        let clearer = Rc::clone(&app);
        cx.clear_notice_with(move || {
            if let Ok(a) = clearer.try_borrow() {
                a.clear_notice();
            }
        });
        let (report, profile) = cx.parts();
        let mut ctx = Ctx {
            app: &mut guard,
            profile,
            report,
            seed: SettingsSeed::ExistingInstall,
        };
        let outcome = director::film(&mut ctx, &config, &id);
        ctx.check_health(&id);
        guard.clear_notice();
        outcome.map_err(|reason| {
            reason.strip_prefix("video-stage: ").map_or_else(
                || Finding::simple("video", format!("{id}: {reason}")),
                |detail| Finding::simple("video-stage", format!("{id}: {detail}")),
            )
        })
    })
}
