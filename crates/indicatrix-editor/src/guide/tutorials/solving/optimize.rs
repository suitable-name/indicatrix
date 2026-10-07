//! Deep Solve, the Optimize tab, Retarget for a new material (Shift and Optimize) and the angle
//! sweep.

use super::{
    RICH, RICH_PAVILION, SETTINGS, STANDARD, TOOLS, check, event, events, tier_at, tier_called,
};
use crate::{
    guide::{Goal, GoalContext, Guide, GuideCategory, GuideStep, StartingState},
    templates::template_spec,
};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![
        deep_solve(),
        optimize(),
        retarget_shift(),
        retarget_optimize(),
        sweep(),
    ]
}

/// Whether the Rich Teaching Design's pavilion mains have moved from where the template put them.
fn pavilion_has_moved(ctx: &GoalContext<'_>) -> bool {
    tier_called(ctx.design, "Pavilion Main")
        .is_some_and(|tier| (tier.angle_deg - RICH_PAVILION).abs() > 0.05)
}

/// Whether the Standard Round Brilliant's tiers have an angle the template did not give them.
fn angles_differ_from_standard(ctx: &GoalContext<'_>) -> bool {
    let Some(spec) = template_spec(i32::try_from(STANDARD).unwrap_or(1)) else {
        return false;
    };
    let original = spec.tiers();
    original.len() == ctx.design.tiers.len()
        && original
            .iter()
            .zip(&ctx.design.tiers)
            .any(|(was, now)| (was.angle_deg - now.angle_deg).abs() > 0.01)
}

/// Whether the Rich Teaching Design is back as the lessons that retarget it found it: Quartz, and
/// the pavilion mains where the template put them.
fn retarget_is_undone(ctx: &GoalContext<'_>) -> bool {
    ctx.design
        .material
        .name
        .as_deref()
        .is_some_and(|name| name.eq_ignore_ascii_case("Quartz"))
        && tier_at(ctx.design, "Pavilion Main", RICH_PAVILION)
}

/// Whether the Rich Teaching Design's pavilion mains are where the template put them.
fn pavilion_is_back(ctx: &GoalContext<'_>) -> bool {
    tier_at(ctx.design, "Pavilion Main", RICH_PAVILION)
}

fn deep_solve() -> Guide {
    Guide::new(
        "optimizing-deep-solve",
        "Deep Solve",
        "Check a catalogue design's solved stone against the proportions printed for it, and read the verdict.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::RequiresOpenDesign)
    .step(
        GuideStep::new(
            "What Deep Solve checks",
            "Deep Solve is a slow, separate check: does this design's geometry reproduce the proportions printed in the catalogue?",
        )
        .actions([
            "It compares the solved stone with the design's own printed Vol/W^3, L/W, C/W, P/W and H/W.",
            "It never changes the design. It reports, and suggests, and nothing is written back unless you Pin a tier it names.",
            "It needs a design with printed proportions: one loaded from the library. A new design or a template has none, and the button is dim. Hover the button to see why.",
            "It also has nothing to repair while every tier is still pinned to its recorded mast: Adopt a tier first (the tier table's IMPORTED column).",
        ])
        .why("If the button is dim, use Skip step: this lesson only reads the result of a run. Load a catalogue design from the Library, adopt a tier, and start the lesson again to run it.")
        .highlight("deep_solve_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Run Deep Solve",
            "Start the check.",
        )
        .actions([
            "Click Deep Solve on the command bar's second row.",
            "The button turns into Cancel Deep Solve, and the status strip shows that Deep Solve is running.",
        ])
        .check("the button reads Cancel Deep Solve.")
        .why("It runs off the main thread, so you can keep working. On the program's own reference designs it tried about 68 solves on average, which can mean minutes on a large design. Cancel stops waiting for it; the work itself finishes in the background and its result is discarded.")
        .goal(event(events::DEEP_SOLVE_STARTED), "a Deep Solve run started")
        .highlight("deep_solve_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Wait for the verdict",
            "Let it finish.",
        )
        .actions([
            "Wait until the button reads Deep Solve again.",
            "Keep working elsewhere meanwhile if you like, but do not edit the design: a result for an edited design is out of date.",
        ])
        .check("the full list of messages shows Deep Solve's verdict.")
        .goal(event(events::DEEP_SOLVE_FINISHED), "the Deep Solve run to finish")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Read the verdict",
            "The verdict says whether the stone matches the printed figures, and what the search had to change.",
        )
        .actions([
            "ACCEPTED means the stone reproduces the printed figures to verification accuracy: a strong outside signal, not a proof. Not accepted means it still deviates.",
            "The report lists the initial score, the score after calibration, the final score, whether it was accepted, how many overrides and anchor moves it applied, and how many full solves it tried.",
            "Click the link at the right end of the status strip to open the full list of messages. A tier whose mast Deep Solve adjusted has its own Pin button: it makes that tier an exact scale value at the verified mast, as one undo step.",
        ])
        .why("If the design was edited since it was loaded, the log adds that the printed figures may no longer describe the design you are holding. A Pin on a run that is out of date does nothing but tell you to run Deep Solve again.")
        .allow(TOOLS),
    )
}

