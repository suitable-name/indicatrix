//! The viewing, output, library and app tutorials: one guided lesson for each function that
//! looks at a design, gets it out of the program, finds one in the library or sets the program
//! up.
//!
//! - `solid`: picking facets in the Solid view, the drag handles, Slice, the Diagram view and the
//!   Cut slider.
//! - `render`: Live Render and its lighting presets, the lighting saved with one design, and
//!   custom materials (simple and with coefficients).
//! - `output`: cutting mode, the four exports (`.asc`, `.gcs`, the HTML cutting sheet, the
//!   diagram PNG), Save and Open.
//! - `library`: importing designs, searching and filtering the library, loading a design from
//!   it, and the Rough Planner.
//! - `app`: the Simple | Advanced switch, high contrast, the interface scale, larger handles,
//!   the command palette, the keyboard, the manual and the glossary.
//!
//! Most of what these lessons wait for happens on screen and leaves no trace in the design: a
//! facet was clicked, the Cut slider moved, a file was exported. The desktop reports those as
//! events (see [`super::viewing_events`]); a lesson waits for one with `Goal::Event`. Where the
//! design does change (a tier moved, a tier added) the goal reads the design, so every route to
//! the result counts. The tests in `viewing_tests` play every lesson in a real `EditorSession`:
//! each step's goal is false before the learner's action and true after it.
//!
//! A lesson about a dialog (Preferences, the Material Editor, the Rough Planner) cannot show its
//! own panel while the dialog is open, so the step that sends the learner there carries every
//! instruction it needs and waits for the one event the dialog raises. The next step is read
//! once the dialog is closed.

mod app;
mod library;
mod output;
mod render;
mod solid;

use crate::guide::{EIGHT_FOLD_INDICES, Goal, GoalContext, Group, Guide, same_index_set};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design};

/// The eight positions of a plain ring on a 96-tooth, 8-fold gear: one every 12 teeth.
const MAIN: [f64; 8] = EIGHT_FOLD_INDICES;

/// The sixteen positions of the Rich Teaching Design's girdle: one every 6 teeth.
const GIRDLE: [f64; 16] = [
    0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0, 90.0,
];

/// A tier of the Rich Teaching Design (card 5 of the New Design dialog) as the lessons start it:
/// its name, angle in degrees, mast and index positions. The viewport lessons start from it, so
/// "has the learner moved something" is a comparison with these numbers.
struct Original {
    name: &'static str,
    angle_deg: f64,
    mast: f64,
    indices: &'static [f64],
}

/// The four tiers of the Rich Teaching Design.
const RICH: [Original; 4] = [
    Original {
        name: "Table",
        angle_deg: 0.0,
        mast: 0.30,
        indices: &[],
    },
    Original {
        name: "Crown Main",
        angle_deg: 34.5,
        mast: 0.60,
        indices: &MAIN,
    },
    Original {
        name: "Girdle",
        angle_deg: 90.0,
        mast: 1.0,
        indices: &GIRDLE,
    },
    Original {
        name: "Pavilion Main",
        angle_deg: -40.0,
        mast: 0.70,
        indices: &MAIN,
    },
];

/// How far an angle may sit from its starting value and still count as untouched. The drag
/// handles snap to a tenth of a degree, so a real drag is always further than this.
const ANGLE_SLACK_DEG: f64 = 0.05;

/// How far a mast may sit from its starting value and still count as untouched (the handles
/// snap to a hundredth).
const MAST_SLACK: f64 = 0.005;

/// Undo only: the Solid viewport and its pills are never locked, so a lesson about them keeps
/// every other panel still and leaves Undo for a slip.
const VIEWPORT: &[Group] = &[Group::History];

/// [`VIEWPORT`] with the tier table, for a lesson that also picks a row.
const VIEWPORT_TABLE: &[Group] = &[Group::TierTable, Group::History];

