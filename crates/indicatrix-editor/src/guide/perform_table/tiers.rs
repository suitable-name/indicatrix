//! The tiers tutorials' recipes (see the parent module).

use super::{MAIN_RING, TableEntry, recipe, skip};
use crate::guide::{CMD_ADOPT_ALL, CMD_DUPLICATE, Perform, StepSeries, TierEntry};
use indicatrix_cut_core::design::{ConcaveTier, ConcaveTool, ToolMotion};

/// The Steps panel's Generate form both ladder lessons fill in: name Step, start 30, step 4,
/// N 3, the eight main indices, no anchor.
fn ladder(linked: bool) -> Perform {
    Perform::Steps(StepSeries {
        name: "Step".to_owned(),
        start: "30".to_owned(),
        step: "4".to_owned(),
        count: 3,
        indices: "0:12:96".to_owned(),
        anchor: String::new(),
        linked,
    })
}

/// A concave tier the way the lessons type it: the facet line, the tool and its D/W, with theta
/// and the displacement left blank (0).
fn groove(
    name: &str,
    angle_deg: f64,
    indices: &[f64],
    tool: ConcaveTool,
    tool_angle_deg: Option<f64>,
    diameter_ratio: f64,
    motion: ToolMotion,
) -> ConcaveTier {
    ConcaveTier {
        name: name.to_owned(),
        angle_deg,
        indices: indices.to_vec(),
        instructions: String::new(),
        tool,
        tool_azimuth_deg: 0.0,
        displacement: [0.0; 3],
        diameter_ratio,
        tool_angle_deg,
        motion,
    }
}

/// "+ Add Concave Tier" with this tier typed in.
fn add_concave(tier: ConcaveTier) -> Perform {
    Perform::ConcaveTier {
        edit: None,
        tier: Box::new(tier),
    }
}

/// The recipes of `tutorials/tiers`, lesson by lesson.
pub(super) fn entries() -> Vec<TableEntry> {
    let mut entries = basics();
    entries.extend(meets());
    entries.extend(series());
    entries.extend(editing());
    entries.extend(concave());
    entries
}

fn basics() -> Vec<TableEntry> {
    let add = "tiers-add-a-tier";
    let edit = "tiers-edit-a-tier";
    let shorthands = "tiers-index-shorthands";
    let quick = "tiers-quick-add";
    vec![
        recipe(
            add,
            "Add the girdle",
            Perform::add_tier("90", 2, "1", "G1", MAIN_RING),
        ),
        recipe(
            add,
            "Add a pavilion tier",
            Perform::add_tier("41", 1, "G1", "P1", MAIN_RING),
        ),
        recipe(
            add,
            "Add a crown tier",
            Perform::add_tier("34.5", 1, "G1", "C1", MAIN_RING),
        ),
        recipe(
            edit,
            "Rename it",
            Perform::Tier(TierEntry::of("Pavilion Main").rename("P1")),
        ),
        recipe(
            edit,
            "Change what it meets",
            Perform::Tier(TierEntry::of("P1").meets(1, "Girdle")),
        ),
        recipe(edit, "Take it back with Undo", Perform::undo()),
        recipe(
            shorthands,
            "Count with start:step:stop",
            Perform::add_tier("90", 2, "1", "G1", "0:12:96"),
        ),
        recipe(
            shorthands,
            "Repeat with a fold count",
            Perform::add_tier("41", 1, "G1", "P1", "12 x8"),
        ),
        recipe(
            shorthands,
            "Start a ring anywhere",
            Perform::add_tier("34.5", 1, "G1", "C1", "6 x8"),
        ),
        // The angle the nudges add up to: the step asks for at least 0.2 degrees above 34.5.
        recipe(
            "tiers-inline-angle",
            "Nudge with the keys",
            Perform::Tier(TierEntry::of("Crown Main").angle("34.7")),
        ),
        // The Quick add buttons pin their own heights; there is no form entry that reproduces
        // the Culet's pavilion side under that name.
        skip(quick, "Quick add a girdle"),
        skip(quick, "Quick add a table"),
        skip(quick, "Quick add a culet"),
        // The chip that detaches one facet has no form equivalent.
        skip("tiers-facet-chips", "Detach one facet"),
    ]
}

