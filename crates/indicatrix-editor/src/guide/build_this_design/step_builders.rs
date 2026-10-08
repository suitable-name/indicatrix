//! The steps of a "Build this design" lesson: one per flat tier (or per group of alike tiers
//! in a large design), one per concave tier, and the start, solve, compare and closing steps.

use super::{
    COMPARE_STEP_TITLE, DIALOG_GEARS, ExactReason, LARGE_DESIGN_TIERS, MAX_TIERS_PER_STEP, Meets,
    PlannedTier, REBUILD_ANGLE_TOL_DEG, REBUILD_MAST_REL_TOL, SOLVE_STEP_TITLE, START_STEP_TITLE,
    TIER_STEP, TIER_STEP_EXACT, TierFacts, TierRecipe, reference_copy,
};
use crate::guide::{
    ALL_GROUPS, CMD_COMPARE, Goal, Group, GuideStep, NEW_DESIGN_CREATED, Perform, TierEntry,
    build_text::{Role, block_label, indices_expansion, indices_phrase, indices_text, number_text},
};
use indicatrix_cut_core::{
    Design, FreshDesignSpec, MaterialSelection, PreformShape,
    design::{ConcaveTier, TierRef},
    is_legacy_123_abc,
};

/// What Next types into the Tier form for one tier of the lesson: the recipe's own fields.
fn tier_entry(recipe: &TierRecipe) -> TierEntry {
    TierEntry::add(
        &recipe.angle,
        recipe.constraint_kind,
        &recipe.constraint_text,
        &recipe.name,
        &recipe.indices,
    )
}

/// One step's worth of the lesson's body.
enum Piece {
    /// These flat tiers (positions in the plan).
    Tiers(Vec<usize>),
    /// This concave tier: its position in the target's concave list, and how many concave
    /// tiers the lesson has added once it is in.
    Concave { index: usize, number: usize },
}

/// Splits the lesson into steps: one per flat tier (or per group of up to
/// [`MAX_TIERS_PER_STEP`] alike, in a large design) and one per concave tier, in cutting
/// order.
fn lesson_pieces(target: &Design, plan: &[PlannedTier]) -> Vec<Piece> {
    let group = plan.len() > LARGE_DESIGN_TIERS;
    let mut pieces: Vec<Piece> = Vec::new();
    let mut next_flat = 0;
    let mut concave_seen = 0;
    for tier in target.cutting_order() {
        match tier {
            TierRef::Concave(index) => {
                concave_seen += 1;
                pieces.push(Piece::Concave {
                    index,
                    number: concave_seen,
                });
            }
            TierRef::Flat(_) => {
                let at = next_flat;
                next_flat += 1;
                if group
                    && let Some(Piece::Tiers(last)) = pieces.last_mut()
                    && last.len() < MAX_TIERS_PER_STEP
                    && last.first().is_some_and(|&first| {
                        plan[first].block == plan[at].block && plan[first].role == plan[at].role
                    })
                {
                    last.push(at);
                } else {
                    pieces.push(Piece::Tiers(vec![at]));
                }
            }
        }
    }
    pieces
}

/// The body steps, in cutting order.
pub(super) fn lesson_steps(target: &Design, plan: &[PlannedTier]) -> Vec<GuideStep> {
    let gear = target.meta.gear_teeth_abs();
    let concave_codes = target.tier_codes().concave;
    lesson_pieces(target, plan)
        .iter()
        .map(|piece| match piece {
            Piece::Tiers(positions) if positions.len() == 1 => {
                tier_step(&plan[positions[0]], positions[0] + 1, plan.len(), gear)
            }
            Piece::Tiers(positions) => group_step(plan, positions, gear),
            Piece::Concave { index, number } => {
                let code = concave_codes
                    .get(*index)
                    .map_or("", |label| label.code.as_str());
                concave_step(&target.concave_tiers[*index], code, *number, gear)
            }
        })
        .collect()
}

