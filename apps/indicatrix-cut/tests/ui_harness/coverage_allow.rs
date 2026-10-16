//! The coverage gate's allow list: inventoried UI actions the harness deliberately never runs.
//!
//! Every uncovered callback or interactive element that is not listed here is a `coverage`
//! finding and fails the run; every entry that matches nothing in the inventory is a finding
//! too (stale). Keep the list short: each entry is a gap the owner reads, and its reason names
//! the code that makes the action unreachable or unsafe in a test run (network, a long GPU job).
//!
//! Keys:
//! * a callback: `Model.name` (snake case), for example `UpdateModel.check_now`;
//! * an element: the `file|Component|id` / `file|Component|"label"` key the report prints for it;
//! * a trailing `*` makes the key a prefix.
//!
//! The direct-invoke table (`scenarios/coverage/`) has NO row for a callback listed here.

/// `(key, reason)`.
pub const ALLOW: &[(&str, &str)] = &[
    // ---- network ----
    (
        "UpdateModel.check_now",
        "network: update_check::start_check asks GitHub for the latest release",
    ),
    (
        "RemoteWorkerModel.test_worker_connection",
        "network: worker_callbacks connects to the typed worker address",
    ),
    (
        "RemoteWorkerModel.claim_token",
        "network: worker_callbacks enrols with the coordinator at the typed address",
    ),
    (
        "RemoteWorkerModel.start_mirror_sync",
        "network: library::remote mirrors the remote library over the wire",
    ),
    (
        "RemoteWorkerModel.save_remote",
        "network: saving an endpoint makes later renders and library probes connect to it",
    ),
    (
        "ExportModel.check_remote_availability",
        "network: render_export::wiring probes the configured remote worker on a thread",
    ),
    // The clipboard and the OS openers are not listed: under the `ui-harness` feature they
    // capture the text / target instead of starting a program (`harness_support`).
    // ---- long GPU jobs ----
    (
        "BatchModel.preview_generate_confirmed",
        "long job: renders previews for the whole filtered set on the GPU",
    ),
    (
        "BatchModel.preview_generate_solid_confirmed",
        "long job: renders solid previews for the whole filtered set",
    ),
    (
        "BatchModel.tilt_generate_confirmed",
        "long job: renders tilt curves for the whole filtered set on the GPU",
    ),
    (
        "ExportModel.start_export",
        "long job: render_export::wiring starts a real final-picture export",
    ),
    (
        "TiltVideoExportModel.start_video_export",
        "long job: tilt::video_export renders every frame of a video",
    ),
    (
        "RenderJobsModel.start_queue",
        "long job: render_jobs::controller starts the queue worker",
    ),
    (
        "RenderJobsModel.run_next",
        "long job: render_jobs::controller renders the next queued job",
    ),
    (
        "LibraryModel.compute_missing_tilt_curves",
        "long job: renders a tilt curve for every library design that lacks one",
    ),
    // ---- elements whose click would do one of the above ----
    (
        "components/preferences_dialog.slint|PreferencesDialog|\"Check Now\"",
        "network: the button calls UpdateModel.check_now (see above); smoke denies it by label",
    ),
    (
        "components/performance_graph/tilt_video_export_section.slint|TiltVideoExportSection|\"Export Tilt Video\"",
        "long job: the button calls TiltVideoExportModel.start_video_export (see above)",
    ),
    (
        "components/export_dialog.slint|ExportDialog|\"Start Export\"",
        "long job: the button calls ExportModel.start_export (see above); the overlay passes deny it (overlay_states::OVERLAY_DENY)",
    ),
    (
        "components/render_jobs/jobs_panel.slint|RenderJobsPanel|\"Start Queue\"",
        "long job: the button calls RenderJobsModel.start_queue (see above); denied in the Render Jobs states (overlay_states::DENY_JOBS)",
    ),
    (
        "components/render_jobs/job_row.slint|JobRowCard|\"Run Next\"",
        "long job: the button calls RenderJobsModel.run_next (see above); denied in the Render Jobs states (overlay_states::DENY_JOBS)",
    ),
    (
        "components/filter_panel.slint|FilterPanel|compute_now_touch",
        "long job: the button calls LibraryModel.compute_missing_tilt_curves (see above)",
    ),
    (
        "components/tilt_batch_dialog.slint|TiltBatchDialog|\"Compute\"",
        "long job: the button calls BatchModel.tilt_generate_confirmed (see above)",
    ),
    (
        "components/preview_batch_dialog.slint|PreviewBatchDialog|\"Quick (Solid)\"",
        "long job: the button calls BatchModel.preview_generate_solid_confirmed (see above)",
    ),
    (
        "components/preferences_dialog.slint|PreferencesDialog|\"Check for New Versions\"",
        "network: switching it on calls UpdateModel.enabled_changed(true), which update_check::start_check turns into a GitHub request when a check is due",
    ),
    (
        "components/performance_graph_dialog.slint|PerformanceGraphDialog|\"Re-Render Fresh\"",
        "long job: the button calls TiltModel.rerender_curve_with_current_material, which renders the tilt curve on the GPU (its direct-invoke row covers the callback)",
    ),
    // ---- elements that exist only while a long job or a network exchange runs ----
    (
        "components/remote_worker_dialog.slint|RemoteWorkerDialog|tip-button-testing-connection",
        "network: the button is the Test Connection / Cancel pair of RemoteWorkerModel.test_worker_connection (allow-listed above); the pass denies it by label (overlay_states::DENY_REMOTE)",
    ),
    (
        "components/remote_worker_dialog.slint|RemoteWorkerDialog|tip-button-testing-connection-2",
        "network: second Test Connection button of the same dialog, same callback and same deny",
    ),
    (
        "components/remote_worker_dialog.slint|RemoteWorkerDialog|tip-button-mirror-in-progress",
        "network: the button calls RemoteWorkerModel.start_mirror_sync (allow-listed above); denied by the label \"mirror\"",
    ),
    (
        "components/remote_worker_dialog.slint|RemoteWorkerDialog|tip-button-claiming-token",
        "network: the button calls RemoteWorkerModel.claim_token (allow-listed above); denied by the label \"claim\"",
    ),
    (
        "components/export_chip.slint|ExportChip|chip_touch",
        "needs a running final-picture export (ExportModel.is_exporting is driven by render_export::wiring after start_export, which is allow-listed as a long job)",
    ),
    (
        "components/export_chip.slint|ExportChip|cancel_touch",
        "needs a running final-picture export (see the chip above)",
    ),
    (
        "components/editor_status_strip.slint|ActivityChip|cancel_touch",
        "needs a running background activity (a solve, search or render that the status strip lists); those are long jobs",
    ),
    (
        "components/editor_inspector/optimize_tab.slint|OptimizeTab|\"Compare...\"",
        "needs finished Optimize candidates (EditorModel.optimize_candidates is filled by a long search on the render thread)",
    ),
    (
        "components/editor_inspector/optimize_tab.slint|OptimizeTab|\"Apply This Candidate\"",
        "needs finished Optimize candidates (see above)",
    ),
    (
        "components/editor_inspector/optimize_tab.slint|OptimizeTab|preview_check",
        "needs finished Optimize candidates (see above)",
    ),
    (
        "locate_window.slint|LocateWindow|\"Solve\"",
        "needs photos loaded into the Locate window; loading them is an OS file dialog onto real image files the profile does not hold",
    ),
    (
        "locate_window.slint|LocateWindow|\"Accept\"",
        "needs a located inclusion, i.e. a solved Locate window (see Solve)",
    ),
    (
        "locate_window.slint|LocateWindow|\"Re-Open\"",
        "needs an accepted location (see Accept)",
    ),
    (
        "components/rough_planner/saved_list.slint|SavedPlanCard|\"Export\"",
        "needs a saved plan (planner_save_and_reopen) and smoke denies the exact label \"export\" (support/smoke.rs DEFAULT_DENY); RoughPlanModel.export_saved is driven by planner_export_import_isrd through the callback",
    ),
    // ---- run-6 follow-up (R5B): needs a GPU render, a long job or real files ----
    (
        "components/retarget_dialog.slint|RetargetDialog|\"Compare…\"",
        "GPU render: retarget_actions opens the Compare window, which renders the design and the proposal (covered by the window:compare pieces through the snapshot route)",
    ),
    (
        "components/retarget_dialog.slint|RetargetDialog|\"Compare Visually…\"",
        "GPU render: opens the Compare window for the snapshot, which renders both designs (the window itself is passed by window:compare:split view)",
    ),
    (
        "components/retarget_dialog.slint|RetargetDialog|action-button-search-done",
        "long job: RetargetModel.search runs the Optimize search over every crown and pavilion angle on a worker (the pass denies the label \"search\")",
    ),
    (
        "components/editor_inspector/variants_section.slint|VariantsSection|\"Compare Pictures\"",
        "GPU render: VariantsModel.compare_pictures opens the Compare window, which renders both chosen variants (the pass denies the label)",
    ),
    (
        "components/editor_command_bar/edit_groups.slint|SolveGroup|deep_solve_btn",
        "long job: Deep Solve starts the slow verified solve on a worker (deep_solve_run::begin_deep_solve_run) and is enabled only for a design with printed proportions; its callback EditorModel.deep_solve has a direct-invoke row",
    ),
    (
        "components/stale_badge.slint|StaleBadge|recompute_touch",
        "long job: Recompute starts a Deep Solve or an Optimize search (the pass denies the label \"recompute\")",
    ),
    (
        "components/performance_graph_dialog.slint|PerformanceGraphDialog|\"All\"",
        "GPU render: the axis chips are enabled only when extra_axes_ready, i.e. the tilt curve of all four axes was rendered (graph_brilliance_extra_axes)",
    ),
    (
        "components/performance_graph_dialog.slint|PerformanceGraphDialog|chip-toggle-narrow-4",
        "GPU render: an axis chip, enabled only when extra_axes_ready (see the All chip)",
    ),
    (
        "components/render_jobs/job_row.slint|JobRowCard|\"Show\"",
        "needs a finished render job with a result file (RenderJobRow.can_show): jobs finish only after a long render",
    ),
    (
        "components/settings_dialog/environment_map_section.slint|EnvironmentMapSection|\"Load\"",
        "real file: loads the HDR file named in SettingsModel.env_map_path through the file dialog (no HDR file in the profile)",
    ),
    (
        "components/solid_viewport/hint_bar.slint|ManipulateHintBar|\"Discard\"",
        "needs a provisional manipulate edit: ManipulateModel.provisional_active is set by dragging a handle on the solid mesh, which the render thread builds",
    ),
    (
        "components/solid_viewport/hint_bar.slint|ManipulateHintBar|\"Flip\"",
        "needs a provisional manipulate edit (see Discard)",
    ),
    (
        "components/solid_viewport/hint_bar.slint|ManipulateHintBar|\"Symmetric\"",
        "needs a provisional manipulate edit (see Discard)",
    ),
    (
        "components/rough_planner/rough_form.slint|RoughForm|\"Remove\"",
        "real file: removing an inclusion needs an imported mesh with an added inclusion (OBJ, STL or PLY through the file dialog)",
    ),
    (
        "rough_colour_window.slint|RoughColourWindow|\"Prepare Photos\"",
        "long job + real files: aligns eight backlit photos picked through file dialogs (the pass denies the label)",
    ),
    (
        "rough_colour_window.slint|RoughColourWindow|\"Start Fit\"",
        "long job: traces light through the stone for every view, minutes on a GPU (the pass denies the label)",
    ),
    (
        "rough_colour_window.slint|RoughColourWindow|\"Refine Geometry\"",
        "long job: repeats the fit while moving the zone boundaries (the pass denies the label)",
    ),
    (
        "rough_colour_window.slint|RoughColourWindow|\"Fit Zone\"",
        "long job: fits a primitive to marks made on the photos (the pass denies the label)",
    ),
    (
        "rough_colour_window.slint|RoughColourWindow|\"Suggest from the Photos\"",
        "long job: looks for color boundaries in the prepared photos (the pass denies the label)",
    ),
    (
        "rough_colour_window.slint|RoughColourWindow|\"Accept\"",
        "needs a finished fit (a suggestion list, and a saved plan for the final Accept: RoughColourWindow.can_accept)",
    ),
    (
        "rough_colour_window.slint|RoughColourWindow|\"Export Report...\"",
        "real files: writes a Markdown report and the pictures to a folder (the pass denies the label)",
    ),
    (
        "rough_colour_window.slint|RoughColourWindow|\"Add Zones...\"",
        "needs a finished fit: zoned_prompt is published only after a fit whose result has no zones",
    ),
    (
        "rig_window.slint|RigWindow|\"Save Calibrated Rig\"",
        "needs a successful calibration from real cube photos (RigWindow.can_save_calibrated)",
    ),
    // ---- round 6 (R6B): dialogs and panels (scenarios/coverage/states_dialogs.rs) ----
    (
        "components/preview_batch_dialog.slint|PreviewBatchDialog|tip-button-from-import-2",
        "long job: the button is \"Generate\" / \"Full Render\" and calls BatchModel.preview_generate_confirmed (allow-listed above); the pass denies the labels \"generate\" and \"full render\" (states_dialogs::DENY_IMPORT_BATCH)",
    ),
    (
        "components/sweep_dialog/sweep_form.slint|SweepForm|sweep-button-running",
        "long job: the button is \"Run Sweep\" (SweepModel.run solves the design at every angle of the range on a worker) or, while one runs, \"Cancel\"; the pass denies the label \"run sweep\" and prepares a finished three-angle sweep outside the pass (states_dialogs::sweep_with_results)",
    ),
    // ---- round 6 (R6A): library row menu (scenarios/coverage/states_editor.rs) ----
    (
        "components/diagram_list.slint|DiagramListView|kebab_item1",
        "GPU render: \"Generate Previews\" calls LibraryModel.generate_previews_for_entry; denied in the states_editor row-menu states",
    ),
    (
        "components/diagram_list.slint|DiagramListView|kebab_item2",
        "GPU render: \"Compute Tilt Curves\" calls LibraryModel.compute_tilt_curves_for_entry; denied in the states_editor row-menu states",
    ),
    #[cfg(not(feature = "physical-color"))]
    (
        "components/material_editor/chromophore_picker.slint|*",
        "build without the physical-color feature: physics_state::PHYSICS_COLOR_UI is false, so color_section_view never shows the physics section (and push_view hides it again after every edit); with the feature the states_dialogs physics state drives it",
    ),
];