fn meets() -> Vec<TableEntry> {
    let named = "tiers-meets-named-facets";
    let millimetres = "tiers-meets-mm-targets";
    let adopt = "tiers-adopt-imported-meets";
    let pin = "tiers-pin-to-mast";
    vec![
        recipe(
            named,
            "Meet a named facet",
            Perform::Tier(TierEntry::of("Upper Girdle").meets(1, "Lower Girdle")),
        ),
        recipe(
            named,
            "Rename a facet others meet",
            Perform::Tier(TierEntry::of("Lower Girdle").rename("LG1")),
        ),
        recipe(
            millimetres,
            "Give the stone a size",
            Perform::Yield {
                girdle_diameter_mm: 6.5,
            },
        ),
        recipe(
            millimetres,
            "Cut to a depth",
            Perform::Tier(TierEntry::of("Pavilion Main").meets(3, "2.2")),
        ),
        recipe(
            millimetres,
            "Set the table width",
            Perform::Tier(TierEntry::of("Table").meets(5, "3.2")),
        ),
        recipe(
            millimetres,
            "Set the girdle thickness",
            Perform::Tier(TierEntry::of("Girdle").meets(4, "0.3")),
        ),
        // The design to adopt from is one the learner opens from a file.
        skip(adopt, "Open an imported design"),
        skip(adopt, "Adopt one tier"),
        recipe(
            adopt,
            "Adopt the rest at once",
            Perform::command(CMD_ADOPT_ALL),
        ),
        recipe(
            pin,
            "Let the solver decide a depth",
            Perform::Tier(TierEntry::of("Upper Girdle").meets(1, "Lower Girdle")),
        ),
        // Pin is a button of the table row.
        skip(pin, "Pin the solved depth"),
    ]
}

fn series() -> Vec<TableEntry> {
    let plain = "tiers-step-series";
    let linked = "tiers-linked-series";
    let mirror = "tiers-mirror-to-other-block";
    let relations = "tiers-relations";
    let arithmetic = "tiers-arithmetic";
    vec![
        recipe(plain, "Generate a crown ladder", ladder(false)),
        recipe(linked, "Generate a linked ladder", ladder(true)),
        // The first rung drives the others, so the edit goes to it alone.
        recipe(
            linked,
            "Move the first rung",
            Perform::Tier(TierEntry::of("Step1").angle("32")),
        ),
        recipe(
            linked,
            "Free a rung",
            Perform::ClearRelation("Step3".to_owned()),
        ),
        recipe(
            mirror,
            "Mirror the pavilion to the crown side",
            Perform::MirrorTier {
                tier: "Pavilion Main".to_owned(),
                suffix: "'".to_owned(),
            },
        ),
        recipe(
            mirror,
            "Mirror a crown tier",
            Perform::MirrorTier {
                tier: "Crown Main".to_owned(),
                suffix: "b".to_owned(),
            },
        ),
        recipe(
            relations,
            "Make Pavilion Main follow Crown Main",
            Perform::Tier(TierEntry::of("Pavilion Main").angle("=[Crown Main]+6.5")),
        ),
        // Pavilion Main moves by itself: only the driver is edited.
        recipe(
            relations,
            "Move the driver",
            Perform::Tier(TierEntry::of("Crown Main").angle("36")),
        ),
        recipe(
            relations,
            "Stop following",
            Perform::ClearRelation("Pavilion Main".to_owned()),
        ),
        recipe(
            arithmetic,
            "Calculate an angle",
            Perform::Tier(TierEntry::of("Crown Main").angle("34.5+0.3")),
        ),
        recipe(
            arithmetic,
            "Calculate a scale value",
            Perform::Tier(TierEntry::of("Table").meets(2, "0.5-0.15")),
        ),
        // The position box under the facet chips.
        skip(arithmetic, "Calculate a facet position"),
    ]
}