/// Whether the Girdle Facet Preset (angle 90, Meets Exact scale value 1) fills in this tier:
/// a girdle at exactly 90 degrees that states its depth.
fn girdle_preset_fits(planned: &PlannedTier) -> bool {
    planned.role == Role::Girdle
        && planned.meets.is_exact()
        && (planned.angle_deg - 90.0).abs() < 1e-9
}

/// The Angle field's action line.
fn angle_line(planned: &PlannedTier) -> String {
    if planned.angle_deg == 0.0 && planned.angle_deg.is_sign_negative() {
        format!(
            "Angle (deg): {} (minus zero: the minus sign makes this the culet, not the table)",
            planned.recipe.angle
        )
    } else if girdle_preset_fits(planned) {
        format!(
            "Angle (deg): {} (or click Girdle Facet Preset, which also sets Meets)",
            planned.recipe.angle
        )
    } else {
        format!("Angle (deg): {}", planned.recipe.angle)
    }
}

/// The sentence on what the Girdle Facet Preset fills in, when it fits `planned`.
fn girdle_preset_note(planned: &PlannedTier) -> Option<String> {
    if !girdle_preset_fits(planned) {
        return None;
    }
    let one = planned
        .recipe
        .constraint_text
        .parse::<f64>()
        .is_ok_and(|value| (value - 1.0).abs() < 1e-9);
    Some(if one {
        "Girdle Facet Preset fills in Angle 90 and Meets Exact scale value 1, so with it only \
         Name and Indices are left to type."
            .to_owned()
    } else {
        format!(
            "Girdle Facet Preset fills in Angle 90 and Meets Exact scale value 1; change the \
             Scale value to {} to match this design.",
            planned.recipe.constraint_text
        )
    })
}

/// The sentence that says why a step shows the Advanced controls, when `needed`.
const fn advanced_note(needed: bool) -> &'static str {
    if needed {
        " This step shows the Advanced controls because the Simple interface leaves Exact scale \
         value out of the Meets list."
    } else {
        ""
    }
}

/// What to type after "Meets:".
fn meets_text(planned: &PlannedTier) -> String {
    let label = planned.meets.kind().label();
    match &planned.meets {
        Meets::Exact(_) => format!("{label} -- {}", planned.recipe.constraint_text),
        Meets::Named(names) => format!("{label} -- {}", names.join(", ")),
        Meets::Unspecified => label.to_owned(),
    }
}

/// What to type after "Indices:".
fn indices_line(planned: &PlannedTier, gear: u32) -> String {
    if planned.indices_text.blank {
        return "leave blank".to_owned();
    }
    let typed = &planned.indices_text.typed;
    indices_expansion(typed, &planned.indices, gear).map_or_else(
        || typed.clone(),
        |expansion| format!("{typed} (that is {expansion})"),
    )
}

/// The goal of a tier step: the tier is in the design.
fn tier_goal(planned: &PlannedTier) -> Goal {
    Goal::TierMatches {
        name: planned.recipe.name.clone(),
        angle_deg: planned.angle_deg,
        tol_deg: REBUILD_ANGLE_TOL_DEG,
        indices: Some(if planned.indices_text.blank {
            Vec::new()
        } else {
            planned.indices.clone()
        }),
        constraint_kind: Some(planned.meets.kind()),
    }
}

