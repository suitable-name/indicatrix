//! Starting a guide: arranging the state it starts from (a new design, a library design, a
//! design that must be open), then opening its first step.
//!
//! The decisions are pure and live in `indicatrix_editor::guide` (`plan_start`,
//! `launch_status`). This module is the shell around them: it asks the editor for the design
//! through the same callbacks the New Design dialog and the Load Selected button use -- so
//! the unsaved-changes question and every other guard apply unchanged -- and then waits for
//! the design to arrive. When the question is on screen, or a Save is still landing, the
//! guide opens only after the design has really been replaced; a Cancel forgets the launch.

use super::runtime::{self, Launch, LaunchWant};
use crate::{
    EditorModel, GuideAllow, GuideModel, GuideStepData, LibraryModel, MainWindow,
    gui::{editor::state::AfterSave, show_toast},
};
use indicatrix_cut_core::{FreshDesignSpec, PreformShape};
use indicatrix_editor::{
    guide::{
        Group, Guide, GuideStep, LIBRARY_GRACE_TICKS, LaunchObservation, LaunchStatus, StartPlan,
        launch_status, plan_start,
    },
    material::gear_index_from_teeth,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// The New Design dialog's "Preform Shape" index for a block.
const BLOCK_SHAPE_INDEX: i32 = 0;
/// The dialog's "Preform Shape" index for a cylinder.
const CYLINDER_SHAPE_INDEX: i32 = 1;
/// The dialog's "Starting Material" index for "(none)".
const NO_MATERIAL_INDEX: i32 = 0;

/// The New Design dialog's Empty form, filled in for a lesson's starting design.
///
/// The guides ask for their design through that form's callback, so the unsaved-changes
/// question and every other guard apply unchanged. The values come from the lesson's own
/// [`FreshDesignSpec`] (`indicatrix_editor::guide::lesson_start_spec`): a template lesson
/// keeps the template's rough and symmetry, so a deep pavilion is not clipped by the blank.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EmptyFormFields {
    shape_index: i32,
    half_width: String,
    length_over_width: String,
    depth: String,
    gear_index: i32,
    gear_custom: String,
    symmetry_order: String,
    mirror: bool,
    material_index: i32,
}

impl EmptyFormFields {
    /// The form's fields for `spec`. A lesson starts without a material, and its rough is a
    /// block or a cylinder (the form's side count is fixed).
    fn of(spec: &FreshDesignSpec) -> Self {
        let gear_index = gear_index_from_teeth(spec.gear_teeth);
        Self {
            shape_index: match spec.preform.shape {
                PreformShape::Block => BLOCK_SHAPE_INDEX,
                PreformShape::Cylinder { .. } => CYLINDER_SHAPE_INDEX,
            },
            half_width: spec.preform.half_width.to_string(),
            length_over_width: spec.preform.length_over_width.to_string(),
            depth: spec.preform.depth.to_string(),
            gear_index,
            // A gear outside the dialog's presets is the "Custom" entry, typed in.
            gear_custom: spec.gear_teeth.to_string(),
            symmetry_order: spec.symmetry_order.to_string(),
            mirror: spec.mirror,
            material_index: NO_MATERIAL_INDEX,
        }
    }
}

/// `allow`'s groups as the Slint `GuideAllow` struct: a group not listed is locked.
pub(super) fn guide_allow(allow: &[Group]) -> GuideAllow {
    GuideAllow {
        new_design: allow.contains(&Group::NewDesign),
        tier_form: allow.contains(&Group::TierForm),
        tier_table: allow.contains(&Group::TierTable),
        design_settings: allow.contains(&Group::DesignSettings),
        solve: allow.contains(&Group::Solve),
        preform_tab: allow.contains(&Group::PreformTab),
        advanced: allow.contains(&Group::Advanced),
        view_tabs: allow.contains(&Group::ViewTabs),
        file_ops: allow.contains(&Group::FileOps),
        history: allow.contains(&Group::History),
        // Concave tiers are not part of the guided lessons: authoring one edits both the
        // tier form and the tier table, so a step leaves it open only when it leaves both of
        // those open (a step that blocks nothing).
        concave_tier: allow.contains(&Group::TierForm) && allow.contains(&Group::TierTable),
    }
}

