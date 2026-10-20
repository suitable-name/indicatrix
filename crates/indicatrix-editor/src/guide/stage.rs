//! The stage of a guided tutorial step: the view, panels, inspector tabs and camera focus.
//!
//! Pure data and derivation rules, with no UI dependencies.

use super::{
    Goal, Guide, GuideStep,
    perform::{Perform, TierEntry},
};

/// The main application view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageView {
    /// The Edit sub-tab (solid viewport and editor layout).
    Edit,
    /// The Live Render view (path-traced viewport).
    LiveRender,
    /// Either view; keeps the previous step's view.
    Either,
}

/// The design library panel state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryPanel {
    /// Collapsed to the rail.
    Hidden,
    /// Open and showing the list.
    Shown,
    /// Keep whatever state was on screen.
    Keep,
}

/// Where the camera aims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraFocus {
    /// Canonical reset pose (0.60, 0.45).
    Whole,
    /// Crown-side view (0.60, 0.55).
    Crown,
    /// Pavilion-side view (0.60, -0.50).
    Pavilion,
    /// Girdle edge-on view (0.60, 0.08).
    Girdle,
    /// Looking straight down on the table (0.60, 1.10).
    Top,
    /// Keep the current pose.
    Keep,
}

impl CameraFocus {
    /// Yaw and pitch in radians according to S-D4.
    #[must_use]
    pub const fn pose(self) -> Option<(f32, f32)> {
        match self {
            Self::Whole => Some((0.60, 0.45)),
            Self::Crown => Some((0.60, 0.55)),
            Self::Pavilion => Some((0.60, -0.50)),
            Self::Girdle => Some((0.60, 0.08)),
            Self::Top => Some((0.60, 1.10)),
            Self::Keep => None,
        }
    }
}

/// The UI staging required for a tutorial step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stage {
    /// View mode (Edit, `LiveRender`, Either).
    pub view: StageView,
    /// Whether the library list should be shown, hidden, or kept.
    pub library: LibraryPanel,
    /// Inspector tab: 0 Tier, 1 Preform, 2 Optimize, 3 Schedule, 4 History.
    pub inspector_tab: Option<i32>,
    /// Whether the tier table section in the inspector should be expanded.
    pub expand_tier_table: bool,
    /// Whether the design settings section in the inspector should be expanded.
    pub expand_settings: bool,
    /// Parameters dock tab ("stone", "lighting", "`render_output`").
    pub parameters_tab: Option<&'static str>,
    /// Solid preview view mode (0 Solid, 1 Path-Traced, 2 Both, 3 Diagram, 4 Ray-Traced).
    pub solid_view_mode: Option<i32>,
    /// Camera focus target.
    pub focus: CameraFocus,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            view: StageView::Either,
            library: LibraryPanel::Keep,
            inspector_tab: None,
            expand_tier_table: false,
            expand_settings: false,
            parameters_tab: None,
            solid_view_mode: None,
            focus: CameraFocus::Keep,
        }
    }
}

/// Per-guide override values.
#[derive(Clone, Copy, Debug, Default)]
pub struct StageOverride {
    pub view: Option<StageView>,
    pub library: Option<LibraryPanel>,
    pub inspector_tab: Option<i32>,
    pub expand_tier_table: Option<bool>,
    pub expand_settings: Option<bool>,
    pub parameters_tab: Option<&'static str>,
    pub solid_view_mode: Option<i32>,
    pub focus: Option<CameraFocus>,
}

impl StageOverride {
    /// An empty override with no fields set.
    pub const EMPTY: Self = Self {
        view: None,
        library: None,
        inspector_tab: None,
        expand_tier_table: None,
        expand_settings: None,
        parameters_tab: None,
        solid_view_mode: None,
        focus: None,
    };

    /// Override setting only `view`.
    #[must_use]
    pub const fn view(v: StageView) -> Self {
        Self {
            view: Some(v),
            ..Self::EMPTY
        }
    }

    /// Override setting only `library`.
    #[must_use]
    pub const fn library(l: LibraryPanel) -> Self {
        Self {
            library: Some(l),
            ..Self::EMPTY
        }
    }

