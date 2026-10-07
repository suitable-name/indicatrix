//! "Build this design": turns a library design into a generated, step-by-step lesson that
//! rebuilds it from an empty design with the tier form.
//!
//! [`build_this_design_guide`] is a pure function of the target design (and its solved
//! depths): the desktop calls it off the UI thread, registers the result as a generated
//! guide and starts it. The lesson holds the target itself (in the goal of its last step),
//! so the learner builds in a NEW design while the original stays untouched.
//!
//! # The lesson
//!
//! 1. **Start a new design**: the target's gear, symmetry and mirror setting, and a preform
//!    shaped like the target's.
//! 2. **One step per tier, in cutting order** (the order the cutting sheet lists them, and
//!    titled with the tier's code as the sheet labels it, `P1 Pavilion Main`):
//!    the exact text to type into the tier form, why the tier exists, and a goal that holds
//!    when a tier with that name, angle (within 0.02 degrees), index set and "Meets" kind is
//!    in the design. The first tier of each block (crown, pavilion, girdle) is the anchor
//!    and states its depth with Exact scale value, because the solver needs one in every
//!    block. The Simple interface leaves that entry out of the Meets list, so a step that
//!    asks for it unlocks the Advanced group (`TIER_STEP_EXACT`); the girdle step also names
//!    the Girdle Facet Preset. A design of more than [`LARGE_DESIGN_TIERS`] tiers groups
//!    consecutive tiers of the same kind into one step with a checklist.
//! 3. **Concave tiers** get a step each (the facet line and the tool line).
//! 4. **Solve and check**, then **Compare with the original**, whose goal is
//!    [`Goal::DesignRebuilt`]: every tier cut as in the original and every solved depth
//!    within [`REBUILD_MAST_REL_TOL`] of it.
//!
//! # Meets that actually reproduce the original
//!
//! A library design imported from `.asc` pins every tier to its recorded depth and keeps
//! what the file's notes said each tier meets only as `imported_meet`, and a reconstruction
//! from those notes is often several percent off. So the generator does not trust them: it
//! types the planned lesson into a scratch design exactly as a learner would (through the
//! real tier-form parser), solves it, and states the depth of every tier that came out more
//! than a small margin (0.4 percent) away. It repeats this at most three times and then
//! states every remaining depth, so the lesson can always be finished.

use super::{
    Goal, Guide, GuideCategory, MeetKind, StartingState,
    build_text::{
        IndicesText, Role, angle_text, indices_text, infer_roles, mast_text, teaching_names,
    },
    is_valid_guide_id,
    model::BUILD_ID_PREFIX,
    rebuilt::{MAST_FLOOR, mast_within},
    steps::{TIER_STEP, TIER_STEP_EXACT},
};
use crate::loading::{TierFormFields, parse_tier_form};
use indicatrix::geometry::meet_solver::{
    Block, MeetConstraint, SolveStrategy, SolvedTier, classify_blocks,
};
use indicatrix_cut_core::{ConstraintTier, Design, design::TierRef};
use std::fmt;

mod step_builders;

use step_builders::{compare_step, final_step, lesson_steps, solve_step, start_step};

/// What a lesson's title starts with; the rest is the design's own title.
pub const BUILD_TITLE_PREFIX: &str = "Build ";

/// The title of a lesson's first step.
pub const START_STEP_TITLE: &str = "Start a new design";

/// The title of the step that solves the rebuilt design.
pub const SOLVE_STEP_TITLE: &str = "Solve and check";

/// The title of the step that compares the rebuild with the original. The desktop installs
/// the original as the held snapshot when this step is entered.
pub const COMPARE_STEP_TITLE: &str = "Compare with the original";

/// A design with more flat tiers than this gets steps that cover several tiers each.
pub const LARGE_DESIGN_TIERS: usize = 40;

/// The most tiers one grouped step covers.
pub const MAX_TIERS_PER_STEP: usize = 6;

/// How far a tier's angle may sit from the original's, in degrees, for a step's goal.
pub const REBUILD_ANGLE_TOL_DEG: f64 = 0.02;

/// How far a solved depth may sit from the original's, as a fraction of it, for the last
/// step's goal.
pub const REBUILD_MAST_REL_TOL: f64 = 0.01;

