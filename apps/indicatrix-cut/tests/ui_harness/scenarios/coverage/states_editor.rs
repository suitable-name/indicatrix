//! Editor states of the click-through passes that run 6 left uncovered (lane R6A): the Solid
//! viewport's toolbar, the empty design, the "pending" Apply buttons, the tier form's draft
//! banners, the tier table's row actions and the library list's hover menu.
//!
//! Proves: in each state every enabled button, tab, check box and combo box of the scope is pressed
//! once and every field touched (the engine of `overlays.rs`), after the state's `open` function
//! reached the page the way a user does: model flags first, then the opener or the typing.
//!
//! Three things the earlier states could not do are done here:
//!
//! * typing: `states_reach::type_into` sets a text field through its accessible set-value action, which fires
//!   the field's `edited` callback exactly as a keystroke does, so the local "changes not applied
//!   yet" flags of the Apply buttons are up when the button is pressed;
//! * hovering: [`hover`] moves the pointer with `slint::Window::dispatch_event(PointerMoved)`
//!   (the testing backend's `ElementHandle` has no hover call, but the window API reaches every
//!   `TouchArea.has-hover`), which is what makes a list row show its "more actions" button;
//! * scrolling: `ElementHandle::scroll` moves a `ScrollView` so a field at the bottom of a dialog is
//!   inside the clip and therefore enumerated.

use super::{
    super::{Piece, dialogs, existing_install},
    overlay_states::{Base, NONE, State, deny, edit_tab, inspector, live_tab, press},
    overlays, state_pieces,
    states_reach::{library_list_open, scroll_scope, type_into},
};
use crate::support::{
    Ctx, HarnessApp,
    app::{pump, wait_until},
    coverage,
    smoke::{DenyMatch, DenyRule},
};
use indicatrix_cut::{
    ConcaveFormData, EditorModel, EditorTierItem, ExportModel, LibraryModel, OpticalModel,
    SolidPreviewModel,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel, platform::WindowEvent};
use std::time::Duration;
use uiprobe::AccessibleRole;

/// The pieces of this file: one `overlay:editor-*:<state>` piece per state, and the Exporting
/// button check.
pub fn pieces() -> Vec<Piece> {
    let groups: [super::StateGroup; 6] = [
        ("editor-viewport", viewport_states),
        ("editor-empty", empty_design_states),
        ("editor-pending-applies", pending_apply_states),
        ("editor-tier-forms", tier_form_states),
        ("editor-tier-rows", tier_row_states),
        ("editor-list-rows", list_row_states),
    ];
    let mut all = Vec::new();
    for (group, states) in groups {
        all.extend(state_pieces(group, existing_install, states));
    }
    all.push(Piece::new(
        "coverage_editor_exporting_button",
        existing_install,
        exporting_button,
    ));
    all.push(Piece::new(
        "coverage_optical_scintillation_checkbox",
        existing_install,
        optical_scintillation_checkbox,
    ));
    all
}

