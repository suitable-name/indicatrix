//! The video harness' own data tables and the checks that tie them to the guide catalogue. Pure
//! (no window, no `crate::` paths), so that `tests/tutorial_videos_targets.rs` can include and
//! unit-test them (`pure_tables_tests.rs`); `tutorial_videos` itself has `harness = false` and
//! runs no `#[test]`. The recorder runs [`check_guide`] before it films a guide, so a stale table
//! fails that video with the reason instead of going wrong half way.

use indicatrix_editor::guide::{
    CMD_ADOPT_ALL, CMD_COMPARE, CMD_DELETE, CMD_DUPLICATE, CMD_MOVE_DOWN, CMD_MOVE_UP, CMD_SOLVE,
    CMD_UNDO, Goal, Guide, Perform,
};
use std::path::PathBuf;

/// Commands filmed through the Ctrl+K palette: the command id and the title of its palette row
/// (`src/gui/commands/table.rs`).
pub const PALETTE_COMMANDS: &[(&str, &str)] = &[
    (CMD_DUPLICATE, "Duplicate Selected Tier"),
    (CMD_MOVE_UP, "Move Selected Tier Earlier"),
    (CMD_MOVE_DOWN, "Move Selected Tier Later"),
    ("help.manual", "User Manual"),
    ("help.shortcuts", "Keyboard Shortcuts"),
    ("help.tutorials", "Tutorials..."),
    ("tools.plan_rough", "Plan Rough..."),
    ("tools.export_render", "Export Render Image..."),
    ("tools.angle_sweep", "Angle Sweep..."),
];

/// Commands filmed by clicking the visible control itself.
pub const WIDGET_COMMANDS: &[&str] = &[
    CMD_SOLVE,
    CMD_UNDO,
    CMD_ADOPT_ALL,
    CMD_DELETE,
    CMD_COMPARE,
    "solve.snapshot",
    "view.live_render",
    "view.snap",
    "view.slice",
];

/// Whether `film_command` has a route for `id`.
#[must_use]
pub fn command_is_routed(id: &str) -> bool {
    PALETTE_COMMANDS.iter().any(|(command, _)| *command == id) || WIDGET_COMMANDS.contains(&id)
}

/// The events `drive_event` (`routes.rs`, `event_extras.rs`) can raise by a visible interaction.
/// `palette_opened` is not here on purpose: the one lesson that waits for it has a script.
pub const ROUTED_EVENTS: &[&str] = &[
    "snapshot_taken",
    "solve_requested",
    "auto_solve_off",
    "auto_solve_on",
    "deep_solve_started",
    "deep_solve_finished",
    "design_saved",
    "sheet_exported",
    "diagram_exported",
    "asc_exported",
    "gcs_exported",
    "library_imported",
    "library_searched",
    "library_search_cleared",
    "library_design_selected",
    "design_replaced",
    "library_filtered",
    "library_filters_reset",
    "high_contrast_changed",
    "large_handles_changed",
    "ui_scale_changed",
    "verdict_opened",
    "facet_picked",
    "tier_selected",
    "snap_toggled",
    "slice_started",
    "slice_drawn",
    "slice_flipped",
    "slice_cuts_stone",
    "slice_symmetric_toggled",
    "diagram_panel_enlarged",
    "live_render_opened",
    "tiers_multi_selected",
    "variants_opened",
    "interface_mode_changed",
    "compare_opened",
    "compare_window_opened",
    "lighting_preset_chosen",
    "design_lighting_saved",
    "design_lighting_forgotten",
    "custom_material_saved",
    "coefficient_material_saved",
    "cutting_step_marked",
    "rough_planner_opened",
    "rough_plan_finished",
    "glossary_opened",
    "shortcuts_opened",
    "help_opened",
    "cut_slider_rough",
    "cut_slider_step",
    "cut_slider_finished",
];

