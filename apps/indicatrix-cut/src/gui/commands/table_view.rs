//! View commands kept separate from the main palette table.

use super::requires::{
    EDITOR, HISTORY_EDITOR, INSPECTOR_OPTIMIZE, INSPECTOR_PREFORM, INSPECTOR_SCHEDULE,
    INSPECTOR_TIER, NONE, SLICE, VIEW_TABS,
};
use super::{Category, Command, MainWindow, command, run};

pub(super) const fn view_tab_3d() -> Command {
    command(
        "view.tab_3d",
        "Go to 3D Spectral Preview",
        Category::View,
        "gem viewer first tab",
        Some("Ctrl+1"),
        NONE,
        |ui: &MainWindow| run::show_tab(ui, run::TAB_3D),
    )
}

pub(super) const fn view_tab_cutting() -> Command {
    command(
        "view.tab_cutting",
        "Go to Cutting Instructions",
        Category::View,
        "cutting table angles tab",
        Some("Ctrl+2"),
        VIEW_TABS,
        |ui: &MainWindow| run::show_tab(ui, run::TAB_CUTTING),
    )
}

pub(super) const fn view_tab_files() -> Command {
    command(
        "view.tab_files",
        "Go to Files & Downloads",
        Category::View,
        "attachments files tab",
        Some("Ctrl+3"),
        VIEW_TABS,
        |ui: &MainWindow| run::show_tab(ui, run::TAB_FILES),
    )
}

pub(super) const fn view_live_render() -> Command {
    command(
        "view.live_render",
        "Show Live Render",
        Category::View,
        "render ray trace view tab",
        Some("Ctrl+E"),
        VIEW_TABS,
        run::show_live_render_view,
    )
}

pub(super) const fn view_edit() -> Command {
    command(
        "view.edit",
        "Show Edit Tab",
        Category::View,
        "editor designer tier table view tab",
        Some("Ctrl+E"),
        EDITOR,
        run::show_edit_view,
    )
}

pub(super) const fn view_solid() -> Command {
    command(
        "view.solid",
        "Solid View",
        Category::View,
        "edit viewport mode shaded",
        Some("1"),
        EDITOR,
        |ui: &MainWindow| run::set_solid_view_mode(ui, 0),
    )
}

pub(super) const fn view_path_traced() -> Command {
    command(
        "view.path_traced",
        "Path-Traced View",
        Category::View,
        "edit viewport mode render ray",
        Some("2"),
        EDITOR,
        |ui: &MainWindow| run::set_solid_view_mode(ui, 1),
    )
}

pub(super) const fn view_ray_traced() -> Command {
    command(
        "view.ray_traced",
        "Ray-Traced View",
        Category::View,
        "deterministic fast whitted ray traced",
        Some("5"),
        EDITOR,
        |ui: &MainWindow| run::set_solid_view_mode(ui, 4),
    )
}

pub(super) const fn view_both() -> Command {
    command(
        "view.both",
        "Solid and Path-Traced View",
        Category::View,
        "edit viewport mode both overlay",
        Some("3"),
        EDITOR,
        |ui: &MainWindow| run::set_solid_view_mode(ui, 2),
    )
}

pub(super) const fn view_diagram() -> Command {
    command(
        "view.diagram",
        "Diagram View",
        Category::View,
        "edit viewport mode gemcad 2d plan",
        Some("4"),
        EDITOR,
        |ui: &MainWindow| run::set_solid_view_mode(ui, 3),
    )
}

pub(super) const fn view_live_solid() -> Command {
    command(
        "view.live_solid",
        "Live Render: Solid Mode",
        Category::View,
        "fast raster shaded",
        None,
        VIEW_TABS,
        |ui: &MainWindow| run::set_live_view_mode(ui, 0),
    )
}

pub(super) const fn view_live_traced() -> Command {
    command(
        "view.live_traced",
        "Live Render: Path-Traced Mode",
        Category::View,
        "ray traced accurate",
        None,
        VIEW_TABS,
        |ui: &MainWindow| run::set_live_view_mode(ui, 1),
    )
}

pub(super) const fn view_live_ray_traced() -> Command {
    command(
        "view.live_ray_traced",
        "Live Render: Ray-Traced Mode",
        Category::View,
        "deterministic fast whitted ray traced",
        None,
        VIEW_TABS,
        |ui: &MainWindow| run::set_live_view_mode(ui, 2),
    )
}

pub(super) const fn view_inspector_tier() -> Command {
    command(
        "view.inspector_tier",
        "Inspector: Tier Tab",
        Category::View,
        "facet form angle indices",
        None,
        INSPECTOR_TIER,
        |ui: &MainWindow| run::show_inspector_tab(ui, 0),
    )
}

pub(super) const fn view_inspector_preform() -> Command {
    command(
        "view.inspector_preform",
        "Inspector: Preform Tab",
        Category::View,
        "rough shape yield girdle",
        None,
        INSPECTOR_PREFORM,
        |ui: &MainWindow| run::show_inspector_tab(ui, 1),
    )
}

pub(super) const fn view_inspector_optimize() -> Command {
    command(
        "view.inspector_optimize",
        "Inspector: Optimize Tab",
        Category::View,
        "weights budget search",
        None,
        INSPECTOR_OPTIMIZE,
        |ui: &MainWindow| run::show_inspector_tab(ui, 2),
    )
}

pub(super) const fn view_inspector_schedule() -> Command {
    command(
        "view.inspector_schedule",
        "Inspector: Schedule Tab",
        Category::View,
        "cutting instructions table",
        None,
        INSPECTOR_SCHEDULE,
        |ui: &MainWindow| run::show_inspector_tab(ui, 3),
    )
}

pub(super) const fn view_inspector_history() -> Command {
    command(
        "view.inspector_history",
        "Inspector: History Tab",
        Category::View,
        "undo steps jump back pictures earlier",
        None,
        EDITOR,
        |ui: &MainWindow| run::show_inspector_tab(ui, 4),
    )
}

pub(super) const fn view_inspector_variants() -> Command {
    command(
        "view.inspector_variants",
        "Inspector: Variants",
        Category::View,
        "saved copies branches versions compare open earlier alternatives",
        None,
        HISTORY_EDITOR,
        run::show_variants,
    )
}

pub(super) const fn view_library_panel() -> Command {
    command(
        "view.library_panel",
        "Show or Hide the Library Panel",
        Category::View,
        "catalogue list sidebar collapse expand",
        None,
        NONE,
        run::toggle_library_panel,
    )
}

pub(super) const fn view_slice() -> Command {
    command(
        "view.slice",
        "Toggle Slice Mode",
        Category::View,
        "cut facet mouse line drag",
        Some("S"),
        SLICE,
        run::toggle_slice_mode,
    )
}

pub(super) const fn view_snap() -> Command {
    command(
        "view.snap",
        "Toggle Snap to Gear Steps",
        Category::View,
        "snapping handles angle depth",
        None,
        NONE,
        run::toggle_snap,
    )
}
