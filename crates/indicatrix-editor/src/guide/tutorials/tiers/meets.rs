//! Meets and targets: named facets, millimetre targets, adopting imported meets, and pinning
//! a solved depth.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{FORM, FORM_ADVANCED, TABLE_ADVANCED, check, meets_named, position, tier_called};
use crate::guide::{
    Goal, GoalContext, Group, Guide, GuideCategory, GuideStep, MeetKind, StartingState,
};
use indicatrix_cut_core::{ConstraintTier, Design, TierTarget};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        meets_named_facets(),
        meets_in_millimetres(),
        adopt_imported_meets(),
        pin_to_mast(),
    ]
}

/// The millimetre target of the tier called `name`.
fn target_of(design: &Design, name: &str) -> Option<TierTarget> {
    position(design, name).and_then(|at| design.tier_target(at))
}

/// Whether a tier still holds a meet instruction its file stated that it has not adopted.
fn is_adoptable(tier: &ConstraintTier) -> bool {
    tier.imported_meet
        .as_ref()
        .is_some_and(|stated| *stated != tier.constraint)
}

/// Whether a tier has adopted the meet instruction its file stated.
fn is_adopted(tier: &ConstraintTier) -> bool {
    tier.imported_meet
        .as_ref()
        .is_some_and(|stated| *stated == tier.constraint)
}

/// Some tier has an imported meet to adopt.
fn has_adoptable(ctx: &GoalContext<'_>) -> bool {
    ctx.design.tiers.iter().any(is_adoptable)
}

/// Some tier has adopted its imported meet.
fn has_adopted(ctx: &GoalContext<'_>) -> bool {
    ctx.design.tiers.iter().any(is_adopted)
}

/// Every imported meet has been adopted, and there was at least one.
fn all_adopted(ctx: &GoalContext<'_>) -> bool {
    has_adopted(ctx) && !has_adoptable(ctx)
}

/// The stone the Meets lessons that let the solver find a depth start from: the Standard
/// Round Brilliant (card 1).
///
/// The Rich Teaching Design does not do. It has one tier in its pavilion, so naming a facet for
/// that tier leaves the pavilion without an exact scale value and the design stops solving. Its
/// crown has no star facets, so an unspecified vertex sinks the table onto the girdle's top
/// edge and cuts the crown away. The Standard Round Brilliant keeps an exact scale value in
/// every block after each step of the lessons, and its table stays near its old height.
const SOLVER_STONE: usize = 1;

