//! Films one tutorial: launches the app on a library, starts the tutorial from the browser and
//! walks its steps the way a learner would, with the recorder running.
//!
//! The recording is abandoned (no file is written, the old one stays) the moment a step cannot
//! be shown honestly: a step the recorder cannot drive, a recipe that gave up, a timeout, a
//! panic, an error toast, private text on screen. `film` then returns the reason.

use crate::{
    config::{Config, LOGICAL_SIZE, SCALE, WINDOW_CENTER},
    pacing, routes,
    scripts::pure_tables::check_guide,
    stage::StageState,
    support::{Ctx, WindowView, app::pump, library_source::relaunch_with_database, sample_designs},
    targets::{self, Resolved, Spot},
};
use indicatrix_cut::{GuideModel, LibraryModel, MainWindow, harness_support};
use indicatrix_editor::guide::{
    CameraFocus, Goal, Guide, GuideCatalog, StartingState, opening_stage, stage_for,
};

use slint::{ComponentHandle, Model};
use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use uiprobe::{
    backend::require_debug_info,
    profile::request_abort,
    recording::{Recorder, RecorderOptions},
    report::Report,
    screenshot, take_panics,
};

/// Whether the Slint debug info check has run in this process.
static DEBUG_INFO_CHECKED: AtomicBool = AtomicBool::new(false);

/// The lesson that needs a catalogue design with an adopted tier instead of a template design.
const CATALOGUE_GUIDE: &str = "optimizing-deep-solve";

/// What `GuideModel` shows right now.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the GuideModel properties it reads; they are independent flags"
)]
pub struct Snap {
    pub open: bool,
    pub id: String,
    pub index: i32,
    pub total: i32,
    pub done: bool,
    pub busy: bool,
    pub failed: bool,
    pub offers_perform: bool,
    pub manual: bool,
    pub title: String,
    pub target: String,
}

impl Snap {
    /// Whether step `n` (0-based) is the open guide's current step.
    #[must_use]
    pub fn at(&self, n: usize) -> bool {
        self.open && usize::try_from(self.index) == Ok(n)
    }
}

/// Reads `GuideModel`.
#[must_use]
pub fn snap(ui: &MainWindow) -> Snap {
    let guide = ui.global::<GuideModel>();
    let current = guide.get_current();
    Snap {
        open: guide.get_open(),
        id: guide.get_guide_id().to_string(),
        index: guide.get_step_index(),
        total: guide.get_total_steps(),
        done: guide.get_step_done(),
        busy: guide.get_perform_busy(),
        failed: guide.get_perform_failed(),
        offers_perform: guide.get_offers_perform(),
        manual: guide.get_current_is_manual(),
        title: current.title.to_string(),
        target: guide.get_highlight_target().to_string(),
    }
}

/// The running recording and everything a step needs to drive the app.
pub struct Film<'a> {
    pub(crate) rec: Recorder<'a>,
    pub(crate) ui: &'a MainWindow,
    pub(crate) cfg: &'a Config,
    /// The tutorial id.
    pub(crate) id: String,
    pub(crate) report: &'a mut Report,
    /// Where the drawn cursor is (logical pixels).
    pub(crate) cursor: (f32, f32),
    noted: HashSet<String>,
    pub(crate) stage_state: StageState,
    /// The step (0-based) the recipe or route being filmed belongs to.
    pub(crate) perform_n: usize,
    /// The secondary window drawn into the video ("help" or "rough planner"), if any.
    pub(crate) attached_window: Option<&'static str>,
}