    /// Override setting both `view` and `library`.
    #[must_use]
    pub const fn view_and_library(v: StageView, l: LibraryPanel) -> Self {
        Self {
            view: Some(v),
            library: Some(l),
            ..Self::EMPTY
        }
    }
}

const fn ov_view(v: StageView) -> StageOverride {
    StageOverride::view(v)
}

const fn ov_both(v: StageView, l: LibraryPanel) -> StageOverride {
    StageOverride::view_and_library(v, l)
}

const fn ov_library(l: LibraryPanel) -> StageOverride {
    StageOverride::library(l)
}

/// Per-guide override table entries.
///
/// Order: specific step title overrides first, then guide-wide (`None`) overrides.
#[rustfmt::skip]
pub const OVERRIDES: &[(&str, Option<&str>, StageOverride)] = &[
    // library-* guides show the library open throughout.
    ("library-import", None, ov_both(StageView::Either, LibraryPanel::Shown)),
    ("library-search", None, ov_both(StageView::Either, LibraryPanel::Shown)),
    ("library-filters", None, ov_both(StageView::Either, LibraryPanel::Shown)),
    ("library-load-design", None, ov_both(StageView::Either, LibraryPanel::Shown)),
    ("library-rough-planner", None, ov_both(StageView::Either, LibraryPanel::Shown)),
    // viewing-live-render step 1 switches to Live Render, so it starts in Edit.
    ("viewing-live-render", Some("Open Live Render"), ov_view(StageView::Edit)),
    // viewing-live-render steps 2 to 5 take place in Live Render.
    ("viewing-live-render", Some("Turn the stone and move the light"), ov_view(StageView::LiveRender)),
    ("viewing-live-render", Some("Choose a lighting rig"), ov_view(StageView::LiveRender)),
    ("viewing-live-render", Some("The Parameters dock"), ov_view(StageView::LiveRender)),
    ("viewing-live-render", Some("The stone's material"), ov_view(StageView::LiveRender)),
    // viewing-design-lighting step 1 switches to Live Render, so it starts in Edit.
    ("viewing-design-lighting", Some("Open Live Render"), ov_view(StageView::Edit)),
    // viewing-design-lighting steps 2 to 5 take place in Live Render.
    ("viewing-design-lighting", Some("Choose the lighting"), ov_view(StageView::LiveRender)),
    ("viewing-design-lighting", Some("Save it for this design"), ov_view(StageView::LiveRender)),
    ("viewing-design-lighting", Some("Forget it again"), ov_view(StageView::LiveRender)),
    ("viewing-design-lighting", Some("Where a design's lighting applies"), ov_view(StageView::LiveRender)),
    // Preference and dialog guides keep the library panel as is. They pin no view: a step that
    // names a control of one view (the larger-handles step points at the solid picture) still
    // gets that view from its highlight target, and the others stay in the current one.
    ("prefs-interface-mode", None, ov_library(LibraryPanel::Keep)),
    ("prefs-high-contrast", None, ov_library(LibraryPanel::Keep)),
    ("prefs-interface-scale", None, ov_library(LibraryPanel::Keep)),
    ("prefs-larger-handles", None, ov_library(LibraryPanel::Keep)),
    ("app-help-viewer", None, ov_library(LibraryPanel::Keep)),
    ("app-glossary", None, ov_library(LibraryPanel::Keep)),
];

