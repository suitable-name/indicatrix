//! Every time constant of a tutorial video in one place, plus the pure text and timing helpers
//! (reading time, glide duration, caption cues) and the two real-time waits.

use crate::support::app::pump;
use std::time::{Duration, Instant};
use uiprobe::recording::Recorder;

/// Time the first picture stays before anything moves (the "Intro" chapter).
pub const INTRO_HOLD: Duration = Duration::from_millis(1000);
/// Time the tutorial browser stays on screen before the cursor goes to the row.
pub const BROWSER_HOLD: Duration = Duration::from_millis(800);
/// Time after the Start press, for the first step to appear.
pub const AFTER_START_HOLD: Duration = Duration::from_millis(300);
/// Frames shown for a step that has just been entered, before it is read (its goal may already
/// hold, and the app may already have moved on).
pub const ENTRY_SETTLE: Duration = Duration::from_millis(150);
/// The reading time of a step whose goal already held on entry: the app moves on by itself
/// after its 700 ms "Done" moment, so the step must not be held longer than that.
pub const ENTRY_DONE_HOLD: Duration = Duration::from_millis(500);
/// Time the cursor rests on the control a step is about.
pub const TARGET_HOLD: Duration = Duration::from_millis(800);
/// Time between the cursor arriving on a button and the click ripple.
pub const BEFORE_CLICK: Duration = Duration::from_millis(150);
/// Time after Next, for the next step to build.
pub const AFTER_NEXT_HOLD: Duration = Duration::from_millis(400);
/// Time held after a stage transition (inspector tab, parameters dock, solid view mode, library toggle).
pub const STAGE_HOLD: Duration = Duration::from_millis(400);
/// Time held after a view switch (Edit <-> Live Render).
pub const VIEW_SWITCH_HOLD: Duration = Duration::from_millis(500);
/// Multiplier applied to fit distance for framing margin (8%).
pub const FIT_MARGIN: f32 = 1.08;
/// Minimum stone radius change (3%) to trigger a camera refit when focus is Keep.
pub const REFIT_THRESHOLD: f64 = 0.03;
/// Time the result of a step the app performed stays on screen before the guide moves on.
/// Shorter than the app's 700 ms "Done" moment, so the step is still the current one.
pub const RESULT_HOLD: Duration = Duration::from_millis(600);
/// Time a settled live render stays on screen after a step that started it.
pub const RENDER_HOLD: Duration = Duration::from_millis(1200);
/// Time the File menu stays open before the cursor goes to its item.
pub const MENU_HOLD: Duration = Duration::from_millis(600);
/// Time the final state stays on screen after the last step.
pub const OUTRO_HOLD: Duration = Duration::from_millis(2000);
/// How long after a finished step the guide may take to show the next one (the "Done" moment
/// is 700 ms) before the video is abandoned.
pub const ADVANCE_LIMIT: Duration = Duration::from_millis(1600);
/// The granularity of the waits that emit frames.
pub const WAIT_STEP: Duration = Duration::from_millis(100);

/// How long one step may take (a solve takes seconds), real time.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to keep waiting after the recipe gave up (a late solve result still completes the
/// step in the app).
pub const GIVE_UP_GRACE: Duration = Duration::from_secs(5);
/// How long a started tutorial may take to open its first step.
pub const START_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the design a tutorial needs may take to appear and solve.
pub const PREPARE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a live render may take to settle after a step (a cut, never shown).
pub const RENDER_CAP: Duration = Duration::from_secs(60);
/// How long the library copy may take to list its first design.
pub const LIBRARY_LOAD: Duration = Duration::from_secs(120);

/// A background wait is shown sped up by this factor (V-D4: 8x).
pub const LAPSE_SPEED: u32 = 8;
/// A background wait never emits more than this much video (V-D4: 4 s); the rest is a cut.
pub const LAPSE_MAX_VIDEO: Duration = Duration::from_secs(4);
/// The video time one sampled frame of a time-lapse stands for (a little over one frame at
/// 30 fps, so every sample emits a frame).
pub const LAPSE_FRAME: Duration = Duration::from_millis(34);

