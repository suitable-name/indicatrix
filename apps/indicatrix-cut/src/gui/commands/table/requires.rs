//! The requirement lists that several commands of [`super::COMMANDS`] share.

use crate::gui::commands::{rules::Check, state::GuideGroup};

// Requirement lists shared by several commands.
pub(super) const NONE: &[Check] = &[];
pub(super) const EDITOR: &[Check] = &[Check::Editor];
pub(super) const NEW_DESIGN: &[Check] = &[Check::Guide(GuideGroup::NewDesign), Check::Editor];
pub(super) const FILE_OPS: &[Check] = &[Check::Guide(GuideGroup::FileOps), Check::Editor];
pub(super) const FILE_OPS_DESIGN: &[Check] = &[
    Check::Guide(GuideGroup::FileOps),
    Check::Editor,
    Check::Design,
];
pub(super) const FILE_OPS_LIBRARY: &[Check] = &[
    Check::Guide(GuideGroup::FileOps),
    Check::Editor,
    Check::LibrarySelected,
];
pub(super) const LIBRARY_SELECTED: &[Check] = &[Check::LibrarySelected];
pub(super) const UNDO: &[Check] = &[
    Check::Guide(GuideGroup::History),
    Check::Editor,
    Check::CanUndo,
];
pub(super) const REDO: &[Check] = &[
    Check::Guide(GuideGroup::History),
    Check::Editor,
    Check::CanRedo,
];
pub(super) const HISTORY_EDITOR: &[Check] = &[Check::Guide(GuideGroup::History), Check::Editor];
pub(super) const HISTORY_DESIGN: &[Check] = &[
    Check::Guide(GuideGroup::History),
    Check::Editor,
    Check::Design,
];
pub(super) const ADVANCED: &[Check] = &[Check::Guide(GuideGroup::Advanced)];
pub(super) const ADVANCED_DESIGN: &[Check] = &[Check::Guide(GuideGroup::Advanced), Check::Design];
pub(super) const TO_SIMPLE: &[Check] = &[
    Check::Guide(GuideGroup::Advanced),
    Check::AdvancedInterfaceOn,
];
pub(super) const TO_ADVANCED: &[Check] =
    &[Check::Guide(GuideGroup::Advanced), Check::SimpleInterfaceOn];
pub(super) const TIER_FORM: &[Check] = &[Check::Guide(GuideGroup::TierForm), Check::Design];
pub(super) const CONCAVE_TIER: &[Check] = &[Check::Guide(GuideGroup::ConcaveTier), Check::Design];
pub(super) const TIER_TABLE: &[Check] = &[Check::Guide(GuideGroup::TierTable), Check::Design];
pub(super) const TIER_ACTION: &[Check] = &[
    Check::Guide(GuideGroup::TierTable),
    Check::Design,
    Check::TierSelected,
];
pub(super) const SOLVE: &[Check] = &[
    Check::Guide(GuideGroup::Solve),
    Check::Editor,
    Check::Design,
    Check::Idle,
];
pub(super) const SOLVE_SETTING: &[Check] = &[Check::Guide(GuideGroup::Solve)];
pub(super) const DEEP_SOLVE: &[Check] = &[
    Check::Guide(GuideGroup::Advanced),
    Check::Design,
    Check::Idle,
    Check::DeepSolveAvailable,
];
pub(super) const OPTIMIZE: &[Check] = &[
    Check::Guide(GuideGroup::Advanced),
    Check::Design,
    Check::Idle,
    Check::OptimizeAvailable,
];
pub(super) const VIEW_TABS: &[Check] = &[Check::Guide(GuideGroup::ViewTabs), Check::Editor];
pub(super) const INSPECTOR_TIER: &[Check] = &[Check::Guide(GuideGroup::TierForm), Check::Editor];
pub(super) const INSPECTOR_PREFORM: &[Check] =
    &[Check::Guide(GuideGroup::PreformTab), Check::Editor];
pub(super) const INSPECTOR_OPTIMIZE: &[Check] =
    &[Check::Guide(GuideGroup::Advanced), Check::Editor];
/// The Schedule tab exists only in the Advanced interface: the inspector sends a Simple-mode
/// request for it straight back to the Tier tab, so the palette says why instead.
pub(super) const INSPECTOR_SCHEDULE: &[Check] = &[Check::Editor, Check::NeedsAdvancedInterface];
pub(super) const SLICE: &[Check] = &[Check::Editor, Check::Design, Check::NotDiagramView];
pub(super) const MATERIAL_EDITOR: &[Check] = &[Check::Guide(GuideGroup::DesignSettings)];