/// The margin the generator's own check of a planned lesson uses: tighter than
/// [`REBUILD_MAST_REL_TOL`], so typing rounding and the small drift of neighbouring tiers
/// cannot push a faithful rebuild out of the last step's tolerance.
const VERIFY_MAST_REL_TOL: f64 = 0.004;

/// How many times the generator solves a planned lesson to check it.
const MAX_VERIFY_ROUNDS: usize = 3;

/// A design of more tiers than this states every depth directly: solving a large design
/// takes seconds, and the lesson would be checked several times over.
const MAX_MEET_TIERS: usize = 60;

/// The steps around the tier steps: start, solve, compare and the closing reading step.
const BOOKEND_STEPS: usize = 4;

/// The gear sizes the New Design dialog lists besides "Custom".
const DIALOG_GEARS: [u32; 6] = [96, 80, 77, 72, 64, 120];

/// Why a lesson cannot be made for a design.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildGuideError {
    /// The design has no flat tiers to rebuild.
    NoTiers,
    /// Every depth of the design is zero: it was reconstructed from an angle table and has
    /// no real cutting data.
    NoUsableMasts,
    /// The design does not solve (the reason is the editor's own sentence).
    DoesNotSolve(String),
    /// The design's key cannot be part of a lesson id.
    BadKey,
    /// A tier of the design cannot be typed into the tier form (the reason names it).
    CannotTeach(String),
    /// The generated lesson failed its own fitness check (a bug, with the reason).
    NotFit(String),
}

impl fmt::Display for BuildGuideError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTiers => f.write_str("This design has no flat tiers to rebuild."),
            Self::NoUsableMasts => f.write_str(
                "This design only has placeholder depths (every depth is zero), so there is \
                 nothing to rebuild it from.",
            ),
            Self::DoesNotSolve(reason) => write!(
                f,
                "This design does not solve, so a lesson could not check the rebuild: {reason}"
            ),
            Self::BadKey => f.write_str("This design has no usable library key for a lesson."),
            Self::CannotTeach(reason) => write!(
                f,
                "A tier of this design cannot be typed into the tier form: {reason}"
            ),
            Self::NotFit(reason) => write!(f, "The lesson could not be built: {reason}"),
        }
    }
}

impl std::error::Error for BuildGuideError {}

/// The tier form entries behind one tier step, as the form takes them. A UI could prefill
/// the form from one; the generator and the tests parse it with the real form parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierRecipe {
    /// The Name field.
    pub name: String,
    /// The Angle field.
    pub angle: String,
    /// The Indices field; empty when it stays blank.
    pub indices: String,
    /// The Meets combo: `0` Unspecified vertex, `1` Named facet(s), `2` Exact scale value.
    pub constraint_kind: i32,
    /// The Meets text field: the facet names, or the depth.
    pub constraint_text: String,
}

impl TierRecipe {
    /// The form entries as [`TierFormFields`], for `gear_teeth_abs` teeth and with the names
    /// of the tiers already in the design.
    #[must_use]
    pub fn fields(&self, gear_teeth_abs: u32, other_tier_names: Vec<String>) -> TierFormFields<'_> {
        TierFormFields {
            angle: &self.angle,
            constraint_kind: self.constraint_kind,
            constraint_text: &self.constraint_text,
            name: &self.name,
            indices: &self.indices,
            gear_teeth_abs,
            imported_meet: None,
            original_notes: None,
            other_tier_names,
        }
    }
}

/// A generated lesson and the form entries behind its tier steps.
#[derive(Clone, Debug)]
pub struct BuildPlan {
    /// The lesson.
    pub guide: Guide,
    /// One recipe per flat tier, in the order the lesson adds them.
    pub recipes: Vec<TierRecipe>,
    /// For each recipe, the position of the tier in the target design's own tier list.
    pub original_positions: Vec<usize>,
}

/// The original a lesson compares the learner's design with.
#[derive(Clone, Copy, Debug)]
pub struct ReferenceDesign<'a> {
    /// The original design, its tiers named as the lesson names them and every tier pinned
    /// to its solved depth.
    pub design: &'a Design,
    /// The solved depth of every tier of `design`, in its tier order.
    pub masts: &'a [f64],
}