const DENY_KEBAB: &[DenyRule] = &[
    deny(
        DenyMatch::Contains,
        "generate previews",
        "GPU render: LibraryModel.generate_previews_for_entry renders the design's pictures",
    ),
    deny(
        DenyMatch::Contains,
        "compute tilt curves",
        "GPU render: LibraryModel.compute_tilt_curves_for_entry renders a tilt curve",
    ),
];

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Moves the pointer to the middle of the first library list row, so the row's hover-only
/// controls show, and returns that row's title. The list is unfolded first: at the passes'
/// 1366 px width the window folds the library onto its rail, and the rail has no rows.
fn hover_first_row(app: &HarnessApp) -> Option<String> {
    library_list_open(app);
    let library = app.ui().global::<LibraryModel>();
    let first = library.get_diagram_list().row_data(0)?;
    let title = first.title.to_string();
    // The selected row shows its "more actions" button without a pointer (the keyboard path);
    // the hover below is only the fallback.
    if library.get_selected_entry_id() != first.id {
        library.invoke_select_diagram(first.id);
        pump(300);
    }
    if app.main_view().has(&format!("More Actions for {title}")) {
        return Some(title);
    }
    // The row is looked up inside the list (another list-item of the same title would take the
    // pointer elsewhere) and located by its element's window position, the way `pointer_click`
    // does.
    let view = overlays::find_scope(app, "DiagramListView").unwrap_or_else(|| app.main_view());
    let row = view
        .controls()
        .into_iter()
        .find(|c| c.role == AccessibleRole::ListItem && c.label.starts_with(&title))?;
    let origin = row.handle.absolute_position();
    let size = row.handle.size();
    let window = app.ui().window();
    let button = format!("More Actions for {title}");
    // The pointer leaves the window first (so no earlier hover or a tooltip's popup state is
    // left over), then visits points of the row, each reached by two moves a pixel apart (a
    // `TouchArea` sets `has-hover` on any move inside it; the second move covers a first one
    // that only registered the pointer), with mock time advancing so the `if` that shows the
    // button is instantiated.
    window.dispatch_event(WindowEvent::PointerExited);
    pump(50);
    let points = [
        (size.width * 0.5, size.height * 0.5),
        (size.width * 0.9, 12.0),
        (size.width * 0.5, 8.0),
        (size.width * 0.25, size.height * 0.75),
    ];
    for (x, y) in points {
        for step in [0.0, 1.0] {
            let at = slint::LogicalPosition::new(origin.x + x + step, origin.y + y);
            window.dispatch_event(WindowEvent::PointerMoved { position: at });
            pump(120);
        }
        if app.main_view().has(&button) {
            return Some(title);
        }
    }
    println!(
        "ui-harness:   library row {title:?}: the pointer at ({}, {}) size ({}, {}) did not show {button:?}; window {:?}, covering the row's middle: {:?}",
        origin.x,
        origin.y,
        size.width,
        size.height,
        window.size().to_logical(window.scale_factor()),
        elements_at(
            &app.main_view().root,
            size.width.mul_add(0.5, origin.x),
            size.height.mul_add(0.5, origin.y)
        )
    );
    Some(title)
}

/// The elements of `root` whose rectangle holds the point `(x, y)` and that can take the pointer
/// or hold a popup (type name `#id`), front-most last: what a failed hover reports, so the next
/// run names whatever sits over the row.
fn elements_at(root: &uiprobe::ElementHandle, x: f32, y: f32) -> Vec<String> {
    let mut found = Vec::new();
    let _: Option<()> = root.visit_descendants(|element| {
        let p = element.absolute_position();
        let s = element.size();
        let inside = x >= p.x && y >= p.y && x <= p.x + s.width && y <= p.y + s.height;
        let name = element
            .type_name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        let takes_pointer = [
            "TouchArea",
            "FocusScope",
            "PopupWindow",
            "Flickable",
            "Menu",
        ]
        .iter()
        .any(|kind| name.contains(kind))
            || element
                .bases()
                .is_some_and(|mut bases| bases.any(|b| b.contains("TouchArea")));
        if inside && takes_pointer && s.width > 0.0 && s.height > 0.0 {
            let id = element.id().map(|i| i.to_string()).unwrap_or_default();
            found.push(format!("{name} #{id} {}x{}", s.width, s.height));
        }
        std::ops::ControlFlow::Continue(())
    });
    found.truncate(25);
    found
}

/// Waits (real time, up to 8 s) until no solve runs, so a late refresh does not overwrite what a
/// state sets on the models afterwards.
fn wait_settled(app: &HarnessApp) {
    let editor = app.ui().global::<EditorModel>();
    let _ = wait_until(Duration::from_secs(8), || !editor.get_solve_running());
    pump(400);
}

// ---------------------------------------------------------------------------------------------
// Solid viewport
// ---------------------------------------------------------------------------------------------

