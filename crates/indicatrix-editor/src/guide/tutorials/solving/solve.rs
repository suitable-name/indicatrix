//! Solve, auto-solve, reading the solver status, the overall verdict and its Fix buttons, the
//! Preform tab's proportions, and the yield and carat weight.

use super::{
    EDIT, EDIT_ADVANCED, EDIT_ADVANCED_SOLVE, EDIT_SOLVE, FIX, PREFORM, RICH, RICH_CROWN, SETTINGS,
    SOLVE, STANDARD, check, event, events, solved_by_button, tier_called,
};
use crate::guide::{Goal, GoalContext, Guide, GuideCategory, GuideStep, MeetKind, StartingState};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        solve(),
        auto_solve(),
        status(),
        verdict(),
        verdict_fixes(),
        preform(),
        yield_and_carat(),
    ]
}

/// The crown main angle the lessons that need an edit move it to.
const EDITED_CROWN: f64 = 36.0;

/// Whether no tier is called "Table".
fn table_is_gone(ctx: &GoalContext<'_>) -> bool {
    tier_called(ctx.design, "Table").is_none()
}

/// Whether the crown main ring has an index position between two gear teeth.
fn crown_is_off_gear(ctx: &GoalContext<'_>) -> bool {
    tier_called(ctx.design, "Crown Main").is_some_and(|tier| {
        tier.indices
            .iter()
            .any(|index| (index - index.round()).abs() > 1e-9)
    })
}

/// Whether every index position of the crown main ring is on a gear tooth.
fn crown_is_on_gear(ctx: &GoalContext<'_>) -> bool {
    tier_called(ctx.design, "Crown Main").is_some_and(|tier| {
        !tier.indices.is_empty()
            && tier
                .indices
                .iter()
                .all(|index| (index - index.round()).abs() <= 1e-9)
    })
}

/// Whether the rough's half-width is 1.8, the value the Preform lesson asks for.
fn rough_is_wider(ctx: &GoalContext<'_>) -> bool {
    (ctx.design.preform.half_width - 1.8).abs() < 0.005
}

fn solve() -> Guide {
    Guide::new(
        "solving-solve",
        "Solve",
        "Edit a design, see it marked not solved, press Solve (or F5) and read the result.",
        GuideCategory::Solving,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "What Solve does",
            "An edit only marks the design as not solved. Solve works out the depth of every tier and builds the stone to check that it closes.",
        )
        .actions([
            "Find the Solve button at the left of the command bar. F5 does the same.",
            "Find the status strip under the tier table: one dot and one line say where the design stands.",
        ])
        .why("Solve is the one action that asks the solver for every mast, the depth of each tier. Everything else, such as Save Tier, Remove or Undo, skips it and marks the design not solved instead, so a slow solve never freezes a typing session.")
        .highlight("solve_button")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Turn auto-solve off",
            "The Auto-solve list can solve for you after every edit. For this lesson switch it off, so you can watch what Solve does.",
        )
        .actions([
            "Open the Auto-solve list next to the Solve button and choose Off.",
            "If it already says Off, choose 1 s and then Off again.",
        ])
        .check("the Auto-solve list reads Off.")
        .why("With auto-solve on, a design that solves quickly is solved again by itself a moment after every edit, so you would never see it marked not solved. The last step of this lesson says how to put it back.")
        .goal(event(events::AUTO_SOLVE_OFF), "the Auto-solve list set to Off")
        .highlight("auto_solve")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Edit the design",
            "Change one angle and watch the design lose its solved state.",
        )
        .actions([
            "Click the Crown Main row in the tier table.",
            "In the Tier tab, change Angle (deg) to 36.",
            "Click Save Tier.",
        ])
        .check("the status strip reads: Not solved -- click Solve to compute masts and validate this design.")
        .why("The MAST and SOLVE columns of the tier table show placeholders instead of numbers that might no longer be true, and the viewport keeps the last solved stone rather than a half-updated one.")
        .goal(
            Goal::tier("Crown Main", EDITED_CROWN),
            "Crown Main at 36.0",
        )
        .highlight("inspector_tier")
        .allow(EDIT),
    )
    .step(
        GuideStep::new(
            "Press Solve",
            "Solve the edited design and see whether it still closes.",
        )
        .actions(["Click Solve on the command bar, or press F5."])
        .check("the strip reads Closed solid with a volume, and its state word says Solved.")
        .why("Above 16 tiers Solve runs in the background: the button reads Solving..., the strip counts the seconds, and you can keep editing meanwhile. An edit that lands before it finishes discards its result.")
        .goal(solved_by_button(), "a solve that closes the stone")
        .highlight("solve_button")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Read the result",
            "A solved design shows real numbers again, and the strip says what Solve found.",
        )
        .actions([
            "The dot is green and the line reads Closed solid and the volume, in the app's model units cubed.",
            "The small state word at the right of the strip says Solved. The other words are Stale, Solving and Failed.",
            "The MAST and SOLVE columns of the tier table show numbers again.",
            "Click the link at the right end of the strip to see every current message in full.",
        ])
        .why("Closed solid means the facets enclose a finite stone with no gaps. The state word is only a summary: read the line for the actual cause when a solve fails (see Reading the solver status).")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Put auto-solve back",
            "Choose the setting you want to work with.",
        )
        .actions([
            "Open the Auto-solve list. 300 ms is the usual choice: a small design then solves itself shortly after you stop typing.",
            "Choose Off if you would rather press Solve yourself.",
        ])
        .why("The setting is remembered across sessions. See the Auto-solve lesson for how the time limit works.")
        .highlight("auto_solve")
        .allow(SOLVE),
    )
}