/// One [`GuideStep`] (number `index` of its guide) as the Slint `GuideStepData` the panel
/// renders.
pub(super) fn step_data(index: usize, step: &GuideStep) -> GuideStepData {
    let actions: Vec<SharedString> = step
        .actions
        .iter()
        .map(|action| SharedString::from(action.as_str()))
        .collect();
    GuideStepData {
        title: step.title.as_str().into(),
        intro: step.intro.as_str().into(),
        actions: ModelRc::new(VecModel::from(actions)),
        check: step.check.as_str().into(),
        why: step.why.as_str().into(),
        waiting: step.waiting.as_str().into(),
        highlight_target: step.highlight_target.as_str().into(),
        completion: step.completion_key(index).into(),
        watches_ui: step.goal.watches_ui(),
        performable: step.can_perform(),
        allow: guide_allow(&step.allow),
    }
}

/// Pushes `guide`'s steps into `GuideModel` and opens its first step. Any guide that was
/// open is replaced.
pub(super) fn open_guide(ui: &MainWindow, guide: &Guide) {
    let steps: Vec<GuideStepData> = guide
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| step_data(index, step))
        .collect();
    let model = ui.global::<GuideModel>();
    model.set_steps(ModelRc::new(VecModel::from(steps)));
    model.set_guide_id(guide.id.as_str().into());
    model.set_guide_title(guide.title.as_str().into());
    runtime::clear_events();
    // The New Design dialog opens at "Start From: Empty", as the guides describe it.
    ui.global::<EditorModel>().set_new_template_index(0);
    model.invoke_begin();
}

/// `GuideModel.start_guide`: starts the guide with this id, after arranging the state it
/// starts from. A guide that cannot start says why in a toast.
pub(super) fn start_guide(ui: &MainWindow, id: &str) {
    let Some(guide) = runtime::guide(id) else {
        show_toast(ui, "That tutorial is not available.", "error");
        return;
    };
    let has_design = ui.global::<EditorModel>().get_has_design();
    match plan_start(&guide.starting_state, has_design) {
        StartPlan::Begin => open_guide(ui, &guide),
        StartPlan::Blocked(reason) => show_toast(ui, reason, "info"),
        StartPlan::CreateNew {
            template_index,
            spec,
        } => begin_new_design(ui, &guide, template_index, &spec),
        StartPlan::OpenLibrary { entry_id } => begin_library_design(ui, &guide, entry_id),
    }
}

/// The id of the design open now, to tell later whether it was replaced.
fn open_design_uuid() -> String {
    runtime::state()
        .and_then(|state| {
            state
                .try_borrow()
                .ok()
                .map(|st| st.design_uuid().to_owned())
        })
        .unwrap_or_default()
}

/// Asks the editor for the new design `spec` describes (no material), seeded from gallery card
/// `template_index`. A template lesson is cut from the template's own rough, a lesson that
/// starts from nothing from the worked example's blank. The editor asks about unsaved changes
/// first; the guide opens once the new design is there.
fn begin_new_design(ui: &MainWindow, guide: &Guide, template_index: i32, spec: &FreshDesignSpec) {
    runtime::set_launch(Some(Launch {
        guide_id: guide.id.clone(),
        want: LaunchWant::NewDesign,
        ticks: 0,
        design_arrived: false,
        uuid_before: open_design_uuid(),
    }));
    request_new_design(ui, spec, template_index);
    evaluate_launch(ui);
}

/// Presses Create on the New Design dialog's Empty form filled in from `spec`, through the
/// callback the dialog's button calls (so the unsaved-changes question and every other guard
/// apply, and the dialog closes itself once the design is made). Next on the "start a new
/// design" step uses it too.
pub(super) fn request_new_design(ui: &MainWindow, spec: &FreshDesignSpec, template_index: i32) {
    let fields = EmptyFormFields::of(spec);
    ui.global::<EditorModel>().invoke_new_design_create(
        fields.shape_index,
        fields.half_width.into(),
        fields.length_over_width.into(),
        fields.depth.into(),
        fields.gear_index,
        fields.gear_custom.into(),
        fields.symmetry_order.into(),
        fields.mirror,
        fields.material_index,
        template_index,
    );
}

/// Asks the editor to open library catalogue entry `entry_id`, through the Load Selected
/// callback (which asks about unsaved changes first). The guide opens once that design is
/// the open one.
fn begin_library_design(ui: &MainWindow, guide: &Guide, entry_id: i64) {
    let Ok(row) = i32::try_from(entry_id) else {
        show_toast(ui, "That library design is not available.", "error");
        return;
    };
    runtime::set_launch(Some(Launch {
        guide_id: guide.id.clone(),
        want: LaunchWant::Library { entry_id },
        ticks: 0,
        design_arrived: false,
        uuid_before: open_design_uuid(),
    }));
    ui.global::<LibraryModel>().set_selected_entry_id(row);
    ui.global::<EditorModel>().invoke_load_selected();
    evaluate_launch(ui);
}