fn optimize() -> Guide {
    Guide::new(
        "optimizing-optimize",
        "Optimize",
        "Search for better angles: choose an objective, look at the ranges, run a short search, pick a candidate and apply it.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(STANDARD))
    .step(
        GuideStep::new(
            "Choose the material",
            "Optimize scores the stone in its own material, so name one first.",
        )
        .actions([
            "Open the Design Settings panel above the tier table.",
            "Set Material to Quartz.",
            "Click Apply Material.",
        ])
        .check("the panel shows Quartz, and Apply Material no longer shows an asterisk.")
        .goal(Goal::Material("Quartz".to_owned()), "the material set to Quartz")
        .highlight("design_settings")
        .allow(SETTINGS),
    )
    .step(
        GuideStep::new(
            "Open the Optimize tab",
            "Everything about a search is on the inspector's Optimize tab: what to favour, what may change, the run and the results.",
        )
        .actions(["Click Optimize in the inspector below the tier table."])
        .check("the tab shows Objective, What may change, Run and Candidates.")
        .why("The Optimize button on the command bar starts a run with the same settings without opening the tab.")
        .goal(Goal::InspectorTab(2), "the Optimize tab open")
        .highlight("optimize_tab")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Choose what to favour",
            "The Objective list sets four weights. The choices only differ in which weight counts most.",
        )
        .actions([
            "Open the Objective list and choose Low windowing.",
            "Read the line under the list: it says what the choice favours.",
        ])
        .check("the Objective list reads Low windowing.")
        .why("Balanced weighs windowing, extinction and tilt brilliance equally. Brilliance, Low extinction and Keep weight lean the other ways; Keep weight also counts how much of the rough is wasted. Lighten dark rough and Intensify pale rough pull the face-up colour lighter or stronger. Custom shows the weight boxes so you can set them yourself.")
        .goal(
            event(events::OPTIMIZE_PRESET_CHOSEN),
            "an entry of the Objective list chosen",
        )
        .highlight("optimize_tab")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Look at what may change",
            "Say which tiers the search may move, and by how much.",
        )
        .actions([
            "Under What may change, see that Vary anchored tiers is ticked. It starts ticked on a design where every tier is pinned, like this one: the search then turns each tier about the edge where it meets the girdle, so the girdle outline stays.",
            "Click Angle ranges... Each tier the search may move can change by 5 degrees either way, never past 0.5 or 89.5 degrees. Type your own MIN and MAX to narrow or widen a range, or press Reset ranges.",
        ])
        .check("a table of the tiers with MIN and MAX boxes.")
        .why("The table, the culet and the girdle never move, and a tier whose angle follows a relation shows follows a relation instead of boxes: it goes along with the tier it reads. Keep the girdle (at least half its thickness) drops a candidate whose girdle gets thinner than half of what it was.")
        .goal(
            event(events::OPTIMIZE_RANGES_OPENED),
            "the angle ranges opened",
        )
        .highlight("optimize_tab")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Run a short search",
            "A small budget keeps this run to a few seconds.",
        )
        .actions([
            "Under Run, set Budget to 100 and Candidates to 2.",
            "Read the Estimated time line above the button, then click Optimize in the tab. A bar shows how far the search has got, and a Cancel button stops it and keeps the best result so far.",
        ])
        .check("a Candidates list under the run settings.")
        .why("Budget is how many trial stones the search may score. The same Seed gives the same result on the same design. Every candidate you ask for costs one more full-quality scoring at the end.")
        .goal(event(events::OPTIMIZE_FINISHED), "an Optimize run to finish")
        .highlight("optimize_tab")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Read and pick a candidate",
            "The list shows what the search found next to the stone you started with.",
        )
        .actions([
            "The first row, Start, is the stone as it is now. The rows under it are the candidates, numbered from 1, best first.",
            "Score, Wind., Bril., Ext. and Yield compare each with Start: a figure is green when it beats Start and red when it is worse.",
            "Click a candidate row (Up and Down move through the list too). Under the list you see its before and after figures and the tiers it would change, with FROM and TO angles.",
            "Picking a row turns Preview on: the viewport shows the candidate before you apply it.",
        ])
        .check("the picked row is highlighted and the tiers it would change are listed under the list.")
        .why("Each candidate differs from the others by at least half a degree on some tier, so a later row is not just the first with a tier nudged. If nothing could be improved the list shows only Start.")
        .goal(
            event(events::OPTIMIZE_CANDIDATE_PICKED),
            "a candidate row clicked",
        )
        .highlight("optimize_tab")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Apply a candidate",
            "A result is only a report until you apply it.",
        )
        .actions(["Click Apply this candidate."])
        .check("the tier table shows the new angles.")
        .why("Apply is one undo step: the new angles, each turned tier's mast and every relation worked out again. It is only offered while the design has not been edited since the run.")
        .goal(
            check("a tier with a new angle", angles_differ_from_standard),
            "a candidate applied",
        )
        .highlight("optimize_tab")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Check the result",
            "Treat the design like after any other edit.",
        )
        .actions([
            "Click Solve and look at the verdict badge and the manufacturability warnings.",
            "Press Ctrl+Z to put the whole design back in one step.",
        ])
        .why("Applying clears the candidate list, because it described the design as it was before. Run Optimize again for a fresh search from the new design. Before you apply, Compare... next to Preview opens the compare window on the picked candidate: the snapshot lessons cover that window.")
        .allow(TOOLS),
    )
}

