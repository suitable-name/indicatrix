//! Starting a guide: what its [`StartingState`] asks of the editor, and how a UI decides
//! whether the design a guide needs has arrived.
//!
//! Both are pure decisions over plain values, so the desktop's runtime (which owns the
//! windows, the unsaved-changes dialog and the clock) stays a thin shell around them.

use super::model::StartingState;
#[cfg(test)]
use crate::EditorSession;
use crate::templates::{template_cards, template_spec};
use indicatrix_cut_core::{FreshDesignSpec, MaterialSelection, PreformSpec};

/// Why a guide that needs a design cannot start while none is open.
pub const NEEDS_A_DESIGN: &str = "Open or create a design first.";

/// The rough a lesson that starts from nothing is cut from.
///
/// A 96-sided cylinder of half-width 1.50, length over width 1.00 and depth 1.50: the blank
/// the worked example asks for in the New Design dialog's Empty form.
pub const EMPTY_START_PREFORM: PreformSpec = PreformSpec::cylinder(96, 1.5, 1.0, 1.5);

/// The new design a lesson starts from, for gallery card `template_index` (`0` is Empty).
///
/// A template card keeps what the card makes: its own rough (a template's pavilion and
/// culet reach deeper than the Empty blank is tall, so on that blank they would be clipped
/// flat), its gear, its symmetry order and its mirror setting, with no material chosen. Card
/// `0`, and any index the template table has no entry for, is the Empty blank
/// ([`EMPTY_START_PREFORM`], 96 teeth, 8-fold, mirrored).
///
/// The desktop fills the New Design form from this spec, and the lesson tests build their
/// session from it (`started_session`), so both start from the same stone.
#[must_use]
pub fn lesson_start_spec(template_index: i32) -> FreshDesignSpec {
    template_spec(template_index).map_or_else(
        || FreshDesignSpec {
            gear_teeth: 96,
            symmetry_order: 8,
            mirror: true,
            material: MaterialSelection::none(),
            preform: EMPTY_START_PREFORM,
        },
        |template| template.fresh_spec(MaterialSelection::none()),
    )
}

/// What starting a guide has to do before its first step.
#[derive(Clone, Debug, PartialEq)]
pub enum StartPlan {
    /// Open the first step now.
    Begin,
    /// The guide cannot start; show this reason where its Start button is.
    Blocked(&'static str),
    /// Create a new design from gallery card `template_index` (`0` is Empty), asking about
    /// unsaved changes first, then open the first step.
    CreateNew {
        /// The New Design dialog's "Start From" index.
        template_index: i32,
        /// The design to create: the card's own rough, gear and symmetry
        /// ([`lesson_start_spec`]).
        spec: FreshDesignSpec,
    },
    /// Open library catalogue entry `entry_id` into the editor, asking about unsaved
    /// changes first, then open the first step.
    OpenLibrary {
        /// The catalogue row id.
        entry_id: i64,
    },
}

/// What starting a guide with `state` does, given whether a design is open.
#[must_use]
pub fn plan_start(state: &StartingState, has_design: bool) -> StartPlan {
    match state {
        StartingState::CurrentDesign => StartPlan::Begin,
        StartingState::RequiresOpenDesign if has_design => StartPlan::Begin,
        StartingState::RequiresOpenDesign => StartPlan::Blocked(NEEDS_A_DESIGN),
        StartingState::NewEmpty => StartPlan::CreateNew {
            template_index: 0,
            spec: lesson_start_spec(0),
        },
        StartingState::Template(card) => match i32::try_from(*card) {
            Ok(template_index) if *card < template_cards().len() => StartPlan::CreateNew {
                template_index,
                spec: lesson_start_spec(template_index),
            },
            _ => StartPlan::Blocked("That starting template is not available."),
        },
        StartingState::LibraryDesign(entry_id) if *entry_id >= 0 => StartPlan::OpenLibrary {
            entry_id: *entry_id,
        },
        StartingState::LibraryDesign(_) => {
            StartPlan::Blocked("That library design is not available.")
        }
    }
}

/// The editor session a lesson with a new-design start begins in: [`plan_start`]'s design,
/// seeded with the card's tiers the way the desktop's New Design does it
/// (`EditorSession::from_template`). `None` for a start that does not create a design.
///
/// The lesson tests build their simulated editor from this, so what they play is the stone
/// the desktop gives the learner.
#[cfg(test)]
pub(super) fn started_session(state: &StartingState) -> Option<EditorSession> {
    match plan_start(state, true) {
        StartPlan::CreateNew {
            template_index,
            spec,
        } => Some(EditorSession::from_template(spec, template_index)),
        _ => None,
    }
}

/// Whether a guide waiting for its starting design should start now, keep waiting, or give
/// up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchStatus {
    /// The design has arrived: open the first step.
    Ready,
    /// Something is still going to decide: keep checking.
    Waiting,
    /// Nothing is going to deliver the design (the user cancelled, or it failed): forget
    /// the launch.
    Abandoned,
}