/// The stage of step `index` of `guide`.
#[must_use]
pub fn stage_for(guide: &Guide, index: usize) -> Stage {
    let Some(step) = guide.steps.get(index) else {
        return Stage::default();
    };

    let mut view: Option<StageView> = None;
    let mut library: Option<LibraryPanel> = None;
    let mut inspector_tab: Option<Option<i32>> = None;
    let mut expand_tier_table: Option<bool> = None;
    let mut expand_settings: Option<bool> = None;
    let mut parameters_tab: Option<Option<&'static str>> = None;
    let mut solid_view_mode: Option<Option<i32>> = None;
    let mut focus: Option<CameraFocus> = None;

    // Rule 1: Per-guide override table.
    // Check specific step title matches first, then guide-level matches.
    for &(g_id, s_title, ov) in OVERRIDES {
        if g_id == guide.id && s_title == Some(step.title.as_str()) {
            apply_override(
                ov,
                &mut view,
                &mut library,
                &mut inspector_tab,
                &mut expand_tier_table,
                &mut expand_settings,
                &mut parameters_tab,
                &mut solid_view_mode,
                &mut focus,
            );
            break;
        }
    }
    for &(g_id, s_title, ov) in OVERRIDES {
        if g_id == guide.id && s_title.is_none() {
            apply_override(
                ov,
                &mut view,
                &mut library,
                &mut inspector_tab,
                &mut expand_tier_table,
                &mut expand_settings,
                &mut parameters_tab,
                &mut solid_view_mode,
                &mut focus,
            );
            break;
        }
    }

    // Rule 2: Highlight target.
    apply_highlight_target(
        &step.highlight_target,
        &mut view,
        &mut library,
        &mut inspector_tab,
        &mut expand_tier_table,
        &mut expand_settings,
        &mut parameters_tab,
    );

    // Rule 3: Goal.
    apply_goal(
        &step.goal,
        step.perform.as_ref(),
        &mut view,
        &mut inspector_tab,
        &mut solid_view_mode,
    );

    // Rule 4: Recipe (Perform).
    if let Some(perf) = &step.perform {
        apply_recipe(
            perf,
            &mut view,
            &mut inspector_tab,
            &mut expand_tier_table,
            &mut parameters_tab,
        );
    }

    // Rule 6: Camera Focus (if not decided by an override).
    if focus.is_none() {
        focus = Some(derive_focus(step));
    }

    // Rule 5: Defaults.
    let resolved_view = view.unwrap_or(StageView::Either);
    let resolved_library = library.unwrap_or(match resolved_view {
        StageView::Edit | StageView::LiveRender => LibraryPanel::Hidden,
        StageView::Either => LibraryPanel::Keep,
    });

    Stage {
        view: resolved_view,
        library: resolved_library,
        inspector_tab: inspector_tab.unwrap_or(None),
        expand_tier_table: expand_tier_table.unwrap_or(false),
        expand_settings: expand_settings.unwrap_or(false),
        parameters_tab: parameters_tab.unwrap_or(None),
        solid_view_mode: solid_view_mode.unwrap_or(None),
        focus: focus.unwrap_or(CameraFocus::Keep),
    }
}

/// The stage that must be in place before the video's first frame (= `stage_for(guide, 0)`,
/// except Live Render guides whose first step asks to open Live Render: those start in Edit).
#[must_use]
pub fn opening_stage(guide: &Guide) -> Stage {
    let mut stage = stage_for(guide, 0);

    // Live Render guides whose first step asks to open Live Render start in Edit.
    if let Some(first_step) = guide.steps.first()
        && (first_step.title == "Open Live Render"
            || first_step.highlight_target == "live_render_tab"
            || matches!(&first_step.goal, Goal::Event(e) if e == "live_render_opened"))
    {
        stage.view = StageView::Edit;
        stage.library = LibraryPanel::Hidden;
    }

    // If still Either, look ahead to the first step with a concrete view, else Live Render.
    if stage.view == StageView::Either {
        stage.view = (1..guide.steps.len())
            .find_map(|i| {
                let v = stage_for(guide, i).view;
                (v != StageView::Either).then_some(v)
            })
            .unwrap_or(StageView::LiveRender);
    }

    stage
}

