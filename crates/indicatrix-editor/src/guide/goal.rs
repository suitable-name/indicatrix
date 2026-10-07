//! What completes a guide step, judged from STATE.
//!
//! A [`Goal`] is plain data and [`goal_met`] is a pure function of it and a
//! [`GoalContext`] (the design as it is now, the editor's solve verdict, the UI events
//! since the step began, the view mode and inspector tab, the material). It never looks
//! at which button was clicked, so every route to a goal counts (quick add, inline edit,
//! undo/redo, auto-solve), and a failed validation, which never changes the design, can
//! never advance a step.

use super::{ANGLE_TOLERANCE_DEG, rebuilt, same_index_set};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design};

/// How a tier's "Meets" setting is stated in the tier form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeetKind {
    /// "Unspecified vertex": the tier closes against whatever vertex the solver finds.
    Unspecified,
    /// "Named facet(s)": the tier closes against the facets it names.
    Named,
    /// "Exact scale value": the tier's depth is stated outright.
    ExactScale,
}

impl MeetKind {
    /// The kind of a tier's [`MeetConstraint`].
    #[must_use]
    pub const fn of(constraint: &MeetConstraint) -> Self {
        match constraint {
            MeetConstraint::MeetExisting => Self::Unspecified,
            MeetConstraint::MeetNamed(_) => Self::Named,
            MeetConstraint::ScaleReference(_) => Self::ExactScale,
        }
    }

    /// The words the tier form uses for the kind.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unspecified => "Unspecified vertex",
            Self::Named => "Named facet(s)",
            Self::ExactScale => "Exact scale value",
        }
    }
}

/// What a guide step waits for.
#[derive(Clone, Debug)]
pub enum Goal {
    /// Nothing: a reading step that advances only through its Next button.
    Manual,
    /// A UI event with this name was reported since the step began (see
    /// [`super::EVENTS`] and the desktop's `GuideModel.event`).
    Event(String),
    /// A tier called `name` exists at `angle_deg` (within `tol_deg`), and, when stated,
    /// with exactly these index positions (an empty list means "leave Indices blank")
    /// and this kind of "Meets" setting.
    TierMatches {
        /// The tier's name; case and surrounding spaces do not matter.
        name: String,
        /// The angle the step asks for.
        angle_deg: f64,
        /// How far the tier's angle may sit from `angle_deg`.
        tol_deg: f64,
        /// The index positions the tier must have, compared as gear positions;
        /// `None` accepts any.
        indices: Option<Vec<f64>>,
        /// The "Meets" kind the tier must have; `None` accepts any.
        constraint_kind: Option<MeetKind>,
    },
    /// Some tier is called `name` (case and surrounding spaces do not matter).
    TierExists(String),
    /// The design has at least this many tiers.
    TierCountAtLeast(usize),
    /// The design's material is this one (case does not matter).
    Material(String),
    /// The design solved to a closed solid (and has at least one tier).
    SolvedClosed,
    /// The yield inputs were applied: the design has a girdle diameter.
    YieldApplied,
    /// The Solid viewport is in this view mode (0 Solid, 1 Path-traced, 2 Both, 3 Diagram).
    ViewMode(i32),
    /// The inspector shows this tab (0 Tier, 1 Preform, 2 Optimize, 3 Schedule, 4 History).
    InspectorTab(i32),
    /// The design is a fresh start for a rebuild: no tiers yet (flat or concave) and this
    /// gear, symmetry and mirror setting.
    FreshDesign {
        /// The index gear's tooth count (its magnitude; the sign is ignored).
        gear_teeth: u32,
        /// The symmetry order.
        symmetry_order: u32,
        /// Whether the schedule carries mirror symmetry.
        mirror: bool,
    },
    /// The design has at least this many concave tiers.
    ConcaveTiersAtLeast(usize),
    /// The design is `target` rebuilt: the same gear and symmetry, every target tier cut
    /// with its name, its angle (within `angle_tol_deg`) and its index positions, whatever
    /// "Meets" settings the learner chose. When `target_masts` is not empty the design must
    /// also have solved to a closed solid whose masts agree with `target_masts` (parallel to
    /// `target.tiers`) within the fraction `mast_rel_tol`. The masts come from the UI's own
    /// solve ([`GoalContext::solved_masts`]); this goal never solves, so a design that has
    /// not been solved yet is not a match yet.
    DesignRebuilt {
        /// The design to rebuild.
        target: Box<Design>,
        /// How far a tier's angle may sit from the target's, in degrees.
        angle_tol_deg: f64,
        /// How far a solved mast may sit from the target's, as a fraction of it.
        mast_rel_tol: f64,
        /// The target's solved masts, or empty to compare only the cut.
        target_masts: Vec<f64>,
    },
    /// A predicate over the whole [`GoalContext`], for a state no other goal can state (a
    /// tier's note, a relation between two tiers, how many tiers are selected). `label`
    /// says in a few words what the predicate checks, for the guide's own tests; `test` is
    /// a plain function (no captured state), so the goal stays cheap to clone and compare.
    Check {
        /// What the predicate checks, in a few words (never empty).
        label: &'static str,
        /// Whether the state holds.
        test: fn(&GoalContext<'_>) -> bool,
    },
    /// Every goal in the list is met. An empty list is met vacuously.
    All(Vec<Self>),
    /// At least one goal in the list is met. An empty list is never met.
    Any(Vec<Self>),
}

impl Goal {
    /// A [`Goal::TierMatches`] for tier `name` at `angle_deg` that accepts any indices and
    /// any "Meets" setting; narrow it with [`Self::with_indices`] and [`Self::with_meet`].
    #[must_use]
    pub fn tier(name: impl Into<String>, angle_deg: f64) -> Self {
        Self::TierMatches {
            name: name.into(),
            angle_deg,
            tol_deg: ANGLE_TOLERANCE_DEG,
            indices: None,
            constraint_kind: None,
        }
    }

