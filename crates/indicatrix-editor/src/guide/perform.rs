//! What the Next button does on a step the learner has not done yet: it does the step for them.
//!
//! A [`Perform`] is plain data (no UI types) that names what the learner would do, in the
//! editor's own words: fill in the New Design form and press Create, type this into the Tier
//! form and press Add Tier or Save Tier, pick this material and press Apply Material, press
//! Solve, run this palette command. A UI carries it out through the very callbacks those
//! actions call, so the edit is one undo step like the learner's own, a bad entry is refused by
//! the same parser with the same message, and the step then completes the usual way (the goal
//! is judged from STATE, never from "Next was pressed"). A perform that does not satisfy its
//! step's goal therefore stays visible as an unfinished step instead of being skipped silently.
//!
//! Where a step's recipe comes from:
//!
//! - [`default_perform`] derives it from the step's [`Goal`] when the goal fully says what the
//!   result is (a material, a view mode, an inspector tab, a solved stone, a tier at an angle);
//! - a few steps need data the goal does not carry (a Meets value, a Check goal's meaning):
//!   `perform_table` lists those by guide id and step title, and a "Build this design" lesson
//!   fills its steps in from the recipes it already generates;
//! - every other step has no recipe and keeps its Skip step button.
//!
//! [`attach_performs`] puts the final recipe on every step of a guide, so a UI only has to look
//! at [`GuideStep::perform`].

use super::{
    EMPTY_START_PREFORM, NEW_DESIGN_CREATED,
    goal::{Goal, MeetKind},
    model::Guide,
    perform_table::{self, Action},
    solving_events::{SNAPSHOT_TAKEN, SOLVE_REQUESTED},
    viewing_events::{
        HELP_OPENED, LIVE_RENDER_OPENED, SHORTCUTS_OPENED, SLICE_STARTED, SNAP_TOGGLED,
    },
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    ConstraintTier, Design, FreshDesignSpec, MaterialSelection, TierTarget, design::ConcaveTier,
};

/// The palette command that presses Solve.
pub const CMD_SOLVE: &str = "solve.run";
/// The palette command that presses Undo.
pub const CMD_UNDO: &str = "edit.undo";
/// The palette command that duplicates the selected tier.
pub const CMD_DUPLICATE: &str = "tiers.duplicate";
/// The palette command that removes the selected tier.
pub const CMD_DELETE: &str = "tiers.delete";
/// The palette command that moves the selected tier one place earlier.
pub const CMD_MOVE_UP: &str = "tiers.move_up";
/// The palette command that moves the selected tier one place later.
pub const CMD_MOVE_DOWN: &str = "tiers.move_down";
/// The palette command behind the tier table's Adopt all.
pub const CMD_ADOPT_ALL: &str = "tiers.adopt_all";
/// The palette command that compares the design with its snapshot.
pub const CMD_COMPARE: &str = "solve.compare";

/// The palette commands a [`Perform::Command`] may run (the ids of the desktop's command
/// table, `gui::commands::table`; a desktop test checks every one of them exists there).
pub const COMMANDS: &[&str] = &[
    CMD_SOLVE,
    CMD_UNDO,
    CMD_DUPLICATE,
    CMD_DELETE,
    CMD_MOVE_UP,
    CMD_MOVE_DOWN,
    CMD_ADOPT_ALL,
    "solve.snapshot",
    CMD_COMPARE,
    "view.live_render",
    "view.snap",
    "view.slice",
    "help.shortcuts",
    "help.manual",
];

/// What the learner types into the Tier form. A blank part of an edit stays as the picked row
/// has it, exactly as the form shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TierEntry {
    /// The row to pick first (any of the tier's names; case and surrounding spaces do not
    /// matter), so the form says Save Tier. `None` is "+ Add Tier": a new tier at the end.
    pub edit: Option<String>,
    /// The Angle field. A new tier needs one; an edit keeps the row's angle when `None`.
    pub angle: Option<String>,
    /// The Name field.
    pub name: Option<String>,
    /// The Indices field, in the form's own shorthand (`0:12:96`, `12 x8`).
    pub indices: Option<String>,
    /// The Meets combo (0 unspecified, 1 named, 2 exact scale, 3 depth, 4 girdle thickness,
    /// 5 table width) and its text. A new tier without one is "Unspecified vertex".
    pub meets: Option<(i32, String)>,
}

