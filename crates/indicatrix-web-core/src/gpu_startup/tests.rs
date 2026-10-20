use super::*;

fn record(state: AdmissionState, reason: Option<&str>) -> AdmissionRecord {
    AdmissionRecord {
        state,
        reason: reason.map(str::to_string),
        ..AdmissionRecord::default()
    }
}

const fn facts_with(
    enabled: bool,
    has_webgpu: bool,
    startup_record: Option<&AdmissionRecord>,
) -> StartupFacts<'_> {
    StartupFacts {
        enabled,
        has_webgpu,
        startup_record,
    }
}

#[test]
fn a_switched_off_setting_skips_everything() {
    // Whatever else is true, "off" wins: no Worker, no test.
    let lost = record(AdmissionState::Probing, None);
    assert_eq!(plan(&facts_with(false, true, None)), StartupPlan::Off);
    assert_eq!(
        plan(&facts_with(false, false, Some(&lost))),
        StartupPlan::Off
    );
}

#[test]
fn a_browser_without_webgpu_starts_no_worker() {
    assert_eq!(plan(&facts_with(true, false, None)), StartupPlan::NoWebGpu);
}

#[test]
fn a_clean_start_runs() {
    assert_eq!(plan(&facts_with(true, true, None)), StartupPlan::Run);
    // A start-up record that finished (nothing in the Probing or Failed state) does not block.
    let fine = record(AdmissionState::Unknown, None);
    assert_eq!(plan(&facts_with(true, true, Some(&fine))), StartupPlan::Run);
    let ok = record(AdmissionState::Ok, None);
    assert_eq!(plan(&facts_with(true, true, Some(&ok))), StartupPlan::Run);
}

#[test]
fn a_lost_last_start_is_not_repeated_by_itself() {
    // The page turns a marker left in `Probing` into `Failed` with this reason at load.
    let converted = record(
        AdmissionState::Failed,
        Some("the browser lost the GPU during the last test"),
    );
    assert_eq!(
        plan(&facts_with(true, true, Some(&converted))),
        StartupPlan::Blocked("the browser lost the GPU during the last test".to_string())
    );
    // A marker that was not converted yet, and one without a reason, still block.
    let raw = record(AdmissionState::Probing, None);
    assert_eq!(
        plan(&facts_with(true, true, Some(&raw))),
        StartupPlan::Blocked(REASON_STARTUP_LOST.to_string())
    );
    let hung = record(
        AdmissionState::Failed,
        Some("the GPU worker did not answer"),
    );
    assert_eq!(
        plan(&facts_with(true, true, Some(&hung))),
        StartupPlan::Blocked("the GPU worker did not answer".to_string())
    );
}

#[test]
fn the_startup_key_is_a_valid_admission_key_the_crash_scan_finds() {
    let key = crate::storage_keys::admission_key(STARTUP_KEY);
    assert_eq!(key, "indicatrix.admission.startup.v1");
    let probing = serde_json::to_string(&record(AdmissionState::Probing, None)).unwrap();
    assert_eq!(
        crate::gpu_admission::find_probing_key([(key.as_str(), probing.as_str())]),
        Some(key)
    );
}

#[test]
fn a_new_adapter_gets_the_full_test_only_when_the_run_is_automatic() {
    let unknown = record(AdmissionState::Unknown, None);
    assert_eq!(adapter_plan(&unknown, true), AdapterPlan::RunTest);
    assert_eq!(adapter_plan(&unknown, false), AdapterPlan::WaitForUser);
}

#[test]
fn an_adapter_that_passed_compiles_again_without_the_self_check() {
    let ok = record(AdmissionState::Ok, None);
    assert_eq!(adapter_plan(&ok, true), AdapterPlan::CompileOnly);
    assert_eq!(adapter_plan(&ok, false), AdapterPlan::CompileOnly);
}

#[test]
fn a_failed_adapter_is_never_tested_again_by_itself() {
    let failed = record(
        AdmissionState::Failed,
        Some("Kernel Surface failed to compile"),
    );
    for auto in [true, false] {
        assert_eq!(
            adapter_plan(&failed, auto),
            AdapterPlan::Blocked("Kernel Surface failed to compile".to_string())
        );
    }
    let no_reason = record(AdmissionState::Failed, None);
    assert_eq!(
        adapter_plan(&no_reason, true),
        AdapterPlan::Blocked("unknown error".to_string())
    );
    let probing = record(AdmissionState::Probing, None);
    assert!(matches!(
        adapter_plan(&probing, true),
        AdapterPlan::Blocked(_)
    ));
}

