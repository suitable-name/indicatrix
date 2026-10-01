//! Showing a list of layouts: the previews, tilt curves and extents are loaded and every
//! row is formatted on a worker thread; the UI thread only wraps the finished strings in
//! Slint values, stores the state and tells the view.
//!
//! Fresh plans and loaded plans both come through [`show_layouts`], so their rows look
//! alike.

use super::{
    Host, on_host, panic_message,
    stages::join_or_resume,
    start::restore_previous,
    state::{ResultsSource, ShownLayouts},
};
use crate::{
    RoughMetric, RoughPlanModel, RoughPlanResult, RoughPlanStoneGroup,
    gui::{
        batch::batch_queue::local_lane_count,
        rough_plan::{
            exclusions::sync_row_flags,
            format::{DesignData, ResultRow, RowContext, effective_id, result_row, to_i32},
            metrics::{ModelGeometry, decode_preview_pixels, palette_swatch},
            view,
        },
    },
};
use indicatrix_cut_core::rough_plan::RoughLayout;
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::{
    collections::{BTreeMap, BTreeSet},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{Mutex, PoisonError},
};
use tracing::warn;

/// The decoded library previews by entry id.
type Previews = BTreeMap<i64, SharedPixelBuffer<Rgba8Pixel>>;

/// What runs on the UI thread once the rows are pushed (the run's tidying up).
pub(in crate::gui::rough_plan) type AfterShown = fn(&Rc<Host>);

/// The formatted rows and the previews they use: plain data that can cross threads.
struct Rendered {
    rows: Vec<ResultRow>,
    previews: Previews,
}

/// Shows `state` as the window's result list.
///
/// Loads what the rows need off the UI thread, formats them, then (back on the UI thread)
/// pushes `results`, `summary`, `keep_count` and `selected_result`, stores the state,
/// calls [`view::results_changed`] and runs `after`, if any. A later call supersedes an
/// earlier one that has not finished; `after` still runs for the superseded one.
pub(in crate::gui::rough_plan) fn show_layouts(
    host: &Rc<Host>,
    state: ShownLayouts,
    after: Option<AfterShown>,
) {
    let generation = {
        let mut session = host.session.borrow_mut();
        session.run.generation += 1;
        session.run.generation
    };
    let (db, weak) = (host.db.clone(), host.window.as_weak());
    let spawned = std::thread::Builder::new()
        .name("rough-plan-rows".to_string())
        .spawn(move || {
            let rendered = catch_unwind(AssertUnwindSafe(|| render(&db, &state)))
                .map_err(|_| "Could not format the results.".to_string());
            let _ = weak.upgrade_in_event_loop(move |_ui| {
                on_host(|host| push(host, state, rendered, generation, after));
            });
        });
    if let Err(e) = spawned {
        warn!("Rough planner: could not start the result thread: {e}");
        host.window
            .global::<RoughPlanModel>()
            .set_error_text("Could not show the results.".into());
        if let Some(after) = after {
            after(host);
        }
    }
}

/// The entry ids whose previews, curves and extents the rows need.
fn used_ids(state: &ShownLayouts) -> BTreeSet<i64> {
    state
        .layouts
        .iter()
        .flat_map(|layout| layout.stones.iter())
        .map(|stone| effective_id(&state.statuses, stone.entry_id))
        .collect()
}

/// Decodes the front preview PNGs on up to [`local_lane_count`] scoped threads. A preview
/// that does not decode is left out.
fn decode_fronts(fronts: &[(i64, Vec<u8>)]) -> Previews {
    let lanes = local_lane_count().min(fronts.len()).max(1);
    let chunk = fronts.len().div_ceil(lanes).max(1);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for part in fronts.chunks(chunk) {
            handles.push(scope.spawn(move || {
                part.iter()
                    .filter_map(|(id, bytes)| {
                        decode_preview_pixels(bytes).map(|pixels| (*id, pixels))
                    })
                    .collect::<Vec<_>>()
            }));
        }
        handles.into_iter().flat_map(join_or_resume).collect()
    })
}

/// Loads the extents, tilt curves, preview materials and decoded front previews of `ids`.
/// The database lock is held per design and only for the reads (the front image alone,
/// not the top one); the previews are decoded after the last read, on several threads.
fn load_design_data(db: &Mutex<Database>, ids: &BTreeSet<i64>) -> (DesignData, Previews) {
    let list: Vec<i64> = ids.iter().copied().collect();
    let mut data = DesignData::default();
    let extents = db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .solid_extents_for(&list);
    match extents {
        Ok(rows) => {
            data.extents = rows
                .into_iter()
                .filter_map(|(id, row)| row.extents.map(|extents| (id, extents)))
                .collect();
        }
        Err(e) => warn!("Rough planner: could not read the design extents: {e}"),
    }

    let mut fronts = Vec::new();
    for &id in ids {
        let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
        if let Ok(Some(material)) = guard.get_preview_material(id) {
            data.materials.insert(id, material);
        }
        if let Ok(Some(front)) = guard.get_front_preview(id) {
            fronts.push((id, front));
        }
        if let Ok(Some(curves)) = guard.get_tilt_curves(id) {
            data.curves.insert(id, curves);
        }
    }
    (data, decode_fronts(&fronts))
}