/// The widest caption line, in characters.
const CAPTION_WIDTH: usize = 80;
/// The shortest and longest time a step is read.
const READ_MIN_SECS: f32 = 2.5;
const READ_MAX_SECS: f32 = 8.0;
/// Reading speed: words per second.
const WORDS_PER_SECOND: f32 = 3.0;

/// How long a step with `words` words of text is held for reading: `words / 3` seconds, clamped
/// to 2.5 to 8 seconds.
#[must_use]
pub fn reading_time(words: usize) -> Duration {
    // A word count is far below 2^24, where f32 is exact (`cast_precision_loss` is allowed
    // workspace-wide).
    let seconds = words as f32 / WORDS_PER_SECOND;
    Duration::from_secs_f32(seconds.clamp(READ_MIN_SECS, READ_MAX_SECS))
}

/// How long a cursor glide over `distance` logical pixels takes: 0.45 s plus 1 s per 2400 px,
/// kept between 0.5 s and 0.9 s.
#[must_use]
pub fn glide(distance: f32) -> Duration {
    Duration::from_secs_f32((0.45 + distance.abs() / 2400.0).clamp(0.5, 0.9))
}

/// The text without the light markup a guide may use (`**bold**`, `` `code` ``), on one line
/// with single spaces.
#[must_use]
pub fn plain(text: &str) -> String {
    let stripped: String = text.chars().filter(|c| !matches!(c, '*' | '`')).collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` broken into lines of at most `width` characters (a longer word has a line of its own).
#[must_use]
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let joined = line.chars().count() + 1 + word.chars().count();
        if !line.is_empty() && joined > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The caption cues of a step: the title and the step's text as lines of at most 80 characters,
/// two lines to a cue. The title opens the first cue.
#[must_use]
pub fn cues(title: &str, body: &[String]) -> Vec<String> {
    let mut lines = vec![plain(title)];
    for piece in body {
        lines.extend(wrap(&plain(piece), CAPTION_WIDTH));
    }
    lines.chunks(2).map(|pair| pair.join("\n")).collect()
}

/// The number of words in `texts`.
#[must_use]
pub fn word_count(title: &str, body: &[String]) -> usize {
    title.split_whitespace().count()
        + body
            .iter()
            .map(|piece| piece.split_whitespace().count())
            .sum::<usize>()
}

/// Pumps until `condition` holds or `timeout` of real time has passed, with mock time advancing
/// by the real time that passed (not a fixed 10 ms per round), so the guide's timers and a
/// recipe's tick budget run at the speed they do in the app while worker threads solve. Shows
/// nothing: in a video this is a cut. (Copy of the UI harness' `wait_real`.)
pub fn wait_real(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let mut last = start;
    loop {
        let now = Instant::now();
        let ms = u64::try_from(now.duration_since(last).as_millis()).unwrap_or(10);
        last = now;
        pump(ms.clamp(5, 100));
        if condition() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The wait of a step the app performs, as a time-lapse: like [`wait_real`], and meanwhile one
/// frame is emitted per [`LAPSE_SPEED`] x [`LAPSE_FRAME`] of real time until [`LAPSE_MAX_VIDEO`]
/// of video exists; after that the wait goes on as a cut. The state at the moment `condition`
/// holds is shown if the budget has not run out. Returns whether `condition` held in time.
///
/// # Errors
///
/// A recorder error.
pub fn wait_lapse(
    rec: &mut Recorder<'_>,
    timeout: Duration,
    mut condition: impl FnMut() -> bool,
) -> Result<bool, String> {
    let start = Instant::now();
    let mut last = start;
    let mut next_frame = start;
    let mut shown = Duration::ZERO;
    loop {
        let now = Instant::now();
        let ms = u64::try_from(now.duration_since(last).as_millis()).unwrap_or(10);
        last = now;
        pump(ms.clamp(5, 100));
        let held = condition();
        if shown < LAPSE_MAX_VIDEO && (held || now >= next_frame) {
            rec.hold(LAPSE_FRAME)?;
            shown += LAPSE_FRAME;
            next_frame = now + LAPSE_FRAME * LAPSE_SPEED;
        }
        if held {
            return Ok(true);
        }
        if start.elapsed() >= timeout {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