/// The Diagram mode's pills (Reset View, Crown, Pavilion, Profile), the Cut slider and the Live
/// Render view's "Show Finished" badge.
fn viewport_states() -> Vec<State> {
    vec![
        State {
            name: "Solid viewport, Diagram mode",
            base: Base::Design,
            scope: Some("SolidViewportView"),
            deny: NONE,
            open: |a| {
                edit_tab(a);
                wait_settled(a);
                a.ui().global::<SolidPreviewModel>().set_view_mode(3);
                pump(300);
            },
        },
        State {
            name: "Solid viewport, Cut slider",
            base: Base::Design,
            scope: Some("SolidViewportView"),
            deny: NONE,
            open: |a| {
                edit_tab(a);
                wait_settled(a);
                // The slider shows when the design has cutting steps and a solid; Rust writes the
                // step count on every replan, so the harness writes a count of its own.
                let model = a.ui().global::<SolidPreviewModel>();
                model.set_cut_step_count(6);
                model.set_has_solid(true);
                model.set_cut_position(3.0);
                model.set_cut_label("After P2 (3 of 6)".into());
                pump(300);
            },
        },
        State {
            name: "Live Render, stone cut back",
            base: Base::Design,
            scope: Some("LiveCutBadge"),
            deny: NONE,
            open: |a| {
                live_tab(a);
                wait_settled(a);
                // The badge shows while the Edit tab's slider is not on Finished; step 0 exists
                // in every design that has a tier.
                let model = a.ui().global::<SolidPreviewModel>();
                model.set_cut_step_count(6);
                model.set_tier_cutoff(0);
                model.set_cut_label("After P1 (1 of 6)".into());
                pump(300);
            },
        },
    ]
}

/// The Live Render toolbar's Export button while an export is running ("Exporting..."): the
/// overlay passes deny the plain label "Export", so the press is made here. Proves the button
/// brings the export window back (`ExportModel.is_open`).
fn exporting_button(ctx: &mut Ctx<'_>) {
    if let Err(message) = overlays::launch_base(&mut *ctx.app, ctx.profile, &ctx.seed, Base::Design)
    {
        ctx.report.add_simple(
            "-",
            "launch",
            format!("exporting button: cannot start: {message}"),
        );
        return;
    }
    live_tab(ctx.app);
    ctx.app.ui().global::<ExportModel>().set_is_exporting(true);
    pump(300);
    let pressed = press(ctx.app, "Exporting...", None);
    ctx.expect(
        pressed,
        "exporting-button",
        "the Live Render toolbar shows no \"Exporting...\" button while ExportModel.is_exporting is set",
    );
    if pressed {
        let opened = ctx.app.ui().global::<ExportModel>().get_is_open();
        if ctx.expect(
            opened,
            "exporting-button-effect",
            "pressing the Exporting... button did not open the export window",
        ) {
            coverage::record_site_id("ToolbarTail::toolbar-button-is-exporting");
        }
    }
    ctx.check_health("exporting button");
}

/// The Optical Diagnostics panel's Scintillation check box: it is a control of the Live Render
/// view's panel, found by its accessible label, and each press flips `OpticalModel.scintillation`
/// (the layer the overlay controller reads to trace the five yaw poses).
fn optical_scintillation_checkbox(ctx: &mut Ctx<'_>) {
    if let Err(message) = overlays::launch_base(&mut *ctx.app, ctx.profile, &ctx.seed, Base::Design)
    {
        ctx.report.add_simple(
            "-",
            "launch",
            format!("optical scintillation checkbox: cannot start: {message}"),
        );
        return;
    }
    live_tab(ctx.app);
    ctx.app.ui().global::<OpticalModel>().set_panel_open(true);
    pump(300);
    // The global handle borrows the app, so each read takes a fresh one and drops it before
    // `ctx.expect` borrows `ctx` mutably.
    let scintillation = |ctx: &Ctx<'_>| ctx.app.ui().global::<OpticalModel>().get_scintillation();
    let pressed = press(ctx.app, "Scintillation layer", None);
    if ctx.expect(
        pressed,
        "optical-scintillation-checkbox",
        "the Optical Diagnostics panel shows no \"Scintillation layer\" check box",
    ) {
        let on = scintillation(ctx);
        ctx.expect(
            on,
            "optical-scintillation-on",
            "pressing the Scintillation check box did not switch the layer on",
        );
        press(ctx.app, "Scintillation layer", None);
        let on = scintillation(ctx);
        ctx.expect(
            !on,
            "optical-scintillation-off",
            "pressing the Scintillation check box again did not switch the layer off",
        );
    }
    ctx.check_health("optical scintillation checkbox");
}