fn retarget_shift() -> Guide {
    Guide::new(
        "optimizing-retarget-shift",
        "Retarget for a new material (Shift)",
        "Move a design from quartz to topaz with Retarget's Shift mode: read the proposal and its verdict, apply, and undo.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Choose the material",
            "Retarget moves a design from the material it is in to another one, so start by naming one.",
        )
        .actions([
            "Open the Design Settings panel above the tier table.",
            "Set Material to Quartz.",
            "Click Apply Material.",
        ])
        .check("the panel shows Quartz, and Apply Material no longer shows an asterisk.")
        .goal(Goal::Material("Quartz".to_owned()), "the material set to Quartz")
        .highlight("design_settings")
        .allow(SETTINGS),
    )
    .step(
        GuideStep::new(
            "How Shift works",
            "Shift is a formula: it keeps each pavilion facet's margin over the critical angle where it was.",
        )
        .actions([
            "A material with a higher refractive index has a smaller critical angle. Moving from quartz to topaz lowers it by a couple of degrees, so each pavilion angle moves by that much and keeps its margin.",
            "The crown follows the pavilion's stretch: every crown angle is scaled so the stone keeps its silhouette and its table size. Crown Handling (Advanced) offers the older rules: a fraction of the shift, or scaling by the critical-angle ratio.",
            "The table and culet keep their angles, and the girdle is not touched at all.",
        ])
        .why("Shift is instant and always gives the same answer. Its limit is that it moves one facet at a time and looks at no others, so for a large change of material the result can be Not valid. The next lesson covers Optimize.")
        .highlight("retarget_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Retarget to topaz",
            "Review the proposal, read its verdict, and apply it.",
        )
        .actions([
            "Click Retarget... on the command bar's second row.",
            "Set Target Material to Topaz. A card under it shows its n_D and critical angle. Leave Mode on Shift.",
            "Read the table: Block, Tier, Old, New, Margin and Risk (Safe, Marginal or Windows) for each crown and pavilion tier. The table and culet are greyed and read Not changed.",
            "Wait until the line under the table no longer says Checking..., and read it: Valid, with the girdle and table figures and the stone's depth (for example depth 61 % (was 58 %)), or Not valid, with the reasons. Under it, Optical comparison (table up) gives windowing, brilliance and extinction for the current stone, the current stone in Topaz, and the retargeted stone.",
            "Click Apply. This lesson goes on when the design has changed, and the dialog closes itself.",
        ])
        .check("the tier table shows new pavilion angles and the Design Settings panel shows Topaz.")
        .why("A change is judged before you can apply it. Apply stays disabled while it is Checking... or Not valid. Apply commits the new angles, the adjusted facet heights and the material as ONE undo step. Cancel closes the dialog and changes nothing.")
        .goal(
            Goal::All(vec![
                Goal::Material("Topaz".to_owned()),
                check("pavilion mains moved", pavilion_has_moved),
            ]),
            "the design retargeted to Topaz",
        )
        .highlight("retarget_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Undo the retarget",
            "One undo takes everything back.",
        )
        .actions(["Press Ctrl+Z, or click Undo on the command bar."])
        .check("the pavilion main is back at its angle and the material is Quartz again.")
        .why("The history entry reads Retarget for Topaz. The angles, the heights and the material change together, so they are undone together.")
        .goal(
            check("the retarget undone", retarget_is_undone),
            "the retarget undone",
        )
        .highlight("retarget_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "When a change is Not valid",
            "A retarget can be refused before it changes anything.",
        )
        .actions([
            "The verdict says Not valid when the stone would stop being a sound design: the facets no longer close, the girdle disappears or gets less than half as thick, the table sits below the top of the girdle, a tier loses facets, facets become too small to cut, or a tier that follows a relation cannot follow it any more.",
            "Compare... still works on a change that is not valid, so you can see what goes wrong.",
            "A row held at a limit shows (held) after its new angle: an angle never crosses the horizontal and is kept between 1 and 89.5 degrees.",
        ])
        .why("When Shift alone is not valid, the dialog adds: Shift alone is not valid here. Use Optimize. That is the next lesson.")
        .allow(TOOLS),
    )
}

