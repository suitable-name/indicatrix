//! Scenarios: the design library on a seeded set of four designs -- listing, search, filters,
//! selection with the viewer tabs, and click-everything with data.
//!
//! Every scenario starts with `ctx.relaunch_with_library(&sample_designs())`, which seeds the
//! library database and relaunches the app; the click-everything engine re-seeds in its
//! `prepare` closure because its own relaunch wipes the database.

use super::{
    Piece, Scenario, existing_install,
    main_window::{click, select_top_tab},
};
use crate::support::{
    Ctx, FixtureDesign, HarnessApp, Profile,
    app::{pump, wait_until},
    sample_designs, seed_library,
};
use indicatrix_cut::{LibraryModel, MainWindow};
use slint::{ComponentHandle, Model};
use std::time::Duration;
use uiprobe::{AccessibleRole, backend::wait_until_labeled};

/// Number of rows the library list shows.
fn list_len(ui: &MainWindow) -> usize {
    ui.global::<LibraryModel>().get_diagram_list().row_count()
}

/// The titles of the listed designs, in list order.
fn list_titles(ui: &MainWindow) -> Vec<String> {
    let list = ui.global::<LibraryModel>().get_diagram_list();
    (0..list.row_count())
        .filter_map(|i| list.row_data(i))
        .map(|row| row.title.to_string())
        .collect()
}

/// Selects list row `index` by clicking its list item (labelled with the design title); drives
/// `select_diagram` when the row cannot be found. Returns the design's id and title.
fn select_row(app: &HarnessApp, index: usize) -> Option<(i32, String)> {
    let ui = app.ui();
    let library = ui.global::<LibraryModel>();
    let row = library.get_diagram_list().row_data(index)?;
    let title = row.title.to_string();
    let item = app.main_view().elements().into_iter().find(|e| {
        e.accessible_role() == Some(AccessibleRole::ListItem)
            && e.accessible_label()
                .is_some_and(|label| label.starts_with(&title))
    });
    if let Some(element) = item {
        element.invoke_accessible_default_action();
    } else {
        library.invoke_select_diagram(row.id);
        // No assertion precedes this: the detail follows in library_select_shows_detail.
        crate::support::coverage::record_callback("LibraryModel.select_diagram");
    }
    pump(150);
    Some((row.id, title))
}

/// A word that occurs in exactly one of the titles (case-insensitive), with the index of that
/// title; the full first title when no word is unique.
fn unique_fragment(titles: &[String]) -> Option<(usize, String)> {
    for (index, title) in titles.iter().enumerate() {
        let others = titles
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, t)| t.to_lowercase())
            .collect::<Vec<_>>();
        let word = title
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 3)
            .find(|w| {
                let lower = w.to_lowercase();
                others.iter().all(|o| !o.contains(&lower))
            });
        if let Some(word) = word {
            return Some((index, word.to_owned()));
        }
    }
    titles.first().map(|t| (0, t.clone()))
}

/// Whether some label of the main window contains `needle` (case-insensitive).
fn window_mentions(app: &HarnessApp, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    app.main_view()
        .labels()
        .iter()
        .any(|label| label.to_lowercase().contains(&needle))
}

/// Forces the layout pass the testing backend never runs by itself (its mock renderer draws
/// nothing, so a `ListView` or an `if` card that appeared after a model change is instantiated
/// only when something evaluates the layout): the window is resized by one pixel and back, which
/// invalidates and re-solves every layout, and the geometry of all controls is read.
///
/// The HEIGHT is nudged, never the width: app.slint's `changed width` re-runs
/// `sync_library_auto_collapse`, which folds a library the harness had just opened back onto its
/// rail whenever the window sits in an auto-collapse band (1366 px does), so a width nudge
/// removed the very list rows and empty-state card the checks below look for.
fn nudge_layout(app: &HarnessApp) {
    let window = app.ui().window();
    let size = window.size().to_logical(window.scale_factor());
    window.set_size(slint::LogicalSize::new(size.width, size.height + 1.0));
    pump(30);
    window.set_size(size);
    pump(30);
    let _ = app.main_view().controls();
}