/// The original a lesson holds, if `guide` is a "Build this design" lesson.
#[must_use]
pub fn reference_design(guide: &Guide) -> Option<ReferenceDesign<'_>> {
    if !is_build_guide_id(&guide.id) {
        return None;
    }
    guide.steps.iter().find_map(|step| find_rebuilt(&step.goal))
}

fn find_rebuilt(goal: &Goal) -> Option<ReferenceDesign<'_>> {
    match goal {
        Goal::DesignRebuilt {
            target,
            target_masts,
            ..
        } => Some(ReferenceDesign {
            design: target,
            masts: target_masts,
        }),
        Goal::All(goals) | Goal::Any(goals) => goals.iter().find_map(find_rebuilt),
        _ => None,
    }
}

/// Whether `id` is the id of a "Build this design" lesson.
#[must_use]
pub fn is_build_guide_id(id: &str) -> bool {
    id.starts_with(BUILD_ID_PREFIX)
}

/// The id of the lesson for the design with this library key.
#[must_use]
pub fn build_guide_id(design_key: &str) -> String {
    format!("{BUILD_ID_PREFIX}{design_key}")
}

/// Whether step `index` of `guide` is the step that compares with the original.
#[must_use]
pub fn is_compare_step(guide: &Guide, index: usize) -> bool {
    is_build_guide_id(&guide.id)
        && guide
            .steps
            .get(index)
            .is_some_and(|step| step.title == COMPARE_STEP_TITLE)
}

/// The label the original gets when a UI holds it as the snapshot to compare against.
#[must_use]
pub fn original_label(guide: &Guide) -> String {
    let title = guide
        .title
        .strip_prefix(BUILD_TITLE_PREFIX)
        .unwrap_or(&guide.title);
    format!("Original: {title}")
}

/// The lesson that rebuilds `target`.
///
/// `solved` is the target's solved depths when the caller has them (parallel to
/// `target.tiers`); without them the generator solves the target itself unless every tier
/// is already pinned to an exact depth. `source_title` names the design in the lesson, and
/// `design_key` is the library's key for it (its UUID), which makes the lesson's id
/// `build:<design_key>` and is where its completion is recorded.
///
/// Solving takes seconds for a large design, so call this off the UI thread.
///
/// # Errors
///
/// A [`BuildGuideError`] saying why the design cannot be turned into a lesson: it has no
/// tiers, only placeholder depths, does not solve, or a tier cannot be typed into the form.
pub fn build_this_design_guide(
    target: &Design,
    solved: Option<&[SolvedTier]>,
    source_title: &str,
    design_key: &str,
) -> Result<Guide, BuildGuideError> {
    build_this_design_plan(target, solved, source_title, design_key).map(|plan| plan.guide)
}

/// [`build_this_design_guide`], keeping the form entries behind the tier steps.
///
/// # Errors
///
/// As [`build_this_design_guide`].
pub fn build_this_design_plan(
    target: &Design,
    solved: Option<&[SolvedTier]>,
    source_title: &str,
    design_key: &str,
) -> Result<BuildPlan, BuildGuideError> {
    let id = build_guide_id(design_key);
    if !is_valid_guide_id(&id) {
        return Err(BuildGuideError::BadKey);
    }
    if target.tiers.is_empty() {
        return Err(BuildGuideError::NoTiers);
    }
    let masts = target_masts(target, solved)?;
    let blocks = classify_blocks(&target.meet_tier_inputs());
    let names = teaching_names(&target.tiers, &blocks);
    let roles = infer_roles(&target.tiers, &names, &blocks);
    let order = flat_order(target);
    let facts = TierFacts {
        blocks,
        names,
        roles,
        masts,
    };
    let mut plan = plan_tiers(target, &facts, &order);
    verify_meets(target, &mut plan)?;

    let title = display_title(source_title);
    let mut steps = lesson_steps(target, &plan);
    let step_count = steps.len() + BOOKEND_STEPS;
    steps.insert(0, start_step(target, &title, step_count));
    steps.push(solve_step());
    steps.push(compare_step(target, &facts));
    steps.push(final_step(target, &title));

    let flat = plan.len();
    let mut guide = Guide::new(
        id,
        format!("{BUILD_TITLE_PREFIX}{title}"),
        format!(
            "Rebuild {title} from an empty design, {flat} {} in cutting order, then compare it \
             with the original.",
            if flat == 1 { "tier" } else { "tiers" }
        ),
        GuideCategory::BuildThisDesign,
    )
    .starting(StartingState::CurrentDesign);
    guide.steps = steps;
    if let Some(problem) = guide.problem() {
        return Err(BuildGuideError::NotFit(problem));
    }
    Ok(BuildPlan {
        guide,
        recipes: plan.iter().map(|tier| tier.recipe.clone()).collect(),
        original_positions: order,
    })
}

