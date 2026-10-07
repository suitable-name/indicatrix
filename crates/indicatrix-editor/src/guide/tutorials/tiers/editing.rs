//! Cheater offsets, notes, and the tier table's structural edits: duplicate, move, delete and
//! multi-select.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{FORM, FORM_ADVANCED, TABLE, check, position, tier_called};
use crate::guide::{Goal, Guide, GuideCategory, GuideStep, StartingState, TIERS_MULTI_SELECTED};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        cheater_offset(),
        tier_notes(),
        duplicate_a_tier(),
        move_a_tier(),
        delete_a_tier(),
        multi_select(),
    ]
}

fn cheater_offset() -> Guide {
    Guide::new(
        "tiers-cheater-offset",
        "Cheater offset",
        "Record a small azimuth correction on a tier, to make up for a machine error or an off-tooth cut.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "A correction on the sheet",
            "A cheater offset turns a tier's facets a little around the stone. It is printed on the cutting sheet beside the tier.",
        )
        .actions([
            "The field is Cheater Offset (deg), under the facet chips in the Tier tab.",
            "A positive offset moves the facets toward higher index numbers; a blank field means none.",
        ])
        .why("It is a cutting-sheet annotation: it changes what the cutter reads, not the stone you see.")
        .highlight("inspector_tier")
        .allow(FORM_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Set an offset",
            "Offsets belong to one tier, and they take effect at once.",
        )
        .actions([
            "Click the Crown Main row.",
            "In the Tier tab, type 0.5 in Cheater Offset (deg).",
            "Click Set, or press Enter.",
        ])
        .check("the field keeps 0.5, and the cutting sheet shows cheater: +0.50 deg for Crown Main.")
        .why("The field takes a sum too, for example 12+0.5.")
        .goal(
            check("Crown Main has a cheater offset", |ctx| {
                position(ctx.design, "Crown Main")
                    .and_then(|at| ctx.design.cheater_offset_deg(at))
                    .is_some()
            }),
            "a cheater offset on Crown Main",
        )
        .highlight("inspector_tier")
        .allow(FORM_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Clear it",
            "An offset can be taken off again.",
        )
        .actions(["Click Clear next to the field."])
        .check("the field reads none.")
        .why("Ctrl+Z works too.")
        .goal(
            check("Crown Main has no cheater offset", |ctx| {
                position(ctx.design, "Crown Main")
                    .is_some_and(|at| ctx.design.cheater_offset_deg(at).is_none())
            }),
            "the cheater offset cleared",
        )
        .highlight("inspector_tier")
        .allow(FORM_ADVANCED),
    )
}