impl TierEntry {
    /// A new tier with every field typed in.
    #[must_use]
    pub fn add(angle: &str, meets_kind: i32, meets_text: &str, name: &str, indices: &str) -> Self {
        Self {
            edit: None,
            angle: Some(angle.to_owned()),
            name: Some(name.to_owned()),
            indices: Some(indices.to_owned()),
            meets: Some((meets_kind, meets_text.to_owned())),
        }
    }

    /// An edit of the row called `name`; chain the fields to change.
    #[must_use]
    pub fn of(name: &str) -> Self {
        Self {
            edit: Some(name.to_owned()),
            ..Self::default()
        }
    }

    /// Changes the Angle field.
    #[must_use]
    pub fn angle(mut self, text: &str) -> Self {
        self.angle = Some(text.to_owned());
        self
    }

    /// Changes the Name field.
    #[must_use]
    pub fn rename(mut self, name: &str) -> Self {
        self.name = Some(name.to_owned());
        self
    }

    /// Changes the Indices field.
    #[must_use]
    pub fn indices(mut self, text: &str) -> Self {
        self.indices = Some(text.to_owned());
        self
    }

    /// Changes the Meets combo and its text.
    #[must_use]
    pub fn meets(mut self, kind: i32, text: &str) -> Self {
        self.meets = Some((kind, text.to_owned()));
        self
    }
}

/// The six values the Tier form hands to its Save action, worked out from a [`TierEntry`] and
/// the design as it is now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierSave {
    /// The row being saved, or `-1` for a new tier.
    pub index: i32,
    /// The Angle field.
    pub angle: String,
    /// The Meets combo.
    pub constraint_kind: i32,
    /// The Meets text.
    pub constraint_text: String,
    /// The Name field.
    pub name: String,
    /// The Indices field.
    pub indices: String,
}

/// The Steps panel's Generate form.
#[derive(Clone, Debug, PartialEq)]
pub struct StepSeries {
    /// The name prefix (`Step` makes Step1, Step2 and so on).
    pub name: String,
    /// The first rung's angle.
    pub start: String,
    /// The angle between rungs.
    pub step: String,
    /// How many rungs.
    pub count: i32,
    /// The Indices every rung shares.
    pub indices: String,
    /// The anchor the first rung meets; blank for the design's own.
    pub anchor: String,
    /// Whether "Keep linked" is ticked.
    pub linked: bool,
}

/// One thing the Next button can do for the learner. See the module docs.
#[derive(Clone, Debug, PartialEq)]
pub enum Perform {
    /// The New Design dialog's Empty form, filled in from `spec`, and Create. The unsaved
    /// changes question still applies; the dialog closes by itself when the design is made.
    NewDesign {
        /// The dialog's "Start From" card (`0` is Empty).
        template_index: i32,
        /// The design the form describes.
        spec: Box<FreshDesignSpec>,
    },
    /// The Tier form's Add Tier or Save Tier.
    Tier(TierEntry),
    /// The concave form's Add or Save: the whole tier, typed in. `edit` names the concave row to
    /// pick first; `None` is "+ Add Concave Tier".
    ConcaveTier {
        /// The concave row to pick first.
        edit: Option<String>,
        /// The tier the form holds when saved.
        tier: Box<ConcaveTier>,
    },
    /// Picks the row called this in the tier table.
    SelectTier(String),
    /// The Steps panel's Generate.
    Steps(StepSeries),
    /// The Steps panel's "Mirror to other block" on the tier called `tier`.
    MirrorTier {
        /// The tier to mirror.
        tier: String,
        /// The name suffix typed beside the button.
        suffix: String,
    },
    /// The Tier form's "Remove relation" on the tier called this.
    ClearRelation(String),
    /// The Tier form's Note field and Set button.
    TierNote {
        /// The tier the note belongs to.
        tier: String,
        /// The note; blank clears it.
        text: String,
    },
    /// The Tier form's Cheater Offset field and Set button.
    CheaterOffset {
        /// The tier the offset belongs to.
        tier: String,
        /// The offset in degrees (a number or a sum); blank clears it.
        text: String,
    },
    /// Design Settings: pick this material and click Apply Material.
    Material(String),
    /// The Preform tab: type this Girdle Diameter and click Apply Yield Inputs.
    Yield {
        /// The girdle diameter in millimetres.
        girdle_diameter_mm: f64,
    },
    /// The Solid viewport's view mode pill (0 Solid, 1 Path-traced, 2 Both, 3 Diagram).
    ViewMode(i32),
    /// The inspector's tab (0 Tier, 1 Preform, 2 Optimize, 3 Schedule, 4 History).
    InspectorTab(i32),
    /// A command of the command palette, by its id (one of [`COMMANDS`]).
    Command(String),
    /// Several, one after the other.
    Sequence(Vec<Self>),
}