fn auto_solve() -> Guide {
    Guide::new(
        "solving-auto-solve",
        "Auto-solve",
        "Let a design solve itself after an edit: the Off, 150 ms, 300 ms, 1 s and 3 s settings and what they mean.",
        GuideCategory::Solving,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "What auto-solve does",
            "After an edit, a small design can solve itself, so you skip the click on Solve.",
        )
        .actions([
            "Find the Auto-solve list next to the Solve button: Off, 150 ms, 300 ms, 1 s and 3 s.",
            "After an edit, if this design's last solve took less than the time you chose, a new solve starts by itself after a short pause.",
            "The pause lets you finish typing, so a burst of keystrokes causes one solve, not one for every key.",
        ])
        .why("The time is a ceiling, not a promise. It is judged against this design's own last solve. A fresh design has no measurement yet, so auto-solve is tried at first.")
        .highlight("auto_solve")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Choose Off",
            "Start with auto-solve off to see what it replaces.",
        )
        .actions([
            "Open the Auto-solve list and choose Off.",
            "If it already says Off, choose 1 s and then Off again.",
        ])
        .check("the Auto-solve list reads Off.")
        .goal(event(events::AUTO_SOLVE_OFF), "the Auto-solve list set to Off")
        .highlight("auto_solve")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Edit while it is off",
            "With auto-solve off, an edit only marks the design as not solved.",
        )
        .actions([
            "Click the Crown Main row in the tier table.",
            "In the Tier tab, change Angle (deg) to 36.",
            "Click Save Tier.",
        ])
        .check("the status strip reads Not solved and stays that way until you press Solve.")
        .goal(
            Goal::tier("Crown Main", EDITED_CROWN),
            "Crown Main at 36.0",
        )
        .highlight("inspector_tier")
        .allow(EDIT),
    )
    .step(
        GuideStep::new(
            "Choose 300 ms",
            "Now let the design solve itself.",
        )
        .actions(["Open the Auto-solve list and choose 300 ms."])
        .check("the Auto-solve list reads 300 ms.")
        .why("Off means edits only ever mark the design not solved, exactly as the program always did. Any other setting turns the automatic solve on for designs that solve within that time.")
        .goal(event(events::AUTO_SOLVE_ON), "the Auto-solve list set to a time")
        .highlight("auto_solve")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Edit and watch it solve itself",
            "Put the angle back and do not press Solve.",
        )
        .actions([
            "Click the Crown Main row, change Angle (deg) to 34.5 and click Save Tier.",
            "Wait a moment without pressing anything.",
        ])
        .check("the strip goes from Not solved to Closed solid on its own.")
        .why("If the solve does not start, auto-solve has switched itself off for this design (see the next step). Press Solve to finish the step.")
        .goal(
            Goal::All(vec![
                Goal::tier("Crown Main", RICH_CROWN),
                Goal::SolvedClosed,
            ]),
            "Crown Main at 34.5 and the stone solved",
        )
        .highlight("inspector_tier")
        .allow(EDIT_SOLVE),
    )
    .step(
        GuideStep::new(
            "When auto-solve gives up",
            "A slow design is not solved after every edit.",
        )
        .actions([
            "Once a design's own solves take longer than the time you chose, auto-solve switches itself off for that design.",
            "The strip says so: Auto-solve off for this design: last solve took 5.9s.",
            "Solve still works by hand; only the automatic trigger stops.",
        ])
        .why("A long solve after every keystroke would freeze the editor. Choose a longer time (1 s or 3 s) if you would like auto-solve on a bigger design. The setting is remembered across sessions.")
        .highlight("auto_solve")
        .allow(SOLVE),
    )
}

