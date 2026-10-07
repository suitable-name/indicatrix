//! Add a tier, edit a tier, the index shorthands, the facet chips, quick add, and the inline
//! angle cell with its nudging.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{FORM, FORM_ADVANCED, MAIN, STAR, TABLE, check, detached_is, tier_called};
use crate::guide::{Goal, Guide, GuideCategory, GuideStep, MeetKind, StartingState};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        add_a_tier(),
        edit_a_tier(),
        index_shorthands(),
        facet_chips(),
        quick_add(),
        inline_angle(),
    ]
}

/// The pavilion main of the Rich Teaching Design, after the Edit lesson renamed it.
const EDITED_NAME: &str = "P1";

fn add_a_tier() -> Guide {
    Guide::new(
        "tiers-add-a-tier",
        "Add a tier",
        "Add a girdle, a pavilion and a crown tier with the Tier form: angle, Meets, name and indices.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::NewEmpty)
    .step(
        GuideStep::new(
            "What a tier is",
            "A tier is one ring of facets that share an angle and a depth. The Indices say where around the stone each facet sits.",
        )
        .actions([
            "Look at the Tier tab in the inspector: Angle, Meets, Name and Indices.",
            "On a 96-tooth gear with 8-fold symmetry a ring has a facet every 12 teeth: 0, 12, 24 and so on up to 84.",
        ])
        .why("Angles are measured from the girdle plane: a crown facet sits above it, a pavilion facet below it, and exactly 90 is the girdle. The tier table shows every angle as a plain positive number; the letter of its code (P, C or G) says which side the tier is on.")
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Add the girdle",
            "The girdle gives the crown and the pavilion something to meet, so it goes in first.",
        )
        .actions([
            "Click + Add Tier.",
            "Click Girdle Facet Preset (90\u{b0}, scale = 1).",
            "Name: G1",
            "Indices: 0, 12, 24, 36, 48, 60, 72, 84",
            "Click Add Tier.",
        ])
        .check("a G1 row in the tier table, with G1 in the CODE column.")
        .why("The preset sets Angle to 90 and Meets to Exact scale value 1, the one pair that makes a real girdle. A 0 degree tier would be a second table.")
        .goal(
            Goal::tier("G1", 90.0)
                .with_indices(&MAIN)
                .with_meet(MeetKind::ExactScale),
            "tier G1 at 90.0 with the eight indices",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Add a pavilion tier",
            "A pavilion tier goes below the girdle. You type its angle as a plain number; the P in its name puts it on the pavilion side.",
        )
        .actions([
            "Click + Add Tier.",
            "Angle (deg): 41",
            "Meets: Named facet(s), Facet names: G1",
            "Name: P1",
            "Indices: 0, 12, 24, 36, 48, 60, 72, 84",
            "Click Add Tier.",
        ])
        .check("a P1 row with P1 in the CODE column and meets G1 in the Meets column.")
        .why("Named facet(s) closes the pavilion against the girdle: the solver finds its depth for you. To solve, though, the pavilion and the crown still need one tier each with an exact scale value, as the girdle has.")
        .goal(
            Goal::tier("P1", -41.0)
                .with_indices(&MAIN)
                .with_meet(MeetKind::Named),
            "tier P1 at 41.0 with the eight indices",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Add a crown tier",
            "A crown tier goes above the girdle. You type its angle as a plain number, and its name starts with C.",
        )
        .actions([
            "Click + Add Tier.",
            "Angle (deg): 34.5",
            "Meets: Named facet(s), Facet names: G1",
            "Name: C1",
            "Indices: 0, 12, 24, 36, 48, 60, 72, 84",
            "Click Add Tier.",
        ])
        .check("a C1 row with C1 in the CODE column.")
        .why("Leave Name blank and the tier is named from its angle (C1, P1, G1), but a name you chose yourself is easier to find in a Meets list.")
        .goal(
            Goal::tier("C1", 34.5)
                .with_indices(&MAIN)
                .with_meet(MeetKind::Named),
            "tier C1 at 34.5 with the eight indices",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Your first three tiers",
            "A girdle, a pavilion and a crown: the frame of a round brilliant.",
        )
        .actions([
            "Add Tier writes a new row; Save Tier, shown when you pick a row, changes that row.",
            "Press Ctrl+Z to undo a tier you did not mean to add.",
            "Continue with Edit a tier, Index shorthands and Quick add.",
        ])
        .why("Every control is unlocked again. Reopen this lesson any time from the tutorial browser."),
    )
}