    /// Requires the index positions `wanted` on a [`Goal::TierMatches`]; any other goal is
    /// returned unchanged.
    #[must_use]
    pub fn with_indices(self, wanted: &[f64]) -> Self {
        match self {
            Self::TierMatches {
                name,
                angle_deg,
                tol_deg,
                constraint_kind,
                ..
            } => Self::TierMatches {
                name,
                angle_deg,
                tol_deg,
                indices: Some(wanted.to_vec()),
                constraint_kind,
            },
            other => other,
        }
    }

    /// Requires the "Meets" kind `kind` on a [`Goal::TierMatches`]; any other goal is
    /// returned unchanged.
    #[must_use]
    pub fn with_meet(self, kind: MeetKind) -> Self {
        match self {
            Self::TierMatches {
                name,
                angle_deg,
                tol_deg,
                indices,
                ..
            } => Self::TierMatches {
                name,
                angle_deg,
                tol_deg,
                indices,
                constraint_kind: Some(kind),
            },
            other => other,
        }
    }

    /// Whether this is the reading-step goal.
    #[must_use]
    pub const fn is_manual(&self) -> bool {
        matches!(self, Self::Manual)
    }

    /// Whether the goal reads the view mode or the inspector tab, which change without
    /// the design changing, or the solved depths ([`Self::wants_solved_masts`]), which a
    /// UI's solve delivers a moment after the design check that follows it: a UI then
    /// re-checks the step on a timer while it is current.
    #[must_use]
    pub fn watches_ui(&self) -> bool {
        match self {
            Self::ViewMode(_) | Self::InspectorTab(_) => true,
            Self::DesignRebuilt { target_masts, .. } => !target_masts.is_empty(),
            Self::All(goals) | Self::Any(goals) => goals.iter().any(Self::watches_ui),
            _ => false,
        }
    }

    /// Whether judging the goal needs the design's solved masts
    /// ([`GoalContext::solved_masts`]), which a UI then supplies from its own cached solve
    /// instead of leaving the goal to wait.
    #[must_use]
    pub fn wants_solved_masts(&self) -> bool {
        match self {
            Self::DesignRebuilt { target_masts, .. } => !target_masts.is_empty(),
            Self::All(goals) | Self::Any(goals) => goals.iter().any(Self::wants_solved_masts),
            _ => false,
        }
    }