fn status() -> Guide {
    Guide::new(
        "solving-status",
        "Reading the solver status",
        "Read the status strip after a good solve and a failed one, fix a missing anchor, and learn what Abandon does.",
        GuideCategory::Solving,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Solve a sound design",
            "First see what success looks like.",
        )
        .actions(["Click Solve on the command bar, or press F5."])
        .check("the strip reads Closed solid with a volume.")
        .goal(solved_by_button(), "a solve that closes the stone")
        .highlight("solve_button")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Read the status strip",
            "The strip is one dot, one line and a link to the full list of messages, and the solver's own state always leads.",
        )
        .actions([
            "The dot is green for a closed stone and red for a problem.",
            "The line says which: Closed solid, or the reason a solve failed.",
            "The small state word at the right is coarser: Stale, Solving, Failed or Solved.",
            "Click the link at the right end of the strip to see every current message in full.",
        ])
        .why("Only one line shows at a time, in this order: Solving..., a real problem (a missing anchor, Degenerate, Unbounded, or the Not solved marker), manufacturability warnings, a rough-fit warning, then Closed solid. A running or failed solve can never hide behind something less important.")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Take away the girdle's anchor",
            "Break the design on purpose to see the commonest failure.",
        )
        .actions([
            "Click the Girdle row in the tier table.",
            "In the Tier tab, set Meets to Unspecified vertex.",
            "Click Save Tier.",
        ])
        .check("the Girdle row no longer shows an exact scale value.")
        .why("Every block, crown, pavilion and girdle, needs one tier with an exact scale value to hang its depths on. The Girdle is the only tier of its block, so now that block has no anchor.")
        .goal(
            Goal::tier("Girdle", 90.0).with_meet(MeetKind::Unspecified),
            "the Girdle meeting an unspecified vertex",
        )
        .highlight("inspector_tier")
        .allow(EDIT_ADVANCED),
    )
    .step(
        GuideStep::new(
            "Solve the broken design",
            "Press Solve and read what a failure says.",
        )
        .actions(["Click Solve on the command bar, or press F5."])
        .check("the dot is red, the state word says Failed and the line reads: Girdle has no anchor: add a tier with an exact scale value.")
        .why("Each missing block gets its own sentence. The two other failures are Degenerate (the facets bound a region that is not a valid solid, with the likely tiers named) and Unbounded (a facet never closes the stone off). Fix the tiers they name.")
        .goal(event(events::SOLVE_REQUESTED), "a solve of the broken design")
        .highlight("solve_button")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Fix it and solve again",
            "Give the girdle its anchor back.",
        )
        .actions([
            "Click the Girdle row, set Meets back to Exact scale value and type 1 in the box beside it.",
            "Click Save Tier. Pressing Ctrl+Z to undo the edit works too.",
            "Click Solve.",
        ])
        .check("the strip reads Closed solid again.")
        .why("The tier table's Add Anchor button on a block's rows does the same for a block that has none.")
        .goal(
            Goal::All(vec![
                Goal::tier("Girdle", 90.0).with_meet(MeetKind::ExactScale),
                event(events::SOLVE_REQUESTED),
                Goal::SolvedClosed,
            ]),
            "the Girdle anchored again and the stone solved",
        )
        .highlight("solve_button")
        .allow(EDIT_ADVANCED_SOLVE),
    )
    .step(
        GuideStep::new(
            "Abandon, chips and warnings",
            "Three more things the strip tells you.",
        )
        .actions([
            "Above 16 tiers Solve runs in the background. The button reads Solving... and an Abandon Solve button appears beside it. Abandon stops waiting and ignores the result; the strip goes back to Stale.",
            "Every task longer than about a fifth of a second (Deep Solve, Optimize, a sweep) shows as a small chip with its running time, and a cancel button where it can really be stopped.",
            "Manufacturability warnings, such as a facet cut away entirely or an index between two gear teeth, show in the strip and in the full list of messages. They never block Solve.",
        ])
        .why("A design with a warning still solves and still exports. The warnings are worth a look before you cut a stone for real; the overall verdict lesson shows how to fix several of them with one click.")
        .allow(SOLVE),
    )
}