fn retarget_optimize() -> Guide {
    Guide::new(
        "optimizing-retarget-optimize",
        "Retarget: Optimize mode",
        "Let Retarget search for the best crown and pavilion angles in the new material, pick an option and apply it.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "Choose the material",
            "Start the design in quartz.",
        )
        .actions([
            "Open the Design Settings panel above the tier table.",
            "Set Material to Quartz.",
            "Click Apply Material.",
        ])
        .check("the panel shows Quartz, and Apply Material no longer shows an asterisk.")
        .goal(Goal::Material("Quartz".to_owned()), "the material set to Quartz")
        .highlight("design_settings")
        .allow(SETTINGS),
    )
    .step(
        GuideStep::new(
            "Shift or Optimize",
            "Shift is a formula. Optimize starts from it and then searches.",
        )
        .actions([
            "Optimize starts from the Shift result and moves every crown and pavilion angle on its own, inside a range you choose, looking for the best score in the new material.",
            "It can find a valid stone when the formula's result is not valid, and a better one than the formula gives, because the crown can move too.",
            "It never tilts the table or culet, moves the girdle or a tier whose angle follows a relation; it keeps at least half the girdle thickness, refits the table and culet heights so they keep their size, and (Keep the design's look, on by default) favours options that keep the design's table size and crown-to-pavilion ratio.",
        ])
        .why("The search is seeded, not random: the same design, settings and target give the same options every time. It runs only when you press Search, in the background.")
        .highlight("retarget_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Search and apply",
            "Search for options in sapphire, pick one and apply it.",
        )
        .actions([
            "Click Retarget... on the command bar's second row.",
            "Set Target Material to Sapphire and Mode to Optimize.",
            "Leave Objective on Balanced and Range on 6 degrees either side, and set Effort to Quick (100 steps). These three show in the Advanced interface; the lesson shows them for now.",
            "Click Search for better angles. A line names the stage and how far it has got; Cancel stops the search without closing the dialog.",
            "Under Options (click one to see it) up to three options are listed, best score first. Click one: the table, the verdict and the comparison switch to it. Only valid options are listed.",
            "Click Apply. It stays dim until an option is picked. This lesson goes on when the design has changed, and the dialog closes itself.",
        ])
        .check("the tier table shows the option's angles and the Design Settings panel shows Sapphire.")
        .why("Searching and picking change nothing. Apply commits the option's angles, heights and the material as one undo step.")
        .goal(
            Goal::All(vec![
                Goal::Material("Sapphire".to_owned()),
                check("pavilion mains moved", pavilion_has_moved),
            ]),
            "the design retargeted to Sapphire",
        )
        .highlight("retarget_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Undo the retarget",
            "One undo takes everything back, however the angles were found.",
        )
        .actions(["Press Ctrl+Z, or click Undo on the command bar."])
        .check("the pavilion main is back at its angle and the material is Quartz again.")
        .goal(
            check("the retarget undone", retarget_is_undone),
            "the retarget undone",
        )
        .highlight("retarget_button")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Reading the options",
            "Each option shows the figures that decided its place.",
        )
        .actions([
            "Each option shows its score (lower is better), windowing, brilliance, extinction, the share of the rough the finished stone gives up (yield loss) and Valid.",
            "A note under the summary says how many results were dropped by the validity check, and why. If the search found nothing better than where it started, the note says what to try: a wider range, another objective, more steps.",
            "Before you press Search the dialog times one step and tells you how long the search will take.",
        ])
        .allow(TOOLS),
    )
}