/// The sentence on why a tier states its meets the way it does.
fn meets_reason(planned: &PlannedTier) -> String {
    match &planned.meets {
        Meets::Exact(ExactReason::Anchor) => format!(
            "This is the first {} tier, so it sets that block's size: state its depth with \
             Exact scale value. The editor needs one such anchor in every block it solves.",
            block_label(planned.block)
        ),
        Meets::Exact(ExactReason::NoMeetInfo) => {
            "The original does not say which facets this tier meets, so state its depth \
             directly with Exact scale value."
                .to_owned()
        }
        Meets::Exact(ExactReason::MeetsLater) => {
            "The facets the original meets are cut later in the lesson, so state this depth \
             directly with Exact scale value."
                .to_owned()
        }
        Meets::Exact(ExactReason::Missed(percent)) => format!(
            "Meeting its neighbours would put this tier about {percent:.1} percent away from \
             the original, so state its depth directly with Exact scale value."
        ),
        Meets::Exact(ExactReason::Everything) => {
            "Meeting facets does not reproduce this design reliably, so every depth is stated \
             directly with Exact scale value."
                .to_owned()
        }
        Meets::Exact(ExactReason::TooLarge) => {
            "This is a large design, so every depth after the anchors is stated directly with \
             Exact scale value. That keeps the lesson quick to check."
                .to_owned()
        }
        Meets::Named(names) => format!(
            "Named facet(s) places this tier where it meets {}, so you do not have to look up \
             its depth.",
            names.join(" and ")
        ),
        Meets::Unspecified => {
            "Unspecified vertex lets the solver find the vertex this tier closes against, so \
             you do not have to look up its depth."
                .to_owned()
        }
    }
}

/// What a step title calls a tier: its code, as the cutting sheet's label column shows it,
/// then its own name when that says more (`P1 Pavilion Main`); just the code for a tier whose
/// name is an old-style one (`1`, `A`) or is the code itself, and just the name when no code
/// is on record.
fn tier_label(planned: &PlannedTier) -> String {
    let name = planned.recipe.name.trim();
    if planned.code.is_empty() {
        return name.to_owned();
    }
    if name.is_empty() || is_legacy_123_abc(name) || name.eq_ignore_ascii_case(&planned.code) {
        planned.code.clone()
    } else {
        format!("{} {name}", planned.code)
    }
}

/// The step that adds one flat tier.
fn tier_step(planned: &PlannedTier, number: usize, total: usize, gear: u32) -> GuideStep {
    let name = &planned.recipe.name;
    let actions = vec![
        "Click + Add Tier.".to_owned(),
        angle_line(planned),
        format!("Meets: {}", meets_text(planned)),
        format!("Name: {name}"),
        format!("Indices: {}", indices_line(planned, gear)),
        "Click Add Tier.".to_owned(),
    ];
    GuideStep::new(
        format!("Add {} ({})", tier_label(planned), planned.role.label()),
        format!("Tier {number} of {total}. {}", planned.role.blurb()),
    )
    .actions(actions)
    .check(format!("a {name} row in the tier table."))
    .why(format!(
        "{} {}{}{}",
        planned.role.why(),
        meets_reason(planned),
        girdle_preset_note(planned).map_or_else(String::new, |note| format!(" {note}")),
        advanced_note(planned.meets.is_exact())
    ))
    .goal(
        tier_goal(planned),
        format!(
            "tier {name} at {} with {}",
            planned.recipe.angle,
            indices_phrase(&planned.indices_text, &planned.indices)
        ),
    )
    .perform(Perform::Tier(tier_entry(&planned.recipe)))
    .highlight("inspector_tier")
    .allow(if planned.meets.is_exact() {
        TIER_STEP_EXACT
    } else {
        TIER_STEP
    })
}