fn edit_a_tier() -> Guide {
    Guide::new(
        "tiers-edit-a-tier",
        "Edit a tier",
        "Select a row, change its angle, name and Meets in the Tier tab, save, and undo.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Change an angle",
            "Picking a row loads its tier into the Tier tab. Save Tier writes your changes back to that same row.",
        )
        .actions([
            "Click the Pavilion Main row in the tier table.",
            "In the Tier tab, change Angle (deg) to 41.",
            "Click Save Tier.",
        ])
        .check("the Pavilion Main row shows 41.00 in the ANGLE column.")
        .why("The top of the form says Edit Tier with the row number, so you know which row Save Tier will change. It never adds a new row.")
        .goal(Goal::tier("Pavilion Main", -41.0), "Pavilion Main at 41.0")
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Rename it",
            "The name is what other tiers use to meet this one, so choose one you can type.",
        )
        .actions([
            "With the row still selected, change Name to P1.",
            "Click Save Tier.",
        ])
        .check("the row is now called P1.")
        .why("A tier that other tiers meet by name keeps working: they follow the new name in the same Undo step.")
        .goal(Goal::tier(EDITED_NAME, -41.0), "the tier renamed to P1")
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Change what it meets",
            "Meets decides how the depth is found. The teaching design pins every depth by hand; a named facet lets the solver find it.",
        )
        .actions([
            "Set Meets to Named facet(s).",
            "Facet names: Girdle",
            "Click Save Tier.",
        ])
        .check("P1's Meets column reads meets Girdle, and the status strip says: Pavilion has no anchor: add a tier with an exact scale value.")
        .why("Exact scale value states the depth outright. Named facet(s) closes this tier against the facets you name. P1 is the only tier of the pavilion, so with its scale value gone the pavilion has nothing exact to hang its depths on, and the solver says so. That message is expected here: the next step undoes the change.")
        .goal(
            Goal::tier(EDITED_NAME, -41.0).with_meet(MeetKind::Named),
            "P1 meeting a named facet",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Take it back with Undo",
            "Every edit can be undone, so trying a change costs nothing.",
        )
        .actions(["Press Ctrl+Z, or click Undo on the command bar."])
        .check("P1's Meets column shows its pinned scale value again, and the status strip no longer says the pavilion has no anchor.")
        .why("Redo (Ctrl+Y) puts the change back. Undo and Redo walk through every edit in order.")
        .goal(
            Goal::tier(EDITED_NAME, -41.0).with_meet(MeetKind::ExactScale),
            "P1 pinned to a scale value again",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Save, Add and Undo",
            "Save Tier changes the row you picked. + Add Tier starts a new row.",
        )
        .actions([
            "Click + Add Tier to start a new tier; the form title then says Add Tier.",
            "If the form holds unsaved changes when you pick another row, it asks whether to keep the draft.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn index_shorthands() -> Guide {
    Guide::new(
        "tiers-index-shorthands",
        "Index shorthands",
        "Type 0:12:96 or 12 x8 in the Indices field instead of listing every position.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::NewEmpty)
    .step(
        GuideStep::new(
            "Typing eight numbers is slow",
            "On a 96-tooth gear with 8-fold symmetry a ring has a facet every 12 teeth: 0, 12, 24 and so on up to 84. Two shorthands in the Indices field write that list for you.",
        )
        .actions([
            "Look at the Indices row of the Tier tab, and hover over its label.",
            "The next steps use each shorthand once.",
        ])
        .why("The wheel is a ring, so the gear's own tooth count (96) is the same position as 0.")
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Count with start:step:stop",
            "A range counts up from the start in steps and stops before the end, like a loop.",
        )
        .actions([
            "Click + Add Tier.",
            "Click Girdle Facet Preset (90\u{b0}, scale = 1).",
            "Name: G1",
            "Indices: 0:12:96",
            "Click Add Tier.",
        ])
        .check("G1's Indices column lists 0, 12, 24, 36, 48, 60, 72, 84.")
        .why("0:12:96 is 0, 12, 24 and so on up to 84; 96 itself is left out.")
        .goal(
            Goal::tier("G1", 90.0).with_indices(&MAIN),
            "tier G1 with the eight indices",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Repeat with a fold count",
            "A position and a fold count, like 12 x8, spread that many facets evenly around the gear.",
        )
        .actions([
            "Click + Add Tier.",
            "Angle (deg): 41",
            "Meets: Named facet(s), Facet names: G1",
            "Name: P1",
            "Indices: 12 x8",
            "Click Add Tier.",
        ])
        .check("P1 lists the same eight positions as G1.")
        .why("12 x8 starts at 12 and repeats 8 times, 96 / 8 = 12 teeth apart: 12, 24 and so on up to 84, then 96, which is 0.")
        .goal(
            Goal::tier("P1", -41.0)
                .with_indices(&MAIN)
                .with_meet(MeetKind::Named),
            "tier P1 with the eight indices",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Start a ring anywhere",
            "The same shorthand starts a ring at any position, so a ring half way between the mains is just as short to type.",
        )
        .actions([
            "Click + Add Tier.",
            "Angle (deg): 34.5",
            "Meets: Named facet(s), Facet names: G1",
            "Name: C1",
            "Indices: 6 x8",
            "Click Add Tier.",
        ])
        .check("C1 lists 6, 18, 30, 42, 54, 66, 78, 90.")
        .why("6 x8 puts the facets half way between the 12-tooth steps. Use it for star and break rings.")
        .goal(
            Goal::tier("C1", 34.5)
                .with_indices(&STAR)
                .with_meet(MeetKind::Named),
            "tier C1 on the star positions",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Mix them freely",
            "Shorthands and plain numbers can share one field, separated by commas or spaces.",
        )
        .actions([
            "For example, 0:24:96 6 x4 lists 0, 24, 48, 72 and 6, 30, 54, 78.",
            "A position off the gear, or one listed twice, is refused with the reason under the field.",
        ])
        .why("The tier form never changes a design until Add Tier or Save Tier succeeds."),
    )
}

fn facet_chips() -> Guide {
    /// The Crown Main ring after the chip lesson added the star ring to it.
    const BOTH_RINGS: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    /// The main ring without its last facet.
    const SEVEN: [f64; 7] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0];
    /// The seven after Rotate by 6 teeth.
    const ROTATED: [f64; 7] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0];
    /// The rotated seven after Mirror.
    const MIRRORED: [f64; 7] = [18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];

    Guide::new(
        "tiers-facet-chips",
        "Facet chips",
        "Add, remove, detach, rotate and mirror single facets of a tier with the chips under Facets.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Add a facet",
            "Each facet of a tier is a chip under Facets in the Tier tab. + Add puts in a new facet together with the facets the symmetry says belong with it.",
        )
        .actions([
            "Click the Crown Main row.",
            "Under Facets, type 6 in the position box.",
            "Click + Add.",
        ])
        .check("Crown Main now has 16 chips: 0, 6, 12 and so on up to 90.")
        .why("Position 6 is half way between the main facets. With 8-fold symmetry its whole ring comes with it.")
        .goal(
            Goal::tier("Crown Main", 34.5).with_indices(&BOTH_RINGS),
            "Crown Main with 16 facets",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Remove a facet",
            "Removing a facet from a complete ring takes the whole ring with it, so the symmetry stays true.",
        )
        .actions(["On the chip for position 6, click the x."])
        .check("the chips are back to 0, 12, 24 and so on up to 84.")
        .why("To take out one facet only, detach it first, as the next step shows. Undo brings a ring back.")
        .goal(
            Goal::tier("Crown Main", 34.5).with_indices(&MAIN),
            "Crown Main back to eight facets",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Detach one facet",
            "A detached facet is hand-managed: the symmetry no longer carries it along, so it can be changed on its own.",
        )
        .actions(["On the chip for position 84, click the detach icon."])
        .check("the chip for 84 looks dimmer than the others.")
        .why("Detach only marks the facet. Nothing moves until you remove or rotate something.")
        .goal(
            check("Crown Main has the facet at 84 detached", |ctx| {
                detached_is(ctx.design, "Crown Main", &[84.0])
            }),
            "the facet at 84 detached",
        )
        .highlight("inspector_tier")
        .allow(FORM_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Remove only that facet",
            "The detached facet leaves its ring behind, so removing it takes out just that one.",
        )
        .actions(["On the chip for position 84, click the x."])
        .check("Crown Main has seven chips: 0, 12 and so on up to 72.")
        .why("A ring with a gap is a real design too, for example a facet left out on purpose.")
        .goal(
            Goal::tier("Crown Main", 34.5).with_indices(&SEVEN),
            "Crown Main with seven facets",
        )
        .highlight("inspector_tier")
        .allow(FORM_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Rotate the tier",
            "Rotate moves every facet of the tier up or down by a number of teeth.",
        )
        .actions([
            "Type 6 in the teeth box below the chips.",
            "Click the first Rotate button, the one that moves facets up to higher index numbers.",
        ])
        .check("the chips read 6, 18, 30 and so on up to 78.")
        .why("Use it to move a break ring half a step, or to line a ring up with another.")
        .goal(
            Goal::tier("Crown Main", 34.5).with_indices(&ROTATED),
            "the ring rotated by 6 teeth",
        )
        .highlight("inspector_tier")
        .allow(FORM_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Mirror the tier",
            "Mirror reflects every facet of the tier to the other side of the stone.",
        )
        .actions(["Click Mirror."])
        .check("the chips read 18, 30, 42 and so on up to 90.")
        .why("Each position becomes 96 minus itself, a reflection about position 0. It takes effect at once; Undo takes it back.")
        .goal(
            Goal::tier("Crown Main", 34.5).with_indices(&MIRRORED),
            "the ring mirrored",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
}