fn tier_notes() -> Guide {
    Guide::new(
        "tiers-tier-notes",
        "Tier notes",
        "Attach a short note to a tier, such as check the meet here, and see it travel with the tier.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Add a note",
            "A note is a reminder for you, or for whoever cuts the stone. It changes no angle or depth.",
        )
        .actions([
            "Click the Pavilion Main row.",
            "In the Tier tab, find Note under the facet chips.",
            "Type check the meet here, then click Set or press Enter.",
        ])
        .check("the field keeps your text, and the cutting sheet prints it with the tier.")
        .why("A note takes effect at once; there is no Save Tier step for it.")
        .goal(
            check("Pavilion Main has a note", |ctx| {
                position(ctx.design, "Pavilion Main")
                    .and_then(|at| ctx.design.tier_note(at))
                    .is_some_and(|note| !note.trim().is_empty())
            }),
            "a note on Pavilion Main",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Clear the note",
            "A blank note means no note.",
        )
        .actions(["Click Clear next to the field."])
        .check("the field is empty.")
        .goal(
            check("Pavilion Main has no note", |ctx| {
                position(ctx.design, "Pavilion Main")
                    .is_some_and(|at| ctx.design.tier_note(at).is_none())
            }),
            "the note cleared",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Notes stay with their tier",
            "A note belongs to its tier, not to a row number.",
        )
        .actions([
            "Add tiers, move tiers or delete other tiers: each note stays with the tier it was written on.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn duplicate_a_tier() -> Guide {
    Guide::new(
        "tiers-duplicate",
        "Duplicate a tier",
        "Copy a tier right below itself, then change the copy: the quickest way to add a second ring.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Duplicate a tier",
            "Duplicate copies a whole tier: its angle, indices and Meets.",
        )
        .actions([
            "Click the copy icon at the end of the Crown Main row, or pick the row and press Ctrl+D.",
        ])
        .check("a new row called Crown Main (2) right below Crown Main.")
        .why("The copy sits right after the original, since cutting order matters. Its name counts up, so it never clashes with another tier.")
        .goal(Goal::TierExists("Crown Main (2)".to_owned()), "a copy called Crown Main (2)")
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Change the copy",
            "A copy is only useful once it differs from the original.",
        )
        .actions([
            "Double-click the ANGLE cell of Crown Main (2).",
            "Type 30 and press Enter.",
        ])
        .check("Crown Main (2) reads 30.00; Crown Main still reads 34.50.")
        .why("Both rings share the original's depth until you change the copy's Meets in the Tier tab.")
        .goal(Goal::tier("Crown Main (2)", 30.0), "the copy at 30.0")
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Take a copy back",
            "A duplicate is one Undo step.",
        )
        .actions([
            "Press Ctrl+Z twice to take back the angle and then the copy.",
            "Press Ctrl+Y twice to put them back.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn move_a_tier() -> Guide {
    Guide::new(
        "tiers-move",
        "Move a tier",
        "Change the cutting order by moving a tier up or down the table.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Move a tier up",
            "The order of the rows decides the order the facets are cut: the pavilion and girdle rows first, then the crown rows, and the table last.",
        )
        .actions([
            "Find the up and down buttons at the end of the Girdle row.",
            "Click the up button once, or pick the row and press Alt+Up.",
        ])
        .check("Girdle now sits above Pavilion Main.")
        .why("A move is one Undo step. Names and Meets do not depend on position, so moving never breaks a link.")
        .goal(
            check("Girdle is above Pavilion Main", |ctx| {
                matches!(
                    (
                        position(ctx.design, "Girdle"),
                        position(ctx.design, "Pavilion Main"),
                    ),
                    (Some(girdle), Some(pavilion)) if girdle < pavilion
                )
            }),
            "Girdle above Pavilion Main",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Put the pavilion first",
            "The worked example cuts the pavilion mains first: they set the depth of the stone, and the girdle follows.",
        )
        .actions(["Move the Pavilion Main row up until it is the first row."])
        .check("Pavilion Main is the first row.")
        .goal(
            check("Pavilion Main is the first tier", |ctx| {
                position(ctx.design, "Pavilion Main") == Some(0)
            }),
            "Pavilion Main as the first row",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Why the order matters",
            "The cutting sheet and cutting mode cut the pavilion and girdle rows first, in table order, then the crown rows, and the table last.",
        )
        .actions([
            "Tiers that others meet should come before the tiers that meet them.",
            "Press Ctrl+Z to put a tier back where it was.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn delete_a_tier() -> Guide {
    Guide::new(
        "tiers-delete",
        "Delete a tier",
        "Remove a tier from the table, and bring it back with Undo.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Remove a tier",
            "Remove takes one tier, with all its facets, out of the design.",
        )
        .actions([
            "Click the trash icon at the end of the Table row, or pick the row and press Delete.",
        ])
        .check("the Table row is gone, and a note says Removed Table, Undo.")
        .why("Nothing else changes: the other tiers keep their angles and depths.")
        .goal(
            check("Table is gone", |ctx| tier_called(ctx.design, "Table").is_none()),
            "the Table tier removed",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Take it back",
            "Undo puts the tier back exactly as it was, in the same place.",
        )
        .actions(["Press Ctrl+Z, or click Undo on the command bar."])
        .check("the Table row is back.")
        .goal(
            Goal::TierExists("Table".to_owned()),
            "the Table tier back",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Removing a tier others meet",
            "Some tiers cannot simply disappear.",
        )
        .actions([
            "If other tiers still meet the tier by name, the table asks Remove anyway? and lists them.",
            "Confirming clears those references, so those tiers meet whatever vertex the solver finds; it is one Undo step.",
            "Tiers that followed it by a relation keep their angle and stop following.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn multi_select() -> Guide {
    Guide::new(
        "tiers-multi-select",
        "Multi-select",
        "Tick several tiers and change or delete them together.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Pick two tiers",
            "A group of ticked tiers can be changed in one go.",
        )
        .actions([
            "Click Select above the tier table to show tick boxes, then tick Crown Main and Pavilion Main.",
            "Or Ctrl+click rows one at a time, or Shift+click to pick a range.",
        ])
        .check("a bar above the table says 2 selected.")
        .why("The ticks change no design data. Space on a row ticks it from the keyboard.")
        .goal(
            Goal::Event(TIERS_MULTI_SELECTED.to_owned()),
            "two tiers ticked",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Offset them together",
            "Offset adds the same number of degrees to every ticked tier.",
        )
        .actions([
            "Click one of the ticked rows so the cursor is on it.",
            "In the bar above the table, type 1 in the deg box and click Offset.",
        ])
        .check("Crown Main reads 35.50 and Pavilion Main has moved by one degree as well.")
        .why("It is one Undo step. A tier stops at 0 rather than crossing to the other side.")
        .goal(
            Goal::All(vec![
                Goal::tier("Crown Main", 35.5),
                Goal::tier("Pavilion Main", -39.0),
            ]),
            "Crown Main at 35.5 and Pavilion Main moved by one degree",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Delete them together",
            "Delete in the bar removes every ticked tier.",
        )
        .actions(["Click Delete in the bar above the table."])
        .check("only Table and Girdle are left.")
        .why("Each tier is its own Undo step, so Ctrl+Z brings them back one at a time.")
        .goal(
            check("Crown Main and Pavilion Main are gone", |ctx| {
                tier_called(ctx.design, "Crown Main").is_none()
                    && tier_called(ctx.design, "Pavilion Main").is_none()
            }),
            "both ticked tiers removed",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "More on groups",
            "A group stays until you clear it.",
        )
        .actions([
            "Clear in the bar drops the group without touching a tier.",
            "Adopt sel. (Advanced) adopts the imported meets of the ticked tiers only.",
        ])
        .why("Every control is unlocked again."),
    )
}
