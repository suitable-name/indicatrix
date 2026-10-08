//! The data model of a guided tutorial: a [`Guide`] is a list of [`GuideStep`]s, each
//! with a [`Goal`] that the editor judges from STATE (see [`super::goal_met`]).
//!
//! A tutorial is plain data. Writing one takes the builder calls below and, at most, a
//! new [`Goal`] kind; nothing in a UI changes. The worked example
//! ([`super::worked_example_guide`]) is built from the static [`super::STEPS`] the web app
//! shares; every other guide is defined directly with [`Guide::new`] and [`GuideStep::new`],
//! or generated at run time (see [`super::GuideCatalog::add_generated`]).

use super::{
    MANUAL,
    goal::Goal,
    perform::Perform,
    steps::{ALL_GROUPS, Group},
};

/// Which section of the tutorial browser a guide is listed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GuideCategory {
    /// First steps: the tour and the worked example.
    GettingStarted,
    /// Adding, editing and arranging tiers.
    Tiers,
    /// Solving a design and reading the result.
    Solving,
    /// Deep Solve, Optimize, Retarget and the other ways to improve a design.
    Optimizing,
    /// Looking at a design: view modes, the solid view, the render.
    Viewing,
    /// Saving, exporting and printing.
    Output,
    /// The library, importing and the rough planner.
    Library,
    /// Preferences and accessibility.
    Preferences,
    /// Generated lessons that rebuild a library design from scratch.
    BuildThisDesign,
}

impl GuideCategory {
    /// Every category, in the order the browser lists them.
    pub const ALL: [Self; 9] = [
        Self::GettingStarted,
        Self::Tiers,
        Self::Solving,
        Self::Optimizing,
        Self::Viewing,
        Self::Output,
        Self::Library,
        Self::Preferences,
        Self::BuildThisDesign,
    ];

    /// The heading the browser shows.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GettingStarted => "Getting started",
            Self::Tiers => "Tiers",
            Self::Solving => "Solving",
            Self::Optimizing => "Optimizing",
            Self::Viewing => "Viewing",
            Self::Output => "Output",
            Self::Library => "Library",
            Self::Preferences => "Preferences",
            Self::BuildThisDesign => "Built from the library",
        }
    }
}

/// What the editor holds when a guide begins, and what starting it has to arrange first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartingState {
    /// Start with whatever is open, even nothing: the guide does not need a design.
    CurrentDesign,
    /// Start with whatever is open, and refuse to start (with a reason) when nothing is.
    RequiresOpenDesign,
    /// Start from a new, empty design (96 teeth, 8-fold, mirror on), asking to save the
    /// current design first when it has unsaved changes.
    NewEmpty,
    /// Start from the template gallery's card with this index (`1` is the first template;
    /// `0` is the Empty card, the same as [`Self::NewEmpty`]), with the same save guard. The
    /// design is cut from the template's own rough (`lesson_start_spec`).
    Template(usize),
    /// Start from this library catalogue entry (its row id), opened into the editor with
    /// the same save guard.
    LibraryDesign(i64),
}

/// One step of a guide: the text the panel shows, what the step waits for, and which
/// controls stay usable meanwhile.
#[derive(Clone, Debug)]
pub struct GuideStep {
    /// Heading.
    pub title: String,
    /// One sentence of context.
    pub intro: String,
    /// The numbered actions, one short line each.
    pub actions: Vec<String>,
    /// What to look for once done (shown after "Look for: "); `""` hides the line.
    pub check: String,
    /// Optional explanation; `""` hides it.
    pub why: String,
    /// The status chip's "Waiting for: ..." text; `""` on a reading step.
    pub waiting: String,
    /// The control outlined while this step is current, or `""` for none. One of
    /// [`super::HIGHLIGHT_TARGETS`].
    pub highlight_target: String,
    /// What completes the step.
    pub goal: Goal,
    /// The groups of controls that stay usable; every other group is locked.
    pub allow: Vec<Group>,
    /// What the Next button does while the step is unfinished: the step's action, done for the
    /// learner through the editor's own paths. `None` on a reading step and on a step that
    /// cannot be done automatically, which keeps its Skip step button. Filled in when the
    /// guide is added to the catalogue (see [`super::perform`]).
    pub perform: Option<Perform>,
}