/// [`VIEWPORT`] with the view tabs (Live Render and the catalogue tabs).
const VIEWPORT_TABS: &[Group] = &[Group::ViewTabs, Group::History];

/// File operations (Save, Open, Load Selected, the export menu) and Undo.
const FILES: &[Group] = &[Group::FileOps, Group::History];

/// The Advanced controls (cutting mode, Preferences, the Simple | Advanced switch) and Undo.
const ADVANCED: &[Group] = &[Group::Advanced, Group::History];

/// Every viewing, output, library and app tutorial, in the order the browser lists them.
#[must_use]
pub fn guides() -> Vec<Guide> {
    let mut guides = Vec::new();
    guides.extend(solid::guides());
    guides.extend(render::guides());
    guides.extend(output::guides());
    guides.extend(library::guides());
    guides.extend(app::guides());
    guides
}

/// A goal that holds when `test` does. `label` says in a few words what it looks for.
const fn check(label: &'static str, test: fn(&GoalContext<'_>) -> bool) -> Goal {
    Goal::Check { label, test }
}

/// A goal that holds once the desktop has reported UI event `name` during the step.
fn event(name: &str) -> Goal {
    Goal::Event(name.to_owned())
}

/// The tier called `name` (case and surrounding spaces do not matter).
fn tier_called<'a>(design: &'a Design, name: &str) -> Option<&'a ConstraintTier> {
    design.tiers.iter().find(|tier| {
        tier.names()
            .iter()
            .any(|known| known.trim().eq_ignore_ascii_case(name.trim()))
    })
}

/// The tier of the design that stands for `original`, if the learner has not renamed or deleted
/// it.
fn counterpart<'a>(design: &'a Design, original: &Original) -> Option<&'a ConstraintTier> {
    tier_called(design, original.name)
}

/// Some tier of the Rich Teaching Design now has another angle.
fn angle_moved(ctx: &GoalContext<'_>) -> bool {
    RICH.iter().any(|original| {
        counterpart(ctx.design, original)
            .is_some_and(|tier| (tier.angle_deg - original.angle_deg).abs() > ANGLE_SLACK_DEG)
    })
}

/// Some tier of the Rich Teaching Design now sits at another mast, or no longer holds an exact
/// scale value.
fn depth_moved(ctx: &GoalContext<'_>) -> bool {
    RICH.iter().any(|original| {
        counterpart(ctx.design, original).is_some_and(|tier| {
            !matches!(
                &tier.constraint,
                MeetConstraint::ScaleReference(mast) if (mast - original.mast).abs() <= MAST_SLACK
            )
        })
    })
}

/// Some tier of the Rich Teaching Design has been turned round the index wheel.
fn index_moved(ctx: &GoalContext<'_>) -> bool {
    let gear = f64::from(ctx.design.meta.gear_teeth_abs());
    RICH.iter().any(|original| {
        counterpart(ctx.design, original)
            .is_some_and(|tier| !same_index_set(&tier.indices, original.indices, gear))
    })
}