#[test]
fn a_page_that_ends_its_run_releases_only_the_markers_of_its_phase() {
    // Worker not answered yet: only the start-up marker exists.
    assert_eq!(
        release_markers(true, false, false),
        MarkerRelease {
            startup: true,
            adapter: AdapterMarker::Keep
        }
    );
    // Full test running: the adapter's record goes back to "never tested".
    assert_eq!(
        release_markers(false, true, false),
        MarkerRelease {
            startup: false,
            adapter: AdapterMarker::Restore(AdmissionState::Unknown)
        }
    );
    // Compile of an adapter that passed earlier: the record goes back to "passed".
    assert_eq!(
        release_markers(false, false, true),
        MarkerRelease {
            startup: false,
            adapter: AdapterMarker::Restore(AdmissionState::Ok)
        }
    );
}

#[test]
fn a_finished_or_idle_page_releases_nothing() {
    // The run is over (passed, failed, blocked): the stored verdict must stay, in particular a
    // `Failed` record is never turned back into something the next load would test again.
    assert_eq!(
        release_markers(false, false, false),
        MarkerRelease {
            startup: false,
            adapter: AdapterMarker::Keep
        }
    );
}

#[test]
fn the_test_wins_over_the_compile_when_both_flags_are_set() {
    // Cannot happen (one run at a time); the stricter restore is the safe answer.
    assert_eq!(
        release_markers(false, true, true).adapter,
        AdapterMarker::Restore(AdmissionState::Unknown)
    );
}

/// The two stored records a run can own, as the page keeps them in storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stored {
    startup: Option<AdmissionState>,
    adapter: Option<AdmissionState>,
}

/// What storage holds while the phase runs: the markers in the `Probing` state.
fn stored_during(starting: bool, testing: bool, admitting: bool) -> Stored {
    Stored {
        startup: starting.then_some(AdmissionState::Probing),
        adapter: (testing || admitting).then_some(AdmissionState::Probing),
    }
}

fn after_release(mut stored: Stored, release: MarkerRelease) -> Stored {
    if release.startup {
        stored.startup = None;
    }
    if let AdapterMarker::Restore(state) = release.adapter {
        // The page forgets an `Unknown` record and rewrites an `Ok` one.
        stored.adapter = (state != AdmissionState::Unknown).then_some(state);
    }
    stored
}

fn after_rearm(mut stored: Stored, rearm: MarkerRearm) -> Stored {
    if rearm.startup {
        stored.startup = Some(AdmissionState::Probing);
    }
    if rearm.adapter {
        stored.adapter = Some(AdmissionState::Probing);
    }
    stored
}

#[test]
fn releasing_then_rearming_gives_the_stored_markers_back() {
    for (starting, testing, admitting) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
        (false, false, false),
    ] {
        let before = stored_during(starting, testing, admitting);
        let released = after_release(before, release_markers(starting, testing, admitting));
        let restored = after_rearm(released, rearm_markers(starting, testing, admitting));
        assert_eq!(
            restored, before,
            "phase starting={starting} testing={testing} admitting={admitting}"
        );
    }
}

#[test]
fn a_release_really_clears_what_a_rearm_writes() {
    // Without the re-arm the released page holds no `Probing` marker at all (that is the point
    // of the release), and the re-arm writes exactly the ones the phase owns.
    for (starting, testing, admitting) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let released = after_release(
            stored_during(starting, testing, admitting),
            release_markers(starting, testing, admitting),
        );
        assert_ne!(released.startup, Some(AdmissionState::Probing));
        assert_ne!(released.adapter, Some(AdmissionState::Probing));
    }
}

#[test]
fn a_finished_phase_rearms_nothing() {
    assert_eq!(
        rearm_markers(false, false, false),
        MarkerRearm {
            startup: false,
            adapter: false
        }
    );
    assert_eq!(
        rearm_markers(true, false, false),
        MarkerRearm {
            startup: true,
            adapter: false
        }
    );
}