/// The rough's bounding box for the saw work: the measured solid's, else the base's.
fn rough_extents(state: &ShownLayouts, geometry: Option<&ModelGeometry>) -> Option<[f64; 3]> {
    geometry.map(|g| g.extents_mm).or_else(|| {
        state
            .plan_model
            .as_ref()
            .map(|model| model.base.bounding_box_extents())
    })
}

/// Loads the data of `state`'s designs and formats one row per layout.
fn render(db: &Mutex<Database>, state: &ShownLayouts) -> Rendered {
    let (designs, previews) = load_design_data(db, &used_ids(state));
    let geometry = state.plan_model.as_ref().and_then(ModelGeometry::of);
    let settings = state.plan_settings.unwrap_or_default();
    let ctx = RowContext {
        titles: &state.titles,
        statuses: &state.statuses,
        shapes: &state.shapes,
        settings: &settings,
        weighed_ct: state.weighed_ct,
        model: geometry.as_ref(),
        rough_extents: rough_extents(state, geometry.as_ref()),
        designs: &designs,
    };
    let rows = state
        .layouts
        .iter()
        .enumerate()
        .map(|(index, layout)| {
            catch_unwind(AssertUnwindSafe(|| result_row(index + 1, layout, &ctx))).unwrap_or_else(
                |payload| {
                    warn!(
                        "Rough planner: could not format result {}: {}",
                        index + 1,
                        panic_message(&*payload)
                    );
                    fallback_row(index + 1, layout)
                },
            )
        })
        .collect();
    Rendered { rows, previews }
}

/// The row of a layout whose formatting failed: its headline figures and a note in place of
/// the details, so the other rows and the row numbering are untouched.
fn fallback_row(rank: usize, layout: &RoughLayout) -> ResultRow {
    ResultRow {
        rank: to_i32(rank),
        total_ct: format!("{:.2} ct", layout.total_carat),
        yield_pct: format!("{:.1} %", layout.yield_fraction * 100.0),
        yield_weight_text: String::new(),
        saw_text: String::new(),
        stone_count: to_i32(layout.stone_count()),
        groups: Vec::new(),
        cut_plan: "This layout could not be formatted.".to_string(),
    }
}

/// The summary line when nothing supplied one (a loaded plan).
fn default_summary(source: &ResultsSource, count: usize) -> String {
    let plural = if count == 1 { "" } else { "s" };
    match source {
        ResultsSource::Planned => format!("{count} layout{plural}"),
        ResultsSource::Loaded { .. } => format!("{count} saved layout{plural}"),
    }
}

/// The Slint result rows. The keep ticks and the Exclude flags start clear (the flags are
/// set once the rows are shown); the thumbnails are the view's.
fn to_slint_results(rows: Vec<ResultRow>, previews: &Previews) -> ModelRc<RoughPlanResult> {
    let results: Vec<RoughPlanResult> = rows
        .into_iter()
        .map(|row| {
            let groups: Vec<RoughPlanStoneGroup> = row
                .groups
                .into_iter()
                .map(|group| {
                    let pixels = previews.get(&i64::from(group.entry_id));
                    let metrics: Vec<RoughMetric> = group
                        .metrics
                        .into_iter()
                        .map(|(label, value)| RoughMetric {
                            label: label.into(),
                            value: value.into(),
                        })
                        .collect();
                    RoughPlanStoneGroup {
                        entry_id: group.entry_id,
                        name: group.name.into(),
                        count: group.count,
                        detail: group.detail.into(),
                        swatch: palette_swatch(group.swatch_index),
                        thumbnail: pixels
                            .map_or_else(Image::default, |p| Image::from_rgba8(p.clone())),
                        has_thumbnail: pixels.is_some(),
                        metrics: ModelRc::new(VecModel::from(metrics)),
                        status: group.status.into(),
                        status_level: group.status_level,
                        linkable: group.linkable,
                        excluded: false,
                    }
                })
                .collect();
            RoughPlanResult {
                rank: row.rank,
                total_ct: row.total_ct.into(),
                yield_pct: row.yield_pct.into(),
                stone_count: row.stone_count,
                groups: ModelRc::new(VecModel::from(groups)),
                cut_plan: row.cut_plan.into(),
                yield_weight_text: row.yield_weight_text.into(),
                saw_text: row.saw_text.into(),
                ..Default::default()
            }
        })
        .collect();
    ModelRc::new(VecModel::from(results))
}

/// What a finished render does on the UI thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushAction {
    /// The rows are the newest: they are shown.
    Publish,
    /// Formatting failed for the newest request: the error is shown.
    Fail,
    /// A newer request took over: nothing is shown.
    Superseded,
}

/// What to do with a render of request `generation` when the session is at
/// `session_generation`: only the newest request shows anything.
const fn push_action(session_generation: u64, generation: u64, rendered_ok: bool) -> PushAction {
    if session_generation != generation {
        PushAction::Superseded
    } else if rendered_ok {
        PushAction::Publish
    } else {
        PushAction::Fail
    }
}