// ---------------------------------------------------------------------------------------------
// The empty design
// ---------------------------------------------------------------------------------------------

/// "+ Add Tier" of the hint over a design with no tier yet.
fn empty_design_states() -> Vec<State> {
    vec![State {
        name: "Edit view, empty design",
        base: Base::Startup,
        scope: None,
        deny: NONE,
        open: |a| {
            let ui = a.ui();
            let editor = ui.global::<EditorModel>();
            editor.set_new_template_index(0);
            editor.set_new_dialog_open(true);
            pump(200);
            // Template 0 ("Empty") is the Empty form's create, not a gallery card's: the
            // gallery's Create has no template spec for index 0 and returns without a design.
            editor.invoke_new_design_create(
                1,
                "1.5".into(),
                "1.0".into(),
                "1.5".into(),
                0,
                "96".into(),
                "8".into(),
                true,
                0,
                0,
            );
            let _ = wait_until(Duration::from_secs(15), || editor.get_has_design());
            edit_tab(a);
            wait_settled(a);
        },
    }]
}

// ---------------------------------------------------------------------------------------------
// Pending Apply buttons and the printed proportions
// ---------------------------------------------------------------------------------------------

/// The Design settings, the Preform tab and the metadata editor with values typed that differ from
/// the design's, then their Apply buttons pressed with the "changes not applied yet" marker up.
fn pending_apply_states() -> Vec<State> {
    vec![
        State {
            name: "Design settings, edited values",
            base: Base::Design,
            scope: Some("EditorDesignSettings"),
            deny: NONE,
            open: design_settings_edited,
        },
        State {
            name: "Inspector: Preform, edited values",
            base: Base::Design,
            scope: Some("EditorInspector"),
            deny: NONE,
            open: preform_edited,
        },
        State {
            name: "Metadata editor, scrolled to the volume",
            base: Base::Library,
            scope: Some("MetadataEditorDialog"),
            deny: NONE,
            open: |a| {
                a.ui()
                    .global::<LibraryModel>()
                    .set_metadata_editor_open(true);
                pump(300);
                // The volume is the last field of the form; the form scrolls, and a field
                // outside the clip is not enumerated.
                if let Some(view) = overlays::find_scope(a, "MetadataEditorDialog") {
                    view.root.scroll(0.0, -4000.0);
                    pump(300);
                }
            },
        },
    ]
}

/// Gear (Custom), symmetry order, title and details, the printed proportions: each typed, then the
/// four Apply buttons pressed.
fn design_settings_edited(a: &HarnessApp) {
    edit_tab(a);
    wait_settled(a);
    let editor = a.ui().global::<EditorModel>();
    editor.set_gear_index(6);
    pump(250);
    let typed = [
        ("Custom Gear Teeth", "100"),
        ("Symmetry Order", "4"),
        ("Design Title", "Harness title"),
        ("Extra Header Lines", "Harness note"),
        ("Footnotes", "Harness footnote"),
        ("Gear Reference Angle in Degrees", "1"),
        ("Printed Volume over Width Cubed", "0.5"),
        ("Printed Length over Width", "1.1"),
        ("Printed Crown Height over Width", "0.3"),
        ("Printed Pavilion Height over Width", "0.4"),
        ("Printed Total Height over Width", "0.7"),
    ];
    for (label, text) in typed {
        let _ = type_into(a, label, text);
    }
    // Apply Printed and Details first: Apply Gear opens a modal question and Apply Symmetry
    // rebuilds the design.
    for (label, id) in [
        (
            "Apply Printed",
            "EditorDesignSettings::tip-button-printed-pending",
        ),
        (
            "Apply Details",
            "EditorDesignSettings::tip-button-meta-pending",
        ),
        (
            "Apply Symmetry",
            "EditorDesignSettings::tip-button-symmetry-pending",
        ),
    ] {
        if press(a, label, None) {
            coverage::record_site_id(id);
        }
    }
    if press(a, "Apply Gear", None) {
        // The gear change asks for a confirmation (the remap dialog); the answer is Cancel.
        if wait_until(Duration::from_secs(3), || editor.get_gear_remap_open()) {
            coverage::record_site_id("EditorDesignSettings::tip-button-gear-pending");
            editor.invoke_gear_remap_cancel();
            pump(250);
        }
    }
}