/// Films tutorial `id` into `<out>/<id>.mp4` (and its `.srt` and `.chapters.txt`).
///
/// # Errors
///
/// Why the recording was abandoned or could not be finished.
pub fn film(ctx: &mut Ctx<'_>, cfg: &Config, id: &str) -> Result<(), String> {
    let catalog = GuideCatalog::new();
    let guide = catalog
        .get(id)
        .ok_or_else(|| format!("the catalogue has no guide with the id {id:?}"))?;
    if cfg.allow_inject {
        return Err("INDICATRIX_VIDEOS_ALLOW_INJECT is forbidden for tutorial recordings".into());
    }
    // The tables that route this guide (scripts, commands, events) must agree with the
    // catalogue as it is now; a stale one stops the video before anything is filmed.
    check_guide(guide).map_err(|reason| format!("harness tables: {reason}"))?;
    launch(ctx, cfg)?;
    ctx.report.note(cfg.library_line());
    let ui = ctx.app.ui().clone_strong();
    screenshot::set_window_size(ui.window(), LOGICAL_SIZE, SCALE);
    pump(400);
    crate::startup::dismiss_batch_offers(&ui)?;
    pump(500);
    // A lesson on "the design you have open" gets its design by filmed clicks (see
    // `Film::film_design_setup`). The imported-meets lesson starts from such a design too: its
    // first step films replacing it with an imported design from the library
    // (`Film::open_imported_library_design`).
    let needs_design = matches!(guide.starting_state, StartingState::RequiresOpenDesign);
    let opening = opening_stage(guide);
    let options = RecorderOptions {
        fps: cfg.fps,
        output: cfg.video_path(id),
        encode_size: cfg.encode_size(),
        ..RecorderOptions::default()
    };
    let rec = Recorder::start(ui.window(), options)?;
    let mut film = Film {
        rec,
        ui: &ui,
        cfg,
        id: id.to_owned(),
        report: &mut *ctx.report,
        cursor: WINDOW_CENTER,
        noted: HashSet::new(),
        stage_state: StageState::default(),
        perform_n: 0,
        attached_window: None,
    };
    if !needs_design {
        film.stage(&opening, false)?;
        film.frame(opening.focus, true, true)?;
    }
    film.play(guide, needs_design)
}

/// Relaunches the app on the library (the copy of the configured database, else the fixtures),
/// checks the Slint debug info once. Startup offers are handled after sizing the window.
fn launch(ctx: &mut Ctx<'_>, cfg: &Config) -> Result<(), String> {
    match &cfg.library {
        Some(database) => {
            relaunch_with_database(ctx, database)?;
            let ui = ctx.app.ui().clone_strong();
            let listed = pacing::wait_real(pacing::LIBRARY_LOAD, || {
                ui.global::<LibraryModel>().get_diagram_list().row_count() > 0
            });
            if !listed {
                return Err("the library copy lists no design".to_owned());
            }
        }
        None => {
            if !ctx.relaunch_with_library(&sample_designs()) {
                return Err("the app did not start on the fixture library".to_owned());
            }
        }
    }
    if !DEBUG_INFO_CHECKED.swap(true, Ordering::SeqCst)
        && let Err(message) = require_debug_info(&ctx.app.main_view().root)
    {
        // The element API needs Slint debug info; without it every query returns nothing.
        request_abort(message.clone());
        return Err(message);
    }
    Ok(())
}