/// The name the lesson calls the design by.
fn display_title(source_title: &str) -> String {
    let title = source_title.trim();
    if title.is_empty() {
        "this design".to_owned()
    } else {
        title.to_owned()
    }
}

/// The solved depth of every tier of `target`, parallel to its tiers.
fn target_masts(
    target: &Design,
    solved: Option<&[SolvedTier]>,
) -> Result<Vec<f64>, BuildGuideError> {
    let pinned = target.tier_targets.is_empty()
        && target
            .tiers
            .iter()
            .all(|tier| matches!(tier.constraint, MeetConstraint::ScaleReference(_)));
    let masts = match solved.filter(|solved| solved.len() == target.tiers.len()) {
        Some(solved) => solved_depths(solved)?,
        None if pinned => target
            .tiers
            .iter()
            .map(|tier| match tier.constraint {
                MeetConstraint::ScaleReference(value) => value,
                _ => 0.0,
            })
            .collect(),
        None => {
            let solved = target
                .solve()
                .map_err(|error| BuildGuideError::DoesNotSolve(error.to_string()))?;
            solved_depths(&solved)?
        }
    };
    if masts.iter().any(|mast| !mast.is_finite()) {
        return Err(BuildGuideError::DoesNotSolve(
            "a depth is not a number.".to_owned(),
        ));
    }
    if masts.iter().all(|mast| mast.abs() < 1e-9) {
        return Err(BuildGuideError::NoUsableMasts);
    }
    Ok(masts)
}

fn solved_depths(solved: &[SolvedTier]) -> Result<Vec<f64>, BuildGuideError> {
    if solved
        .iter()
        .any(|tier| matches!(tier.strategy, SolveStrategy::Failed))
    {
        Err(BuildGuideError::DoesNotSolve(
            "the solver could not place every tier.".to_owned(),
        ))
    } else {
        Ok(solved.iter().map(|tier| tier.mast).collect())
    }
}

/// The positions of the flat tiers in the order the lesson adds them: the target's cutting
/// order.
fn flat_order(target: &Design) -> Vec<usize> {
    target
        .cutting_order()
        .into_iter()
        .filter_map(|tier| match tier {
            TierRef::Flat(index) => Some(index),
            TierRef::Concave(_) => None,
        })
        .collect()
}

/// What the generator knows about each tier of the target, by its position in the target.
struct TierFacts {
    blocks: Vec<Block>,
    names: Vec<String>,
    roles: Vec<Role>,
    masts: Vec<f64>,
}

/// Why a tier states its depth with Exact scale value.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ExactReason {
    /// The first tier of its block: the solver needs one anchor per block.
    Anchor,
    /// The original does not say what the tier meets, or names something the lesson cannot
    /// match.
    NoMeetInfo,
    /// What the original meets is cut later in the lesson.
    MeetsLater,
    /// Meeting it put the tier this many percent away from the original.
    Missed(f64),
    /// The meets did not reproduce the design reliably.
    Everything,
    /// The design is too large to check meets for.
    TooLarge,
}

/// What a tier's "Meets" setting is in the lesson.
#[derive(Clone, Debug, PartialEq)]
enum Meets {
    Exact(ExactReason),
    Named(Vec<String>),
    Unspecified,
}

impl Meets {
    const fn kind(&self) -> MeetKind {
        match self {
            Self::Exact(_) => MeetKind::ExactScale,
            Self::Named(_) => MeetKind::Named,
            Self::Unspecified => MeetKind::Unspecified,
        }
    }