#[test]
fn gpu_state_codes_round_trip() {
    for state in [
        GpuState::Off,
        GpuState::Unknown,
        GpuState::Testing,
        GpuState::Ok,
        GpuState::Failed,
        GpuState::Starting,
    ] {
        assert_eq!(GpuState::from_code(state.code()), state);
    }
    // The slint file's comparisons (1 unknown, 2 testing, 3 ok, 4 failed) keep their numbers.
    assert_eq!(GpuState::Unknown.code(), 1);
    assert_eq!(GpuState::Testing.code(), 2);
    assert_eq!(GpuState::Ok.code(), 3);
    assert_eq!(GpuState::Failed.code(), 4);
    assert_eq!(GpuState::from_code(99), GpuState::Off);
}

#[test]
fn progress_only_moves_forward_and_stays_in_the_bar() {
    let total = crate::gpu_admission::ISOTROPIC_KERNELS_ORDER.len();
    for with_self_check in [true, false] {
        let mut stages = vec![Stage::Adapter, Stage::Limits];
        stages.extend((0..=total).map(|done| Stage::Compile { done, total }));
        if with_self_check {
            stages.push(Stage::SelfCheck);
        }
        stages.push(Stage::Done);
        let mut last = -1.0_f32;
        for stage in stages {
            let value = progress(stage, with_self_check);
            assert!(
                (0.0..=1.0).contains(&value),
                "{stage:?} is {value}, outside the bar"
            );
            assert!(
                value > last,
                "{stage:?} ({value}) did not advance from {last}"
            );
            last = value;
        }
        assert!((last - 1.0).abs() < f32::EPSILON, "the run ends at the top");
    }
}

#[test]
fn compile_progress_is_linear_in_kernels_and_clamped() {
    let at = |done, total| progress(Stage::Compile { done, total }, true);
    let start = at(0, 4);
    let mid = at(2, 4);
    let end = at(4, 4);
    assert!(((mid - start) - (end - mid)).abs() < 1e-6);
    // More answers than kernels (a repeated message) cannot overshoot; zero kernels cannot divide.
    assert!((at(9, 4) - end).abs() < f32::EPSILON);
    assert!(at(0, 0).is_finite());
}

#[test]
fn a_run_without_the_self_check_uses_more_of_the_bar_for_the_compile() {
    let full = Stage::Compile {
        done: 11,
        total: 11,
    };
    assert!(progress(full, false) > progress(full, true));
}

#[test]
fn stage_labels_name_the_real_step() {
    assert!(stage_label(Stage::Adapter, None).contains("adapter"));
    assert_eq!(
        stage_label(Stage::Compile { done: 3, total: 11 }, None),
        "Compiling the GPU kernels (3/11)"
    );
    assert_eq!(
        stage_label(Stage::Limits, Some("NVIDIA RTX")),
        "Checking the adapter limits (NVIDIA RTX)"
    );
    assert_eq!(stage_label(Stage::SelfCheck, None), "Running a test render");
}

#[test]
fn the_adapter_name_falls_back_step_by_step() {
    let mut facts = AdapterFacts {
        vendor: "nvidia",
        architecture: "ampere",
        device: "2204",
        description: "NVIDIA GeForce RTX 3090",
        backend: "BrowserWebGpu",
        max_storage_buffers_per_stage: 16,
        max_storage_binding: 1 << 30,
        max_buffer: 1 << 32,
    };
    assert_eq!(adapter_display_name(&facts), "NVIDIA GeForce RTX 3090");
    facts.description = "  ";
    assert_eq!(adapter_display_name(&facts), "nvidia ampere");
    facts.vendor = "";
    facts.architecture = "";
    assert_eq!(adapter_display_name(&facts), "2204");
    facts.device = "";
    assert_eq!(
        adapter_display_name(&facts),
        "unnamed BrowserWebGpu adapter"
    );
    facts.backend = "";
    assert_eq!(adapter_display_name(&facts), "unnamed adapter");
}

fn input<'a>(state: GpuState) -> SummaryInput<'a> {
    SummaryInput {
        enabled: true,
        no_webgpu: false,
        state,
        workers: 7,
        adapter: Some("NVIDIA GeForce RTX 3090"),
        detail: "",
        admitted: false,
        scene_reason: None,
    }
}

#[test]
fn the_summary_says_when_the_gpu_is_off() {
    let off = SummaryInput {
        enabled: false,
        ..input(GpuState::Off)
    };
    assert_eq!(
        summary(&off),
        "GPU rendering is off. Rendering on the CPU (7 workers)."
    );
    assert_eq!(backend_line(&off), "CPU \u{b7} 7 workers (GPU off)");
}