impl GuideStep {
    /// A reading step: `title` and one sentence of `intro`, completed by the Next button,
    /// locking nothing. Add the rest with the builder calls.
    #[must_use]
    pub fn new(title: impl Into<String>, intro: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            intro: intro.into(),
            actions: Vec::new(),
            check: String::new(),
            why: String::new(),
            waiting: String::new(),
            highlight_target: String::new(),
            goal: Goal::Manual,
            allow: ALL_GROUPS.to_vec(),
            perform: None,
        }
    }

    /// Sets the numbered action lines.
    #[must_use]
    pub fn actions<I, S>(mut self, actions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.actions = actions.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the "Look for" line.
    #[must_use]
    pub fn check(mut self, check: impl Into<String>) -> Self {
        self.check = check.into();
        self
    }

    /// Sets the explanation shown small under the step.
    #[must_use]
    pub fn why(mut self, why: impl Into<String>) -> Self {
        self.why = why.into();
        self
    }

    /// Sets what the step waits for and the "Waiting for: ..." text that says so. A step
    /// with a goal other than [`Goal::Manual`] needs both.
    #[must_use]
    pub fn goal(mut self, goal: Goal, waiting: impl Into<String>) -> Self {
        self.goal = goal;
        self.waiting = waiting.into();
        self
    }

    /// Outlines the control named `target` while the step is current.
    #[must_use]
    pub fn highlight(mut self, target: impl Into<String>) -> Self {
        self.highlight_target = target.into();
        self
    }

    /// Locks every group of controls except `allow`.
    #[must_use]
    pub fn allow(mut self, allow: &[Group]) -> Self {
        self.allow = allow.to_vec();
        self
    }

    /// Sets what the Next button does on this step while it is unfinished (see
    /// [`Self::perform`]). A step needs this only when neither its goal nor the catalogue's
    /// recipe table implies one.
    #[must_use]
    pub fn perform(mut self, perform: Perform) -> Self {
        self.perform = Some(perform);
        self
    }

    /// Whether the step advances only through its Next button.
    #[must_use]
    pub const fn is_manual(&self) -> bool {
        self.goal.is_manual()
    }

    /// Whether Next can do this step for the learner.
    #[must_use]
    pub const fn can_perform(&self) -> bool {
        self.perform.is_some()
    }

    /// The key a UI reports (through `GuideModel.notify`) when step number `index` of its
    /// guide is done: [`MANUAL`] for a reading step, which never completes that way, and a
    /// key naming the step otherwise. Only the current step's own key counts.
    #[must_use]
    pub fn completion_key(&self, index: usize) -> String {
        if self.is_manual() {
            MANUAL.to_owned()
        } else {
            format!("goal:{index}")
        }
    }
}

/// One tutorial: a titled, categorised list of steps and the state it starts from.
#[derive(Clone, Debug)]
pub struct Guide {
    /// A stable id: lower-case words joined by hyphens. Progress is saved under it, so it
    /// must not change once a guide has shipped.
    pub id: String,
    /// The name the browser and the panel show.
    pub title: String,
    /// One line saying what the tutorial teaches.
    pub summary: String,
    /// The browser section it is listed under.
    pub category: GuideCategory,
    /// What the editor must hold before the first step.
    pub starting_state: StartingState,
    /// The steps, in order.
    pub steps: Vec<GuideStep>,
}