#[expect(
    clippy::option_option,
    clippy::too_many_arguments,
    reason = "internal helper populating unassigned fields; Option<Option<T>> distinguishes unset vs explicitly set to None"
)]
const fn apply_override(
    ov: StageOverride,
    view: &mut Option<StageView>,
    library: &mut Option<LibraryPanel>,
    inspector_tab: &mut Option<Option<i32>>,
    expand_tier_table: &mut Option<bool>,
    expand_settings: &mut Option<bool>,
    parameters_tab: &mut Option<Option<&'static str>>,
    solid_view_mode: &mut Option<Option<i32>>,
    focus: &mut Option<CameraFocus>,
) {
    if view.is_none() && ov.view.is_some() {
        *view = ov.view;
    }
    if library.is_none() && ov.library.is_some() {
        *library = ov.library;
    }
    if inspector_tab.is_none() && ov.inspector_tab.is_some() {
        *inspector_tab = Some(ov.inspector_tab);
    }
    if expand_tier_table.is_none() && ov.expand_tier_table.is_some() {
        *expand_tier_table = ov.expand_tier_table;
    }
    if expand_settings.is_none() && ov.expand_settings.is_some() {
        *expand_settings = ov.expand_settings;
    }
    if parameters_tab.is_none() && ov.parameters_tab.is_some() {
        *parameters_tab = Some(ov.parameters_tab);
    }
    if solid_view_mode.is_none() && ov.solid_view_mode.is_some() {
        *solid_view_mode = Some(ov.solid_view_mode);
    }
    if focus.is_none() && ov.focus.is_some() {
        *focus = ov.focus;
    }
}

#[expect(
    clippy::option_option,
    reason = "Option<Option<T>> distinguishes unset vs explicitly set to None"
)]
fn apply_highlight_target(
    target: &str,
    view: &mut Option<StageView>,
    library: &mut Option<LibraryPanel>,
    inspector_tab: &mut Option<Option<i32>>,
    expand_tier_table: &mut Option<bool>,
    expand_settings: &mut Option<bool>,
    parameters_tab: &mut Option<Option<&'static str>>,
) {
    match target {
        "inspector_tier" => {
            let _ = view.get_or_insert(StageView::Edit);
            let _ = inspector_tab.get_or_insert(Some(0));
        }
        "preform_tab" => {
            let _ = view.get_or_insert(StageView::Edit);
            let _ = inspector_tab.get_or_insert(Some(1));
        }
        "optimize_tab" => {
            let _ = view.get_or_insert(StageView::Edit);
            let _ = inspector_tab.get_or_insert(Some(2));
        }
        "history_tab" => {
            let _ = view.get_or_insert(StageView::Edit);
            let _ = inspector_tab.get_or_insert(Some(4));
        }
        "tier_table" => {
            let _ = view.get_or_insert(StageView::Edit);
            let _ = expand_tier_table.get_or_insert(true);
        }
        "design_settings" => {
            let _ = view.get_or_insert(StageView::Edit);
            let _ = expand_settings.get_or_insert(true);
        }
        "solve_button"
        | "auto_solve"
        | "deep_solve_button"
        | "retarget_button"
        | "cutting_mode_button"
        | "snapshot_button"
        | "compare_button"
        | "export_asc_button"
        | "export_menu"
        | "save_button"
        | "open_button"
        | "load_button"
        | "verdict_badge"
        | "solid_viewport"
        | "view_modes"
        | "cut_slider"
        | "snap_pill"
        | "slice_pill"
        | "new_design_dialog" => {
            let _ = view.get_or_insert(StageView::Edit);
        }
        "lighting_combo" => {
            let _ = view.get_or_insert(StageView::LiveRender);
        }
        "stone_tab" => {
            let _ = parameters_tab.get_or_insert(Some("stone"));
            let _ = view.get_or_insert(StageView::Either);
        }
        "lighting_tab" => {
            let _ = parameters_tab.get_or_insert(Some("lighting"));
            let _ = view.get_or_insert(StageView::Either);
        }
        "render_output_tab" => {
            let _ = parameters_tab.get_or_insert(Some("render_output"));
            let _ = view.get_or_insert(StageView::Either);
        }
        "live_render_tab" | "mode_switch" => {
            let _ = view.get_or_insert(StageView::Either);
        }
        "library_search" | "library_filters" | "library_import" => {
            let _ = library.get_or_insert(LibraryPanel::Shown);
        }
        _ => {}
    }
}