/// The Preform tab: shape sizes and the yield inputs typed, then Apply Yield Inputs and Apply
/// Preform pressed (their reader labels say "changes not applied yet" while the marker is up).
fn preform_edited(a: &HarnessApp) {
    inspector(a, 1);
    let typed = [
        ("Half-Width (Model Units, Girdle Half-Width = 1)", "1.3"),
        ("Length / Width (Ratio)", "1.5"),
        ("Depth (Model Units, Girdle Half-Width = 1)", "1.2"),
        ("Girdle Y-Offset (mm)", "0.1"),
        ("Girdle Diameter (mm)", "6.5"),
        ("Specific Gravity Override", "3.5"),
    ];
    for (label, text) in typed {
        let _ = type_into(a, label, text);
    }
    if press(a, "Apply Yield Inputs, changes not applied yet", None) {
        coverage::record_site_id("PreformTab::tip-button-yield-pending");
    }
    if press(a, "Apply Preform, changes not applied yet", None) {
        coverage::record_site_id("PreformTab::tip-button-preform-pending");
    }
}

// ---------------------------------------------------------------------------------------------
// The tier form
// ---------------------------------------------------------------------------------------------

/// A concave tier as the inspector's form hands it to `EditorModel.save_concave_tier`.
fn concave_data(name: &str, angle: &str) -> ConcaveFormData {
    ConcaveFormData {
        name: name.into(),
        angle_deg: angle.into(),
        indices: "0, 24, 48, 72".into(),
        tool: "CYL".into(),
        diameter_ratio: "0.25".into(),
        ..Default::default()
    }
}

/// The unsaved-draft banner (the selection moved on), the tier-changed-under-the-draft banner,
/// Remove Relation (a tier that follows another), and the same two banners of the concave form.
fn tier_form_states() -> Vec<State> {
    let scope = Some("EditorInspector");
    vec![
        State {
            name: "Tier form, draft kept while the selection moved",
            base: Base::Design,
            scope,
            deny: NONE,
            open: |a| {
                inspector(a, 0);
                let _ = type_into(a, "Angle in Degrees", "33.3");
                a.ui().global::<EditorModel>().set_selected_tier_index(1);
                pump(300);
            },
        },
        State {
            name: "Tier form, tier changed under the draft",
            base: Base::Design,
            scope,
            deny: NONE,
            open: |a| {
                inspector(a, 0);
                let _ = type_into(a, "Angle in Degrees", "33.3");
                let _ = dialogs::nudge_first_tier(a.ui());
                pump(400);
            },
        },
        State {
            name: "Tier form, tier that follows a relation",
            base: Base::Design,
            scope,
            deny: NONE,
            open: tier_following_relation,
        },
        State {
            name: "Concave form, draft kept while the selection moved",
            base: Base::Design,
            scope,
            deny: NONE,
            open: |a| {
                inspector(a, 0);
                a.ui().global::<EditorModel>().invoke_add_concave_tier();
                pump(300);
                let _ = type_into(a, "Facet Name", "Harness draft");
                a.ui().global::<EditorModel>().set_selected_tier_index(0);
                pump(300);
            },
        },
        State {
            name: "Concave form, tier changed under the draft",
            base: Base::Design,
            scope,
            deny: NONE,
            open: concave_changed_under_draft,
        },
        State {
            name: "Concave form, cone tool",
            base: Base::Design,
            scope,
            deny: NONE,
            open: concave_cone_selected,
        },
    ]
}