    /// Every event name the goal waits for.
    #[must_use]
    pub fn events(&self) -> Vec<&str> {
        match self {
            Self::Event(name) => vec![name.as_str()],
            Self::All(goals) | Self::Any(goals) => goals.iter().flat_map(Self::events).collect(),
            _ => Vec::new(),
        }
    }

    /// The first thing wrong with the goal, or `None` when it can be met.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        self.problem_at(true)
    }

    fn problem_at(&self, top_level: bool) -> Option<String> {
        match self {
            Self::Manual if !top_level => {
                Some("a reading step cannot sit inside All or Any".into())
            }
            Self::Manual | Self::SolvedClosed | Self::YieldApplied => None,
            Self::Event(name) => {
                (!super::EVENTS.contains(&name.as_str())).then(|| format!("unknown event {name:?}"))
            }
            Self::TierMatches {
                name,
                angle_deg,
                tol_deg,
                ..
            } => {
                if name.trim().is_empty() {
                    Some("a tier goal needs a tier name".into())
                } else if !angle_deg.is_finite() || !tol_deg.is_finite() || *tol_deg < 0.0 {
                    Some(format!(
                        "tier {name:?} has an angle or tolerance that is not a number"
                    ))
                } else {
                    None
                }
            }
            Self::TierExists(name) | Self::Material(name) => name
                .trim()
                .is_empty()
                .then(|| "a goal needs a non-empty name".into()),
            Self::TierCountAtLeast(count) => {
                (*count == 0).then(|| "a tier count of zero is always met".into())
            }
            Self::ViewMode(mode) => {
                (!(0..=3).contains(mode)).then(|| format!("view mode {mode} does not exist"))
            }
            Self::InspectorTab(tab) => {
                (!(0..=4).contains(tab)).then(|| format!("inspector tab {tab} does not exist"))
            }
            Self::FreshDesign {
                gear_teeth,
                symmetry_order,
                ..
            } => (*gear_teeth == 0 || *symmetry_order == 0)
                .then(|| "a fresh design needs a gear and a symmetry order".into()),
            Self::ConcaveTiersAtLeast(count) => {
                (*count == 0).then(|| "a concave tier count of zero is always met".into())
            }
            Self::DesignRebuilt {
                target,
                angle_tol_deg,
                mast_rel_tol,
                target_masts,
            } => rebuilt::problem(target, *angle_tol_deg, *mast_rel_tol, target_masts),
            Self::Check { label, .. } => label
                .trim()
                .is_empty()
                .then(|| "a check goal needs a label".into()),
            Self::All(goals) | Self::Any(goals) => {
                if goals.is_empty() {
                    return Some("All and Any need at least one goal".into());
                }
                goals.iter().find_map(|goal| goal.problem_at(false))
            }
        }
    }
}

/// What a UI knows about the editor when it asks whether a goal is met.
#[derive(Clone, Copy, Debug)]
pub struct GoalContext<'a> {
    /// The design as it is now.
    pub design: &'a Design,
    /// The editor's own solve verdict is "solved" and not a problem.
    pub solved_closed: bool,
    /// The UI events reported since the current step began.
    pub events: &'a [String],
    /// The Solid viewport's view mode, if the UI can say.
    pub view_mode: Option<i32>,
    /// The inspector's tab, if the UI can say.
    pub inspector_tab: Option<i32>,
    /// The material the user sees applied, when that is not simply the design's own.
    pub material: Option<&'a str>,
    /// The solved masts of `design`, parallel to its tiers, if the UI has them; otherwise
    /// a goal that needs them is not met yet (no goal solves the design on the caller's
    /// thread).
    pub solved_masts: Option<&'a [f64]>,
}

impl<'a> GoalContext<'a> {
    /// A context holding only the design: nothing solved, no events, no UI state.
    #[must_use]
    pub const fn new(design: &'a Design) -> Self {
        Self {
            design,
            solved_closed: false,
            events: &[],
            view_mode: None,
            inspector_tab: None,
            material: None,
            solved_masts: None,
        }
    }