    const fn is_exact(&self) -> bool {
        matches!(self, Self::Exact(_))
    }
}

/// One flat tier of the lesson.
struct PlannedTier {
    /// The tier's code in the target's cutting order (`P1`, `G1`, `C1`, `T`, `Culet`): the
    /// label the finished sheet and the tier table's code column show, and what a step's
    /// title calls the tier.
    code: String,
    block: Block,
    role: Role,
    angle_deg: f64,
    indices: Vec<f64>,
    indices_text: IndicesText,
    mast: f64,
    meets: Meets,
    recipe: TierRecipe,
}

impl PlannedTier {
    /// Sets the tier to state its depth directly.
    fn pin(&mut self, reason: ExactReason) {
        self.meets = Meets::Exact(reason);
        self.recipe.constraint_kind = 2;
        self.recipe.constraint_text = mast_text(self.mast);
    }
}

/// The tier form entries for a tier named `name` at `angle_deg` with these index positions,
/// meets and (for an exact scale value) depth.
fn recipe_for(
    name: &str,
    angle_deg: f64,
    indices: &IndicesText,
    meets: &Meets,
    mast: f64,
) -> TierRecipe {
    let (constraint_kind, constraint_text) = match meets {
        Meets::Unspecified => (0, String::new()),
        Meets::Named(names) => (1, names.join(", ")),
        Meets::Exact(_) => (2, mast_text(mast)),
    };
    TierRecipe {
        name: name.to_owned(),
        angle: angle_text(angle_deg),
        indices: indices.typed.clone(),
        constraint_kind,
        constraint_text,
    }
}

/// Plans every flat tier, in lesson order.
fn plan_tiers(target: &Design, facts: &TierFacts, order: &[usize]) -> Vec<PlannedTier> {
    let gear = target.meta.gear_teeth_abs();
    let mut position = vec![0_usize; target.tiers.len()];
    for (at, &original) in order.iter().enumerate() {
        position[original] = at;
    }
    let large = target.tiers.len() > MAX_MEET_TIERS;
    let codes = target.tier_codes().flat;
    let mut anchored: Vec<Block> = Vec::new();
    let mut plan = Vec::with_capacity(order.len());
    for &original in order {
        let tier = &target.tiers[original];
        let block = facts.blocks[original];
        let anchor = !anchored.contains(&block);
        if anchor {
            anchored.push(block);
        }
        let meets = if anchor {
            Meets::Exact(ExactReason::Anchor)
        } else if large {
            Meets::Exact(ExactReason::TooLarge)
        } else {
            intended_meets(target, original, facts, &position)
        };
        let typed = indices_text(&tier.indices, gear, tier.angle_deg);
        let mast = facts.masts[original];
        let recipe = recipe_for(&facts.names[original], tier.angle_deg, &typed, &meets, mast);
        plan.push(PlannedTier {
            code: codes
                .get(original)
                .map(|label| label.code.clone())
                .unwrap_or_default(),
            block,
            role: facts.roles[original],
            angle_deg: tier.angle_deg,
            indices: tier.indices.clone(),
            indices_text: typed,
            mast,
            meets,
            recipe,
        });
    }
    plan
}

/// The meets the lesson would teach for tier `original`, from what the original states.
fn intended_meets(
    target: &Design,
    original: usize,
    facts: &TierFacts,
    position: &[usize],
) -> Meets {
    let tier = &target.tiers[original];
    let stated = match &tier.constraint {
        MeetConstraint::ScaleReference(_) => match &tier.imported_meet {
            Some(imported @ (MeetConstraint::MeetExisting | MeetConstraint::MeetNamed(_))) => {
                imported
            }
            _ => return Meets::Exact(ExactReason::NoMeetInfo),
        },
        constraint => constraint,
    };
    match stated {
        MeetConstraint::MeetExisting => Meets::Unspecified,
        MeetConstraint::MeetNamed(list) => {
            resolve_named(list, original, &target.tiers, &facts.names, position)
                .map_or_else(Meets::Exact, Meets::Named)
        }
        MeetConstraint::ScaleReference(_) => Meets::Exact(ExactReason::NoMeetInfo),
    }
}