#[test]
fn the_summary_says_when_the_browser_has_no_webgpu() {
    let none = SummaryInput {
        no_webgpu: true,
        adapter: None,
        ..input(GpuState::Off)
    };
    assert_eq!(
        summary(&none),
        "Rendering on the CPU (7 workers): this browser has no WebGPU."
    );
    assert_eq!(backend_line(&none), "CPU \u{b7} 7 workers (no WebGPU)");
}

#[test]
fn the_summary_names_the_adapter_when_the_gpu_renders() {
    let on = SummaryInput {
        admitted: true,
        ..input(GpuState::Ok)
    };
    assert_eq!(
        summary(&on),
        "Rendering on the GPU (NVIDIA GeForce RTX 3090) with 7 workers on the CPU."
    );
    assert_eq!(backend_line(&on), "GPU + CPU \u{b7} 7 workers");
}

#[test]
fn a_passed_gpu_that_has_not_joined_yet_is_not_called_the_renderer() {
    let waiting = input(GpuState::Ok);
    let text = summary(&waiting);
    assert!(
        text.starts_with("The GPU is ready (NVIDIA GeForce RTX 3090)"),
        "{text}"
    );
    assert!(!text.starts_with("Rendering on the GPU"), "{text}");
    assert_eq!(
        backend_line(&waiting),
        "CPU \u{b7} 7 workers (GPU starting)"
    );
}

#[test]
fn a_stone_the_gpu_cannot_trace_is_called_a_cpu_render_with_the_reason() {
    let anisotropic = SummaryInput {
        admitted: true,
        scene_reason: Some("anisotropic stones not yet supported"),
        ..input(GpuState::Ok)
    };
    let text = summary(&anisotropic);
    assert!(text.starts_with("Rendering on the CPU (7 workers): this stone cannot use the GPU"));
    assert!(text.contains("anisotropic stones not yet supported"));
    assert!(text.contains("The GPU is ready (NVIDIA GeForce RTX 3090)."));
    assert_eq!(
        backend_line(&anisotropic),
        "CPU \u{b7} 7 workers (GPU: anisotropic stones not yet supported)"
    );
}

#[test]
fn a_failed_gpu_carries_its_reason_into_the_sentence() {
    let failed = SummaryInput {
        detail: "GPU test failed: Kernel Surface failed to compile",
        adapter: None,
        ..input(GpuState::Failed)
    };
    assert_eq!(
        summary(&failed),
        "Rendering on the CPU (7 workers): GPU test failed: Kernel Surface failed to compile."
    );
    // A reason that ends in a full stop gets no second one.
    let stopped = SummaryInput {
        detail: "GPU unavailable: no adapter. Reload the page to try again.",
        ..failed
    };
    assert!(summary(&stopped).ends_with("try again."));
    assert!(!summary(&stopped).ends_with(".."));
    // No reason at all still says something.
    let blank = SummaryInput {
        detail: " ",
        ..failed
    };
    assert_eq!(
        summary(&blank),
        "Rendering on the CPU (7 workers): the GPU is not available."
    );
    assert_eq!(
        backend_line(&failed),
        "CPU \u{b7} 7 workers (GPU test failed)"
    );
}

#[test]
fn the_waiting_states_say_the_cpu_renders_meanwhile() {
    let starting = summary(&input(GpuState::Starting));
    assert!(
        starting.contains("Rendering on the CPU (7 workers) meanwhile"),
        "{starting}"
    );
    let testing = summary(&input(GpuState::Testing));
    assert!(
        testing.contains("Testing the GPU (NVIDIA GeForce RTX 3090)"),
        "{testing}"
    );
    let unknown = summary(&input(GpuState::Unknown));
    assert!(unknown.contains("has not been tested yet"), "{unknown}");
    assert_eq!(
        backend_line(&input(GpuState::Unknown)),
        "CPU \u{b7} 7 workers (GPU not tested)"
    );
}

#[test]
fn one_worker_is_singular() {
    let one = SummaryInput {
        enabled: false,
        workers: 1,
        ..input(GpuState::Off)
    };
    assert_eq!(
        summary(&one),
        "GPU rendering is off. Rendering on the CPU (1 worker)."
    );
}