#[expect(
    clippy::option_option,
    reason = "Option<Option<T>> distinguishes unset vs explicitly set to None"
)]
fn apply_goal(
    goal: &Goal,
    perform: Option<&Perform>,
    view: &mut Option<StageView>,
    inspector_tab: &mut Option<Option<i32>>,
    solid_view_mode: &mut Option<Option<i32>>,
) {
    match goal {
        Goal::TierMatches { .. }
        | Goal::TierExists(_)
        | Goal::TierCountAtLeast(_)
        | Goal::ConcaveTiersAtLeast(_)
        | Goal::DesignRebuilt { .. }
        | Goal::SolvedClosed
        | Goal::YieldApplied
        | Goal::Material(_) => {
            if view.is_none() {
                *view = Some(StageView::Edit);
            }
        }
        Goal::InspectorTab(t) => {
            if view.is_none() {
                *view = Some(StageView::Edit);
            }
            // If the step's recipe is Perform::InspectorTab, the stage leaves the tab alone
            // and the director films the switch. Otherwise, stage starts at the tab before t.
            let is_perform_tab = matches!(perform, Some(Perform::InspectorTab(_)));
            if is_perform_tab {
                *inspector_tab = Some(None);
            } else {
                let before = i32::from(*t == 0);
                *inspector_tab = Some(Some(before));
            }
        }
        Goal::ViewMode(_) => {
            if view.is_none() {
                *view = Some(StageView::Edit);
            }
            if solid_view_mode.is_none() {
                *solid_view_mode = Some(None);
            }
        }
        Goal::All(inner) | Goal::Any(inner) => {
            for g in inner {
                apply_goal(g, perform, view, inspector_tab, solid_view_mode);
            }
        }
        _ => {}
    }
}

#[expect(
    clippy::option_option,
    reason = "Option<Option<T>> distinguishes unset vs explicitly set to None"
)]
fn apply_recipe(
    perf: &Perform,
    view: &mut Option<StageView>,
    inspector_tab: &mut Option<Option<i32>>,
    expand_tier_table: &mut Option<bool>,
    parameters_tab: &mut Option<Option<&'static str>>,
) {
    match perf {
        Perform::Tier(_)
        | Perform::ConcaveTier { .. }
        | Perform::SelectTier(_)
        | Perform::Steps(_)
        | Perform::MirrorTier { .. }
        | Perform::ClearRelation(_)
        | Perform::TierNote { .. }
        | Perform::CheaterOffset { .. } => {
            if view.is_none() {
                *view = Some(StageView::Edit);
            }
            if inspector_tab.is_none() {
                *inspector_tab = Some(Some(0));
            }
            if expand_tier_table.is_none() {
                *expand_tier_table = Some(true);
            }
        }
        Perform::Yield { .. } => {
            if view.is_none() {
                *view = Some(StageView::Edit);
            }
            if inspector_tab.is_none() {
                *inspector_tab = Some(Some(1));
            }
        }
        Perform::Material(_) => {
            if parameters_tab.is_none() {
                *parameters_tab = Some(Some("stone"));
            }
        }
        Perform::Command(id) => {
            if id.starts_with("solve.") || id.starts_with("tiers.") || id == "edit.undo" {
                if view.is_none() {
                    *view = Some(StageView::Edit);
                }
            } else if id == "view.live_render" {
                // Command switches view, leave view untouched so transition happens during step.
            }
        }
        Perform::Sequence(parts) => {
            for part in parts {
                apply_recipe(part, view, inspector_tab, expand_tier_table, parameters_tab);
            }
        }
        Perform::NewDesign { .. } if view.is_none() => {
            *view = Some(StageView::Edit);
        }
        _ => {}
    }
}