/// Brings the library list back when the window folded it onto its rail. At the harness's
/// 1366 px width the Edit view is narrower than `library_wants_auto_collapse` allows (app.slint
/// ~411: the library folds below ~1386 px), so the main window launches with `CollapsedLibraryRail`
/// instead of `DiagramListView` and no list row or "No diagrams found" card exists to find.
fn ensure_library_open(ctx: &Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    let library = ui.global::<LibraryModel>();
    if !library.get_panel_collapsed() {
        return;
    }
    if !click(ctx.app, "Show the Library List") {
        // GAP: the rail button is not reachable by label; the property it sets is driven.
        library.set_panel_collapsed(false);
    }
    pump(100);
    nudge_layout(ctx.app);
}

/// How many pieces the library click-everything pass (143 s in one piece) is cut into: each piece
/// presses the controls whose key hashes into its part (`support::part`).
const CLICK_PARTS: usize = 3;

/// The pieces of this file.
pub fn pieces() -> Vec<Piece> {
    let mut all: Vec<Piece> = vec![
        Scenario {
            name: "library_lists_seeded_designs",
            seed: existing_install,
            run: library_lists_seeded_designs,
        }
        .into(),
        Scenario {
            name: "library_search_and_filters",
            seed: existing_install,
            run: library_search_and_filters,
        }
        .into(),
        Scenario {
            name: "library_select_shows_detail",
            seed: existing_install,
            run: library_select_shows_detail,
        }
        .into(),
    ];
    for index in 0..CLICK_PARTS {
        all.push(
            Piece::new(
                format!(
                    "library_click_everything_with_data:{}of{CLICK_PARTS}",
                    index + 1
                ),
                existing_install,
                library_click_everything_with_data,
            )
            .part(index, CLICK_PARTS),
        );
    }
    all
}

/// Four seeded designs. Proves the list shows exactly those four, by title, with matching
/// counts, and sweeps clean.
fn library_lists_seeded_designs(ctx: &mut Ctx<'_>) {
    let designs = sample_designs();
    if !ctx.relaunch_with_library(&designs) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let library = ui.global::<LibraryModel>();
    let titles = list_titles(&ui);
    ctx.expect(
        titles.len() == 4 && designs.len() == 4,
        "library-row-count",
        format!(
            "{} row(s) listed for {} seeded design(s), expected 4",
            titles.len(),
            designs.len()
        ),
    );
    for design in &designs {
        let wanted = design.title.to_lowercase();
        let listed = titles.iter().any(|t| t.to_lowercase().contains(&wanted));
        ctx.expect(
            listed,
            "library-title",
            format!(
                "the design {:?} is not in the list {titles:?}",
                design.title
            ),
        );
    }
    ctx.expect(
        library.get_total_count() == 4 && library.get_matched_count() == 4,
        "library-counts",
        format!(
            "total_count={}, matched_count={}",
            library.get_total_count(),
            library.get_matched_count()
        ),
    );
    ensure_library_open(ctx);
    // The ListView instantiates its rows when its viewport is laid out, a pass after the model
    // changed: give it real pumps before concluding nothing is drawn.
    let app = &*ctx.app;
    nudge_layout(app);
    let drawn = wait_until(Duration::from_secs(10), || {
        app.main_view()
            .elements()
            .into_iter()
            .any(|e| e.accessible_role() == Some(AccessibleRole::ListItem))
    });
    ctx.expect(
        drawn,
        "library-rows-visible",
        "no list item is drawn although the model holds rows",
    );
    ctx.check_health("library listed");
    ctx.sweep_main_at_sizes("library with four designs");
}

