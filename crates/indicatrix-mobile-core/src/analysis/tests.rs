//! Tests of the analysis thread: requests, supersession, staleness, notification and
//! shutdown, on the standard round brilliant template.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use indicatrix_cut_core::templates::TEMPLATES;
use indicatrix_editor::EditorSession;
use indicatrix_web_core::solve::TiltAxisData;

use super::*;
use crate::{
    design_props::{lighting_view, set_light_values},
    design_state::{Installed, begin_solve, install, run_solve},
    platform::Platform,
    platform_mock::MockPlatform,
};

/// A state over template 0 with its solve installed. The mock must outlive the state.
fn solved_state() -> (AppState, Arc<MockPlatform>) {
    let mock = Arc::new(MockPlatform::default());
    let platform: Arc<dyn Platform> = mock.clone();
    let mut state = AppState::new(platform);
    let template = &TEMPLATES[0];
    state.session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        1,
    );
    let job = begin_solve(&mut state).expect("a design with no answer gets a job");
    let response = run_solve(&job, &AtomicBool::new(false));
    let installed = install(
        &mut state,
        job.generation,
        response,
        Duration::from_millis(5),
    );
    assert_eq!(installed, Installed::Applied);
    (state, mock)
}

/// Waits until the analysis thread has nothing queued or running.
fn wait_idle(state: &AppState) {
    let deadline = Instant::now() + Duration::from_secs(300);
    while state.analysis.busy() {
        assert!(Instant::now() < deadline, "the analysis never finished");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Moves the light so every stored key differs.
fn nudge_light(state: &mut AppState) {
    let mut values = lighting_view(state).values;
    values.azimuth_deg = (values.azimuth_deg + 90.0) % 360.0;
    set_light_values(state, values);
}

fn synthetic_axis(azimuth_deg: f32, offset: f32) -> TiltAxisData {
    let curve = |scale: f32| -> Vec<f32> {
        (0..TILT_POINTS)
            .map(|i| {
                let step = scale * i as f32;
                (offset + step).clamp(0.0, 100.0)
            })
            .collect()
    };
    TiltAxisData {
        azimuth_deg,
        brilliance: curve(0.3),
        extinction: curve(0.2),
        windowing: curve(0.1),
    }
}

fn synthetic_tilt() -> TiltReading {
    TiltReading {
        axes: vec![
            synthetic_axis(0.0, 1.0),
            synthetic_axis(45.0, 2.0),
            synthetic_axis(90.0, 3.0),
            synthetic_axis(135.0, 4.0),
        ],
    }
}

#[test]
fn nothing_to_measure_without_a_solve() {
    let mock = Arc::new(MockPlatform::default());
    let platform: Arc<dyn Platform> = mock;
    let mut state = AppState::new(platform);
    let error = request_metrics(&mut state).expect_err("no solve yet");
    assert!(error.contains("Solve"), "{error}");
    assert!(request_tilt(&mut state).is_err());
    assert!(!state.analysis.busy());
    assert_eq!(metrics_view(&state).status, AnalysisStatus::NeedsSolve);
    assert_eq!(tilt_view(&state).status, AnalysisStatus::NeedsSolve);
}

#[test]
fn metrics_for_a_solved_template_are_five_finite_values() {
    let (mut state, _mock) = solved_state();
    assert_eq!(metrics_view(&state).status, AnalysisStatus::NotComputed);
    request_metrics(&mut state).expect("queued");
    wait_idle(&state);

    let reading = state.analysis.latest_metrics().expect("a result");
    for value in [
        reading.brilliance_pct,
        reading.fire_index,
        reading.scintillation_pct,
        reading.windowing_pct,
        reading.extinction_pct,
    ] {
        assert!(value.is_finite(), "{reading:?}");
    }
    assert!(!reading.under_hdr);
    let view = metrics_view(&state);
    assert_eq!(view.status, AnalysisStatus::Current);
    assert_eq!(view.rows.len(), 5);
    let labels: Vec<&str> = view.rows.iter().map(|row| row.label).collect();
    assert_eq!(
        labels,
        [
            "Brilliance",
            "Fire",
            "Scintillation",
            "Windowing",
            "Extinction"
        ]
    );
    assert!(
        view.rows
            .iter()
            .all(|row| !row.value.is_empty() && !row.meaning.is_empty())
    );

    // A change of light makes the numbers stale, not gone.
    nudge_light(&mut state);
    let after_nudge = metrics_view(&state);
    assert_eq!(after_nudge.status, AnalysisStatus::Stale);
    assert_eq!(after_nudge.rows.len(), 5);
}

#[test]
fn metrics_are_scored_under_the_chosen_hdr_map() {
    let (mut state, _mock) = solved_state();
    // A flat Radiance file whose pixels avoid the run-length markers.
    let (width, height) = (32_usize, 16_usize);
    let mut bytes =
        format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes();
    for y in 0..height {
        for x in 0..width {
            bytes.extend([
                100 + u8::try_from(x * 3 % 100).expect("below 100"),
                100 + u8::try_from(y * 5 % 100).expect("below 100"),
                120,
                129,
            ]);
        }
    }
    let file = crate::platform::PickedFile {
        display_name: "sky.hdr".to_owned(),
        bytes,
    };
    crate::hdr::import(&mut state, &file).expect("the map imports");
    assert!(state.hdr.is_selected());

    request_metrics(&mut state).expect("queued");
    wait_idle(&state);
    let reading = state.analysis.latest_metrics().expect("a result");
    assert!(reading.under_hdr, "the map lights the measurement");
    assert!(reading.brilliance_pct.is_finite());
    let view = metrics_view(&state);
    assert_eq!(view.status, AnalysisStatus::Current);
    assert_eq!(view.message, "Scored under the HDR map.");

    // Choosing a rig again changes what the numbers are scored under: they go stale.
    let name = crate::design_props::lighting_rows()[0].name.clone();
    crate::design_props::set_lighting(&mut state, &name).expect("a real rig");
    assert_eq!(metrics_view(&state).status, AnalysisStatus::Stale);
}

#[test]
fn a_design_edit_makes_the_numbers_stale() {
    let (mut state, _mock) = solved_state();
    request_metrics(&mut state).expect("queued");
    wait_idle(&state);
    assert_eq!(metrics_view(&state).status, AnalysisStatus::Current);
    // Any design edit moves the generation; no solve is current for the new one.
    let before = state.session.current_generation();
    let angle = state.session.design.tiers[4].angle_deg.abs();
    let _ = crate::design_state::set_angle_value(&mut state, 4, angle + 0.5, Duration::ZERO);
    assert_ne!(before, state.session.current_generation());
    let view = metrics_view(&state);
    assert_eq!(view.status, AnalysisStatus::Stale);
    assert_eq!(view.rows.len(), 5, "the old numbers stay visible");
}

#[test]
fn a_newer_request_supersedes_and_the_notifier_fires() {
    let (mut state, _mock) = solved_state();
    let calls = Arc::new(AtomicUsize::new(0));
    {
        let calls = Arc::clone(&calls);
        state.analysis.set_notifier(Some(Arc::new(move || {
            calls.fetch_add(1, Ordering::Relaxed);
        })));
    }
    request_metrics(&mut state).expect("first");
    request_metrics(&mut state).expect("second supersedes the first");
    wait_idle(&state);
    assert_eq!(metrics_view(&state).status, AnalysisStatus::Current);
    assert!(calls.load(Ordering::Relaxed) >= 2, "queued and finished");
    state.analysis.set_notifier(None);
}

#[test]
fn a_running_sweep_is_cancelled_when_its_inputs_change() {
    let (mut state, _mock) = solved_state();
    request_tilt(&mut state).expect("queued");
    assert!(state.analysis.tilt_busy());
    assert!(!supersede_stale(&state), "nothing changed yet");
    assert!(state.analysis.tilt_busy());

    nudge_light(&mut state);
    assert!(supersede_stale(&state), "the light moved under the sweep");
    assert!(!state.analysis.tilt_busy());
    // The worker winds down; nothing is stored for the cancelled sweep.
    std::thread::sleep(Duration::from_millis(50));
    state.analysis.shutdown();
    assert!(state.analysis.latest_tilt().is_none());
}

#[test]
fn shutdown_ends_the_thread_and_refuses_new_work() {
    let (mut state, _mock) = solved_state();
    request_tilt(&mut state).expect("queued");
    let started = Instant::now();
    state.analysis.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "shutdown is bounded"
    );
    assert!(!state.analysis.busy());
    assert_eq!(
        request_metrics(&mut state),
        Err("The app is closing.".to_owned())
    );
}

#[test]
fn cancelling_the_sweep_stops_it_being_busy() {
    let (mut state, _mock) = solved_state();
    request_tilt(&mut state).expect("queued");
    state.analysis.cancel_tilt();
    assert!(!state.analysis.tilt_busy());
    state.analysis.shutdown();
}

#[test]
fn metric_rows_read_with_units() {
    let rows = metric_rows(&MetricsReading {
        brilliance_pct: 62.34,
        fire_index: 3.21,
        scintillation_pct: 40.0,
        windowing_pct: 5.55,
        extinction_pct: 12.0,
        under_hdr: false,
    });
    assert_eq!(rows[0].value, "62.3 %");
    assert_eq!(rows[1].value, "3.2");
    assert_eq!(rows[2].value, "40.0 %");
    assert_eq!(rows.len(), 5);
}

#[test]
fn tilt_points_map_to_degrees() {
    assert_eq!(tilt_at(0), -90.0);
    assert_eq!(tilt_at(90), 0.0);
    assert_eq!(tilt_at(180), 90.0);
}

#[test]
fn tilt_paths_and_names_cover_four_axes() {
    let reading = synthetic_tilt();
    assert_eq!(
        TiltReading::axis_names(),
        ["0\u{b0}", "45\u{b0}", "90\u{b0}", "135\u{b0}"]
    );
    for axis in 0..4 {
        let paths = reading.paths(axis).expect("well-formed curves");
        assert!(paths.iter().all(|path| path.starts_with('M')), "{paths:?}");
    }
    assert!(reading.paths(4).is_none());
    assert!(reading.axis(2).is_some());
}

#[test]
fn tilt_data_copies_as_a_tab_separated_table() {
    let reading = synthetic_tilt();
    let tsv = reading.to_tsv();
    let lines: Vec<&str> = tsv.lines().collect();
    assert_eq!(lines.len(), 1 + TILT_POINTS);
    let header: Vec<&str> = lines[0].split('\t').collect();
    assert_eq!(header.len(), 1 + 4 * 3);
    assert_eq!(header[0], "Tilt (deg)");
    assert_eq!(header[1], "Brilliance 0");
    assert_eq!(header[12], "Windowing 135");
    // The first body row is -90 degrees, the middle one is table-up.
    assert!(lines[1].starts_with("-90\t"), "{}", lines[1]);
    assert!(lines[91].starts_with("0\t"), "{}", lines[91]);
    assert_eq!(lines[1].split('\t').count(), 13);
}
