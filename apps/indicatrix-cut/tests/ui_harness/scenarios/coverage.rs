//! The direct-invoke pass of the coverage gate (`support/coverage.rs`).
//!
//! Proves: every global callback that has a Rust handler and was not already driven by an
//! earlier scenario (recorded with `coverage::record_callback`) is invoked through the compiled
//! API from a state where it applies, and invoking it neither panics nor ends the run. The
//! rows come from the generated table in `coverage/rows_*.rs`; the groups are the shared states
//! of `coverage/setups.rs`, relaunched once per group (and before every `fresh` row).
//!
//! Callbacks on the allow list (`coverage_allow.rs`) have no row. The gate itself runs at the
//! end of the whole run (`coverage::finish`, called from `main`).
//!
//! Deviation from `Ctx::check_health`: an error toast after a row is only a note here. Rows
//! call handlers with empty arguments ("select a design first" toasts are the correct
//! answer); a panic, an unexpected file dialog and a closed main window are findings.

use super::{Piece, existing_install};
use crate::support::{
    Ctx, Finding,
    app::{pump, settle, wait_until},
    coverage,
    smoke::take_panics,
};
use indicatrix_cut::{EditorModel, harness_support};
use slint::ComponentHandle;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    time::Duration,
};
use uiprobe::timing;

#[macro_use]
mod table;
mod overlay_states;
mod overlays;
mod rows_checked;
mod rows_editor;
mod rows_library;
mod rows_startup;
mod rows_windows;
mod setups;
mod states_dialogs;
mod states_editor;
mod states_more;
mod states_reach;
mod windows_more;
mod windows_reach;

use table::{Invoke, Row, Setup};

/// The pieces of this file: the click-through passes inside the overlays and secondary states the
/// other scenarios never clicked through (`overlays.rs`, one piece per state or window state), then
/// the direct-invoke chunks (`coverage_invoke:<group>:<n>`), which are `last` pieces.
pub fn pieces() -> Vec<Piece> {
    let mut all = Vec::new();
    all.extend(overlays::pieces());
    all.extend(windows_more::pieces());
    all.extend(states_editor::pieces());
    all.extend(states_dialogs::pieces());
    all.extend(states_reach::pieces());
    all.extend(windows_reach::pieces());
    all.extend(invoke_pieces());
    all
}

/// A group name and the function listing its states, as the `states_*` files table them for
/// [`state_pieces`].
type StateGroup = (&'static str, fn() -> Vec<overlay_states::State>);

/// A pass name and the scenario body that runs it (`overlays.rs`'s secondary-window passes).
type Pass = (&'static str, fn(&mut Ctx<'_>));

/// One piece per state of `states`, named `overlay:<group>:<state name>`: the state's click pass
/// runs in a piece (and so in a worker) of its own. `states` is called again inside the piece (a
/// `State` is plain data and `fn` pointers, not `Clone`) and the piece runs the entry at its index.
fn state_pieces(
    group: &'static str,
    seed: fn() -> crate::support::SettingsSeed,
    states: fn() -> Vec<overlay_states::State>,
) -> Vec<Piece> {
    states()
        .iter()
        .enumerate()
        .map(|(index, state)| {
            Piece::new(
                format!("overlay:{group}:{}", state.name),
                seed,
                move |ctx| {
                    let all = states();
                    if let Some(state) = all.get(index) {
                        overlays::run_states(ctx, std::slice::from_ref(state));
                    }
                },
            )
        })
        .collect()
}

/// Rows per direct-invoke chunk: the editor group (292 rows) and the startup group (99) are cut
/// into pieces of at most this many rows, so no chunk runs much longer than a minute.
const CHUNK: usize = 40;

/// The rows of `setup`, in the order the group runs them (shared-state rows first, then the
/// `fresh` ones; the sort is stable). Chunk `n` is rows `n * CHUNK ..` of this list, computed
/// BEFORE the already-driven callbacks are filtered out, so every process cuts the same chunks.
fn group_rows(rows: &[&'static Row], setup: Setup) -> Vec<&'static Row> {
    let mut group: Vec<&'static Row> = rows
        .iter()
        .copied()
        .filter(|row| row.setup == setup)
        .collect();
    group.sort_by_key(|row| row.fresh);
    group
}

/// The rows of chunk `chunk` of `setup` that no other piece has driven yet.
fn chunk_todo(setup: Setup, chunk: usize) -> Vec<&'static Row> {
    let already = coverage::recorded_callbacks();
    group_rows(&all_rows(), setup)
        .chunks(CHUNK)
        .nth(chunk)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|row| !already.contains(&row.key()))
        .collect()
}