/// Search by a title fragment, clear it, open the filter panel, toggle a filter, force the empty
/// state with an impossible refractive-index range and reset. Proves the list follows each.
fn library_search_and_filters(ctx: &mut Ctx<'_>) {
    let designs = sample_designs();
    if !ctx.relaunch_with_library(&designs) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let library = ui.global::<LibraryModel>();
    let total = list_len(&ui);
    let Some((_, fragment)) = unique_fragment(&list_titles(&ui)) else {
        ctx.expect(false, "library-titles", "the library list has no titles");
        return;
    };

    library.set_search_text(fragment.as_str().into());
    library.invoke_search_changed(fragment.as_str().into());
    if ctx.wait_for(
        &format!("search {fragment:?} narrows the list to one design"),
        Duration::from_secs(8),
        || list_len(&ui) == 1,
    ) {
        crate::support::coverage::record_callback("LibraryModel.search_changed");
    }
    ctx.check_health("library search");
    ctx.sweep_main_at_sizes("library search: one result");
    if !click(ctx.app, "Clear Search") {
        // GAP: the clear button is not reachable by label; the callback the typing fires is driven.
        library.set_search_text("".into());
        library.invoke_search_changed("".into());
    }
    ctx.wait_for(
        "clearing the search shows every design again",
        Duration::from_secs(8),
        || list_len(&ui) == total,
    );

    let opened = click(ctx.app, "Filters");
    pump(200);
    let panel_drawn = ctx.app.main_view().has("Advanced Filters");
    ctx.expect(
        opened && panel_drawn,
        "filter-panel",
        "the Filters button did not open the Advanced Filters panel",
    );
    ctx.sweep_main_at_sizes("library filter panel open");
    toggle_my_designs(ctx, &ui, total);
    force_empty_state(ctx, &ui, total);
    if !click(ctx.app, "Close the Filters") {
        ctx.press("Escape");
    }
    ctx.check_health("library filters");
}

/// "My Designs Only" on and off: the flag follows and the list returns to every design.
fn toggle_my_designs(ctx: &mut Ctx<'_>, ui: &MainWindow, total: usize) {
    let library = ui.global::<LibraryModel>();
    for wanted in [true, false] {
        if !click(ctx.app, "My Designs Only") {
            // GAP: the toggle is not reachable by label; its two lines are driven.
            library.set_local_only_filter(wanted);
            library.invoke_filters_changed();
        }
        pump(400);
        ctx.expect(
            library.get_local_only_filter() == wanted,
            "filter-my-designs",
            format!(
                "My Designs Only is {}, expected {wanted}",
                library.get_local_only_filter()
            ),
        );
        if wanted {
            ctx.report.note(format!(
                "library: My Designs Only shows {} of {total} seeded design(s)",
                list_len(ui)
            ));
        }
    }
    ctx.wait_for(
        "My Designs Only off shows every design again",
        Duration::from_secs(8),
        || list_len(ui) == total,
    );
}

/// A refractive-index range no design has: the list must empty and say so; Reset Filters must
/// bring every design back.
fn force_empty_state(ctx: &mut Ctx<'_>, ui: &MainWindow, total: usize) {
    let library = ui.global::<LibraryModel>();
    library.set_ri_filter_min(2.99);
    library.invoke_filters_changed();
    if ctx.wait_for(
        "an impossible RI range empties the list",
        Duration::from_secs(8),
        || list_len(ui) == 0,
    ) {
        crate::support::coverage::record_callback("LibraryModel.filters_changed");
    }
    // The empty-state card is instantiated by the pump after the model changed, and only while
    // the library list is not folded onto its rail.
    ensure_library_open(ctx);
    let app = &*ctx.app;
    nudge_layout(app);
    let message = wait_until(Duration::from_secs(10), || {
        app.main_view().has("No diagrams found")
    });
    ctx.expect(
        message,
        "library-empty-state",
        "the list is empty but no \"No diagrams found\" message is shown",
    );
    ctx.sweep_main_at_sizes("library empty state");
    if !click(ctx.app, "Reset Filters") {
        // GAP: Reset Filters is not reachable by label; the range is restored through the model.
        library.set_ri_filter_min(library.get_ri_bounds_min());
        library.invoke_filters_changed();
    }
    ctx.wait_for(
        "Reset Filters shows every design again",
        Duration::from_secs(8),
        || list_len(ui) == total,
    );
}

