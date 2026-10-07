//! The step ladder (plain and linked), mirroring to the other block, relations between tiers,
//! and arithmetic in the number fields.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{FORM, MAIN, STAR, TABLE, TABLE_ADVANCED, check, is_driven, tier_called};
use crate::guide::{
    Goal, GoalContext, Guide, GuideCategory, GuideStep, StartingState, same_index_set,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        step_series(),
        linked_series(),
        mirror_to_other_block(),
        tier_relations(),
        arithmetic_in_fields(),
    ]
}

/// A tier of this design sits at `angle_deg` (within 0.05 degrees) on the main ring.
fn has_main_ring_at(ctx: &GoalContext<'_>, angle_deg: f64) -> bool {
    let gear = f64::from(ctx.design.meta.gear_teeth_abs());
    ctx.design.tiers.iter().any(|tier| {
        (tier.angle_deg - angle_deg).abs() < 0.05 && same_index_set(&tier.indices, &MAIN, gear)
    })
}

/// The three rungs of the ladders the two ladder lessons build, at these angles.
fn rungs(first: f64, step: f64) -> Goal {
    Goal::All(
        (0..3)
            .map(|rung| {
                Goal::tier(
                    format!("Step{}", rung + 1),
                    step.mul_add(f64::from(rung), first),
                )
                .with_indices(&MAIN)
            })
            .collect(),
    )
}