/// Every explicit script of `scripts.rs`: guide id, step index (0-based) and the step title the
/// script was written for. [`check_guide`] compares the title with the catalogue and the keys
/// with the `match` arms of `scripts.rs`, so a renamed or reordered step stops the video.
pub const SCRIPTED: &[(&str, usize, &str)] = &[
    ("app-command-palette", 1, "Run a command"),
    ("app-keyboard-shortcuts", 1, "Switch the view with a key"),
    ("app-keyboard-shortcuts", 2, "And back"),
    ("app-keyboard-shortcuts", 3, "Swap tabs"),
    ("app-keyboard-shortcuts", 4, "Every key in one list"),
    (
        "new-design-walkthrough",
        8,
        "Yield and rendering (optional)",
    ),
    ("optimizing-optimize", 2, "Choose what to favour"),
    ("optimizing-optimize", 3, "Look at what may change"),
    ("optimizing-optimize", 4, "Run a short search"),
    ("optimizing-optimize", 5, "Read and pick a candidate"),
    ("optimizing-optimize", 6, "Apply a candidate"),
    ("optimizing-optimize", 7, "Check the result"),
    ("optimizing-retarget-optimize", 1, "Shift or Optimize"),
    ("optimizing-retarget-optimize", 2, "Search and apply"),
    ("optimizing-retarget-optimize", 3, "Undo the retarget"),
    ("optimizing-retarget-optimize", 4, "Reading the options"),
    ("optimizing-retarget-shift", 1, "How Shift works"),
    ("optimizing-retarget-shift", 2, "Retarget to topaz"),
    ("optimizing-retarget-shift", 3, "Undo the retarget"),
    ("optimizing-retarget-shift", 4, "When a change is Not valid"),
    ("optimizing-sweep", 0, "What a sweep tells you"),
    ("optimizing-sweep", 1, "Run a sweep and use an angle"),
    ("optimizing-sweep", 2, "Undo it"),
    (
        "optimizing-sweep",
        3,
        "The chart, the tilt option and the CSV",
    ),
    ("output-open", 1, "Open it"),
    ("solving-history", 1, "Change the crown"),
    ("solving-history", 2, "Change the pavilion"),
    ("solving-history", 3, "Go back one step"),
    ("solving-history", 4, "Go back to Start"),
    ("solving-history", 5, "Go forward again"),
    ("solving-history", 6, "Undo from where you stand"),
    ("solving-history", 7, "A new change drops the undone steps"),
    ("solving-preform", 2, "Make the rough wider"),
    ("solving-raw-text", 0, "Take a snapshot"),
    ("solving-raw-text", 1, "Edit the instructions as text"),
    ("solving-raw-text", 2, "Undo the text edit"),
    (
        "solving-raw-text",
        3,
        "The saved file and what the text leaves out",
    ),
    ("solving-snapshot-compare", 1, "Change the crown"),
    ("solving-snapshot-compare", 4, "Read the compare window"),
    ("solving-variants", 0, "Open the Variants view"),
    ("solving-variants", 1, "Save the first variant"),
    ("solving-variants", 2, "Change the crown"),
    ("solving-variants", 3, "Save a second variant"),
    ("solving-variants", 4, "Go back to the first variant"),
    ("solving-variants", 5, "Undo the open"),
    ("solving-variants", 6, "Compare two designs"),
    ("solving-variants", 7, "Tidy up"),
    ("solving-verdict-fixes", 3, "Add a table with a Fix"),
    (
        "solving-verdict-fixes",
        4,
        "Type an index between two teeth",
    ),
    ("solving-verdict-fixes", 5, "Snap it to the teeth"),
    ("solving-verdict-fixes", 6, "The other three Fix buttons"),
    ("solving-yield-carat", 2, "Give the stone a size"),
    ("tiers-adopt-imported-meets", 0, "Open an imported design"),
    ("tiers-adopt-imported-meets", 1, "Adopt one tier"),
    ("tiers-adopt-imported-meets", 2, "Adopt the rest at once"),
    ("tiers-arithmetic", 1, "Calculate a scale value"),
    ("tiers-arithmetic", 2, "Calculate a facet position"),
    ("tiers-arithmetic", 3, "Where arithmetic works"),
    ("tiers-duplicate", 1, "Change the copy"),
    ("tiers-facet-chips", 0, "Add a facet"),
    ("tiers-facet-chips", 1, "Remove a facet"),
    ("tiers-facet-chips", 2, "Detach one facet"),
    ("tiers-facet-chips", 3, "Remove only that facet"),
    ("tiers-facet-chips", 4, "Rotate the tier"),
    ("tiers-facet-chips", 5, "Mirror the tier"),
    ("tiers-inline-angle", 0, "Edit an angle in the table"),
    ("tiers-inline-angle", 1, "Nudge with the keys"),
    ("tiers-inline-angle", 2, "Nudge with the mouse wheel"),
    ("tiers-linked-series", 1, "Move the first rung"),
    ("tiers-linked-series", 3, "Relations you write yourself"),
    ("tiers-multi-select", 1, "Offset them together"),
    ("tiers-multi-select", 2, "Delete them together"),
    ("tiers-pin-to-mast", 1, "Pin the solved depth"),
    ("tiers-pin-to-mast", 2, "Pin and Adopt"),
    ("tiers-quick-add", 0, "Quick add a girdle"),
    ("tiers-quick-add", 1, "Quick add a table"),
    ("tiers-quick-add", 2, "Quick add a culet"),
    ("tiers-relations", 2, "Move the driver"),
    ("tiers-step-series", 2, "Edit one rung"),
    ("viewing-diagram", 3, "Drag a handle on the diagram"),
    ("viewing-drag-handles", 0, "Select a tier"),
    ("viewing-drag-handles", 1, "Drag the angle handle"),
    ("viewing-drag-handles", 2, "One drag, one undo"),
    ("viewing-drag-handles", 3, "Drag the depth handle"),
    (
        "viewing-drag-handles",
        4,
        "Turn the tier with the index handle",
    ),
    ("viewing-drag-handles", 5, "Switch snapping off"),
    ("viewing-drag-handles", 6, "Switch snapping back on"),
    ("viewing-drag-handles", 7, "Cancel a drag"),
    ("viewing-slice", 0, "Switch Slice on"),
    ("viewing-slice", 1, "Draw a line"),
    ("viewing-slice", 2, "Flip the side"),
    ("viewing-slice", 3, "Cut it in"),
    ("viewing-slice", 4, "Symmetric or single"),
    ("viewing-slice", 5, "Keep the facet"),
    ("viewing-slice", 6, "Undo the new tier"),
    ("viewing-slice", 7, "Done"),
    ("viewing-solid-picking", 4, "Click a different tier"),
    ("viewing-solid-picking", 5, "Pick from the list instead"),
];