/// Selects designs and visits the three viewer tabs. Proves the selection id, the detail title,
/// the cutting instructions and the title drawn on screen follow the selected design.
fn library_select_shows_detail(ctx: &mut Ctx<'_>) {
    let designs = sample_designs();
    if !ctx.relaunch_with_library(&designs) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let library = ui.global::<LibraryModel>();
    let Some((id, title)) = select_row(ctx.app, 0) else {
        ctx.expect(false, "library-select", "the library list is empty");
        return;
    };
    let needle = title.to_lowercase();
    ctx.wait_for(
        "the detail follows the selection",
        Duration::from_secs(8),
        || {
            library.get_selected_entry_id() == id
                && library
                    .get_current_detail()
                    .title
                    .to_lowercase()
                    .contains(&needle)
        },
    );
    let title_drawn = window_mentions(ctx.app, &title);
    ctx.expect(
        title_drawn,
        "detail-title-drawn",
        format!("no label of the 3D tab mentions {title:?}"),
    );
    ctx.check_health("design selected");
    ctx.sweep_main_at_sizes("library design selected: 3D tab");

    select_top_tab(ctx.app, 1);
    ctx.wait_for("cutting instructions load", Duration::from_secs(8), || {
        library.get_current_angles().row_count() > 0
    });
    ctx.check_health("Cutting Instructions tab");
    ctx.sweep_main_at_sizes("library design selected: Cutting Instructions tab");

    select_top_tab(ctx.app, 2);
    pump(300);
    let files = library.get_current_files();
    match files.row_data(0) {
        Some(file) => {
            let drawn = window_mentions(ctx.app, &file.name);
            ctx.expect(
                drawn,
                "files-listed",
                format!("the file {:?} is in the model but not drawn", file.name),
            );
        }
        None => ctx
            .report
            .note("library: the selected design lists no files".to_owned()),
    }
    ctx.check_health("Files tab");
    ctx.sweep_main_at_sizes("library design selected: Files tab");

    // A second selection replaces the detail.
    select_top_tab(ctx.app, 0);
    if let Some((second_id, second_title)) = select_row(ctx.app, 1) {
        let second = second_title.to_lowercase();
        ctx.wait_for(
            "the detail switches to the second design",
            Duration::from_secs(8),
            || {
                library.get_selected_entry_id() == second_id
                    && library
                        .get_current_detail()
                        .title
                        .to_lowercase()
                        .contains(&second)
            },
        );
    }
    ctx.check_health("second design selected");
}

/// Shuts the app down, seeds `designs`, launches again and selects the first row: the state of
/// the library click-everything pass, re-established after every relaunch the engine makes.
fn reseed_and_select(app: &mut HarnessApp, profile: &Profile, designs: &[FixtureDesign]) {
    app.shutdown();
    if let Err(e) = seed_library(profile, designs) {
        eprintln!("ui-harness: cannot seed the library: {e}");
    }
    match HarnessApp::launch() {
        Ok(fresh) => *app = fresh,
        Err(e) => {
            eprintln!("ui-harness: cannot relaunch after seeding: {e}");
            return;
        }
    }
    let wanted = designs.len();
    let _ = wait_until_labeled("library-load", Duration::from_secs(10), || {
        list_len(app.ui()) >= wanted
    });
    let _ = select_row(app, 0);
}

/// The click-everything pass over the library with four designs and the first one selected.
/// Delete and remove controls are skipped by the engine's default deny list.
fn library_click_everything_with_data(ctx: &mut Ctx<'_>) {
    let designs = sample_designs();
    let profile = ctx.profile;
    let _stats = ctx.smoke_main(
        "library with four designs, one selected",
        &mut |app: &mut HarnessApp| reseed_and_select(app, profile, &designs),
    );
    ctx.check_health("library click-everything");
}
