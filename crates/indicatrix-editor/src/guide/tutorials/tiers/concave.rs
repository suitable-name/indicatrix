//! The concave tools: a cylinder and a cone in the first lesson, a sphere, a disc and the
//! plunging motion in the second.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{FORM, check};
use crate::guide::{Guide, GuideCategory, GuideStep, StartingState};
use indicatrix_cut_core::{
    Design,
    design::{ConcaveTier, ConcaveTool, ToolMotion},
};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![cylinder_and_cone(), sphere_and_disc()]
}

/// The concave tier called `name` (case and surrounding spaces do not matter).
fn concave_called<'a>(design: &'a Design, name: &str) -> Option<&'a ConcaveTier> {
    design
        .concave_tiers
        .iter()
        .find(|tier| tier.name.trim().eq_ignore_ascii_case(name.trim()))
}

/// Whether the concave tier called `name` has `tool` as its tool and `angle_deg` as its facet
/// angle.
fn concave_is(design: &Design, name: &str, tool: ConcaveTool, angle_deg: f64) -> bool {
    concave_called(design, name)
        .is_some_and(|tier| tier.tool == tool && (tier.angle_deg - angle_deg).abs() < 1e-6)
}

/// Whether the concave tier called `name` has `motion` as its motion.
fn concave_moves(design: &Design, name: &str, motion: ToolMotion) -> bool {
    concave_called(design, name).is_some_and(|tier| tier.motion == motion)
}

fn cylinder_and_cone() -> Guide {
    Guide::new(
        "tiers-concave-cylinder-cone",
        "Concave tiers: cylinder and cone",
        "Cut a groove with a cylinder and a bevel with a cone, using the concave tier form.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "A tier cut with a tool",
            "A concave tier is ground with a shaped tool instead of a flat lap, so it leaves a groove, a bowl or a dimple.",
        )
        .actions([
            "It is written on two lines: the facet line (name, angle, indices) and the tool line (tool, theta, displacement, D/W).",
            "The concave form is in the Tier tab; the + Add Concave Tier button in the tier table's toolbar opens it blank, in the Simple and the Advanced interface alike.",
        ])
        .why("Concave rows come after the flat ones in the table, and nothing can meet a concave tier by name.")
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Add a cylinder groove",
            "A cylinder leaves a straight groove, like a mandrel pressed along the stone.",
        )
        .actions([
            "Click + Add Concave Tier in the tier table's toolbar.",
            "Name: Groove",
            "Angle (deg): -42",
            "Indices: 0, 24, 48, 72",
            "Tool: Cylinder (CYL)",
            "D/W: 0.25",
            "Tick Reciprocating (stroked back and forth).",
            "Click Add Concave Tier.",
        ])
        .check("a Groove row with a Tool badge in the MEETS column and Tool cut in the SOLVE column.")
        .why("D/W is the tool's diameter over the stone width, so 0.25 is a tool a quarter as wide as the stone. Theta and X / Y / Z may stay blank: blank means 0.")
        .goal(
            check("a cylinder tier Groove at -42 degrees, reciprocating", |ctx| {
                concave_is(ctx.design, "Groove", ConcaveTool::Cylinder, -42.0)
                    && concave_moves(ctx.design, "Groove", ToolMotion::Reciprocating)
            }),
            "a cylinder concave tier called Groove at -42.0",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Add a cone",
            "A cone and a disc need one more number: the included angle of the tool.",
        )
        .actions([
            "Click + Add Concave Tier in the tier table's toolbar.",
            "Name: Bevel",
            "Angle (deg): -44",
            "Indices: 6, 30, 54, 78",
            "Tool: Cone (CON)",
            "Tool angle: 60",
            "D/W: 0.3",
            "Click Add Concave Tier.",
        ])
        .check("a Bevel row next to Groove, and Tool angle enabled for as long as Cone is the tool.")
        .why("The Tool angle field is enabled for a cone and a disc only, and it must lie between 0 and 180 degrees. A cylinder, circle or sphere has no angle to give.")
        .goal(
            check("a cone tier Bevel at -44 degrees with a 60 degree tool", |ctx| {
                concave_is(ctx.design, "Bevel", ConcaveTool::Cone, -44.0)
                    && concave_called(ctx.design, "Bevel")
                        .is_some_and(|tier| tier.tool_angle_deg == Some(60.0))
            }),
            "a cone concave tier called Bevel with a 60.0 degree tool",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Edit what you cut",
            "A concave tier is edited in the inspector, not in the table.",
        )
        .actions([
            "Click the Groove row: the concave form loads it.",
            "Change a field, then click Save Concave Tier.",
            "The angle cell in the table is read-only for a concave row.",
        ])
        .why("A concave tier moves only among the concave rows, can be duplicated with Ctrl+D, and removing one never asks Remove anyway?. Every control is unlocked again."),
    )
}