impl Perform {
    /// The palette command with this id.
    #[must_use]
    pub fn command(id: &str) -> Self {
        Self::Command(id.to_owned())
    }

    /// Presses Solve.
    #[must_use]
    pub fn solve() -> Self {
        Self::command(CMD_SOLVE)
    }

    /// Presses Undo.
    #[must_use]
    pub fn undo() -> Self {
        Self::command(CMD_UNDO)
    }

    /// Picks the row called `name`.
    #[must_use]
    pub fn select(name: &str) -> Self {
        Self::SelectTier(name.to_owned())
    }

    /// Adds a tier typed with every field.
    #[must_use]
    pub fn add_tier(
        angle: &str,
        meets_kind: i32,
        meets_text: &str,
        name: &str,
        indices: &str,
    ) -> Self {
        Self::Tier(TierEntry::add(angle, meets_kind, meets_text, name, indices))
    }

    /// Picks the row called `name` and moves it `times` places earlier (more than it can go
    /// leaves it first).
    #[must_use]
    pub fn move_up(name: &str, times: usize) -> Self {
        let mut parts = vec![Self::select(name)];
        parts.extend((0..times).map(|_| Self::command(CMD_MOVE_UP)));
        Self::Sequence(parts)
    }

    /// Picks the row called `name` and removes it, as Delete does.
    #[must_use]
    pub fn delete(name: &str) -> Self {
        Self::select(name).then(Self::command(CMD_DELETE))
    }

    /// This, then `next`.
    #[must_use]
    pub fn then(self, next: Self) -> Self {
        let mut parts = match self {
            Self::Sequence(parts) => parts,
            other => vec![other],
        };
        parts.push(next);
        Self::Sequence(parts)
    }

    /// The first thing wrong with the recipe, or `None` when a UI can carry it out.
    ///
    /// Checked for every step that has one when a guide is added to the catalogue, so a typo
    /// in a command id or a view mode fails loudly instead of leaving a Next button that does
    /// nothing.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::NewDesign { spec, .. } => (spec.gear_teeth == 0 || spec.symmetry_order == 0)
                .then(|| "a new design needs a gear and a symmetry order".to_owned()),
            Self::Tier(entry) => entry_problem(entry),
            Self::ConcaveTier { edit, tier } => {
                if tier.name.trim().is_empty() {
                    Some("a concave tier needs a name".to_owned())
                } else if edit.as_deref().is_some_and(|name| name.trim().is_empty()) {
                    Some("a concave edit needs the name of the row".to_owned())
                } else {
                    None
                }
            }
            Self::SelectTier(name)
            | Self::ClearRelation(name)
            | Self::MirrorTier { tier: name, .. }
            | Self::TierNote { tier: name, .. }
            | Self::CheaterOffset { tier: name, .. }
            | Self::Material(name) => name
                .trim()
                .is_empty()
                .then(|| "a perform needs a non-empty name".to_owned()),
            Self::Steps(series) => (series.count < 1 || series.name.trim().is_empty())
                .then(|| "a step series needs a name and at least one rung".to_owned()),
            Self::Yield { girdle_diameter_mm } => (!girdle_diameter_mm.is_finite()
                || *girdle_diameter_mm <= 0.0)
                .then(|| "a girdle diameter must be a positive number".to_owned()),
            Self::ViewMode(mode) => {
                (!(0..=3).contains(mode)).then(|| format!("view mode {mode} does not exist"))
            }
            Self::InspectorTab(tab) => {
                (!(0..=4).contains(tab)).then(|| format!("inspector tab {tab} does not exist"))
            }
            Self::Command(id) => (!COMMANDS.contains(&id.as_str()))
                .then(|| format!("{id:?} is not a command a perform may run")),
            Self::Sequence(parts) => {
                if parts.is_empty() {
                    Some("an empty sequence does nothing".to_owned())
                } else {
                    parts.iter().find_map(Self::problem)
                }
            }
        }
    }
}