fn quick_add() -> Guide {
    Guide::new(
        "tiers-quick-add",
        "Quick add",
        "Add a girdle, a table and a culet with one click each from the Quick add buttons.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::NewEmpty)
    .step(
        GuideStep::new(
            "Quick add a girdle",
            "The Quick add buttons under the tier table's toolbar add a common first tier with one click.",
        )
        .actions(["Click Girdle next to Quick add."])
        .check("a Girdle row with G1 in the CODE column, pinned to scale 1.")
        .why("Quick add pins the tier to an exact scale value, so the design has something to solve from. 90 degrees is a girdle.")
        .goal(
            Goal::tier("Girdle", 90.0).with_meet(MeetKind::ExactScale),
            "a Girdle tier at 90.0",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Quick add a table",
            "The table is the flat facet on top of the crown.",
        )
        .actions(["Click Table next to Quick add."])
        .check("a Table row at 0.00, pinned to scale 0.32.")
        .why("A table is a crown facet at 0 degrees. Change its scale value in the Tier tab when you know the size you want.")
        .goal(
            Goal::tier("Table", 0.0).with_meet(MeetKind::ExactScale),
            "a Table tier at 0.0",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Quick add a culet",
            "The culet is the small flat facet at the bottom of the pavilion.",
        )
        .actions(["Click Culet next to Quick add."])
        .check("a Culet row with Culet in the CODE column, pinned to scale 0.88.")
        .why("The culet is also 0 degrees, but it belongs to the pavilion and not the crown: the table's code is T, the culet's is Culet, and the minus sign on its zero angle puts it on the pavilion side.")
        .goal(
            Goal::All(vec![
                Goal::tier("Culet", 0.0).with_meet(MeetKind::ExactScale),
                check("the Culet is on the pavilion side", |ctx| {
                    tier_called(ctx.design, "Culet")
                        .is_some_and(|culet| culet.angle_deg.is_sign_negative())
                }),
            ]),
            "a Culet tier on the pavilion side",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Make them yours",
            "Each Quick add tier is a single facet with a starting value, not a finished design.",
        )
        .actions([
            "Pick a row and change its angle, name or scale value in the Tier tab.",
            "Add the rings around them with + Add Tier.",
        ])
        .why("Every control is unlocked again."),
    )
}

fn inline_angle() -> Guide {
    Guide::new(
        "tiers-inline-angle",
        "Inline angle and nudging",
        "Edit an angle right in the tier table, and nudge it with the keys and the mouse wheel.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Edit an angle in the table",
            "The ANGLE column is editable in place, with no trip to the Tier tab.",
        )
        .actions([
            "Double-click the ANGLE cell of the Pavilion Main row, or pick the row and press F2.",
            "Type 41 and press Enter.",
        ])
        .check("the Pavilion Main row shows 41.00.")
        .why("Enter or clicking away keeps the edit; Escape throws it away. The facet keeps its side of the stone: you type the plain number and a pavilion row stays a pavilion row.")
        .goal(Goal::tier("Pavilion Main", -41.0), "Pavilion Main at 41.0")
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Nudge with the keys",
            "While a cell is open for editing, the arrow keys change the angle a little at a time.",
        )
        .actions([
            "Double-click the ANGLE cell of the Crown Main row.",
            "Press Up a few times: each press adds 0.1 degrees. Shift adds 1, Ctrl adds 0.01.",
            "Press Enter.",
        ])
        .check("the Crown Main angle is at least 0.2 degrees above 34.50.")
        .why("The stone updates as you press, and a run of presses is one Undo step.")
        .goal(
            check("Crown Main is at least 0.2 degrees above 34.5", |ctx| {
                tier_called(ctx.design, "Crown Main").is_some_and(|tier| tier.angle_deg >= 34.7 - 1e-9)
            }),
            "Crown Main nudged up",
        )
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Nudge with the mouse wheel",
            "Scrolling over an ANGLE cell nudges it too, with Ctrl held when the cell is not open.",
        )
        .actions([
            "Hold Ctrl and scroll over the Crown Main ANGLE cell.",
            "Each notch moves it by 0.1 degrees. Scroll until it reads 34.50 again.",
        ])
        .check("the Crown Main angle reads 34.50.")
        .why("Without Ctrl the wheel scrolls the table as usual, so a stray scroll never changes an angle.")
        .goal(Goal::tier("Crown Main", 34.5), "Crown Main back at 34.5")
        .highlight("tier_table")
        .allow(TABLE),
    )
    .step(
        GuideStep::new(
            "Undo a run of nudges",
            "Nudges that follow each other quickly merge into one Undo step.",
        )
        .actions([
            "Press Ctrl+Z once to step back through a run of nudges in one go.",
            "Press Ctrl+Y to put it back.",
        ])
        .why("Every control is unlocked again."),
    )
}