fn sweep() -> Guide {
    Guide::new(
        "optimizing-sweep",
        "Angle Sweep",
        "Try one tier's angle over a range, read the table of figures for every angle and use the best one.",
        GuideCategory::Optimizing,
    )
    .starting(StartingState::Template(RICH))
    .step(
        GuideStep::new(
            "What a sweep tells you",
            "A sweep answers: what does 40.5 degrees do to this pavilion compared with 41.5?",
        )
        .actions([
            "It tries one tier's angle over a range, one angle after another, on a private copy of the design.",
            "You get a table with a row for every angle, a chart of the figures against the angle, and the same rows as CSV text.",
            "Your design does not change while the sweep runs. Only the Use button changes it.",
        ])
        .why("The table, the culet and a girdle tier cannot be swept, and neither can a tier whose angle follows a relation. A sweep tries at most 200 angles.")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Run a sweep and use an angle",
            "Sweep the pavilion mains, then make the best angle the real one.",
        )
        .actions([
            "Choose Edit > Angle Sweep... (or press Ctrl+K and type sweep).",
            "Set Tier to the Pavilion Main entry. Set From to 38, To to 42 and Step to 1. The line under the form says how many angles it tries and about how long it takes.",
            "Click Run sweep. A bar shows how many angles are done. Cancel keeps the rows already finished.",
            "Read the table. The best figure in each column is bold and green. The row marked Current is your design as it is.",
            "Click a row that is not Current. The button at the bottom then names it, for example Use 42.00\u{b0}. Click it.",
            "This lesson goes on when the tier has its new angle. Then press Close (or Esc) to leave the dialog.",
        ])
        .check("the Pavilion Main row of the tier table shows a new angle.")
        .why("Brilliance, windowing and extinction are scored looking straight down on the table, with the quick score Optimize uses. Higher brilliance is better; lower windowing and extinction are better. An angle that gives no stone has a Not valid row with the reason under Notes. Esc closes the dialog, and stops a running sweep first.")
        .goal(
            check("pavilion mains moved", pavilion_has_moved),
            "a sweep angle used on the Pavilion Main",
        )
        .highlight("tier_table")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "Undo it",
            "Using a sweep angle is one undo step.",
        )
        .actions(["Press Ctrl+Z, or click Undo on the command bar."])
        .check("the pavilion main is back at its first angle.")
        .goal(
            check("the pavilion main back", pavilion_is_back),
            "the pavilion main back at its first angle",
        )
        .highlight("tier_table")
        .allow(TOOLS),
    )
    .step(
        GuideStep::new(
            "The chart, the tilt option and the CSV",
            "The same run gives you more than the table.",
        )
        .actions([
            "The chart draws one line per figure over the angles. Each line is scaled to its own lowest and highest value, so compare their shapes, not their heights. The legend under it gives the real numbers. The buttons above the chart switch a line on and off, and a click on the chart selects the nearest angle.",
            "Also average the tilt performance (slower) adds the tilt brilliance, windowing and extinction of the Tilt Performance graph to every angle, at about a second and a half of work per angle.",
            "Copy CSV puts the table on the clipboard and Save CSV... writes it to a file for a spreadsheet.",
        ])
        .why("The rows are not saved with the design and are gone when the dialog closes. The figures are the quick table-up score under one lighting; confirm a choice with the Tilt Performance graph.")
        .allow(TOOLS),
    )
}