/// The first thing wrong with a tier entry.
fn entry_problem(entry: &TierEntry) -> Option<String> {
    match entry.edit.as_deref() {
        None => {
            if entry
                .angle
                .as_deref()
                .is_none_or(|angle| angle.trim().is_empty())
            {
                Some("a new tier needs an angle".to_owned())
            } else {
                None
            }
        }
        Some(name) if name.trim().is_empty() => {
            Some("an edit needs the name of the row".to_owned())
        }
        Some(_) => None,
    }
}

/// The tier called `name` (any of its names; case and surrounding spaces do not matter).
#[must_use]
pub fn tier_position(design: &Design, name: &str) -> Option<usize> {
    design.tiers.iter().position(|tier| has_name(tier, name))
}

/// The concave tier called `name` (case and surrounding spaces do not matter).
#[must_use]
pub fn concave_position(design: &Design, name: &str) -> Option<usize> {
    design
        .concave_tiers
        .iter()
        .position(|tier| tier.name.trim().eq_ignore_ascii_case(name.trim()))
}

fn has_name(tier: &ConstraintTier, name: &str) -> bool {
    tier.names()
        .into_iter()
        .any(|known| known.trim().eq_ignore_ascii_case(name.trim()))
}

/// The Meets combo and text the form shows for the tier at `index`.
fn form_meets(design: &Design, index: usize) -> (i32, String) {
    if let Some(target) = design.tier_target(index) {
        return match target {
            TierTarget::DepthMm(mm) => (3, mm.to_string()),
            TierTarget::GirdleThicknessMm(mm) => (4, mm.to_string()),
            TierTarget::TableWidthMm(mm) => (5, mm.to_string()),
        };
    }
    match design.tiers.get(index).map(|tier| &tier.constraint) {
        Some(MeetConstraint::MeetNamed(names)) => (1, names.join(", ")),
        Some(MeetConstraint::ScaleReference(value)) => (2, value.to_string()),
        Some(MeetConstraint::MeetExisting) | None => (0, String::new()),
    }
}

/// An index list as the Indices field shows it.
fn index_text(indices: &[f64]) -> String {
    indices
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The values the Tier form's Save action receives for `entry` in `design`.
///
/// An edit starts from the picked row, as the form fills itself when a row is clicked (the
/// angle without its sign: the table prints a pavilion angle as a plain number and the row
/// keeps its side), and replaces the fields the entry names.
///
/// # Errors
///
/// A sentence for the learner: the row to edit does not exist, or a new tier has no angle.
pub fn resolve_tier(design: &Design, entry: &TierEntry) -> Result<TierSave, String> {
    let Some(wanted) = entry.edit.as_deref() else {
        let angle = entry
            .angle
            .clone()
            .ok_or_else(|| "A new tier needs an angle.".to_owned())?;
        let (constraint_kind, constraint_text) = entry.meets.clone().unwrap_or((0, String::new()));
        return Ok(TierSave {
            index: -1,
            angle,
            constraint_kind,
            constraint_text,
            name: entry.name.clone().unwrap_or_default(),
            indices: entry.indices.clone().unwrap_or_default(),
        });
    };
    let position = tier_position(design, wanted)
        .ok_or_else(|| format!("There is no tier called '{}' to change.", wanted.trim()))?;
    let tier = &design.tiers[position];
    let (constraint_kind, constraint_text) = entry
        .meets
        .clone()
        .unwrap_or_else(|| form_meets(design, position));
    Ok(TierSave {
        index: i32::try_from(position).map_err(|_| "The design has too many tiers.".to_owned())?,
        angle: entry
            .angle
            .clone()
            .unwrap_or_else(|| tier.angle_deg.abs().to_string()),
        constraint_kind,
        constraint_text,
        name: entry.name.clone().unwrap_or_else(|| tier.name.clone()),
        indices: entry
            .indices
            .clone()
            .unwrap_or_else(|| index_text(&tier.indices)),
    })
}

/// The palette command that does what raises UI event `name`, when one does it and nothing
/// else needs to happen first.
fn event_command(name: &str) -> Option<&'static str> {
    match name {
        SOLVE_REQUESTED => Some(CMD_SOLVE),
        SNAPSHOT_TAKEN => Some("solve.snapshot"),
        LIVE_RENDER_OPENED => Some("view.live_render"),
        SHORTCUTS_OPENED => Some("help.shortcuts"),
        HELP_OPENED => Some("help.manual"),
        SNAP_TOGGLED => Some("view.snap"),
        SLICE_STARTED => Some("view.slice"),
        _ => None,
    }
}