/// Guides whose every step is routed by one script module instead of the `match` of `scripts.rs`:
/// the guide id and its step count.
pub const MODULE_SCRIPTED: &[(&str, usize)] = &[("viewing-optical-diagnostics", 5)];

/// The source of `scripts.rs`, whose `match` arms must agree with [`SCRIPTED`].
const SCRIPTS_SOURCE: &str = include_str!("scripts.rs");

/// Where a guide's output files go: one folder per guide, relative to the profile's working
/// directory, so two guides that save or export the same file name never meet and no frame or
/// toast can show an absolute path.
#[must_use]
pub fn output_path(guide_id: &str, file: &str) -> PathBuf {
    PathBuf::from("tutorial-output").join(guide_id).join(file)
}

/// The `("guide-id", index) =>` arms found in the source of `scripts.rs`.
#[must_use]
pub fn scripted_keys(source: &str) -> Vec<(String, usize)> {
    source
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("(\"")?;
            let (id, rest) = rest.split_once("\", ")?;
            let (index, rest) = rest.split_once(')')?;
            rest.trim_start().starts_with("=>").then_some(())?;
            Some((id.to_owned(), index.parse::<usize>().ok()?))
        })
        .collect()
}

/// Whether a goal is made of events only.
#[must_use]
pub fn event_only(goal: &Goal) -> bool {
    match goal {
        Goal::Event(_) => true,
        Goal::All(goals) | Goal::Any(goals) => goals.iter().all(event_only),
        _ => false,
    }
}