/// What the editor state says about the design a launch waits for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StateFacts {
    /// The unsaved-changes question was answered but its action has not run yet.
    decision_pending: bool,
    /// The open design came from the catalogue entry the launch asked for.
    source_matches: bool,
    /// The open design is not the one that was open when the launch began.
    uuid_changed: bool,
}

/// Reads [`StateFacts`] from the editor state, if it can be read right now.
fn state_facts(launch: &Launch) -> StateFacts {
    let Some(state) = runtime::state() else {
        return StateFacts::default();
    };
    let Ok(st) = state.try_borrow() else {
        // The state is being written (a save landing, a design replaced): not settled.
        return StateFacts {
            decision_pending: true,
            ..StateFacts::default()
        };
    };
    StateFacts {
        decision_pending: st.pending_unsaved_action.is_some()
            || matches!(st.after_save, Some(AfterSave::Resume(_))),
        source_matches: matches!(
            launch.want,
            LaunchWant::Library { entry_id } if st.source_entry_id == Some(entry_id)
        ),
        uuid_changed: st.design_uuid() != launch.uuid_before,
    }
}

/// What the UI sees of the design a launch waits for.
fn observe(ui: &MainWindow, launch: &Launch) -> LaunchObservation {
    let dialog_open = ui.global::<EditorModel>().get_unsaved_dialog_open();
    let StateFacts {
        decision_pending,
        source_matches,
        uuid_changed,
    } = state_facts(launch);
    let design_arrived = match launch.want {
        LaunchWant::NewDesign => launch.design_arrived,
        LaunchWant::Library { .. } => {
            uuid_changed || (source_matches && !dialog_open && !decision_pending)
        }
    };
    LaunchObservation {
        design_arrived,
        dialog_open,
        decision_pending,
        ticks: launch.ticks,
    }
}

/// How many timer ticks a launch keeps waiting with nothing visibly pending.
const fn grace_ticks(want: &LaunchWant) -> u32 {
    match want {
        // The New Design callback finishes before it returns.
        LaunchWant::NewDesign => 0,
        // A remote design downloads in the background.
        LaunchWant::Library { .. } => LIBRARY_GRACE_TICKS,
    }
}