/// A saved concave tier cut with a cone, selected: the form's "Tool Included Angle in Degrees"
/// is enabled only for a cone or a disc, and the form's tool is private to the inspector, so it
/// is reached by loading such a tier.
fn concave_cone_selected(a: &HarnessApp) {
    inspector(a, 0);
    let editor = a.ui().global::<EditorModel>();
    let before = editor.get_tiers().row_count();
    let cone = ConcaveFormData {
        tool: "CON".into(),
        tool_angle_deg: "60".into(),
        ..concave_data("Harness cone", "-40")
    };
    editor.invoke_save_concave_tier(-1, cone);
    if !wait_until(Duration::from_secs(8), || {
        editor.get_tiers().row_count() > before
    }) {
        return;
    }
    pump(300);
    let position = i32::try_from(editor.get_tiers().row_count()).unwrap_or(1) - 1;
    editor.set_selected_tier_index(position);
    editor.set_inspector_height(2400.0);
    pump(300);
}

/// A linked step series made from tier 0 ("Steps / Mirror" with "Keep Linked": every tier after
/// the first follows the first), then a following tier selected, so the Tier form shows its
/// relation and "Remove Relation". (The earlier route typed `=<code>-2` into tier 1 through
/// `save_tier`; the relation never showed within its waits, so the state spent 150 s waiting and
/// offered no Remove Relation. The linked series is the route the checked direct-invoke row
/// proves.)
fn tier_following_relation(a: &HarnessApp) {
    inspector(a, 0);
    wait_settled(a);
    let editor = a.ui().global::<EditorModel>();
    let Some(first) = editor.get_tiers().row_data(0) else {
        return;
    };
    let digits: String = first
        .angle_full
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let Ok(angle) = digits.parse::<f64>() else {
        return;
    };
    let indices = first
        .indices_full
        .replace(',', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(",");
    a.ui()
        .global::<indicatrix_cut::RelationModel>()
        .invoke_generate_step_series_linked(
            "L".into(),
            format!("{:.2}", angle + 3.0).into(),
            "1.5".into(),
            3,
            indices.as_str().into(),
            "".into(),
        );
    let following = || {
        editor
            .get_tiers()
            .iter()
            .position(|row| !row.relation_text.is_empty())
    };
    let _ = wait_until(Duration::from_secs(8), || following().is_some());
    if let Some(row) = following().and_then(|row| i32::try_from(row).ok()) {
        editor.set_selected_tier_index(row);
        pump(300);
    }
}

/// A saved concave tier selected, a name typed over it, then the tier re-saved with another angle
/// (what a drag would do): the form keeps the draft and says the design changed.
fn concave_changed_under_draft(a: &HarnessApp) {
    inspector(a, 0);
    let editor = a.ui().global::<EditorModel>();
    let before = editor.get_tiers().row_count();
    editor.invoke_save_concave_tier(-1, concave_data("Harness", "-40"));
    let added = wait_until(Duration::from_secs(8), || {
        editor.get_tiers().row_count() > before
    });
    if !added {
        return;
    }
    pump(300);
    let position = i32::try_from(editor.get_tiers().row_count()).unwrap_or(1) - 1;
    editor.set_selected_tier_index(position);
    pump(300);
    let _ = type_into(a, "Facet Name", "Harness draft");
    editor.invoke_save_concave_tier(position, concave_data("Harness", "-41"));
    pump(500);
}

// ---------------------------------------------------------------------------------------------
// The tier table
// ---------------------------------------------------------------------------------------------

/// Every action a tier row can offer (Complete Orbit, Adopt, Pin, Detach or Reattach, Add Anchor)
/// and the TIERS section header. The rows of a freshly solved template show few of them, so the
/// state writes the row flags that make each one show (the model is `in`, Rust-owned, and the
/// handlers behind the buttons work on the real design).
fn tier_row_states() -> Vec<State> {
    vec![State {
        name: "Tier table, every row action",
        base: Base::Design,
        scope: Some("EditorTierTable"),
        deny: NONE,
        open: |a| {
            edit_tab(a);
            wait_settled(a);
            let editor = a.ui().global::<EditorModel>();
            let count = editor.get_tiers().row_count();
            // The header folds the table and unfolds it again (two presses).
            let header = format!("TIERS ({count})");
            let _ = press(a, &header, None);
            let _ = press(a, &header, None);
            editor.set_tier_table_collapsed(false);
            let rows: Vec<EditorTierItem> = editor
                .get_tiers()
                .iter()
                .enumerate()
                .map(|(index, mut row)| {
                    row.strategy_is_uncertain = false;
                    row.kind = 0;
                    match index % 3 {
                        0 => {
                            row.constraint_kind = 0;
                            row.needs_anchor = true;
                            row.imported_meet_text = "meets tier 2".into();
                            row.orbit_status = "3/4 orbit".into();
                            row.orbit_incomplete = true;
                            row.is_detached = false;
                        }
                        1 => {
                            row.orbit_status = "orbit x4".into();
                            row.orbit_incomplete = false;
                            row.is_detached = true;
                        }
                        _ => {
                            row.constraint_kind = 0;
                            row.orbit_status = "orbit x4".into();
                            row.orbit_incomplete = false;
                            row.is_detached = false;
                        }
                    }
                    row
                })
                .collect();
            editor.set_tiers(ModelRc::new(VecModel::from(rows)));
            pump(400);
            // Adopt, Pin, Detach and Add Anchor are the right-hand columns of a table that
            // scrolls sideways inside the dock; scrolled out of the clip they are not
            // enumerated, so the table is scrolled to its right edge.
            scroll_scope(a, "EditorTierTable", -4000.0, 0.0);
        },
    }]
}

// ---------------------------------------------------------------------------------------------
// The library list
// ---------------------------------------------------------------------------------------------

/// The hover-only controls of a library list row: the "more actions" button and its menu, then the
/// tag chips and the New Tag field (reached through the menu's Add Tag).
fn list_row_states() -> Vec<State> {
    let scope = Some("DiagramListView");
    vec![
        State {
            name: "Library row, more-actions menu",
            base: Base::Library,
            scope,
            deny: DENY_KEBAB,
            open: |a| {
                if let Some(title) = hover_first_row(a) {
                    let _ = press(a, &format!("More Actions for {title}"), None);
                }
            },
        },
        State {
            name: "Library row, tag chips and the New Tag field",
            base: Base::Library,
            scope,
            deny: DENY_KEBAB,
            open: |a| {
                let library = a.ui().global::<LibraryModel>();
                let Some(row) = library.get_diagram_list().row_data(0) else {
                    return;
                };
                library.invoke_add_tag_to_entry(row.id, "harness".into());
                let _ = wait_until(Duration::from_secs(8), || {
                    library
                        .get_diagram_list()
                        .row_data(0)
                        .is_some_and(|row| row.tags.row_count() > 0)
                });
                pump(300);
                if let Some(title) = hover_first_row(a)
                    && press(a, &format!("More Actions for {title}"), None)
                {
                    let _ = press(a, "Add Tag", None);
                }
            },
        },
        State {
            name: "Library list, filter bar",
            base: Base::Library,
            scope,
            deny: DENY_KEBAB,
            open: |a| {
                // The bar above the list ("Showing ... Clear") shows while the just-imported
                // filter holds ids, as after an import; its Clear empties it.
                let library = a.ui().global::<LibraryModel>();
                let ids: Vec<i32> = library
                    .get_diagram_list()
                    .iter()
                    .map(|row| row.id)
                    .collect();
                library.set_recent_import_filter(ModelRc::new(VecModel::from(ids)));
                library.set_id_filter_label("Showing a harness filter.".into());
                library_list_open(a);
            },
        },
    ]
}