fn step_series() -> Guide {
    Guide::new(
        "tiers-step-series",
        "Generate steps",
        "Write a whole ladder of tiers, each a fixed angle step from the last, with one form.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(4))
    .step(
        GuideStep::new(
            "A ladder of tiers",
            "A step cut has a row of tiers whose angles climb by a fixed step. Generate steps writes the whole ladder in one go.",
        )
        .actions([
            "Click Steps / Mirror above the tier table to open the ladder form.",
            "Its fields are the name, the start angle, the step, the count N, the indices and an optional anchor.",
        ])
        .why("Each rung after the first meets the one before it. The first rung meets the anchor you give, or, left blank, an anchor already in the design.")
        .highlight("tier_table")
        .allow(TABLE_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Generate a crown ladder",
            "Three crown rungs, 4 degrees apart, all on the main ring.",
        )
        .actions([
            "Fill in the form: name Step, start 30, step 4, N 3, indices 0:12:96.",
            "Leave anchor empty.",
            "Leave Keep linked unticked.",
            "Click Generate.",
        ])
        .check("three new rows, Step1, Step2 and Step3, at 30.00, 34.00 and 38.00.")
        .why("The rungs are named Step1, Step2 and so on, counting on past any name already in use.")
        .goal(rungs(30.0, 4.0), "Step1, Step2 and Step3 at 30, 34 and 38")
        .highlight("tier_table")
        .allow(TABLE_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Edit one rung",
            "After Generate, each rung is an ordinary tier.",
        )
        .actions([
            "Double-click the ANGLE cell of Step2.",
            "Type 35 and press Enter.",
        ])
        .check("Step2 reads 35.00; Step1 and Step3 did not move.")
        .why("A plain ladder is only a quick way to add tiers. The next lesson, Keep linked, ties the rungs together.")
        .goal(Goal::tier("Step2", 35.0), "Step2 at 35.0")
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "One Undo for the ladder",
            "The whole ladder went in as one Undo step.",
        )
        .actions([
            "Press Ctrl+Z twice to take back the edit and then the whole ladder.",
            "Press Ctrl+Y twice to put them back.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn linked_series() -> Guide {
    Guide::new(
        "tiers-linked-series",
        "Keep linked",
        "Generate a ladder whose rungs follow its first tier, so one edit moves them all.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(4))
    .step(
        GuideStep::new(
            "Generate a linked ladder",
            "Keep linked gives every rung after the first a relation to the first: its angle plus its own number of steps.",
        )
        .actions([
            "Click Steps / Mirror to open the ladder form.",
            "Fill in the form: name Step, start 30, step 4, N 3, indices 0:12:96.",
            "Tick Keep linked.",
            "Click Generate.",
        ])
        .check("Step2 and Step3 show a link icon in the ANGLE column.")
        .why("Step2 becomes Step1 + 4 and Step3 becomes Step1 + 8.")
        .goal(
            Goal::All(vec![
                rungs(30.0, 4.0),
                check("Step2 and Step3 follow Step1", |ctx| {
                    is_driven(ctx.design, "Step2")
                        && is_driven(ctx.design, "Step3")
                        && !is_driven(ctx.design, "Step1")
                }),
            ]),
            "a linked ladder Step1 to Step3",
        )
        .highlight("tier_table")
        .allow(TABLE_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Move the first rung",
            "The linked rungs follow the first, so the first is the one to edit.",
        )
        .actions([
            "Double-click the ANGLE cell of Step1.",
            "Type 32 and press Enter.",
        ])
        .check("Step2 reads 36.00 and Step3 reads 40.00 without being touched.")
        .why("A rung with a link icon follows its relation, so one edit moves the whole ladder, in one Undo step.")
        .goal(rungs(32.0, 4.0), "Step1 at 32 with Step2 and Step3 following")
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Free a rung",
            "A rung can leave the ladder and become an ordinary tier again.",
        )
        .actions([
            "Click the Step3 row.",
            "In the Tier tab, click Remove relation.",
        ])
        .check("Step3 loses its link icon and keeps 40.00.")
        .why("Removing a relation keeps the angle the tier has now. From then on you can edit it on its own.")
        .goal(
            Goal::All(vec![
                Goal::tier("Step3", 40.0),
                check("Step3 is free and Step2 still follows Step1", |ctx| {
                    !is_driven(ctx.design, "Step3") && is_driven(ctx.design, "Step2")
                }),
            ]),
            "Step3 free of its relation",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Relations you write yourself",
            "A linked ladder is made of ordinary relations, so you can write your own too.",
        )
        .actions([
            "Start the Angle field of a tier with =, for example =Step1+2.",
            "The Tier relations lesson shows how.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn mirror_to_other_block() -> Guide {
    Guide::new(
        "tiers-mirror-to-other-block",
        "Mirror to other block",
        "Copy a tier to the opposite side of the girdle: the angle size and everything else are kept, only the block changes.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Mirror the pavilion to the crown side",
            "Mirror to other block copies the tier you picked to the opposite block of the stone.",
        )
        .actions([
            "Click the Pavilion Main row.",
            "Click Steps / Mirror above the tier table.",
            "Leave the name suffix as it is.",
            "Click Mirror.",
        ])
        .check("a new last row with 40.00 in the ANGLE column and the same indices.")
        .why("The copy goes to the opposite block (a pavilion tier becomes a crown tier) with the same angle size, and everything else is kept. The copy's name gets the suffix, so it never clashes with the original.")
        .goal(
            check("a tier at 40 degrees on the main ring", |ctx| {
                has_main_ring_at(ctx, 40.0)
            }),
            "a copy of Pavilion Main at 40.0",
        )
        .highlight("tier_table")
        .allow(TABLE_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Mirror a crown tier",
            "Mirroring works from the crown side too.",
        )
        .actions([
            "Click the Crown Main row.",
            "Change the name suffix to b.",
            "Click Mirror.",
        ])
        .check("a new last row with 34.50 in the ANGLE column and a P code in the CODE column.")
        .why("A tier with several names, such as P1/P2, gets the suffix on each of them.")
        .goal(
            check("a pavilion tier at 34.5 degrees on the main ring", |ctx| {
                has_main_ring_at(ctx, -34.5)
            }),
            "a copy of Crown Main on the pavilion at 34.5",
        )
        .highlight("tier_table")
        .allow(TABLE_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Where the copies go",
            "Mirrored tiers are added at the end of the table. The cutting sheet still cuts the pavilion and girdle rows first, then the crown rows, then the table, so a copy joins the other rows of its side in table order.",
        )
        .actions([
            "Use the up and down buttons of a row, or Alt+Up and Alt+Down, to move a copy where you want it.",
            "Press Ctrl+Z to take a copy back.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn tier_relations() -> Guide {
    Guide::new(
        "tiers-relations",
        "Tier relations",
        "Make one tier's angle follow another's, so moving one moves the other.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "A tier that follows another",
            "A relation makes a tier's angle follow other tiers' angles, so moving one moves the rest.",
        )
        .actions([
            "Start the Angle field with =, for example =[Crown Main]+6.5.",
            "A name with spaces goes in square brackets.",
            "A girdle, a table and a culet cannot follow a relation.",
        ])
        .why("Relations work on angle sizes. A pavilion tier keeps its own side of the stone.")
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Make Pavilion Main follow Crown Main",
            "Pavilion Main will always sit 6.5 degrees steeper than Crown Main.",
        )
        .actions([
            "Click the Pavilion Main row.",
            "In the Tier tab, set Angle (deg) to =[Crown Main]+6.5",
            "Click Save Tier.",
        ])
        .check("Pavilion Main shows a link icon in the ANGLE column and reads 41.00.")
        .why("34.5 plus 6.5 is 41, and the tier keeps its pavilion side.")
        .goal(
            Goal::All(vec![
                Goal::tier("Pavilion Main", -41.0),
                check("Pavilion Main follows a relation", |ctx| {
                    is_driven(ctx.design, "Pavilion Main")
                }),
            ]),
            "Pavilion Main following Crown Main",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Move the driver",
            "Edit the tier that is followed and watch the other one move.",
        )
        .actions([
            "Double-click the ANGLE cell of Crown Main.",
            "Type 36 and press Enter.",
        ])
        .check("Pavilion Main moved to 42.50 by itself.")
        .why("The relation is worked out in the same Undo step as your edit.")
        .goal(
            Goal::All(vec![
                Goal::tier("Crown Main", 36.0),
                Goal::tier("Pavilion Main", -42.5),
            ]),
            "Crown Main at 36 and Pavilion Main following it",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Stop following",
            "A tier can leave a relation and keep the angle it has.",
        )
        .actions([
            "Click the Pavilion Main row.",
            "In the Tier tab, click Remove relation.",
        ])
        .check("the link icon is gone and the angle stays 42.50.")
        .why("A tier that follows a relation cannot be nudged or edited by hand, which is why the table explains it when you try.")
        .goal(
            Goal::All(vec![
                Goal::tier("Pavilion Main", -42.5),
                check("Pavilion Main follows nothing", |ctx| {
                    !is_driven(ctx.design, "Pavilion Main")
                }),
            ]),
            "Pavilion Main free of its relation",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Writing relations",
            "A relation is arithmetic over other tiers' angle sizes.",
        )
        .actions([
            "=C1-4 follows C1, 4 degrees shallower.",
            "=(P1+P3)/2 sits half way between P1 and P3.",
            "A relation that would loop, or give an angle outside 0 to 90 degrees, is refused with the reason.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn arithmetic_in_fields() -> Guide {
    Guide::new(
        "tiers-arithmetic",
        "Arithmetic in fields",
        "Type a sum such as 34.5+0.3 or 96/16 into a number field and let the program work it out.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Calculate an angle",
            "Any number field takes + - * / and brackets, worked out when you save.",
        )
        .actions([
            "Click the Crown Main row.",
            "In the Tier tab, set Angle (deg) to 34.5+0.3",
            "Click Save Tier.",
        ])
        .check("Crown Main reads 34.80.")
        .why("On a pavilion tier the field holds a plain number too, so write the sum the same way, for example 40-0.5: the tier keeps its side of the stone.")
        .goal(Goal::tier("Crown Main", 34.8), "Crown Main at 34.8")
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Calculate a scale value",
            "The Exact scale value takes a sum too.",
        )
        .actions([
            "Click the Table row.",
            "Meets: Exact scale value, Scale value: 0.5-0.15",
            "Click Save Tier.",
        ])
        .check("the Table row's Meets column reads 0.35.")
        .why("A sum that cannot be worked out, such as 1/0, is refused with the reason under the field.")
        .goal(
            check("Table is pinned to 0.35", |ctx| {
                tier_called(ctx.design, "Table").is_some_and(|table| {
                    matches!(
                        &table.constraint,
                        MeetConstraint::ScaleReference(value) if (value - 0.35).abs() < 1e-6
                    )
                })
            }),
            "Table pinned to a scale value of 0.35",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Calculate a facet position",
            "The position box under the facet chips takes a sum as well.",
        )
        .actions([
            "Click the Crown Main row.",
            "Under Facets, type 96/16 in the position box.",
            "Click + Add.",
        ])
        .check("Crown Main gains the ring at 6, 18, 30 and so on up to 90.")
        .why("96/16 is 6, half way between the main facets. Its whole ring comes with it.")
        .goal(
            check("Crown Main has both rings", |ctx| {
                let gear = f64::from(ctx.design.meta.gear_teeth_abs());
                tier_called(ctx.design, "Crown Main").is_some_and(|tier| {
                    STAR.iter()
                        .all(|star| tier.indices.iter().any(|have| same_index_set(&[*have], &[*star], gear)))
                })
            }),
            "the ring at 6, 18 and so on added",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Where arithmetic works",
            "Wherever a field wants a number it will also take a sum.",
        )
        .actions([
            "The Angle field of the Tier tab and of the table's own angle cell.",
            "The Scale value and the millimetre values of Meets.",
            "The position box, the Rotate teeth box, the Offset box and the cheater offset.",
            "The concave tier form's number fields.",
        ])
        .why("Every control is unlocked again."),
    )
}