/// The step that adds several flat tiers of one kind, as a checklist.
fn group_step(plan: &[PlannedTier], positions: &[usize], gear: u32) -> GuideStep {
    let tiers: Vec<&PlannedTier> = positions.iter().map(|&at| &plan[at]).collect();
    let (Some(first), Some(last)) = (tiers.first(), tiers.last()) else {
        return GuideStep::new("Add tiers", "Add the tiers listed.");
    };
    let mut actions = vec![
        "For each tier below: click + Add Tier, fill in the fields as listed, and click Add \
         Tier."
            .to_owned(),
    ];
    for planned in &tiers {
        actions.push(format!(
            "{}: angle {}, indices {}, meets {}",
            planned.recipe.name,
            planned.recipe.angle,
            indices_line(planned, gear),
            meets_text(planned)
        ));
    }
    let first_name = &first.recipe.name;
    let last_name = &last.recipe.name;
    // The title says which rows of the sheet the step covers, so it names the codes; the
    // names are in the checklist below.
    let first_code = if first.code.is_empty() {
        first_name
    } else {
        &first.code
    };
    let last_code = if last.code.is_empty() {
        last_name
    } else {
        &last.code
    };
    let anchors = tiers
        .iter()
        .any(|planned| matches!(planned.meets, Meets::Exact(ExactReason::Anchor)));
    let note = if anchors {
        format!(
            " The first tier of the {} sets that block's size, so it states its depth with \
             Exact scale value.",
            block_label(first.block)
        )
    } else {
        String::new()
    };
    let needs_advanced = tiers.iter().any(|planned| planned.meets.is_exact());
    GuideStep::new(
        format!("Add {first_code} to {last_code} ({})", first.role.plural()),
        format!(
            "Tiers {} to {} of {}. {}",
            positions[0] + 1,
            positions[positions.len() - 1] + 1,
            plan.len(),
            first.role.blurb()
        ),
    )
    .actions(actions)
    .check(format!(
        "rows {first_name} to {last_name} in the tier table."
    ))
    .why(format!(
        "{} This is a large design, so these {} tiers share one step.{note}{}",
        first.role.why(),
        tiers.len(),
        advanced_note(needs_advanced)
    ))
    .goal(
        Goal::All(tiers.iter().map(|planned| tier_goal(planned)).collect()),
        format!("tiers {first_name} to {last_name}"),
    )
    .perform(Perform::Sequence(
        tiers
            .iter()
            .map(|planned| Perform::Tier(tier_entry(&planned.recipe)))
            .collect(),
    ))
    .highlight("inspector_tier")
    .allow(if needs_advanced {
        TIER_STEP_EXACT
    } else {
        TIER_STEP
    })
}

/// The step that adds one concave tier. `code` is the tier's code in the target's cutting
/// order (`P3`: it continues the count of the flat pavilion tiers), empty when none is on record.
fn concave_step(tier: &ConcaveTier, code: &str, number: usize, gear: u32) -> GuideStep {
    let name = if tier.name.trim().is_empty() {
        "(no name)".to_owned()
    } else {
        tier.name.trim().to_owned()
    };
    let label = if code.is_empty() || name.eq_ignore_ascii_case(code) {
        name.clone()
    } else {
        format!("{code} {name}")
    };
    let indices = indices_text(&tier.indices, gear, tier.angle_deg);
    let typed = if indices.blank {
        "0".to_owned()
    } else {
        indices.typed
    };
    let [x, y, z] = tier.displacement;
    let mut actions = vec![
        "Click + Add Concave Tier in the tier table's toolbar.".to_owned(),
        format!("Name: {}", tier.name.trim()),
        format!("Angle (deg): {}", number_text(tier.angle_deg, 4)),
        format!("Indices: {typed}"),
    ];
    if !tier.instructions.trim().is_empty() {
        actions.push(format!("Instructions: {}", tier.instructions.trim()));
    }
    actions.push(format!("Tool: {:?} ({})", tier.tool, tier.tool.code()));
    actions.push(format!(
        "Theta (deg): {}; X: {}, Y: {}, Z: {}",
        number_text(tier.tool_azimuth_deg, 3),
        number_text(x, 3),
        number_text(y, 3),
        number_text(z, 3)
    ));
    actions.push(format!("D/W: {}", number_text(tier.diameter_ratio, 3)));
    if let Some(angle) = tier.tool_angle_deg.filter(|_| tier.tool.takes_angle()) {
        actions.push(format!("Tool angle (deg): {}", number_text(angle, 2)));
    }
    actions.push(format!(
        "Reciprocating: {}",
        if tier.motion.word() == "reciprocating" {
            "ticked"
        } else {
            "unticked (plunge)"
        }
    ));
    actions.push("Click Add Concave Tier.".to_owned());
    // An unnamed concave tier is typed with a blank Name, which the form refuses: no recipe.
    let mut step = GuideStep::new(
        format!("Add {label} (concave tier {number})"),
        "A concave tier is cut with a shaped tool instead of a flat lap.".to_owned(),
    )
    .actions(actions)
    .check(format!("a {name} row with a Tool badge in the tier table."))
    .why(format!(
        "Concave tiers are cut after the flat facets of their section. On the cutting sheet \
         this one reads: {}.",
        tier.second_line_fields().join("  ")
    ))
    .goal(
        Goal::ConcaveTiersAtLeast(number),
        format!("concave tier {number} to be added"),
    )
    .highlight("tier_table")
    .allow(TIER_STEP);
    if !tier.name.trim().is_empty() {
        step.perform = Some(Perform::ConcaveTier {
            edit: None,
            tier: Box::new(tier.clone()),
        });
    }
    step
}