fn verdict() -> Guide {
    Guide::new(
        "solving-verdict",
        "The overall verdict",
        "Read the Good, Check or Problem badge, open its reasons and see how a lower-index material changes it.",
        GuideCategory::Solving,
    )
    .starting(StartingState::Template(STANDARD))
    .step(
        GuideStep::new(
            "Solve the design",
            "The verdict is worked out from a solve, so press Solve first.",
        )
        .actions(["Click Solve on the command bar, or press F5."])
        .check("the strip reads Closed solid.")
        .why("The verdict is recomputed whenever a solve lands: the button, auto-solve or a finished preview update. It never solves on its own.")
        .goal(solved_by_button(), "a solve that closes the stone")
        .highlight("solve_button")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Find the verdict badge",
            "One small badge sums up the whole design.",
        )
        .actions([
            "Look at the left end of the status strip, beside the solver state.",
            "The badge shows one word: Good, Check or Problem. This stone reads Check, not Good: the template's culet is a pin-prick on purpose, too small to polish.",
            "Hover over it for the verdict as a plain sentence. Here it counts the things to look at, as in: Check 3 things. A stone with nothing to report reads: Looks good: closes, no warnings.",
        ])
        .why("Good means the design closes, no manufacturability warning applies, no pavilion facet lets light out of the bottom and no proportion is outside its usual range. Check means it can be cut but something deserves a look. Problem means it does not close, or an optical figure is far past its limit.")
        .highlight("verdict_badge")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Open the reasons",
            "The word is a summary; the list behind it says what to look at.",
        )
        .actions(["Click the badge, or Tab to it and press Enter."])
        .check("a list of reasons, worst first, each with a Show button and, where a safe tool exists, a Fix button. One reason is about the Culet being too small to polish: it has Show but no Fix.")
        .why("Esc closes the list. If the design has nothing to report, the list says so.")
        .goal(event(events::VERDICT_OPENED), "the verdict's reasons opened")
        .highlight("verdict_badge")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Read the reasons",
            "Every reason is one plain sentence, worst first.",
        )
        .actions([
            "Show selects the tier the sentence is about, exactly as clicking its row would.",
            "Fix appears only where a safe tool exists. The next lesson uses it.",
            "A reason with no Fix, such as a proportion outside its usual range or an unsafe meet name, is yours to change: edit the design, then look at the verdict again.",
        ])
        .why("After any edit the badge stays visible but dimmed, and its Fix buttons wait, until the next solve brings the new verdict.")
        .highlight("verdict_badge")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Make the verdict worse",
            "Cut the same stone in a material with a lower refractive index and watch it start to leak light.",
        )
        .actions([
            "Open the Design Settings panel above the tier table.",
            "Set Material to Opal. Its refractive index, 1.45, is lower than most gems.",
            "Click Apply Material.",
        ])
        .check("the Design Settings panel shows Opal and its critical angle, and Apply Material no longer shows an asterisk.")
        .why("A lower index means a larger critical angle: a pavilion facet that was steep enough to keep light inside in diamond now lets it out. That shows as windowing.")
        .goal(Goal::Material("Opal".to_owned()), "the material set to Opal")
        .highlight("design_settings")
        .allow(SETTINGS),
    )
    .step(
        GuideStep::new(
            "Look again",
            "Solve the design in its new material and read the verdict again.",
        )
        .actions([
            "Click Solve, or wait for auto-solve.",
            "Click the verdict badge.",
        ])
        .check("the badge still says Check (or Problem, if the measured windowing is above 30 %), and a new reason, besides the culet's, names the pavilion facets that let light out.")
        .why("Windowing reasons are listed tier by tier, at most four, with a Steepen button on each pavilion facet that can be fixed. If the measured windowing is above 30 % the badge says Problem instead of Check.")
        .goal(
            Goal::All(vec![event(events::VERDICT_OPENED), Goal::SolvedClosed]),
            "the new verdict opened",
        )
        .highlight("verdict_badge")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Where the limits come from",
            "The optical limits are named constants, and some are only starting points.",
        )
        .actions([
            "Windowing, light straight out of the bottom: Check above 15 %, Problem above 30 %.",
            "Extinction, light lost inside: Check above 40 %, Problem above 60 %.",
            "Brilliance, light back to the eye with the table up: Check below 25 %, Problem below 10 %.",
            "They are measured only when the stone closes and the design names a material.",
        ])
        .why("Trust the windowing limits most. The extinction and brilliance limits are loose starting points that flag a stone that is plainly poor, not a way to rank good ones. Press Ctrl+Z to put the material back.")
        .allow(SOLVE),
    )
}