/// What a UI sees while a guide waits for its starting design.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaunchObservation {
    /// The design the guide asked for is now the open design.
    pub design_arrived: bool,
    /// The unsaved-changes question is on screen.
    pub dialog_open: bool,
    /// The question was answered but its action has not run yet: the editor still holds the
    /// pending New/Load, or is waiting for a Save to land before it resumes one.
    pub decision_pending: bool,
    /// Checks made so far since the launch began (one per timer tick).
    pub ticks: u32,
}

/// How long a launch that opens a library design keeps waiting with nothing visibly
/// pending, in ticks: a remote design downloads in the background.
pub const LIBRARY_GRACE_TICKS: u32 = 48;

/// The status of a launch, given what the UI sees and how many ticks of grace the kind of
/// launch gets when nothing is visibly pending (`0` for a New Design, which is synchronous).
#[must_use]
pub const fn launch_status(observation: &LaunchObservation, grace_ticks: u32) -> LaunchStatus {
    if observation.design_arrived {
        LaunchStatus::Ready
    } else if observation.dialog_open
        || observation.decision_pending
        || observation.ticks < grace_ticks
    {
        LaunchStatus::Waiting
    } else {
        LaunchStatus::Abandoned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::static_guides;
    use indicatrix_cut_core::{
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, ManufacturabilityWarning,
        check_manufacturability,
    };

    #[test]
    fn a_guide_on_the_current_design_just_begins() {
        assert_eq!(
            plan_start(&StartingState::CurrentDesign, false),
            StartPlan::Begin
        );
        assert_eq!(
            plan_start(&StartingState::CurrentDesign, true),
            StartPlan::Begin
        );
    }

    #[test]
    fn a_guide_that_needs_a_design_says_so_when_none_is_open() {
        assert_eq!(
            plan_start(&StartingState::RequiresOpenDesign, true),
            StartPlan::Begin
        );
        assert_eq!(
            plan_start(&StartingState::RequiresOpenDesign, false),
            StartPlan::Blocked(NEEDS_A_DESIGN)
        );
    }

    #[test]
    fn a_new_design_guide_creates_whether_or_not_one_is_open() {
        for has_design in [false, true] {
            assert_eq!(
                plan_start(&StartingState::NewEmpty, has_design),
                StartPlan::CreateNew {
                    template_index: 0,
                    spec: lesson_start_spec(0),
                }
            );
        }
    }

    #[test]
    fn a_template_guide_uses_the_gallery_card_index() {
        assert_eq!(
            plan_start(&StartingState::Template(1), true),
            StartPlan::CreateNew {
                template_index: 1,
                spec: lesson_start_spec(1),
            }
        );
        assert_eq!(
            plan_start(&StartingState::Template(0), true),
            StartPlan::CreateNew {
                template_index: 0,
                spec: lesson_start_spec(0),
            }
        );
        let past_the_end = template_cards().len();
        assert!(matches!(
            plan_start(&StartingState::Template(past_the_end), true),
            StartPlan::Blocked(_)
        ));
        assert!(matches!(
            plan_start(&StartingState::Template(usize::MAX), true),
            StartPlan::Blocked(_)
        ));
    }

    #[test]
    fn a_library_guide_opens_its_catalogue_entry() {
        assert_eq!(
            plan_start(&StartingState::LibraryDesign(42), false),
            StartPlan::OpenLibrary { entry_id: 42 }
        );
        assert!(matches!(
            plan_start(&StartingState::LibraryDesign(-1), false),
            StartPlan::Blocked(_)
        ));
    }

    /// The tiers whose facets never reach the surface of `design`'s solved stone.
    fn vanishing_facets(design: &Design) -> Vec<String> {
        let solved = design
            .solve()
            .unwrap_or_else(|error| panic!("the starting design does not solve: {error}"));
        check_manufacturability(design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2)
            .into_iter()
            .filter_map(|warning| match warning {
                ManufacturabilityWarning::VanishingFacet { tier_name, .. } => Some(tier_name),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_template_lesson_is_cut_from_the_templates_own_rough() {
        for card in 1..template_cards().len() {
            let index = i32::try_from(card).expect("a small card number");
            let template = template_spec(index).expect("the template exists");
            let StartPlan::CreateNew {
                template_index,
                spec,
            } = plan_start(&StartingState::Template(card), false)
            else {
                panic!("card {card} does not create a design");
            };
            assert_eq!(template_index, index);
            assert_eq!(
                spec.preform, template.preform,
                "{}: the rough",
                template.name
            );
            assert_eq!(
                (spec.gear_teeth, spec.symmetry_order, spec.mirror),
                (
                    template.gear_teeth,
                    template.symmetry_order,
                    template.mirror
                ),
                "{}: the schedule",
                template.name
            );
            assert_eq!(
                spec.material,
                MaterialSelection::none(),
                "{}",
                template.name
            );
        }
    }

    #[test]
    fn an_empty_lesson_start_is_the_worked_examples_blank_and_the_startup_stone() {
        let StartPlan::CreateNew {
            template_index: 0,
            spec,
        } = plan_start(&StartingState::NewEmpty, false)
        else {
            panic!("an empty start creates card 0");
        };
        assert_eq!(spec.preform, PreformSpec::cylinder(96, 1.5, 1.0, 1.5));
        let startup = EditorSession::fresh().design;
        assert_eq!(spec.preform, startup.preform);
        assert_eq!(
            (spec.gear_teeth, spec.symmetry_order, spec.mirror),
            (
                startup.meta.gear_teeth,
                startup.meta.symmetry_order,
                startup.meta.mirror
            )
        );
        // An index the template table has no entry for is the empty blank as well.
        assert_eq!(lesson_start_spec(-1), lesson_start_spec(0));
        assert_eq!(lesson_start_spec(i32::MAX), lesson_start_spec(0));
    }

    #[test]
    fn a_start_that_creates_no_design_has_no_session() {
        assert!(started_session(&StartingState::CurrentDesign).is_none());
        assert!(started_session(&StartingState::RequiresOpenDesign).is_none());
        assert!(started_session(&StartingState::LibraryDesign(7)).is_none());
        let session = started_session(&StartingState::Template(1)).expect("a template start");
        assert_eq!(
            session.design.tiers.len(),
            8,
            "the Standard Round Brilliant"
        );
    }

    /// The Standard Round Brilliant's culet plane lies 0.88 below the middle of the stone,
    /// the Empty blank's floor 0.75: on that blank the culet never reaches the surface.
    #[test]
    fn the_empty_blank_would_clip_the_round_brilliants_culet_and_its_own_rough_does_not() {
        let mut clipped_spec = lesson_start_spec(1);
        clipped_spec.preform = EMPTY_START_PREFORM;
        let clipped = EditorSession::from_template(clipped_spec, 1);
        let vanishing = vanishing_facets(&clipped.design);
        assert!(
            vanishing.iter().any(|name| name == "Culet"),
            "on the Empty blank: {vanishing:?}"
        );

        let own = started_session(&StartingState::Template(1)).expect("a template start");
        assert_eq!(vanishing_facets(&own.design), Vec::<String>::new());
    }

    /// Every template a lesson starts from, as the learner sees it in the first step.
    #[test]
    fn no_template_lesson_starts_on_a_stone_with_a_vanishing_facet() {
        let mut cards: Vec<usize> = static_guides()
            .iter()
            .filter_map(|guide| match guide.starting_state {
                StartingState::Template(card) => Some(card),
                _ => None,
            })
            .collect();
        cards.sort_unstable();
        cards.dedup();
        assert!(!cards.is_empty(), "the catalogue has template lessons");
        for card in cards {
            let session =
                started_session(&StartingState::Template(card)).expect("a template start");
            let vanishing = vanishing_facets(&session.design);
            assert!(
                vanishing.is_empty(),
                "template card {card} starts with facets that never appear: {vanishing:?}"
            );
        }
    }

    fn seen(arrived: bool, dialog: bool, pending: bool, ticks: u32) -> LaunchObservation {
        LaunchObservation {
            design_arrived: arrived,
            dialog_open: dialog,
            decision_pending: pending,
            ticks,
        }
    }

    #[test]
    fn a_launch_is_ready_the_moment_its_design_arrives() {
        assert_eq!(
            launch_status(&seen(true, false, false, 0), 0),
            LaunchStatus::Ready
        );
        // Even with the question still marked open: the design is what counts.
        assert_eq!(
            launch_status(&seen(true, true, true, 9), 0),
            LaunchStatus::Ready
        );
    }

    #[test]
    fn a_launch_waits_while_the_save_question_is_open_or_being_carried_out() {
        assert_eq!(
            launch_status(&seen(false, true, false, 3), 0),
            LaunchStatus::Waiting
        );
        assert_eq!(
            launch_status(&seen(false, false, true, 3), 0),
            LaunchStatus::Waiting
        );
    }

    #[test]
    fn a_cancelled_launch_is_abandoned() {
        assert_eq!(
            launch_status(&seen(false, false, false, 0), 0),
            LaunchStatus::Abandoned
        );
    }

    #[test]
    fn a_library_launch_gets_a_grace_period_for_a_slow_download() {
        let grace = LIBRARY_GRACE_TICKS;
        assert_eq!(
            launch_status(&seen(false, false, false, grace - 1), grace),
            LaunchStatus::Waiting
        );
        assert_eq!(
            launch_status(&seen(false, false, false, grace), grace),
            LaunchStatus::Abandoned
        );
    }
}
