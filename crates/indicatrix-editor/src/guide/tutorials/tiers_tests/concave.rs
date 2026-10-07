//! Walks of the lessons in `tiers/concave.rs`.

use super::{Sim, act, guide_named, read, walk};
use crate::{
    guide::Group,
    loading::{ConcaveTierFormFields, concave_tier_form_fields, parse_concave_tier_form},
    tier_save::concave_tier_save_edit,
};
use indicatrix_cut_core::design::{ConcaveTool, ToolMotion};

/// The concave form with the fields the lessons fill in; the rest stays blank.
fn fields(
    name: &str,
    angle: &str,
    tool: &str,
    ratio: &str,
    reciprocating: bool,
) -> ConcaveTierFormFields {
    ConcaveTierFormFields {
        name: name.to_owned(),
        angle_deg: angle.to_owned(),
        indices: "0, 24, 48, 72".to_owned(),
        tool: tool.to_owned(),
        diameter_ratio: ratio.to_owned(),
        reciprocating,
        ..ConcaveTierFormFields::default()
    }
}

/// The concave form's gear: the design's own.
fn gear(sim: &Sim) -> i32 {
    i32::try_from(sim.session.design.meta.gear_teeth_abs()).expect("a small gear")
}

/// "Add Concave Tier" with `form` typed in.
fn add_concave(sim: &mut Sim, form: &ConcaveTierFormFields) {
    let tier = parse_concave_tier_form(form, gear(sim))
        .unwrap_or_else(|message| panic!("the concave form is refused: {message}"));
    let edit = concave_tier_save_edit(&sim.session.design, None, tier);
    sim.apply(edit);
}

fn add_groove(sim: &mut Sim) {
    add_concave(sim, &fields("Groove", "-42", "CYL", "0.25", true));
}

fn add_bevel(sim: &mut Sim) {
    let mut form = fields("Bevel", "-44", "CON", "0.3", false);
    form.indices = "6, 30, 54, 78".to_owned();
    form.tool_angle_deg = "60".to_owned();
    add_concave(sim, &form);
}

fn add_dimple(sim: &mut Sim) {
    add_concave(sim, &fields("Dimple", "-40", "SPH", "0.2", false));
}

fn add_wheel(sim: &mut Sim) {
    let mut form = fields("Wheel", "-38", "DSC", "2", true);
    form.indices = "12, 36, 60, 84".to_owned();
    form.tool_angle_deg = "90".to_owned();
    add_concave(sim, &form);
}

/// Click the Wheel row, untick Reciprocating, Save Concave Tier.
fn plunge_the_wheel(sim: &mut Sim) {
    let at = sim
        .session
        .design
        .concave_tiers
        .iter()
        .position(|tier| tier.name == "Wheel")
        .expect("the Wheel is there");
    let mut form = concave_tier_form_fields(&sim.session.design.concave_tiers[at]);
    form.reciprocating = false;
    let tier = parse_concave_tier_form(&form, gear(sim)).expect("the form is accepted");
    let edit = concave_tier_save_edit(&sim.session.design, Some(at), tier);
    sim.apply(edit);
}

/// The Simple interface hides the command bar's "+ Concave" button but keeps the tier table's
/// "+ Add Concave Tier", so the lessons must name only the second, and every step that tells
/// the learner to click it must leave the tier form and the tier table open.
#[test]
fn the_lessons_name_the_add_button_that_both_interfaces_show() {
    for id in ["tiers-concave-cylinder-cone", "tiers-concave-sphere-disc"] {
        let guide = guide_named(id);
        for step in &guide.steps {
            for text in [&step.title, &step.intro, &step.check, &step.why]
                .into_iter()
                .chain(&step.actions)
            {
                assert!(
                    !text.contains("+ Concave") && !text.contains("command bar"),
                    "{id}, step {:?} names a control the Simple interface hides: {text:?}",
                    step.title
                );
            }
            let clicks_add = step
                .actions
                .iter()
                .any(|action| action.starts_with("Click + Add Concave Tier"));
            if clicks_add {
                assert!(
                    step.actions[0].contains("tier table's toolbar"),
                    "{id}, step {:?} does not say where the button is",
                    step.title
                );
                assert!(
                    step.allow.contains(&Group::TierForm) && step.allow.contains(&Group::TierTable),
                    "{id}, step {:?} locks the button it tells the learner to click",
                    step.title
                );
            }
        }
    }
}

#[test]
fn a_cylinder_and_a_cone_are_cut_as_concave_tiers() {
    let sim = walk(
        "tiers-concave-cylinder-cone",
        vec![read(), act(add_groove), act(add_bevel), read()],
    );
    let tiers = &sim.session.design.concave_tiers;
    assert_eq!(tiers.len(), 2);
    assert_eq!(tiers[0].tool, ConcaveTool::Cylinder);
    assert_eq!(tiers[1].tool, ConcaveTool::Cone);
    assert_eq!(tiers[1].tool_angle_deg, Some(60.0));
}

#[test]
fn a_sphere_and_a_disc_and_the_plunging_motion() {
    let sim = walk(
        "tiers-concave-sphere-disc",
        vec![
            read(),
            act(add_dimple),
            act(add_wheel),
            act(plunge_the_wheel),
            read(),
        ],
    );
    let tiers = &sim.session.design.concave_tiers;
    assert_eq!(tiers.len(), 2);
    assert!(tiers.iter().all(|tier| tier.motion == ToolMotion::Plunge));
}