/// The direct-invoke chunks: `coverage_invoke:<group>:<n>`. They are registered as `last` pieces,
/// so in a parallel run they see the coverage of every worker and invoke only what is left; a
/// chunk with nothing left skips the relaunch altogether. Every chunk arms the coverage gate (the
/// gate is armed in the process that reports it: a worker sends its armed flag with every `done`).
fn invoke_pieces() -> Vec<Piece> {
    let rows = all_rows();
    let mut pieces = Vec::new();
    for setup in Setup::ORDER {
        let total = group_rows(&rows, setup).len();
        for chunk in 0..total.div_ceil(CHUNK) {
            pieces.push(
                Piece::new(
                    format!("coverage_invoke:{}:{chunk}", setup.name()),
                    existing_install,
                    move |ctx| invoke_chunk(ctx, setup, chunk),
                )
                .needed_if(move || {
                    coverage::arm_gate();
                    !chunk_todo(setup, chunk).is_empty()
                })
                .last(),
            );
        }
    }
    pieces
}

/// Every row, in file order (the order inside a group follows from it). A checked row
/// (`rows_checked.rs`: valid arguments and an asserted effect) replaces the generated row with the
/// same key, whose arguments are only the empty values of the parameter types.
fn all_rows() -> Vec<&'static Row> {
    let checked = rows_checked::rows();
    let replaced: Vec<String> = checked.iter().map(|row| row.key()).collect();
    let mut rows = rows_startup::rows();
    rows.extend(rows_library::rows());
    rows.extend(rows_editor::rows());
    rows.extend(rows_windows::rows());
    rows.retain(|row| !replaced.contains(&row.key()));
    rows.extend(checked);
    rows
}

/// Callbacks that copy to the clipboard, with whether a default-argument row must capture text.
/// The harness build captures the text instead of starting the OS clipboard tool. `false`: the
/// callback needs data none of the groups has (a finished sweep), so a capture is not demanded.
const COPY_ROWS: &[(&str, bool)] = &[
    ("LibraryModel.copy_text", true),
    ("LibraryModel.copy_cutting_table", true),
    ("LibraryModel.copy_cutting_row", true),
    ("ViewportModel.copy_metrics", true),
    ("TiltModel.copy_curve_data", true),
    ("ShortcutsModel.copy_markdown", true),
    ("SweepModel.copy_csv", false),
];

/// Callbacks that open a URL, folder or file, with whether a row must capture a target. `false`:
/// the callback needs a finished render job (GAP: no group creates one).
const OPEN_ROWS: &[(&str, bool)] = &[
    ("LibraryModel.open_diagram_url", true),
    ("HelpModel.open_manual_files", true),
    ("EditorModel.reveal_last_saved_path", true),
    ("RenderJobsModel.show_result", false),
];

/// The expectation table entry of `key` in `table`, if it has one.
fn expectation(table: &[(&str, bool)], key: &str) -> Option<bool> {
    table
        .iter()
        .find(|(entry, _)| *entry == key)
        .map(|(_, must)| *must)
}

/// Invokes the callbacks of one chunk of one group that no other piece reached.
fn invoke_chunk(ctx: &mut Ctx<'_>, setup: Setup, chunk: usize) {
    coverage::arm_gate();
    let todo = chunk_todo(setup, chunk);
    if todo.is_empty() {
        return;
    }
    println!(
        "ui-harness:   coverage group {} chunk {chunk}: {} row(s) left",
        setup.name(),
        todo.len()
    );
    let invoked = run_group(ctx, setup, &todo);
    println!(
        "ui-harness:   coverage group {} chunk {chunk}: {invoked} callback(s) invoked directly",
        setup.name()
    );
}

/// Prepares `setup`, recording a finding when that fails.
fn prepare(ctx: &mut Ctx<'_>, setup: Setup) -> bool {
    match timing::time(format!("setup:{}", setup.name()), || {
        setups::prepare(ctx, setup)
    }) {
        Ok(()) => {
            let _ = take_panics();
            harness_support::clear_picker_answers();
            let _ = harness_support::take_picker_failures();
            true
        }
        Err(message) => {
            ctx.report.add_simple(
                "-",
                "coverage",
                format!("direct-invoke setup `{}` failed: {message}", setup.name()),
            );
            false
        }
    }
}

/// Runs the rows of one group; returns how many were invoked.
fn run_group(ctx: &mut Ctx<'_>, setup: Setup, rows: &[&Row]) -> usize {
    let mut ready = prepare(ctx, setup);
    if !ready {
        return 0;
    }
    let mut invoked = 0;
    // Shared-state rows first, then the `fresh` ones (the table is written in that order, the
    // sort is stable).
    let mut ordered: Vec<&Row> = rows.to_vec();
    ordered.sort_by_key(|row| row.fresh);
    for row in ordered {
        if row.fresh || !ready {
            ready = prepare(ctx, setup);
            if !ready {
                return invoked;
            }
        }
        invoked += 1;
        run_row(ctx, row);
        ready = setups::still_usable(ctx, setup);
    }
    invoked
}