/// The first step: a new design with the original's gear, symmetry and preform.
pub(super) fn start_step(target: &Design, title: &str, step_count: usize) -> GuideStep {
    let meta = &target.meta;
    let gear = meta.gear_teeth_abs();
    let symmetry = meta.symmetry_order;
    let preform = target.preform;
    let shape = match preform.shape {
        PreformShape::Block => "Block",
        PreformShape::Cylinder { .. } => "Cylinder",
    };
    let gear_line = if DIALOG_GEARS.contains(&gear) {
        format!("Index Gear: {gear}.")
    } else {
        format!("Index Gear: Custom, then Custom Teeth: {gear}.")
    };
    let mirror = if meta.mirror { "on" } else { "off" };
    let repeat = if symmetry > 0 && gear.is_multiple_of(symmetry) {
        format!(" put one repeat every {} teeth", gear / symmetry)
    } else {
        String::new()
    };
    let mirror_note = if meta.mirror {
        "Mirror on means each repeat is also mirrored."
    } else {
        "Mirror off means each repeat is cut once and not mirrored."
    };
    // The New Design form as the actions describe it: Empty, this preform, gear, symmetry and
    // mirror, no material.
    let create = i32::try_from(gear)
        .ok()
        .map(|gear_teeth| Perform::NewDesign {
            template_index: 0,
            spec: Box::new(FreshDesignSpec {
                gear_teeth,
                symmetry_order: symmetry,
                mirror: meta.mirror,
                material: MaterialSelection::none(),
                preform,
            }),
        });
    let mut step = GuideStep::new(
        START_STEP_TITLE,
        format!(
            "This lesson rebuilds {title} in {step_count} steps, one tier at a time in cutting \
             order, and ends by comparing your stone with the original."
        ),
    )
    .actions([
        "Click New Design... on the command bar (or File > New Design...).".to_owned(),
        "Start From: Empty.".to_owned(),
        format!(
            "Preform Shape: {shape} -- Half-Width {}, Length / Width {}, Depth {}.",
            number_text(preform.half_width, 2),
            number_text(preform.length_over_width, 2),
            number_text(preform.depth, 2)
        ),
        format!("{gear_line} Symmetry Order: {symmetry}. Mirror: {mirror}."),
        "Starting Material: (none).".to_owned(),
        "Click Create.".to_owned(),
    ])
    .check("the preform in the viewport, with a \"no tiers yet\" hint above it.")
    .why(format!(
        "{gear} teeth with {symmetry}-fold symmetry{repeat}. {mirror_note} The preform is only \
         the rough the stone is cut from: every tier you add trims it, so its size matters \
         only in being bigger than the finished stone. The original stays untouched in the \
         library; you build in a new design."
    ))
    .goal(
        Goal::All(vec![
            Goal::Event(NEW_DESIGN_CREATED.to_owned()),
            Goal::FreshDesign {
                gear_teeth: gear,
                symmetry_order: symmetry,
                mirror: meta.mirror,
            },
        ]),
        format!("a new design with {gear} teeth and {symmetry}-fold symmetry"),
    )
    .highlight("new_design_dialog")
    .allow(&[Group::NewDesign]);
    step.perform = create;
    step
}