/// The fresh-design recipe for a goal that states the gear, the symmetry and the mirror.
fn fresh_design(gear_teeth: u32, symmetry_order: u32, mirror: bool) -> Option<Perform> {
    Some(Perform::NewDesign {
        template_index: 0,
        spec: Box::new(FreshDesignSpec {
            gear_teeth: i32::try_from(gear_teeth).ok()?,
            symmetry_order,
            mirror,
            material: MaterialSelection::none(),
            preform: EMPTY_START_PREFORM,
        }),
    })
}

/// The parts of the default recipe for `goal`, or `None` when the goal does not say enough.
fn default_parts(goal: &Goal) -> Option<Vec<Perform>> {
    match goal {
        Goal::Manual
        | Goal::TierExists(_)
        | Goal::TierCountAtLeast(_)
        | Goal::ConcaveTiersAtLeast(_)
        | Goal::DesignRebuilt { .. }
        | Goal::YieldApplied
        | Goal::Check { .. }
        | Goal::Any(_) => None,
        Goal::Event(name) => event_command(name).map(|id| vec![Perform::command(id)]),
        Goal::TierMatches {
            name,
            angle_deg,
            indices,
            constraint_kind,
            ..
        } => {
            // Only the Meets kinds that need no further text can be derived; a named facet or
            // an exact scale value needs a recipe of its own.
            let mut entry = TierEntry::of(name).angle(&angle_deg.abs().to_string());
            match constraint_kind {
                None => {}
                Some(MeetKind::Unspecified) => entry = entry.meets(0, ""),
                Some(MeetKind::Named | MeetKind::ExactScale) => return None,
            }
            if let Some(list) = indices {
                entry = entry.indices(&index_text(list));
            }
            Some(vec![Perform::Tier(entry)])
        }
        Goal::Material(name) => Some(vec![Perform::Material(name.clone())]),
        Goal::SolvedClosed => Some(vec![Perform::solve()]),
        Goal::ViewMode(mode) => Some(vec![Perform::ViewMode(*mode)]),
        Goal::InspectorTab(tab) => Some(vec![Perform::InspectorTab(*tab)]),
        Goal::FreshDesign {
            gear_teeth,
            symmetry_order,
            mirror,
        } => fresh_design(*gear_teeth, *symmetry_order, *mirror).map(|design| vec![design]),
        Goal::All(goals) => {
            let mut parts: Vec<Perform> = Vec::new();
            let mut events: Vec<&str> = Vec::new();
            for goal in goals {
                if let Goal::Event(name) = goal {
                    events.push(name);
                } else {
                    parts.extend(default_parts(goal)?);
                }
            }
            // An event the other parts raise by themselves needs nothing of its own; any other
            // event is an action of its own.
            for name in events {
                let raised_by_parts = (name == NEW_DESIGN_CREATED
                    && parts
                        .iter()
                        .any(|part| matches!(part, Perform::NewDesign { .. })))
                    || event_command(name).is_some_and(|id| {
                        parts
                            .iter()
                            .any(|part| matches!(part, Perform::Command(known) if known == id))
                    });
                if !raised_by_parts {
                    parts.extend(default_parts(&Goal::Event(name.to_owned()))?);
                }
            }
            Some(parts)
        }
    }
}

/// The recipe the step's own goal implies, or `None` when the goal does not say enough to
/// derive one (a Check goal, an exact Meets value, a dialog's outcome).
#[must_use]
pub fn default_perform(goal: &Goal) -> Option<Perform> {
    let mut parts = default_parts(goal)?;
    match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => Some(Perform::Sequence(parts)),
    }
}

/// Puts the final recipe on every action step of `guides`: the one `perform_table` lists for it,
/// else the one its goal implies. A step that already carries a recipe (a generated lesson's)
/// keeps it, and a reading step never gets one.
pub(super) fn attach_performs(guides: &mut [Guide]) {
    let table = perform_table::entries();
    for guide in guides.iter_mut() {
        for step in &mut guide.steps {
            if step.perform.is_some() || step.is_manual() {
                continue;
            }
            let listed = table
                .iter()
                .find(|entry| entry.guide == guide.id && entry.step == step.title);
            step.perform = match listed {
                Some(entry) => match &entry.action {
                    Action::Do(perform) => Some(perform.clone()),
                    Action::SkipOnly => None,
                },
                None => default_perform(&step.goal),
            };
        }
    }
}