/// Why `goal` cannot be driven by the director's route for a step with neither a script nor a
/// recipe, or `None` when it can.
#[must_use]
pub fn recipe_gap(goal: &Goal) -> Option<String> {
    let unrouted = |name: &str| {
        (!ROUTED_EVENTS.contains(&name)).then(|| {
            format!(
                "event {name:?} has no visible route (add one to drive_event or write a script)"
            )
        })
    };
    match goal {
        Goal::Manual => None,
        Goal::YieldApplied => Some("YieldApplied needs an authored script with its value".into()),
        _ if event_only(goal) => goal.events().into_iter().find_map(unrouted),
        Goal::All(goals) => goals.iter().find_map(|member| match member {
            Goal::Event(name) => unrouted(name.as_str()),
            Goal::SolvedClosed | Goal::ViewMode(_) | Goal::InspectorTab(_) | Goal::Material(_) => {
                None
            }
            other => Some(format!(
                "composite member {other:?} needs an authored script"
            )),
        }),
        other => Some(format!("goal {other:?} needs an authored script")),
    }
}

fn walk_commands<'a>(perform: &'a Perform, out: &mut Vec<&'a str>) {
    match perform {
        Perform::Command(id) => out.push(id.as_str()),
        Perform::Sequence(parts) => parts.iter().for_each(|part| walk_commands(part, out)),
        _ => {}
    }
}

/// Checks the tables against `guide`: every explicit script names an existing step by its title,
/// the keys of `scripts.rs` and [`SCRIPTED`] agree, every recipe command has a route, and every
/// step without a script or recipe has a goal the director can drive.
///
/// # Errors
///
/// The first mismatch, as a sentence.
pub fn check_guide(guide: &Guide) -> Result<(), String> {
    check_guide_with(guide, SCRIPTS_SOURCE)
}

/// [`check_guide`] against an explicit `scripts.rs` source (for tests).
///
/// # Errors
///
/// The first mismatch, as a sentence.
pub fn check_guide_with(guide: &Guide, scripts_source: &str) -> Result<(), String> {
    let id = guide.id.as_str();
    let module_steps = MODULE_SCRIPTED
        .iter()
        .find(|(name, _)| *name == id)
        .map(|&(_, steps)| steps);
    if let Some(steps) = module_steps {
        return if guide.steps.len() == steps {
            Ok(())
        } else {
            Err(format!(
                "{id}: its script module covers {steps} step(s), the guide has {}",
                guide.steps.len()
            ))
        };
    }
    let table: Vec<(usize, &str)> = SCRIPTED
        .iter()
        .filter(|(name, _, _)| *name == id)
        .map(|&(_, index, title)| (index, title))
        .collect();
    let mut source: Vec<usize> = scripted_keys(scripts_source)
        .into_iter()
        .filter(|(name, _)| name == id)
        .map(|(_, index)| index)
        .collect();
    source.sort_unstable();
    let mut listed: Vec<usize> = table.iter().map(|&(index, _)| index).collect();
    listed.sort_unstable();
    if source != listed {
        return Err(format!(
            "{id}: scripts.rs scripts steps {source:?} but pure_tables::SCRIPTED lists {listed:?}"
        ));
    }
    for &(index, title) in &table {
        let step = guide.steps.get(index).ok_or_else(|| {
            format!(
                "{id}: scripted step {index} does not exist (the guide has {} steps)",
                guide.steps.len()
            )
        })?;
        if step.title != title {
            return Err(format!(
                "{id}: step {index} is now titled {:?}, its script was written for {title:?}",
                step.title
            ));
        }
    }
    for (index, step) in guide.steps.iter().enumerate() {
        if let Some(perform) = &step.perform {
            let mut commands = Vec::new();
            walk_commands(perform, &mut commands);
            if let Some(unknown) = commands
                .into_iter()
                .find(|command| !command_is_routed(command))
            {
                return Err(format!(
                    "{id}: step {index} runs command {unknown:?}, which film_command cannot film"
                ));
            }
        }
        let scripted = table.iter().any(|&(at, _)| at == index);
        if scripted || step.is_manual() || step.perform.is_some() {
            continue;
        }
        if let Some(gap) = recipe_gap(&step.goal) {
            return Err(format!("{id}: step {index} ({:?}): {gap}", step.title));
        }
    }
    Ok(())
}