fn verdict_fixes() -> Guide {
    Guide::new(
        "solving-verdict-fixes",
        "The verdict's Fix buttons",
        "Use the verdict's Fix buttons: add a missing table and snap an index to the gear, each as one undo step.",
        GuideCategory::Solving,
    )
    .starting(StartingState::Template(STANDARD))
    .step(
        GuideStep::new(
            "Solve the design",
            "Start from a solved, sound stone.",
        )
        .actions(["Click Solve on the command bar, or press F5."])
        .check("the strip reads Closed solid.")
        .goal(solved_by_button(), "a solve that closes the stone")
        .highlight("solve_button")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Take the table away",
            "A crown with no table is a reason the verdict can fix.",
        )
        .actions([
            "Click the Table row in the tier table.",
            "Click Delete on the command bar.",
        ])
        .check("the Table row is gone from the tier table.")
        .why("You can undo it. The stone still closes: without a flat top the crown mains meet in a point.")
        .goal(check("no tier called Table", table_is_gone), "the Table tier removed")
        .highlight("tier_table")
        .allow(FIX),
    )
    .step(
        GuideStep::new(
            "Solve again",
            "The verdict is worked out again from each solve.",
        )
        .actions(["Click Solve, or wait for auto-solve."])
        .check("the strip reads Closed solid and the verdict badge is no longer dimmed.")
        .goal(Goal::SolvedClosed, "the edited stone solved")
        .highlight("solve_button")
        .allow(SOLVE),
    )
    .step(
        GuideStep::new(
            "Add a table with a Fix",
            "Open the verdict and let it put the table back.",
        )
        .actions([
            "Click the verdict badge.",
            "Find the reason that says the crown has no table and click Add a table beside it.",
        ])
        .check("a Table row is back at the end of the tier table, and a message says what was added.")
        .why("A Fix is planned on a copy of the design first and refused, with a plain sentence and no change, when it cannot be made safely. When it goes ahead it is one undo step: a single Ctrl+Z puts everything back.")
        .goal(Goal::TierExists("Table".to_owned()), "a Table tier")
        .highlight("verdict_badge")
        .allow(FIX),
    )
    .step(
        GuideStep::new(
            "Type an index between two teeth",
            "Make a second reason for the verdict to fix.",
        )
        .actions([
            "Click the Crown Main row in the tier table.",
            "In the Indices box of the Tier tab, change 12 to 12.5.",
            "Click Save Tier.",
        ])
        .check("the Crown Main indices now include 12.5.")
        .why("A tooth of the 96-tooth gear is a whole number. A position such as 12.5 lies between two teeth and cannot be cut exactly.")
        .goal(
            check("a crown index between two teeth", crown_is_off_gear),
            "a Crown Main index between two teeth",
        )
        .highlight("inspector_tier")
        .allow(EDIT),
    )
    .step(
        GuideStep::new(
            "Snap it to the teeth",
            "Let the verdict round the position for you.",
        )
        .actions([
            "Click Solve, or wait for auto-solve, so the verdict is current.",
            "Click the verdict badge and find the reason that an index position sits between gear teeth.",
            "Click Snap to teeth beside it.",
        ])
        .check("the Crown Main indices are whole numbers again.")
        .why("Positions that become the same tooth merge into one. The change is one undo step.")
        .goal(
            check("every crown index on a gear tooth", crown_is_on_gear),
            "every Crown Main index on a gear tooth",
        )
        .highlight("verdict_badge")
        .allow(FIX),
    )
    .step(
        GuideStep::new(
            "The other three Fix buttons",
            "Three more fixes exist, for reasons this stone does not have.",
        )
        .actions([
            "Remove: after a confirmation, takes out the facets of a tier that later tiers cut away entirely (the whole tier when none is left), once it has checked that the finished stone is unchanged.",
            "Move later: moves a tier to just after the last tier it meets, so the schedule is cut in a workable order.",
            "Steepen: turns a pavilion facet that is below the critical angle to the critical angle plus 2 degrees about its girdle-side edge, so the girdle stays where it is.",
        ])
        .why("A fix is refused, with the reason and no change, when it cannot be made safely: for example a pavilion angle that follows a relation, an angle the material would need beyond 89.5 degrees, or a result that no longer solves. While a lesson locks the tier table the Fix buttons are dimmed.")
        .allow(SOLVE),
    )
}