    /// Sets the solve verdict.
    #[must_use]
    pub const fn solved_closed(mut self, solved_closed: bool) -> Self {
        self.solved_closed = solved_closed;
        self
    }

    /// Sets the events since the step began.
    #[must_use]
    pub const fn events(mut self, events: &'a [String]) -> Self {
        self.events = events;
        self
    }

    /// Sets the view mode.
    #[must_use]
    pub const fn view_mode(mut self, mode: i32) -> Self {
        self.view_mode = Some(mode);
        self
    }

    /// Sets the inspector tab.
    #[must_use]
    pub const fn inspector_tab(mut self, tab: i32) -> Self {
        self.inspector_tab = Some(tab);
        self
    }

    /// Sets the material the user sees applied.
    #[must_use]
    pub const fn material(mut self, name: &'a str) -> Self {
        self.material = Some(name);
        self
    }

    /// Sets the solved masts of the design.
    #[must_use]
    pub const fn solved_masts(mut self, masts: &'a [f64]) -> Self {
        self.solved_masts = Some(masts);
        self
    }
}

/// Whether `goal` is met in `ctx`. A [`Goal::Manual`] is never met from state.
#[must_use]
pub fn goal_met(goal: &Goal, ctx: &GoalContext<'_>) -> bool {
    match goal {
        Goal::Manual => false,
        Goal::Event(name) => ctx.events.iter().any(|event| event == name),
        Goal::TierMatches {
            name,
            angle_deg,
            tol_deg,
            indices,
            constraint_kind,
        } => {
            let gear = f64::from(ctx.design.meta.gear_teeth_abs());
            ctx.design.tiers.iter().any(|tier| {
                has_name(tier, name)
                    && (tier.angle_deg - angle_deg).abs() <= *tol_deg
                    && indices
                        .as_deref()
                        .is_none_or(|wanted| same_index_set(&tier.indices, wanted, gear))
                    && constraint_kind.is_none_or(|kind| MeetKind::of(&tier.constraint) == kind)
            })
        }
        Goal::TierExists(name) => ctx.design.tiers.iter().any(|tier| has_name(tier, name)),
        Goal::TierCountAtLeast(count) => ctx.design.tiers.len() >= *count,
        Goal::Material(name) => ctx
            .material
            .or(ctx.design.material.name.as_deref())
            .is_some_and(|current| current.trim().eq_ignore_ascii_case(name.trim())),
        // A zero-tier design "solves" to its bare preform -- not the stone a step asks for.
        Goal::SolvedClosed => ctx.solved_closed && !ctx.design.tiers.is_empty(),
        Goal::YieldApplied => ctx.design.girdle_diameter_mm.is_some(),
        Goal::ViewMode(mode) => ctx.view_mode == Some(*mode),
        Goal::InspectorTab(tab) => ctx.inspector_tab == Some(*tab),
        Goal::FreshDesign {
            gear_teeth,
            symmetry_order,
            mirror,
        } => {
            let design = ctx.design;
            design.tiers.is_empty()
                && design.concave_tiers.is_empty()
                && design.meta.gear_teeth_abs() == *gear_teeth
                && design.meta.symmetry_order == *symmetry_order
                && design.meta.mirror == *mirror
        }
        Goal::ConcaveTiersAtLeast(count) => ctx.design.concave_tiers.len() >= *count,
        Goal::DesignRebuilt {
            target,
            angle_tol_deg,
            mast_rel_tol,
            target_masts,
        } => rebuilt::met(ctx, target, *angle_tol_deg, *mast_rel_tol, target_masts),
        Goal::Check { test, .. } => test(ctx),
        Goal::All(goals) => goals.iter().all(|goal| goal_met(goal, ctx)),
        Goal::Any(goals) => goals.iter().any(|goal| goal_met(goal, ctx)),
    }
}

/// Whether `tier` is known by `name` (case and surrounding spaces do not matter).
fn has_name(tier: &ConstraintTier, name: &str) -> bool {
    tier.names()
        .into_iter()
        .any(|known| known.trim().eq_ignore_ascii_case(name.trim()))
}