fn derive_focus(step: &GuideStep) -> CameraFocus {
    // Check for Whole stone targets/goals/recipes first.
    if matches!(
        &step.goal,
        Goal::SolvedClosed | Goal::YieldApplied | Goal::FreshDesign { .. }
    ) {
        return CameraFocus::Whole;
    }
    if matches!(
        step.highlight_target.as_str(),
        "new_design_dialog" | "solve_button" | "auto_solve" | "deep_solve_button" | "verdict_badge"
    ) {
        return CameraFocus::Whole;
    }
    if let Some(perf) = &step.perform {
        if matches!(perf, Perform::NewDesign { .. } | Perform::Yield { .. }) {
            return CameraFocus::Whole;
        }
        if let Perform::Command(cmd) = perf
            && cmd.starts_with("solve.")
        {
            return CameraFocus::Whole;
        }
    }

    // Next, check tier from Goal.
    if let Some(focus) = focus_from_goal(&step.goal) {
        return focus;
    }

    // Next, check recipe (Perform).
    if let Some(perf) = &step.perform
        && let Some(focus) = focus_from_perform(perf)
    {
        return focus;
    }

    // Check step title and intro words.
    let text = format!("{} {}", step.title, step.intro).to_lowercase();
    if text.contains("crown") {
        return CameraFocus::Crown;
    }
    if text.contains("pavilion") || text.contains("culet") {
        return CameraFocus::Pavilion;
    }
    if text.contains("girdle") {
        return CameraFocus::Girdle;
    }
    if text.contains("table") {
        return CameraFocus::Top;
    }

    CameraFocus::Keep
}

fn focus_from_goal(goal: &Goal) -> Option<CameraFocus> {
    match goal {
        Goal::TierMatches {
            name, angle_deg, ..
        } => focus_from_name_and_angle(name, Some(*angle_deg)),
        Goal::TierExists(name) => focus_from_name_and_angle(name, None),
        Goal::All(inner) | Goal::Any(inner) => inner.iter().find_map(focus_from_goal),
        _ => None,
    }
}

fn focus_from_perform(perf: &Perform) -> Option<CameraFocus> {
    match perf {
        Perform::Tier(entry) => focus_from_tier_entry(entry),
        Perform::SelectTier(name)
        | Perform::MirrorTier { tier: name, .. }
        | Perform::ClearRelation(name)
        | Perform::TierNote { tier: name, .. }
        | Perform::CheaterOffset { tier: name, .. } => focus_from_name_and_angle(name, None),
        Perform::Steps(series) => {
            let angle = series.start.parse::<f64>().ok();
            focus_from_name_and_angle(&series.name, angle)
        }
        Perform::Sequence(parts) => parts.iter().find_map(focus_from_perform),
        _ => None,
    }
}

fn focus_from_tier_entry(entry: &TierEntry) -> Option<CameraFocus> {
    let name = entry.name.as_deref().or(entry.edit.as_deref())?;
    let angle = entry.angle.as_deref().and_then(|a| a.parse::<f64>().ok());
    focus_from_name_and_angle(name, angle)
}

fn focus_from_name_and_angle(name: &str, angle: Option<f64>) -> Option<CameraFocus> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        if let Some(deg) = angle {
            return focus_from_angle(deg);
        }
        return None;
    }

    let first_char = trimmed.chars().next()?.to_ascii_uppercase();
    match first_char {
        'C' => return Some(CameraFocus::Crown),
        'P' => return Some(CameraFocus::Pavilion),
        'G' => return Some(CameraFocus::Girdle),
        'T' => return Some(CameraFocus::Top),
        _ => {}
    }

    let lower = trimmed.to_lowercase();
    if lower.contains("crown")
        || lower.contains("star")
        || lower.contains("break")
        || lower.contains("kite")
    {
        return Some(CameraFocus::Crown);
    }
    if lower.contains("pavilion") || lower.contains("culet") {
        return Some(CameraFocus::Pavilion);
    }
    if lower.contains("girdle") {
        return Some(CameraFocus::Girdle);
    }
    if lower.contains("table") {
        return Some(CameraFocus::Top);
    }

    if let Some(deg) = angle {
        return focus_from_angle(deg);
    }

    None
}

fn focus_from_angle(angle_deg: f64) -> Option<CameraFocus> {
    if (angle_deg - 90.0).abs() <= 0.05 {
        Some(CameraFocus::Girdle)
    } else if angle_deg.abs() <= 0.05 {
        Some(CameraFocus::Top)
    } else if angle_deg > 0.0 && angle_deg < 90.0 {
        Some(CameraFocus::Crown)
    } else if angle_deg < 0.0 {
        Some(CameraFocus::Pavilion)
    } else {
        None
    }
}