fn preform() -> Guide {
    Guide::new(
        "solving-preform",
        "The Preform tab",
        "Read the stone's proportions, change the rough's size with Apply Preform and solve again.",
        GuideCategory::Solving,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Open the Preform tab",
            "The Preform tab describes the whole design, not one tier.",
        )
        .actions(["Click Preform in the inspector below the tier table."])
        .check("the tab shows Preform, Proportions and Yield.")
        .goal(Goal::InspectorTab(1), "the Preform tab open")
        .highlight("preform_tab")
        .allow(PREFORM),
    )
    .step(
        GuideStep::new(
            "Read the proportions",
            "These are the figures a cutter quotes, worked out from the last solve.",
        )
        .actions([
            "Table and L/W are on one line, then Crown Height, Pavilion Depth, Total Depth and Girdle Thickness.",
            "Table %, Crown Angle, Pavilion Angle, Total Depth % and Girdle % carry a chip: Within, Near or Outside a reference window.",
            "A dash means the design does not solve now, or has no girdle to measure from.",
        ])
        .why("The windows depend on the design's shape and material. They are guidance, not a grade: a chip reading Outside is a prompt to look closer, not a verdict on the design's worth.")
        .highlight("preform_tab")
        .allow(PREFORM),
    )
    .step(
        GuideStep::new(
            "Make the rough wider",
            "The preform is the rough the stone is cut from.",
        )
        .actions([
            "In the Preform group, change Half-Width from 1.5 to 1.8. The slider under it works too.",
            "Click Apply Preform. While changes wait, the button shows an asterisk.",
        ])
        .check("Half-Width reads 1.8 and the Apply Preform button no longer has an asterisk.")
        .why("The stone's girdle half-width is 1 in these units, so a rough is never narrower than 1. A slider only fills its field in: nothing changes in the design until you click Apply Preform. Every number box also takes a calculation such as 1.5+0.3.")
        .goal(
            check("a rough half-width of 1.8", rough_is_wider),
            "the preform's Half-Width at 1.8",
        )
        .highlight("preform_tab")
        .allow(PREFORM),
    )
    .step(
        GuideStep::new(
            "Solve and read again",
            "The proportions describe the finished stone, so they come from a solve.",
        )
        .actions([
            "Click Solve, or wait for auto-solve.",
            "Read the Proportions group again.",
        ])
        .check("the proportions show numbers again after the solve.")
        .why("Apply Preform, like every edit, marks the design not solved, and the proportions read a dash until the next solve rather than showing figures that might be out of date.")
        .goal(Goal::SolvedClosed, "the design solved")
        .highlight("solve_button")
        .allow(PREFORM),
    )
    .step(
        GuideStep::new(
            "Block, Cylinder and the Simple interface",
            "A few more things the tab offers.",
        )
        .actions([
            "The rough's shape is Block or Cylinder. Both ranges are the same, because both have to enclose the same stone.",
            "The Simple interface keeps the shape, Half-Width, Length / Width, Depth, the proportions and the Girdle Diameter. The Girdle Y-Offset and the Specific Gravity Override are Advanced; they keep their values.",
        ])
        .why("Switch to Advanced in Preferences to see them. The Yield group has its own lesson.")
        .allow(PREFORM),
    )
}