/// The lesson names of the tiers `list` names, when every one exists, is another tier and
/// is added before tier `this`.
fn resolve_named(
    list: &[String],
    this: usize,
    tiers: &[ConstraintTier],
    names: &[String],
    position: &[usize],
) -> Result<Vec<String>, ExactReason> {
    let mut resolved: Vec<String> = Vec::new();
    for wanted in list {
        let wanted = wanted.trim();
        let found = tiers
            .iter()
            .position(|tier| tier.names().contains(&wanted))
            .or_else(|| {
                tiers
                    .iter()
                    .position(|tier| tier.names().iter().any(|n| n.eq_ignore_ascii_case(wanted)))
            })
            .filter(|&found| found != this)
            .ok_or(ExactReason::NoMeetInfo)?;
        if position[found] > position[this] {
            return Err(ExactReason::MeetsLater);
        }
        if !resolved.contains(&names[found]) {
            resolved.push(names[found].clone());
        }
    }
    if resolved.is_empty() {
        Err(ExactReason::NoMeetInfo)
    } else {
        Ok(resolved)
    }
}

/// Types the planned tiers into a scratch design exactly as the learner will: through the
/// tier form's own parser.
fn rebuild(target: &Design, plan: &[PlannedTier]) -> Result<Design, BuildGuideError> {
    let gear = target.meta.gear_teeth_abs();
    let mut tiers = Vec::with_capacity(plan.len());
    let mut names: Vec<String> = Vec::with_capacity(plan.len());
    for planned in plan {
        let tier =
            parse_tier_form(planned.recipe.fields(gear, names.clone())).map_err(|error| {
                BuildGuideError::CannotTeach(format!("{}: {error}", planned.recipe.name))
            })?;
        names.push(planned.recipe.name.clone());
        tiers.push(tier);
    }
    Ok(Design::new(target.preform, target.meta.clone(), tiers))
}

/// The tiers of `plan` (by position) whose solved depth is off the original's, with how many
/// percent.
fn deviations(solved: &[SolvedTier], plan: &[PlannedTier]) -> Vec<(usize, f64)> {
    solved
        .iter()
        .zip(plan)
        .enumerate()
        .filter(|(_, (solved, planned))| {
            matches!(solved.strategy, SolveStrategy::Failed)
                || !mast_within(solved.mast, planned.mast, VERIFY_MAST_REL_TOL)
        })
        .map(|(at, (solved, planned))| {
            let relative = (solved.mast - planned.mast).abs() / planned.mast.abs().max(MAST_FLOOR);
            (at, relative * 100.0)
        })
        .collect()
}

/// Solves the planned lesson, states the depth of every tier that does not come out as the
/// original, and repeats; after the last round every depth that still rests on a meet is
/// stated too, so the lesson always reproduces the original.
fn verify_meets(target: &Design, plan: &mut [PlannedTier]) -> Result<(), BuildGuideError> {
    for round in 0..MAX_VERIFY_ROUNDS {
        if plan.iter().all(|planned| planned.meets.is_exact()) {
            return Ok(());
        }
        let Ok(solved) = rebuild(target, plan)?.solve() else {
            break;
        };
        if solved.len() != plan.len() {
            break;
        }
        let off = deviations(&solved, plan);
        if off.is_empty() {
            return Ok(());
        }
        if round + 1 == MAX_VERIFY_ROUNDS {
            break;
        }
        for (at, percent) in off {
            if !plan[at].meets.is_exact() {
                plan[at].pin(ExactReason::Missed(percent));
            }
        }
    }
    for planned in plan.iter_mut().filter(|planned| !planned.meets.is_exact()) {
        planned.pin(ExactReason::Everything);
    }
    Ok(())
}

/// The original with the lesson's tier names and every tier pinned to its depth: what the
/// last step compares with and what the compare window shows.
fn reference_copy(target: &Design, facts: &TierFacts) -> Design {
    let mut copy = target.clone();
    for ((tier, name), &mast) in copy.tiers.iter_mut().zip(&facts.names).zip(&facts.masts) {
        tier.name.clone_from(name);
        tier.constraint = MeetConstraint::ScaleReference(mast);
        tier.imported_meet = None;
        tier.original_notes = None;
    }
    copy.tier_targets.clear();
    copy.tier_relations.clear();
    copy
}