fn sphere_and_disc() -> Guide {
    Guide::new(
        "tiers-concave-sphere-disc",
        "Concave tiers: sphere, disc and plunge",
        "Cut a dimple with a sphere and a groove with a disc, and choose whether the tool is stroked or plunged.",
        GuideCategory::Tiers,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Two more tools",
            "A sphere leaves a round dimple; a disc is a wheel with a V-shaped rim.",
        )
        .actions([
            "A sphere has no angle, so its Tool angle field stays disabled.",
            "A disc needs a tool angle, just like a cone.",
            "Reciprocating says whether the tool is stroked back and forth (ticked) or pressed straight in (unticked).",
        ])
        .why("The form opens with Reciprocating unticked, so a new tier is plunged unless you tick it.")
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Add a sphere dimple",
            "A dimple is pressed straight in, so leave Reciprocating unticked.",
        )
        .actions([
            "Click + Add Concave Tier in the tier table's toolbar.",
            "Name: Dimple",
            "Angle (deg): -40",
            "Indices: 0, 24, 48, 72",
            "Tool: Sphere (SPH)",
            "D/W: 0.2",
            "Leave Reciprocating unticked.",
            "Click Add Concave Tier.",
        ])
        .check("a Dimple row in the table, and the Tool angle field greyed out while Sphere is chosen.")
        .why("A sphere with a tool angle is refused: the form names the field it is about, in red.")
        .goal(
            check("a sphere tier Dimple at -40 degrees, plunged", |ctx| {
                concave_is(ctx.design, "Dimple", ConcaveTool::Sphere, -40.0)
                    && concave_moves(ctx.design, "Dimple", ToolMotion::Plunge)
            }),
            "a sphere concave tier called Dimple at -40.0, plunged",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Add a disc",
            "A disc is stroked across the stone, so tick Reciprocating this time.",
        )
        .actions([
            "Click + Add Concave Tier in the tier table's toolbar.",
            "Name: Wheel",
            "Angle (deg): -38",
            "Indices: 12, 36, 60, 84",
            "Tool: Disc (DSC)",
            "Tool angle: 90",
            "D/W: 2",
            "Tick Reciprocating.",
            "Click Add Concave Tier.",
        ])
        .check("a Wheel row in the table.")
        .why("D/W may be larger than 1: the tool can be wider than the stone, up to ten stone widths.")
        .goal(
            check("a disc tier Wheel at -38 degrees with a 90 degree tool, reciprocating", |ctx| {
                concave_is(ctx.design, "Wheel", ConcaveTool::Disc, -38.0)
                    && concave_moves(ctx.design, "Wheel", ToolMotion::Reciprocating)
                    && concave_called(ctx.design, "Wheel")
                        .is_some_and(|tier| tier.tool_angle_deg == Some(90.0))
            }),
            "a disc concave tier called Wheel with a 90.0 degree tool, reciprocating",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "Switch a tool to plunge",
            "Changing the motion of a tier you already added is a Save, not a new tier.",
        )
        .actions([
            "Click the Wheel row.",
            "Untick Reciprocating.",
            "Click Save Concave Tier.",
        ])
        .check("the tool line of Wheel reads plunge when you hover its Tool badge.")
        .goal(
            check("Wheel is plunged", |ctx| {
                concave_moves(ctx.design, "Wheel", ToolMotion::Plunge)
            }),
            "Wheel switched to plunge",
        )
        .highlight("inspector_tier")
        .allow(FORM),
    )
    .step(
        GuideStep::new(
            "What the tool line says",
            "The cutting sheet prints each concave tier as two lines in one band.",
        )
        .actions([
            "The second line holds the tool code, theta, X / Y / Z, D/W, the tool angle (for a cone or disc) and the motion.",
            "Open Cutting mode to see the same two lines while you cut.",
        ])
        .why("Every control is unlocked again."),
    )
}