/// The step that solves the rebuilt design.
pub(super) fn solve_step() -> GuideStep {
    GuideStep::new(
        SOLVE_STEP_TITLE,
        "Solve computes every tier's depth and checks that the result is a closed stone.",
    )
    .actions([
        "Click Solve on the command bar (or press F5).",
        "Read the status strip at the bottom of the Edit tab.",
    ])
    .check("the status strip reports \"Closed solid\" and the stone's volume.")
    .why(
        "If auto-solve already finished, this step completes by itself. A \"no anchor\", \
         Degenerate or Unbounded message names the tier to check: look at its angle, its \
         indices and its Meets setting against the step that added it.",
    )
    .goal(Goal::SolvedClosed, "a solve that closes")
    .perform(Perform::solve())
    .highlight("solve_button")
    .allow(&[
        Group::Solve,
        Group::TierForm,
        Group::TierTable,
        Group::History,
    ])
}

/// The step that compares the rebuild with the original.
pub(super) fn compare_step(target: &Design, facts: &TierFacts) -> GuideStep {
    let rebuilt = Goal::DesignRebuilt {
        target: Box::new(reference_copy(target, facts)),
        angle_tol_deg: REBUILD_ANGLE_TOL_DEG,
        mast_rel_tol: REBUILD_MAST_REL_TOL,
        target_masts: facts.masts.clone(),
    };
    let goal = if target.concave_tiers.is_empty() {
        rebuilt
    } else {
        Goal::All(vec![
            rebuilt,
            Goal::ConcaveTiersAtLeast(target.concave_tiers.len()),
        ])
    };
    GuideStep::new(
        COMPARE_STEP_TITLE,
        "Your stone is solved. The last check lines it up against the original design.",
    )
    .actions([
        "Click Compare on the command bar. The original is held as the snapshot to compare \
         against, so the button is ready as soon as this step opens.",
        "Read the table: every tier should read Same.",
        "Click Compare visually... to see the original and your stone side by side.",
    ])
    .check("every row reads Same, and the status strip still shows a closed solid.")
    .why(
        "This step finishes by itself once every tier is cut as in the original and every \
         solved depth is within about one percent of it. A row that reads Changed names the \
         tier to look at again. Next opens the comparison for you; if the stone will not \
         match, Skip step then finishes without a perfect match.",
    )
    .goal(goal, "a design that matches the original")
    .perform(Perform::command(CMD_COMPARE))
    .allow(&[
        Group::Advanced,
        Group::Solve,
        Group::TierForm,
        Group::TierTable,
        Group::History,
    ])
}

/// The closing reading step.
pub(super) fn final_step(target: &Design, title: &str) -> GuideStep {
    let mut actions = vec![
        "Save your rebuild with Save (Ctrl+S), or keep editing.".to_owned(),
        "Open the original from the Library any time to look at it again.".to_owned(),
    ];
    if let Some(material) = target
        .material
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        actions.push(format!(
            "The original is cut in {material}: set Material to {material} in Design Settings \
             and click Apply Material to see the same margins."
        ));
    }
    GuideStep::new(
        format!("You rebuilt {title}"),
        "A new design with the original's tiers, solved to the same stone.",
    )
    .actions(actions)
    .why(
        "Every control is unlocked again. Change one angle and watch what moves, or run \
         Compare again to see exactly what you changed.",
    )
    .allow(ALL_GROUPS)
}