fn yield_and_carat() -> Guide {
    Guide::new(
        "solving-yield-carat",
        "Yield and carat weight",
        "Set the material and the girdle diameter, apply the yield inputs, solve and read the yield and carat weight.",
        GuideCategory::Solving,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Choose the material",
            "A weight needs a density, and the density comes from the material.",
        )
        .actions([
            "Open the Design Settings panel above the tier table.",
            "Set Material to Quartz.",
            "Click Apply Material.",
        ])
        .check("the panel shows Quartz, and Apply Material no longer shows an asterisk.")
        .why("Quartz has a specific gravity of 2.65. A design with no material has no density to estimate a weight from, unless you type a Specific Gravity Override.")
        .goal(Goal::Material("Quartz".to_owned()), "the material set to Quartz")
        .highlight("design_settings")
        .allow(SETTINGS),
    )
    .step(
        GuideStep::new(
            "Open the Preform tab",
            "The Yield group sits at the bottom of the Preform tab.",
        )
        .actions(["Click Preform in the inspector below the tier table."])
        .check("the tab shows Preform, Proportions and Yield, with Yield at the bottom.")
        .goal(Goal::InspectorTab(1), "the Preform tab open")
        .highlight("preform_tab")
        .allow(PREFORM),
    )
    .step(
        GuideStep::new(
            "Give the stone a size",
            "The design is drawn in model units. A real size in millimetres turns them into a weight.",
        )
        .actions([
            "In the Yield group, set Girdle Diameter (mm) to 8.",
            "Click Apply Yield Inputs.",
        ])
        .check("Apply Yield Inputs is no longer highlighted.")
        .why("Eff. RI and its source are shown above the fields. Specific Gravity Override takes a number or a calculation and has no slider, because it is measured or looked up, not tuned. Leave it blank to use the material's own.")
        .goal(Goal::YieldApplied, "a girdle diameter applied")
        .highlight("preform_tab")
        .allow(PREFORM),
    )
    .step(
        GuideStep::new(
            "Solve and read the yield",
            "The results are blank until the next solve.",
        )
        .actions([
            "Click Solve, or wait for auto-solve.",
            "Read Volumetric Yield, Est. Carat Weight and Specific Gravity Used in the Yield group.",
        ])
        .check("the three lines show numbers instead of a dash.")
        .why("Volumetric Yield is the finished stone's volume as a share of the rough's. Est. Carat Weight is the volume at the girdle diameter you gave, times the specific gravity.")
        .goal(Goal::SolvedClosed, "the design solved")
        .highlight("solve_button")
        .allow(PREFORM),
    )
    .step(
        GuideStep::new(
            "Where yield is used",
            "The yield is not only a number to read.",
        )
        .actions([
            "The Optimize tab's Keep weight objective favours angles that waste less of the rough.",
            "The angle sweep table has a Yield % column.",
            "The Rough Planner uses the carat estimate to fit stones into a rough.",
        ])
        .why("A smaller Y-Offset or a tighter preform changes the yield without changing the stone: try both, and solve after each.")
        .allow(PREFORM),
    )
}