impl Film<'_> {
    /// Holds the recording for `duration`, after checking that the toast on screen shows no
    /// private text (a toast can come and go between two step-end scans).
    ///
    /// # Errors
    ///
    /// Private text in the toast, or the recorder's own error.
    pub(crate) fn hold(&mut self, duration: Duration) -> Result<(), String> {
        if self.ui.get_toast_visible() {
            let message = self.ui.get_toast_message().to_string();
            if let Some(word) = self.cfg.private_hit(&message) {
                return Err(format!("private text in a toast ({word:?}): {message:?}"));
            }
        }
        self.rec.hold(duration)
    }

    /// The whole tutorial, from the intro to the outro, then the files.
    fn play(mut self, guide: &Guide, needs_design: bool) -> Result<(), String> {
        self.rec.chapter("Intro");
        self.hold(pacing::INTRO_HOLD)?;
        self.advanced_prelude(&guide.id)?;
        if needs_design {
            // Deep Solve is dim on a template design, which has no printed proportions, and on a
            // design whose tiers are all pinned; its first step tells the learner to load a
            // catalogue design and adopt a tier first, so that lesson's Setup films exactly that.
            let catalogue = guide.id == CATALOGUE_GUIDE;
            if catalogue {
                self.film_catalogue_setup()?;
            } else {
                self.film_design_setup()?;
            }
            let opening = opening_stage(guide);
            self.stage(&opening, false)?;
            self.frame(opening.focus, true, true)?;
            if catalogue {
                self.adopt_for_deep_solve()?;
            }
        }
        self.open_and_start(guide)?;
        let shown = snap(self.ui).total;
        let total = usize::try_from(shown)
            .map_err(|_| format!("the panel reports {shown} steps, which is not a count"))?;
        if total != guide.steps.len() {
            return Err(format!(
                "the panel has {total} step(s), the guide {}",
                guide.steps.len()
            ));
        }
        for n in 0..total {
            let goal = guide.steps.get(n).map(|step| &step.goal);
            self.step(guide, n, total, goal)?;
        }
        self.rec.caption("");
        self.hold(pacing::OUTRO_HOLD)?;
        self.health("the end")?;
        if snap(self.ui).open {
            return Err("the guide panel is still open after its last step".to_owned());
        }
        let summary = self.rec.finish()?;
        let seconds = summary.video_time.as_secs();
        println!(
            "tutorial-videos: wrote videos/{} ({:02}:{:02}, {} frames)",
            summary
                .output
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            seconds / 60,
            seconds % 60,
            summary.frames
        );
        println!(
            "tutorial-videos: {}: stages: {} view switches, {} library toggles, {} camera moves",
            self.id,
            self.stage_state.view_switches,
            self.stage_state.library_toggles,
            self.stage_state.camera_moves
        );
        Ok(())
    }

    /// Opens the tutorial browser, moves to the guide's Start button, presses it and waits for
    /// the first step.
    fn open_and_start(&mut self, guide: &Guide) -> Result<(), String> {
        let tutorials = self.ui.global::<indicatrix_cut::TutorialsModel>();
        // Native menu item bounds are popup-local in ElementHandle. Use the
        // real, in-window command palette so both the filmed cursor and click
        // address the same visible control on every backend.
        self.film_command("help.tutorials")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || tutorials.get_browser_open()) {
            return Err("Tutorials... command did not open the browser".into());
        }
        pump(150);
        self.rec.refresh()?;
        self.hold(pacing::BROWSER_HOLD)?;
        let row = routes::browser_row(self.ui, &guide.id)
            .ok_or_else(|| format!("the tutorial browser has no row for {}", guide.id))?;
        if !row.blocked_reason.is_empty() {
            return Err(format!(
                "the row says it cannot start: {}",
                row.blocked_reason
            ));
        }
        let wanted = format!("{} {}", row.start_label, row.title);
        let mut button = targets::run_label(&self.view(), &wanted);
        if button.is_none() {
            // The row is below the fold of the list: search for it, so it is the first row.
            let search = targets::run_label(&self.view(), "Search Tutorials")
                .ok_or("tutorial browser search field is missing")?;
            self.click_and_type(&search, row.title.as_str())?;
            if tutorials.get_query() != row.title {
                return Err("tutorial search query did not read back".into());
            }
            self.hold(Duration::from_millis(300))?;
            button = targets::run_label(&self.view(), &wanted);
        }
        let button = button.ok_or_else(|| format!("the button {wanted:?} is not on screen"))?;
        self.ui.global::<GuideModel>().set_presentation_hold(true);
        self.click_and_invoke(&button)?;
        let ui = self.ui;
        let opened = pacing::wait_real(pacing::START_TIMEOUT, || {
            let s = snap(ui);
            s.open && s.id == guide.id
        });
        if !opened {
            return Err(format!(
                "the guide did not open its first step within {} s (toast: {})",
                pacing::START_TIMEOUT.as_secs(),
                self.ui.get_toast_message()
            ));
        }
        self.rec.refresh()?;
        self.hold(pacing::AFTER_START_HOLD)?;
        self.health("tutorial browser")
    }

    /// One step: chapter and caption, the reading time, the cursor to the control the step is
    /// about, the action, and the check that the guide moved on.
    fn step(
        &mut self,
        guide: &Guide,
        n: usize,
        total: usize,
        goal: Option<&Goal>,
    ) -> Result<(), String> {
        self.hold(pacing::ENTRY_SETTLE)?;
        let entry = snap(self.ui);
        if !entry.at(n) {
            // The goal held on entry and the "Done" moment already moved on.
            let text = format!(
                "{}: step {} went by before it could be shown",
                self.id,
                n + 1
            );
            return Err(text);
        }
        let label = format!("{} step {}: {}", self.id, n + 1, entry.title);
        self.read(n, &entry)?;
        let now = snap(self.ui);
        if !now.at(n) {
            return self.verify_advanced(n, total);
        }

        let wanted_stage = stage_for(guide, n);
        self.stage(&wanted_stage, true)?;
        self.frame(wanted_stage.focus, false, true)?;

        let now = snap(self.ui);
        if !now.at(n) {
            return self.verify_advanced(n, total);
        }
        let step_perform = guide.steps.get(n).and_then(|step| step.perform.as_ref());
        // Skip hovering the outer tab pill when a filmed recipe is about to drive the controls inside it.
        let skip_point = step_perform.is_some()
            && !now.done
            && matches!(
                now.target.as_str(),
                "inspector_tier" | "preform_tab" | "new_design_dialog" | "solve_button"
            );
        if !now.target.is_empty() && !skip_point {
            self.point_at(&now.target)?;
        }
        let now = snap(self.ui);
        if !now.at(n) {
            return self.verify_advanced(n, total);
        }
        if now.done && !now.manual {
            // The goal already held when the step was shown (the app did it by itself): running
            // its recipe again would be a second activation of the same action.
            self.hold(pacing::RESULT_HOLD)?;
            self.advance_step(n)?;
        } else if self.explicit_script(n)? {
            self.hold(pacing::RESULT_HOLD)?;
            if now.manual {
                self.press_next()?;
            } else {
                if !self.wait_done(n, pacing::STEP_TIMEOUT)? {
                    return Err(format!(
                        "{label}: authored actions finished but the full goal is false"
                    ));
                }
                self.advance_step(n)?;
            }
        } else if now.manual {
            self.press_next()?;
        } else if now.offers_perform || step_perform.is_some() {
            self.perform_step(n, step_perform, &label)?;
            self.frame(CameraFocus::Keep, false, true)?;
        } else {
            self.without_recipe(n, goal, &label)?;
            self.frame(CameraFocus::Keep, false, true)?;
        }
        self.verify_advanced(n, total)?;
        self.health(&label)
    }

    /// The chapter, the caption cues and the reading hold of step `n`.
    fn read(&mut self, n: usize, entry: &Snap) -> Result<(), String> {
        let current = self.ui.global::<GuideModel>().get_current();
        let mut body: Vec<String> = vec![current.intro.to_string()];
        body.extend(
            current
                .actions
                .iter()
                .enumerate()
                .map(|(i, action)| format!("{}. {action}", i + 1)),
        );
        if !current.check.is_empty() {
            body.push(format!("Look for: {}", current.check));
        }
        let title = current.title.to_string();
        self.rec.chapter(&pacing::plain(&title));
        let cues = pacing::cues(&title, &body);
        let mut reading = pacing::reading_time(pacing::word_count(&title, &body));
        if entry.done && !entry.manual {
            reading = reading.min(pacing::ENTRY_DONE_HOLD);
        }
        let count = u32::try_from(cues.len().max(1)).unwrap_or(1);
        let each = reading / count;
        for cue in &cues {
            self.rec.caption(cue);
            self.hold(each)?;
            if !snap(self.ui).at(n) {
                break;
            }
        }
        Ok(())
    }

    /// Glides the cursor to the control the step is about and rests there. An unknown target
    /// is a note, and a target not on screen after staging is a finding.
    fn point_at(&mut self, target: &str) -> Result<(), String> {
        match targets::resolve(&self.view(), target) {
            Resolved::Found(spot) => {
                self.glide_to(spot.center())?;
                self.hold(pacing::TARGET_HOLD)
            }
            Resolved::Unmapped => {
                self.note_once(
                    target,
                    format!("target {target:?} has no entry in targets.rs; the cursor stays"),
                );
                Ok(())
            }
            Resolved::NotOnScreen => Err(format!(
                "video-stage: target {target:?} was not found on screen after staging"
            )),
        }
    }

    /// Glides the cursor to `to`, taking longer for a longer way.
    pub(crate) fn glide_to(&mut self, to: (f32, f32)) -> Result<(), String> {
        let distance = (to.0 - self.cursor.0).hypot(to.1 - self.cursor.1);
        self.rec.move_to(to, pacing::glide(distance))?;
        self.cursor = to;
        Ok(())
    }

    /// Glides to `spot`, rests a moment and draws the click ripple (nothing is pressed).
    pub(crate) fn click_on(&mut self, spot: &Spot) -> Result<(), String> {
        self.glide_to(spot.center())?;
        self.hold(pacing::BEFORE_CLICK)?;
        self.rec.click()
    }

    /// The Next button, shown being pressed, then `GuideModel.next` (the callback it ends in).
    fn press_next(&mut self) -> Result<(), String> {
        let button =
            targets::panel_button(&self.view()).ok_or("guide Next button is not visible")?;
        self.click_and_invoke(&button)?;
        pump(30);
        self.hold(pacing::AFTER_NEXT_HOLD)
    }

    /// Lets the guide move on from step `n` by itself (the "Done" moment); where it stays and
    /// the Next button is the way on, presses it.
    pub(crate) fn advance_step(&mut self, n: usize) -> Result<(), String> {
        self.ui.global::<GuideModel>().set_presentation_hold(false);
        let result = self.advance_released_step(n);
        self.ui.global::<GuideModel>().set_presentation_hold(true);
        result
    }

    fn advance_released_step(&mut self, n: usize) -> Result<(), String> {
        let mut waited = Duration::ZERO;
        while waited < pacing::ADVANCE_LIMIT {
            if !snap(self.ui).at(n) {
                return Ok(());
            }
            self.hold(pacing::WAIT_STEP)?;
            waited += pacing::WAIT_STEP;
        }
        let s = snap(self.ui);
        if !s.at(n) {
            Ok(())
        } else if s.manual {
            self.press_next()
        } else {
            Err("the step is done but the guide did not move on".to_owned())
        }
    }

    /// A step with no recipe: what it asks for is done by the route a learner takes, where the
    /// recorder can show that route; otherwise the video is abandoned.
    fn without_recipe(&mut self, n: usize, goal: Option<&Goal>, label: &str) -> Result<(), String> {
        let Some(goal) = goal else {
            return Err(format!("{label}: GAP, no goal information"));
        };
        if matches!(goal, Goal::YieldApplied) {
            return Err(format!(
                "{label}: YieldApplied needs an authored script with its value"
            ));
        } else if routes::event_only(goal) {
            let events: Vec<String> = goal.events().into_iter().map(str::to_owned).collect();
            for name in &events {
                self.drive_event(n, name)?;
            }
        } else if let Goal::All(goals) = goal {
            let mut events = Vec::new();
            for sub_goal in goals {
                match sub_goal {
                    Goal::Event(name) => events.push(name.as_str()),
                    Goal::SolvedClosed => {
                        self.film_command(indicatrix_editor::guide::CMD_SOLVE)?;
                    }
                    Goal::ViewMode(mode) => {
                        self.film_view_mode(*mode)?;
                    }
                    Goal::InspectorTab(tab) => {
                        self.film_inspector_tab(*tab)?;
                    }
                    Goal::Material(mat) => {
                        self.film_material(mat)?;
                    }
                    _ => {
                        return Err(format!(
                            "{label}: composite member {} requires an authored script",
                            routes::goal_kind(sub_goal)
                        ));
                    }
                }
            }
            for name in &events {
                self.drive_event(n, name)?;
            }
        } else {
            return Err(format!(
                "{label}: GAP, goal {} has no recipe and cannot be driven",
                routes::goal_kind(goal)
            ));
        }
        if !self.wait_done(n, pacing::STEP_TIMEOUT.min(Duration::from_secs(5)))? {
            return Err(format!(
                "{label}: the goal was not met after the step's route; waiting={:?}",
                self.ui.global::<GuideModel>().get_current().waiting
            ));
        }
        self.advance_step(n)
    }

    /// Holds in small steps until step `n` is done or left; whether it was.
    pub(crate) fn wait_done(&mut self, n: usize, limit: Duration) -> Result<bool, String> {
        let mut waited = Duration::ZERO;
        loop {
            let s = snap(self.ui);
            if !s.at(n) || s.done {
                return Ok(true);
            }
            if waited >= limit {
                return Ok(false);
            }
            self.hold(pacing::WAIT_STEP)?;
            waited += pacing::WAIT_STEP;
        }
    }

    /// After step `n` was finished: the guide is at step `n + 1`, or closed after the last step.
    fn verify_advanced(&mut self, n: usize, total: usize) -> Result<(), String> {
        let mut waited = Duration::ZERO;
        loop {
            let s = snap(self.ui);
            let advanced = if n + 1 < total { s.at(n + 1) } else { !s.open };
            if advanced {
                return Ok(());
            }
            if waited >= pacing::ADVANCE_LIMIT {
                return Err(format!(
                    "after step {} the guide shows open={} step_index={}",
                    n + 1,
                    s.open,
                    s.index
                ));
            }
            self.hold(pacing::WAIT_STEP)?;
            waited += pacing::WAIT_STEP;
        }
    }

    /// The main window as a queryable view.
    pub(crate) fn view(&self) -> WindowView {
        WindowView::of("MainWindow", self.ui)
    }

    /// Adds a note to the report, once per `key`.
    pub(crate) fn note_once(&mut self, key: &str, text: String) {
        if self.noted.insert(key.to_owned()) {
            self.report.note(text);
        }
    }

    /// Abandons the video on a panic, an unexpected file dialog, an error or warning toast, or
    /// private text on screen.
    fn health(&self, label: &str) -> Result<(), String> {
        let panics = take_panics();
        if !panics.is_empty() {
            return Err(format!("{label}: panic: {}", panics.join("; ")));
        }
        let dialogs = harness_support::take_picker_failures();
        if !dialogs.is_empty() {
            return Err(format!(
                "{label}: unexpected file dialog: {}",
                dialogs.join("; ")
            ));
        }
        if self.ui.get_toast_visible() {
            let kind = self.ui.get_toast_type().to_string();
            if kind == "error" || kind == "warning" {
                return Err(format!(
                    "{label}: {kind} toast: {}",
                    self.ui.get_toast_message()
                ));
            }
        }
        let mut windows = vec![("main", routes::visible_texts(&self.view()))];
        if let Some(help) = harness_support::help_window() {
            windows.push((
                "Help",
                routes::visible_texts(&WindowView::of("HelpWindow", &help)),
            ));
        }
        if let Some(rough) = harness_support::rough_planner_window() {
            windows.push((
                "Rough Planner",
                routes::visible_texts(&WindowView::of("RoughPlannerWindow", &rough)),
            ));
        }
        for (window, texts) in windows {
            for text in texts {
                if let Some(word) = self.cfg.private_hit(&text) {
                    return Err(format!(
                        "{label}: private text in the {window} window ({word:?}): {text:?}"
                    ));
                }
            }
        }
        Ok(())
    }
}