fn editing() -> Vec<TableEntry> {
    let cheater = "tiers-cheater-offset";
    let notes = "tiers-tier-notes";
    let duplicate = "tiers-duplicate";
    let moving = "tiers-move";
    let delete = "tiers-delete";
    let multi = "tiers-multi-select";
    vec![
        recipe(
            cheater,
            "Set an offset",
            Perform::CheaterOffset {
                tier: "Crown Main".to_owned(),
                text: "0.5".to_owned(),
            },
        ),
        recipe(
            cheater,
            "Clear it",
            Perform::CheaterOffset {
                tier: "Crown Main".to_owned(),
                text: String::new(),
            },
        ),
        recipe(
            notes,
            "Add a note",
            Perform::TierNote {
                tier: "Pavilion Main".to_owned(),
                text: "check the meet here".to_owned(),
            },
        ),
        recipe(
            notes,
            "Clear the note",
            Perform::TierNote {
                tier: "Pavilion Main".to_owned(),
                text: String::new(),
            },
        ),
        recipe(
            duplicate,
            "Duplicate a tier",
            Perform::select("Crown Main").then(Perform::command(CMD_DUPLICATE)),
        ),
        recipe(moving, "Move a tier up", Perform::move_up("Girdle", 1)),
        // Far more places than the table has rows: the row stops at the top.
        recipe(
            moving,
            "Put the pavilion first",
            Perform::move_up("Pavilion Main", 12),
        ),
        recipe(delete, "Remove a tier", Perform::delete("Table")),
        recipe(delete, "Take it back", Perform::undo()),
        // Ticking rows is a gesture of the table.
        skip(multi, "Pick two tiers"),
        recipe(
            multi,
            "Delete them together",
            Perform::delete("Crown Main").then(Perform::delete("Pavilion Main")),
        ),
    ]
}

fn concave() -> Vec<TableEntry> {
    let cylinder = "tiers-concave-cylinder-cone";
    let sphere = "tiers-concave-sphere-disc";
    vec![
        recipe(
            cylinder,
            "Add a cylinder groove",
            add_concave(groove(
                "Groove",
                -42.0,
                &[0.0, 24.0, 48.0, 72.0],
                ConcaveTool::Cylinder,
                None,
                0.25,
                ToolMotion::Reciprocating,
            )),
        ),
        recipe(
            cylinder,
            "Add a cone",
            add_concave(groove(
                "Bevel",
                -44.0,
                &[6.0, 30.0, 54.0, 78.0],
                ConcaveTool::Cone,
                Some(60.0),
                0.3,
                ToolMotion::Plunge,
            )),
        ),
        recipe(
            sphere,
            "Add a sphere dimple",
            add_concave(groove(
                "Dimple",
                -40.0,
                &[0.0, 24.0, 48.0, 72.0],
                ConcaveTool::Sphere,
                None,
                0.2,
                ToolMotion::Plunge,
            )),
        ),
        recipe(
            sphere,
            "Add a disc",
            add_concave(groove(
                "Wheel",
                -38.0,
                &[12.0, 36.0, 60.0, 84.0],
                ConcaveTool::Disc,
                Some(90.0),
                2.0,
                ToolMotion::Reciprocating,
            )),
        ),
        recipe(
            sphere,
            "Switch a tool to plunge",
            Perform::ConcaveTier {
                edit: Some("Wheel".to_owned()),
                tier: Box::new(groove(
                    "Wheel",
                    -38.0,
                    &[12.0, 36.0, 60.0, 84.0],
                    ConcaveTool::Disc,
                    Some(90.0),
                    2.0,
                    ToolMotion::Plunge,
                )),
            },
        ),
    ]
}