/// Checks the launch in progress (if any): opens the guide when its design has arrived,
/// keeps waiting while the editor is still deciding, forgets the launch when nothing will
/// deliver the design. Called right after the editor was asked, whenever it reports a new
/// design, and on every `GuideModel.launch_tick`.
pub(super) fn evaluate_launch(ui: &MainWindow) {
    let model = ui.global::<GuideModel>();
    let Some(launch) = runtime::launch() else {
        model.set_launch_pending(false);
        return;
    };
    match launch_status(&observe(ui, &launch), grace_ticks(&launch.want)) {
        LaunchStatus::Ready => {
            runtime::set_launch(None);
            model.set_launch_pending(false);
            if let Some(guide) = runtime::guide(&launch.guide_id) {
                open_guide(ui, &guide);
            }
        }
        LaunchStatus::Waiting => {
            runtime::count_launch_tick();
            model.set_launch_pending(true);
        }
        LaunchStatus::Abandoned => {
            runtime::set_launch(None);
            model.set_launch_pending(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The very constant `EditorModel.new_design_create`'s callback passes, not a copy of it.
    use crate::gui::editor::callbacks::FIXED_CYLINDER_PREFORM_SIDES;
    use indicatrix_cut_core::PreformSpec;
    use indicatrix_editor::{
        guide::{Goal, GuideCategory, StartingState, lesson_start_spec},
        loading::{parse_new_design_form, parse_preform_form},
        material::gear_choice_to_teeth,
        templates::template_cards,
    };
    use slint::Model as _;

    /// The design the New Design dialog's Empty form builds from `fields`: the same parsing,
    /// in the same order, as `EditorModel.new_design_create`'s callback.
    fn design_the_form_builds(fields: &EmptyFormFields) -> FreshDesignSpec {
        let gear_teeth =
            gear_choice_to_teeth(fields.gear_index, &fields.gear_custom).expect("a gear");
        let preform = parse_preform_form(
            fields.shape_index,
            &fields.half_width,
            &fields.length_over_width,
            &fields.depth,
            FIXED_CYLINDER_PREFORM_SIDES,
        )
        .expect("a preform");
        parse_new_design_form(
            gear_teeth,
            preform,
            &fields.symmetry_order,
            fields.mirror,
            fields.material_index,
        )
        .expect("a design")
    }

    #[test]
    fn the_form_builds_exactly_the_design_every_lesson_start_asks_for() {
        for card in 0..template_cards().len() {
            let spec = match plan_start(&StartingState::Template(card), false) {
                StartPlan::CreateNew { spec, .. } => spec,
                other => panic!("card {card} does not create a design: {other:?}"),
            };
            assert_eq!(
                design_the_form_builds(&EmptyFormFields::of(&spec)),
                spec,
                "card {card}"
            );
        }
    }

    #[test]
    fn a_template_lesson_asks_the_form_for_the_templates_own_rough() {
        // The Standard Round Brilliant's rough is 2.2 deep: its culet lies 0.88 below the
        // middle, which the 1.50-deep blank of an empty start would clip flat.
        let fields = EmptyFormFields::of(&lesson_start_spec(1));
        assert_eq!(fields.shape_index, CYLINDER_SHAPE_INDEX);
        assert_eq!(
            (
                fields.half_width.as_str(),
                fields.length_over_width.as_str(),
                fields.depth.as_str()
            ),
            ("1.5", "1", "2.2")
        );
        assert_eq!(fields.symmetry_order, "8");
        assert!(fields.mirror);
        assert_eq!(fields.material_index, NO_MATERIAL_INDEX);

        // An empty start keeps the worked example's blank, 1.50 deep.
        let empty = EmptyFormFields::of(&lesson_start_spec(0));
        assert_eq!(empty.depth, "1.5");
        assert_eq!(
            design_the_form_builds(&empty).preform,
            PreformSpec::cylinder(FIXED_CYLINDER_PREFORM_SIDES, 1.5, 1.0, 1.5)
        );
    }

    #[test]
    fn only_a_step_with_a_recipe_reaches_the_panel_as_performable() {
        use indicatrix_editor::guide::Perform;
        let guide = sample_guide();
        assert!(!step_data(0, &guide.steps[0]).performable, "a reading step");
        assert!(
            !step_data(1, &guide.steps[1]).performable,
            "an action step without a recipe only skips"
        );
        let with_recipe = guide.steps[1].clone().perform(Perform::InspectorTab(1));
        assert!(step_data(1, &with_recipe).performable);
    }

    #[test]
    fn a_step_leaves_exactly_the_groups_it_allows_open() {
        let allow = guide_allow(&[Group::Solve, Group::History]);
        assert!(allow.solve && allow.history);
        assert!(!allow.new_design && !allow.tier_form && !allow.tier_table);
        assert!(!allow.design_settings && !allow.preform_tab && !allow.advanced);
        assert!(!allow.view_tabs && !allow.file_ops);
    }

    #[test]
    fn concave_tiers_are_open_only_when_both_the_form_and_the_table_are() {
        assert!(guide_allow(indicatrix_editor::guide::ALL_GROUPS).concave_tier);
        assert!(!guide_allow(&[Group::TierForm]).concave_tier);
        assert!(!guide_allow(&[Group::TierTable]).concave_tier);
        assert!(guide_allow(&[Group::TierForm, Group::TierTable]).concave_tier);
        assert!(!guide_allow(&[]).concave_tier);
    }

    fn sample_guide() -> Guide {
        Guide::new("sample", "Sample", "A sample.", GuideCategory::Tiers)
            .step(GuideStep::new("Read", "Read this."))
            .step(
                GuideStep::new("Show the inspector", "Open a tab.")
                    .actions(["Click the Preform tab."])
                    .check("the Preform tab.")
                    .goal(Goal::InspectorTab(1), "the Preform tab")
                    .highlight("preform_tab")
                    .allow(&[Group::PreformTab]),
            )
    }

    #[test]
    fn a_reading_step_reaches_the_panel_as_manual() {
        let guide = sample_guide();
        let data = step_data(0, &guide.steps[0]);
        assert_eq!(data.completion.as_str(), "manual");
        assert!(!data.watches_ui);
        assert!(data.allow.new_design, "a reading step locks nothing");
    }

    #[test]
    fn an_automatic_step_reaches_the_panel_with_its_key_target_and_locks() {
        let guide = sample_guide();
        let data = step_data(1, &guide.steps[1]);
        assert_eq!(data.completion.as_str(), "goal:1");
        assert!(
            data.watches_ui,
            "the inspector tab changes without the design changing"
        );
        assert_eq!(data.highlight_target.as_str(), "preform_tab");
        assert_eq!(data.title.as_str(), "Show the inspector");
        assert_eq!(data.waiting.as_str(), "the Preform tab");
        assert!(data.allow.preform_tab && !data.allow.solve);
        assert_eq!(data.actions.row_count(), 1);
    }
}