/// The UI-thread half of [`show_layouts`]. A push that a newer one superseded shows
/// nothing (the newer rows win), but still runs `after`, so a finished run never stays
/// "running".
fn push(
    host: &Rc<Host>,
    state: ShownLayouts,
    rendered: Result<Rendered, String>,
    generation: u64,
    after: Option<AfterShown>,
) {
    let current = host.session.borrow().run.generation;
    match (push_action(current, generation, rendered.is_ok()), rendered) {
        (PushAction::Publish, Ok(rendered)) => publish(host, state, rendered),
        (PushAction::Fail, Err(message)) => {
            // A run that could not show its results gives the old ones back; anything
            // else (a saved plan being opened) leaves an empty list.
            if !restore_previous(host) {
                host.session.borrow_mut().run.clear();
                let model = host.window.global::<RoughPlanModel>();
                model.set_results(ModelRc::new(VecModel::<RoughPlanResult>::default()));
                model.set_keep_count(0);
                model.set_results_unsaved(false);
                view::results_changed(host);
            }
            host.window
                .global::<RoughPlanModel>()
                .set_error_text(message.into());
        }
        _ => {}
    }
    if let Some(after) = after {
        after(host);
    }
}

/// Pushes the rows and stores the state they show.
fn publish(host: &Rc<Host>, state: ShownLayouts, rendered: Rendered) {
    let model = host.window.global::<RoughPlanModel>();
    let count = state.layouts.len();
    let summary = {
        let mut session = host.session.borrow_mut();
        session
            .run
            .summary
            .take()
            .unwrap_or_else(|| default_summary(&state.source, count))
    };
    model.set_results(to_slint_results(rendered.rows, &rendered.previews));
    // Fresh and loaded plans alike show the designs that are excluded from the planner now.
    sync_row_flags(host);
    model.set_selected_result(if count == 0 { -1 } else { 0 });
    model.set_selected_group(-1);
    model.set_expanded_index(-1);
    model.set_summary(summary.into());
    model.set_keep_count(0);
    model.set_thumbnails_pending(count > 0);
    if state.source == ResultsSource::Planned {
        model.set_loaded_banner("".into());
    }
    // A fresh plan is unsaved until a save covers it; a saved plan shown again is saved.
    let unsaved = state.source == ResultsSource::Planned && count > 0;
    model.set_results_unsaved(unsaved);
    model.set_confirm_replace_open(false);
    {
        let mut session = host.session.borrow_mut();
        let run = &mut session.run;
        run.results_unsaved = unsaved;
        // The new results are in: the ones a run set aside and a question about them
        // are history.
        run.replaced = None;
        run.pending = None;
        run.layouts = state.layouts;
        run.plan_model = state.plan_model;
        run.plan_settings = state.plan_settings;
        run.material_name = state.material_name;
        run.weighed_ct = state.weighed_ct;
        run.candidate_source = state.candidate_source;
        run.titles = state.titles;
        run.statuses = state.statuses;
        run.shapes = state.shapes;
        run.keep = vec![false; count];
        run.source = state.source;
    }
    view::results_changed(host);
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::{CutOrder, CutPlan};

    #[test]
    fn only_the_newest_request_shows_anything() {
        // The session is at generation 7 when the render of request `generation` ends.
        assert_eq!(push_action(7, 7, true), PushAction::Publish);
        assert_eq!(push_action(7, 7, false), PushAction::Fail);
        // A later request (another plan or a saved plan opened) bumped the generation,
        // so neither the rows nor the error of the older one is shown.
        assert_eq!(push_action(8, 7, true), PushAction::Superseded);
        assert_eq!(push_action(8, 7, false), PushAction::Superseded);
        assert_eq!(push_action(0, 1, true), PushAction::Superseded);
    }

    #[test]
    fn a_row_that_cannot_be_formatted_keeps_its_headline_figures_and_its_number() {
        // 1.234 ct reads "1.23 ct" at two decimals; a yield of 0.4567 reads "45.7 %".
        let layout = RoughLayout {
            cut_order: CutOrder::Xyz,
            stones: Vec::new(),
            cut_plan: CutPlan { slabs: Vec::new() },
            total_carat: 1.234,
            total_volume_mm3: 10.0,
            yield_fraction: 0.4567,
            exact_fit: false,
        };
        let row = fallback_row(3, &layout);
        assert_eq!(row.rank, 3);
        assert_eq!(row.total_ct, "1.23 ct");
        assert_eq!(row.yield_pct, "45.7 %");
        assert_eq!(row.stone_count, 0);
        assert!(row.groups.is_empty());
        assert_eq!(row.cut_plan, "This layout could not be formatted.");
    }

    #[test]
    fn the_default_summary_names_the_source() {
        assert_eq!(default_summary(&ResultsSource::Planned, 1), "1 layout");
        let loaded = ResultsSource::Loaded {
            plan_id: 1,
            name: "Aqua".to_string(),
            created_at: 0,
        };
        assert_eq!(default_summary(&loaded, 4), "4 saved layouts");
        assert_eq!(default_summary(&loaded, 1), "1 saved layout");
    }
}