impl Guide {
    /// An empty guide that starts from the current design. Add steps with [`Self::step`].
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        summary: impl Into<String>,
        category: GuideCategory,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            summary: summary.into(),
            category,
            starting_state: StartingState::CurrentDesign,
            steps: Vec::new(),
        }
    }

    /// Sets the state the guide starts from.
    #[must_use]
    pub const fn starting(mut self, state: StartingState) -> Self {
        self.starting_state = state;
        self
    }

    /// Appends a step.
    #[must_use]
    pub fn step(mut self, step: GuideStep) -> Self {
        self.steps.push(step);
        self
    }

    /// The first thing wrong with this guide, or `None` when it is fit to run.
    ///
    /// Checked for every built-in guide by the tests and for every generated guide when
    /// it is added to the catalogue, so a typo in a highlight target or an event name
    /// fails loudly instead of silently never completing.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        guide_problem(self)
    }
}

/// The start of the id of a guide generated for a library design.
///
/// A [`super::build_this_design_guide`] id is `build:` followed by the design's key, the UUID
/// the library knows it by (or a catalogue address). Progress is saved under the whole id, so
/// the tutorial browser shows the lesson as done again whenever it is rebuilt.
pub const BUILD_ID_PREFIX: &str = "build:";

/// The longest key a [`BUILD_ID_PREFIX`] id may carry (a UUID is 36 characters).
const MAX_BUILD_KEY_LEN: usize = 300;

/// Whether `id` is a stable guide id: lower-case ASCII words joined by single hyphens, or
/// [`BUILD_ID_PREFIX`] and then a key of visible ASCII characters (no spaces).
#[must_use]
pub fn is_valid_guide_id(id: &str) -> bool {
    if let Some(key) = id.strip_prefix(BUILD_ID_PREFIX) {
        return !key.is_empty()
            && key.len() <= MAX_BUILD_KEY_LEN
            && key.chars().all(|c| c.is_ascii_graphic());
    }
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn guide_problem(guide: &Guide) -> Option<String> {
    if !is_valid_guide_id(&guide.id) {
        return Some(format!("{:?} is not a valid guide id", guide.id));
    }
    if guide.title.trim().is_empty() || guide.summary.trim().is_empty() {
        return Some(format!("guide {:?} needs a title and a summary", guide.id));
    }
    if guide.steps.is_empty() {
        return Some(format!("guide {:?} has no steps", guide.id));
    }
    for (index, step) in guide.steps.iter().enumerate() {
        if let Some(problem) = step_problem(step) {
            return Some(format!(
                "guide {:?}, step {} ({:?}): {problem}",
                guide.id,
                index + 1,
                step.title
            ));
        }
        if guide.steps[..index].iter().any(|s| s.title == step.title) {
            return Some(format!(
                "guide {:?} has two steps titled {:?}",
                guide.id, step.title
            ));
        }
    }
    None
}

fn step_problem(step: &GuideStep) -> Option<String> {
    if step.title.trim().is_empty() || step.intro.trim().is_empty() {
        return Some("needs a title and an intro".to_owned());
    }
    if step.actions.iter().any(|action| action.trim().is_empty()) {
        return Some("has a blank action line".to_owned());
    }
    if !super::HIGHLIGHT_TARGETS.contains(&step.highlight_target.as_str()) {
        return Some(format!(
            "unknown highlight target {:?}",
            step.highlight_target
        ));
    }
    if let Some(problem) = step.goal.problem() {
        return Some(problem);
    }
    if let Some(perform) = &step.perform {
        if step.is_manual() {
            return Some("a reading step has nothing to perform".to_owned());
        }
        if let Some(problem) = perform.problem() {
            return Some(format!("its perform is not fit to run: {problem}"));
        }
    }
    if !step.is_manual() {
        if step.actions.is_empty() {
            return Some("an automatic step needs at least one action".to_owned());
        }
        if step.waiting.trim().is_empty() {
            return Some("an automatic step needs its \"waiting for\" text".to_owned());
        }
        if step.check.trim().is_empty() {
            return Some("an automatic step needs a \"look for\" line".to_owned());
        }
    }
    None
}