fn meets_named_facets() -> Guide {
    Guide::new(
        "tiers-meets-named-facets",
        "Meets: named facets",
        "Choose how a tier finds its depth: an unspecified vertex, named facets, or an exact scale value.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(SOLVER_STONE))
    .step(
        GuideStep::new(
            "Three ways to set a depth",
            "The Meets setting of a tier decides how its depth is found.",
        )
        .actions([
            "Unspecified vertex: the tier closes against whatever vertex the solver finds.",
            "Named facet(s): the tier closes against the facets you name.",
            "Exact scale value: you state the depth outright.",
        ])
        .why("To solve, each block of the stone (the crown, the pavilion and the girdle) needs one tier with an exact scale value. The other tiers of that block can then meet facets or a vertex instead.")
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Meet a named facet",
            "Naming the facet a tier meets lets the solver find the depth for you.",
        )
        .actions([
            "Click the Upper Girdle row.",
            "Set Meets to Named facet(s).",
            "Facet names: Lower Girdle",
            "Click Save Tier.",
        ])
        .check("Upper Girdle's Meets column reads meets Lower Girdle.")
        .why("Several names are separated by commas. The solver puts the depth where this tier's facets cut the named facets: here, where the upper girdle facets touch the lower girdle facets. The crown still has other tiers with an exact scale value, so the stone keeps solving.")
        .goal(
            check("Upper Girdle meets Lower Girdle", |ctx| {
                meets_named(ctx.design, "Upper Girdle", "Lower Girdle")
            }),
            "Upper Girdle meeting Lower Girdle",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Let the solver choose",
            "An unspecified vertex names nothing: the tier closes against whichever vertex the solver finds.",
        )
        .actions([
            "Click the Table row.",
            "Set Meets to Unspecified vertex.",
            "Click Save Tier.",
        ])
        .check("the Table row's Meets column reads meet.")
        .why("The solver cuts the table down to a vertex of the facets already there, and this crown, with its star facets, gives it one close to where the table was. Use it for a table you do not want to size by hand, and look at the result: on a crown of main facets only, with no star facets, the next vertex below the point where they meet is the girdle, so the table would sink there and cut the crown away. Undo (Ctrl+Z) brings the table back.")
        .goal(
            Goal::tier("Table", 0.0).with_meet(MeetKind::Unspecified),
            "Table on an unspecified vertex",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Rename a facet others meet",
            "Upper Girdle now meets Lower Girdle by name. See what happens when Lower Girdle changes its name.",
        )
        .actions([
            "Click the Lower Girdle row.",
            "Change Name to LG1.",
            "Click Save Tier.",
        ])
        .check("Upper Girdle's Meets column now reads meets LG1.")
        .why("Renaming a tier updates every tier that meets it by name, in the same Undo step.")
        .goal(
            check("Upper Girdle meets LG1", |ctx| {
                tier_called(ctx.design, "LG1").is_some()
                    && meets_named(ctx.design, "Upper Girdle", "LG1")
            }),
            "Upper Girdle meeting LG1",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Names must stay unique",
            "A name is how other tiers find a tier, so two tiers may not share one.",
        )
        .actions([
            "A tier cannot be left unnamed while another tier meets it.",
            "Join several names with / to give one tier more than one name, for example P1/P2.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn meets_in_millimetres() -> Guide {
    Guide::new(
        "tiers-meets-mm-targets",
        "Meets in millimetres",
        "State a depth, a girdle thickness or a table width in millimetres instead of scale units.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Give the stone a size",
            "Millimetre targets are converted through the girdle diameter, so the design needs one first.",
        )
        .actions([
            "Open the Preform tab in the inspector.",
            "Girdle Diameter (mm): 6.5",
            "Click Apply Yield Inputs.",
        ])
        .check("the Preform tab shows the yield figures after the next solve.")
        .why("The girdle diameter is the stone's real width. It turns the design's own units into millimetres; without it these targets cannot be solved.")
        .goal(Goal::YieldApplied, "the yield inputs to be applied")
        .highlight("preform_tab")
        .allow(&[Group::PreformTab, Group::History]),
    )
    .step(
        GuideStep::new(
            "Cut to a depth",
            "Cut to depth (mm) states how deep a facet's plane sits, measured the way the MAST column's millimetre tooltip measures it.",
        )
        .actions([
            "Open the Tier tab and click the Pavilion Main row.",
            "Set Meets to Cut to depth (mm).",
            "Value (mm): 2.2",
            "Click Save Tier.",
        ])
        .check("the Pavilion Main row's Meets column reads 2.20 mm depth.")
        .why("The value is turned into a scale value just before each solve, so it follows the girdle diameter if you change it.")
        .goal(
            check("Pavilion Main has a depth target", |ctx| {
                matches!(
                    target_of(ctx.design, "Pavilion Main"),
                    Some(TierTarget::DepthMm(_))
                )
            }),
            "a depth in millimetres on Pavilion Main",
        )
        .highlight("inspector_tier")
        .allow(FORM_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Set the table width",
            "Table width (mm) searches the tier's depth until the finished table is that wide.",
        )
        .actions([
            "Click the Table row.",
            "Set Meets to Table width (mm).",
            "Value (mm): 3.2",
            "Click Save Tier.",
        ])
        .check("the Table row's Meets column reads table 3.20 mm.")
        .why("This costs several solves rather than one, so the status strip may take a moment to settle.")
        .goal(
            check("Table has a width target", |ctx| {
                matches!(
                    target_of(ctx.design, "Table"),
                    Some(TierTarget::TableWidthMm(_))
                )
            }),
            "a width in millimetres on Table",
        )
        .highlight("inspector_tier")
        .allow(FORM_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Set the girdle thickness",
            "Girdle thickness (mm) searches the tier's depth until the girdle band is that thick.",
        )
        .actions([
            "Click the Girdle row.",
            "Set Meets to Girdle thickness (mm).",
            "Value (mm): 0.3",
            "Click Save Tier.",
        ])
        .check("the Girdle row's Meets column reads girdle 0.30 mm.")
        .why("Put it on the tier that makes the girdle: the girdle row itself.")
        .goal(
            check("Girdle has a thickness target", |ctx| {
                matches!(
                    target_of(ctx.design, "Girdle"),
                    Some(TierTarget::GirdleThicknessMm(_))
                )
            }),
            "a thickness in millimetres on Girdle",
        )
        .highlight("inspector_tier")
        .allow(FORM_ADVANCED),
    )
    .step(
        GuideStep::new(
            "When a target cannot be met",
            "A search needs a width it can reach. A table wider than the stone, for instance, has no answer.",
        )
        .actions([
            "If the status strip says a target could not be bracketed, widen or remove it.",
            "Choose Exact scale value or Named facet(s) again in Meets to remove a target; saving does it for you.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn adopt_imported_meets() -> Guide {
    Guide::new(
        "tiers-adopt-imported-meets",
        "Adopt imported meets",
        "Switch the tiers of an imported design from pinned depths to the meets its file stated.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::RequiresOpenDesign)
    .step(
        GuideStep::new(
            "Open an imported design",
            "Adopt works on a design imported from a GemCad .asc file. Import pins every tier to its depth and keeps the file's own meet instruction aside.",
        )
        .actions([
            "Open a .asc file with File > Open, or load an imported design from the library.",
            "Look for Adopt buttons in the tier table, and for a cyan line starting File says under Solved in the Tier tab.",
        ])
        .check("an Adopt button in the tier table.")
        .why("A design you built yourself has nothing to adopt. Use Back to leave this lesson.")
        .goal(
            check("a tier has an imported meet to adopt", has_adoptable),
            "an imported design with meets to adopt",
        )
        .highlight("tier_table")
        .allow(&[
            Group::FileOps,
            Group::TierTable,
            Group::Advanced,
            Group::History,
        ]),
    )
    .step(
        GuideStep::new(
            "Adopt one tier",
            "Adopt switches a tier from a fixed number to the instruction its file stated.",
        )
        .actions(["Click Adopt on one row of the tier table."])
        .check("that row's Meets column changes from a pinned value to a meet, and its Adopt button goes.")
        .why("A tier that meets other facets can be moved by Solve and Optimize; a pinned tier cannot.")
        .goal(
            check("a tier has adopted its imported meet", has_adopted),
            "one tier adopted",
        )
        .highlight("tier_table")
        .allow(TABLE_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Adopt the rest at once",
            "Adopt all does every remaining tier in one go.",
        )
        .actions(["Click Adopt all above the tier table."])
        .check("no Adopt button is left in the tier table.")
        .why("Adopt all is one Undo step, so a single Ctrl+Z brings every pinned value back.")
        .goal(
            check("every imported meet is adopted", all_adopted),
            "every imported meet adopted",
        )
        .highlight("tier_table")
        .allow(TABLE_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Adopt and Pin",
            "Adopt and Pin are opposites: Adopt trades a pinned depth for a meet, Pin trades a meet for a pinned depth.",
        )
        .actions([
            "Adopt sel. in the multi-select bar adopts only the ticked tiers.",
            "Continue with Pin to mast.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn pin_to_mast() -> Guide {
    Guide::new(
        "tiers-pin-to-mast",
        "Pin to mast",
        "Freeze the depth the solver found for a tier as an exact scale value.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(SOLVER_STONE))
    .step(
        GuideStep::new(
            "Let the solver decide a depth",
            "Pin needs a tier whose depth the solver decides, so first make Upper Girdle meet the lower girdle.",
        )
        .actions([
            "Click the Upper Girdle row.",
            "Set Meets to Named facet(s).",
            "Facet names: Lower Girdle",
            "Click Save Tier.",
        ])
        .check("the Upper Girdle row's Meets column reads meets Lower Girdle.")
        .why("The stone keeps solving because the crown still has other tiers with an exact scale value.")
        .goal(
            Goal::tier("Upper Girdle", 41.0).with_meet(MeetKind::Named),
            "Upper Girdle meeting a named facet",
        )
        .highlight("inspector_tier")
        .allow(FORM_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Pin the solved depth",
            "Pin takes the depth the solver found and writes it back as an exact scale value.",
        )
        .actions([
            "Wait until the status strip says the stone is solved.",
            "In the Upper Girdle row, click Pin in the PIN column.",
        ])
        .check("the Meets column shows a pinned scale value, and the Pin button goes.")
        .why("Pin a tier before Optimize to hold it where it is while the other tiers move. There is no Pin button while a tier is unsolved or already pinned.")
        .goal(
            Goal::tier("Upper Girdle", 41.0).with_meet(MeetKind::ExactScale),
            "Upper Girdle pinned to its solved depth",
        )
        .highlight("tier_table")
        .allow(TABLE_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Pin and Adopt",
            "Pin turns a meet into a number. Adopt turns an imported number back into a meet.",
        )
        .actions([
            "Press Ctrl+Z to undo the pin and meet the lower girdle again.",
            "Use Adopt on an imported design to switch to the meets its file stated.",
        ])
        .why("Every control is unlocked again."),
    )
}