/// Invokes one row and checks what it left behind.
fn run_row(ctx: &mut Ctx<'_>, row: &Row) {
    let key = row.key();
    let _ = take_panics();
    let _ = harness_support::take_picker_failures();
    let _ = harness_support::take_clipboard_writes();
    let _ = harness_support::take_opened();
    // A handler that opens a file dialog gets "cancelled"; a few in a row cover chains.
    for _ in 0..3 {
        harness_support::queue_picker_cancel();
    }
    let ui = ctx.app.ui();
    if key == "EditorModel.reveal_last_saved_path" {
        // The button only exists once something was saved; point it at the profile's folder.
        if let Ok(dir) = std::env::current_dir() {
            let saved = dir.join("saved.isdd").to_string_lossy().into_owned();
            ui.global::<EditorModel>()
                .set_last_saved_path(saved.as_str().into());
        }
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| call(row, ui)));
    // Was `pump(60); settle(60)`: 60 ms of mock time (debounce timers) and 60 ms of REAL time for
    // whatever worker thread the handler started. Nothing is observable for a row in general (the
    // copy and opener rows have their own wait in `check_captures`), so the mock time stays and
    // the real pause drops to 20 ms (events a worker queues meanwhile are drained by the next
    // row's pumps; a worker panic later than that can be missed, as it could before).
    pump(60);
    settle(20);
    check_captures(ctx, &key);
    match &outcome {
        // The window the row needs was gone: nothing ran, so nothing is recorded.
        Ok(Ok(false)) => {
            ctx.report.add_simple(
                "-",
                "coverage",
                format!("{key}: the window its group needs was not open when the row ran"),
            );
        }
        Ok(Ok(true)) | Err(_) => coverage::record_callback(&key),
        // The handler ran, but its observable effect was not there.
        Ok(Err(message)) => {
            coverage::record_callback(&key);
            ctx.report
                .add_simple("MainWindow", "coverage-effect", format!("{key}: {message}"));
        }
    }
    harness_support::clear_picker_answers();

    let panics = take_panics();
    if outcome.is_err() || !panics.is_empty() {
        let first = panics.first().map_or("panic payload lost", String::as_str);
        let detail: String = first.chars().take(300).collect();
        ctx.report.add(Finding::new(
            "MainWindow",
            key.clone(),
            "panic",
            format!("direct invoke with default arguments: {detail}"),
        ));
    }
    if !harness_support::take_picker_failures().is_empty() {
        ctx.report.note(format!(
            "coverage pass: {key} opened more file dialogs than the three cancels queued"
        ));
    }
    if !ctx.app.is_open() {
        ctx.report.add(Finding::new(
            "MainWindow",
            key.clone(),
            "main-closed",
            "the main window is no longer shown after the direct invoke",
        ));
        return;
    }
    let ui = ctx.app.ui();
    if ui.get_toast_visible() {
        let kind = ui.get_toast_type().to_string();
        if kind == "error" || kind == "warning" {
            ctx.report.note(format!(
                "coverage pass: {key} raised a {kind} toast with default arguments: {}",
                ui.get_toast_message()
            ));
        }
        ui.set_toast_visible(false);
    }
}

/// A copy row must have put non-empty text on the (captured) clipboard and an opener row must
/// have handed a target to the (captured) opener, when its setup has the data for it. The copy
/// runs on a worker thread, so this waits for the capture.
fn check_captures(ctx: &mut Ctx<'_>, key: &str) {
    let copy = expectation(COPY_ROWS, key);
    let open = expectation(OPEN_ROWS, key);
    let want_copy = copy == Some(true);
    let want_open = open == Some(true);
    let mut writes: Vec<String> = Vec::new();
    let mut opened: Vec<String> = Vec::new();
    let _ = wait_until(Duration::from_secs(3), || {
        writes.extend(harness_support::take_clipboard_writes());
        opened.extend(harness_support::take_opened());
        (!want_copy || !writes.is_empty()) && (!want_open || !opened.is_empty())
    });
    if want_copy && writes.is_empty() {
        ctx.report.add(Finding::new(
            "MainWindow",
            key.to_owned(),
            "coverage-capture",
            "a copy callback put nothing on the (captured) clipboard",
        ));
    } else if want_copy && writes.iter().all(|text| text.trim().is_empty()) {
        ctx.report.add(Finding::new(
            "MainWindow",
            key.to_owned(),
            "coverage-capture",
            "a copy callback copied only empty text",
        ));
    }
    if want_open && opened.is_empty() {
        ctx.report.add(Finding::new(
            "MainWindow",
            key.to_owned(),
            "coverage-capture",
            "an opener callback handed no target to the (captured) opener",
        ));
    }
}

/// The call itself. `Ok(false)` when the secondary window the row needs is not open (closed by
/// an earlier row), in which case nothing was invoked; `Err` is the failed effect check of a
/// [`Invoke::Checked`] row (the callback did run).
fn call(row: &Row, ui: &indicatrix_cut::MainWindow) -> Result<bool, String> {
    Ok(match row.invoke {
        Invoke::Main(f) => {
            f(ui);
            true
        }
        Invoke::Checked(f) => {
            f(ui)?;
            true
        }
        Invoke::Planner(f) => harness_support::rough_planner_window().is_some_and(|w| {
            f(&w);
            true
        }),
        Invoke::Compare(f) => harness_support::compare_window().is_some_and(|w| {
            f(&w);
            true
        }),
        Invoke::Help(f) => harness_support::help_window().is_some_and(|w| {
            f(&w);
            true
        }),
    })
}