/// Every tier of the Rich Teaching Design is as the lesson started it.
fn rich_untouched(ctx: &GoalContext<'_>) -> bool {
    ctx.design.tiers.len() == RICH.len()
        && !angle_moved(ctx)
        && !depth_moved(ctx)
        && !index_moved(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditorSession, templates::template_spec};
    use indicatrix_cut_core::MaterialSelection;
    use std::time::Duration;

    /// A fresh Rich Teaching Design, as the New Design dialog's card 5 makes it.
    fn rich_session() -> EditorSession {
        let spec = template_spec(5).expect("the template exists");
        EditorSession::from_template(spec.fresh_spec(MaterialSelection::none()), 5)
    }

    fn row(session: &EditorSession, name: &str) -> usize {
        session
            .design
            .tiers
            .iter()
            .position(|tier| tier.names().iter().any(|known| known.trim() == name))
            .unwrap_or_else(|| panic!("no tier is called {name:?}"))
    }

    const NOW: Duration = Duration::from_millis(100);

    #[test]
    fn the_numbers_in_rich_are_the_real_template() {
        let session = rich_session();
        assert_eq!(session.design.tiers.len(), RICH.len());
        let gear = f64::from(session.design.meta.gear_teeth_abs());
        for original in &RICH {
            let tier = counterpart(&session.design, original)
                .unwrap_or_else(|| panic!("the template has no tier {:?}", original.name));
            assert!(
                (tier.angle_deg - original.angle_deg).abs() < 1e-9,
                "{}: angle {} against {}",
                original.name,
                tier.angle_deg,
                original.angle_deg
            );
            assert!(
                matches!(
                    &tier.constraint,
                    MeetConstraint::ScaleReference(mast) if (mast - original.mast).abs() < 1e-9
                ),
                "{}: constraint {:?} against mast {}",
                original.name,
                tier.constraint,
                original.mast
            );
            assert!(
                same_index_set(&tier.indices, original.indices, gear),
                "{}: indices {:?} against {:?}",
                original.name,
                tier.indices,
                original.indices
            );
        }
    }

    #[test]
    fn the_untouched_design_has_no_move() {
        let session = rich_session();
        let ctx = GoalContext::new(&session.design);
        assert!(!angle_moved(&ctx));
        assert!(!depth_moved(&ctx));
        assert!(!index_moved(&ctx));
        assert!(rich_untouched(&ctx));
    }

    #[test]
    fn an_angle_drag_counts_as_an_angle_and_nothing_else() {
        let mut session = rich_session();
        let at = row(&session, "Crown Main");
        session
            .set_tier_angle(at, 38.0, NOW)
            .expect("the drag applies")
            .expect("the angle changed");
        let ctx = GoalContext::new(&session.design);
        assert!(angle_moved(&ctx));
        assert!(!index_moved(&ctx));
        assert!(!rich_untouched(&ctx));
    }

    #[test]
    fn a_depth_drag_counts_as_a_depth() {
        let mut session = rich_session();
        let at = row(&session, "Pavilion Main");
        session
            .pin_tier_mast(at, 0.65, NOW)
            .expect("the drag applies")
            .expect("the mast changed");
        let ctx = GoalContext::new(&session.design);
        assert!(depth_moved(&ctx));
        assert!(!angle_moved(&ctx));
        assert!(!rich_untouched(&ctx));
    }

    #[test]
    fn turning_a_ring_counts_as_an_index_move() {
        let mut session = rich_session();
        let at = row(&session, "Crown Main");
        session
            .rotate_tier_indices(at, 2, NOW)
            .expect("the drag applies")
            .expect("the tier turned");
        let ctx = GoalContext::new(&session.design);
        assert!(index_moved(&ctx));
        assert!(!angle_moved(&ctx));
        assert!(!rich_untouched(&ctx));
    }

    #[test]
    fn the_same_positions_in_another_order_are_not_a_move() {
        let session = rich_session();
        let mut design = session.design.clone();
        let at = row(&session, "Girdle");
        design.tiers[at].indices.reverse();
        assert!(!index_moved(&GoalContext::new(&design)));
    }

    #[test]
    fn a_tier_that_is_gone_is_not_untouched() {
        let mut session = rich_session();
        session.design.tiers.pop();
        assert!(!rich_untouched(&GoalContext::new(&session.design)));
    }

    #[test]
    fn tiers_are_found_by_name_ignoring_case_and_spaces() {
        // The lessons compare by name, so a tier the learner renamed simply stops counting.
        let session = rich_session();
        let mut design = session.design.clone();
        let at = row(&session, "Crown Main");
        design.tiers[at].angle_deg = 50.0;
        assert!(angle_moved(&GoalContext::new(&design)));
        assert!(tier_called(&design, " crown main ").is_some());
        assert!(tier_called(&design, "No Such Tier").is_none());
    }
}
