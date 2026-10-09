//! What the Rough Planner needs to colour its previews and to turn stones (`zoning` feature
//! only).
//!
//! The plan's stored colour and the cutter's pose choice as plain data, the colour of a
//! planned stone, and the "Stone orientation" option on the results.
//!
//! Window-free and outside `gui::rough_plan` (the planner's code is never run by the tests): the
//! planner's view calls [`load_inputs`] when its results change, hands the [`PreviewInputs`] to its
//! worker threads, and they call [`posed_layout`] and [`stone_colours`]. The draw is the
//! rasteriser's, which paints flat facet colours, so the colour of a stone is the colour it shows
//! face-up: the stone's preview material (`store::planner_stone_material`, the host with the
//! rough's zones moved into the stone's frame) is read for its zones, and each zone's share of the
//! face-up path comes from the same cheap predictor the pose choice uses.

use super::store::{
    AdoptOutcome, AdoptRequest, HostMaterial, PoseChoices, adopt_stone, choose_and_save_poses,
    load_pose_choices, load_rough_colour, posed_stone_pose,
};
use crate::gui::optics::crystal_optics::gem_material_from_row;
use anyhow::Result;
use indicatrix::{
    color::{Illuminant, body_colors},
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::GemMaterial,
        zoning::ZonedAbsorption,
    },
};
use indicatrix_cut_core::rough_plan::{
    PlacedStone, RoughLayout,
    zoned_plan::{
        DesignPlacement, PoseGoal, StonePlacement, face_up_prediction, stone_preview_material,
    },
};
use indicatrix_vault::{db::sqlite::Database, model::material::CustomMaterialRow};

/// The host material of a plan, owned (a custom row is kept as the row, because adopting copies
/// the row, not the material built from it).
#[derive(Debug, Clone)]
pub enum OwnedHost {
    /// A catalogue (custom) material.
    Custom(CustomMaterialRow),
    /// A built-in preset.
    BuiltIn(GemMaterial),
}

impl OwnedHost {
    /// The host as the material the preview is built from.
    #[must_use]
    pub fn material(&self) -> GemMaterial {
        match self {
            Self::Custom(row) => gem_material_from_row(row),
            Self::BuiltIn(material) => material.clone(),
        }
    }

    /// The host as `store::adopt_stone` takes it.
    #[must_use]
    pub const fn as_host(&self) -> HostMaterial<'_> {
        match self {
            Self::Custom(row) => HostMaterial::Custom(row),
            Self::BuiltIn(material) => HostMaterial::BuiltIn(material),
        }
    }
}

/// The host material called `name` (compared ASCII case-insensitively): a custom material first
/// (the planner's list shows those under their own names), else a built-in preset.
///
/// `None` for a
/// name that is neither, or when the vault cannot be read.
#[must_use]
pub fn resolve_host(db: &Database, name: &str) -> Option<OwnedHost> {
    let custom = db
        .get_custom_materials()
        .ok()?
        .into_iter()
        .find(|row| row.name.eq_ignore_ascii_case(name));
    if let Some(row) = custom {
        return Some(OwnedHost::Custom(row));
    }
    GemMaterial::by_name(name).map(OwnedHost::BuiltIn)
}

/// Everything the previews of one saved plan need.
#[derive(Debug, Clone)]
pub struct PreviewInputs {
    /// The plan's saved plan id.
    pub plan_id: i64,
    /// The rough's zones, rough frame, mm.
    pub rough_zoned: ZonedAbsorption,
    /// The plan's host material.
    pub host: GemMaterial,
    /// The cutter's stored pose choices.
    pub choices: PoseChoices,
}

/// The inputs of plan `plan_id`, or `None` when it has no rough colour or its host is unknown.
///
/// # Errors
///
/// The stored colour or the pose choices cannot be read.
pub fn load_inputs(db: &Database, plan_id: i64, host_name: &str) -> Result<Option<PreviewInputs>> {
    let Some(colour) = load_rough_colour(db, plan_id)? else {
        return Ok(None);
    };
    let Some(host) = resolve_host(db, host_name) else {
        return Ok(None);
    };
    Ok(Some(PreviewInputs {
        plan_id,
        rough_zoned: colour.zoned,
        host: host.material(),
        choices: load_pose_choices(db, plan_id)?,
    }))
}

/// `layout` (result number `layout_index`) with every stone in the pose the cutter chose.
#[must_use]
pub fn posed_layout(layout: &RoughLayout, layout_index: u32, choices: &PoseChoices) -> RoughLayout {
    let mut out = layout.clone();
    for (index, stone) in out.stones.iter_mut().enumerate() {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        stone.pose = posed_stone_pose(choices, layout_index, index, stone);
    }
    out
}

/// The colour `stone` (already in its chosen pose) shows face-up, from the plan's zones.
///
/// Each zone's absorption weighted by its share of the face-up path (the predictor of the pose
/// choice), seen under D65 over the face-up path length. `None` when the placement is unusable.
#[must_use]
pub fn stone_colour(
    inputs: &PreviewInputs,
    stone: &PlacedStone,
    design: DesignPlacement,
) -> Option<[u8; 3]> {
    let placement = StonePlacement {
        pose: stone.pose,
        design,
    };
    let material = stone_preview_material(&inputs.host, &inputs.rough_zoned, &placement)?;
    let zoning = material.zoning.as_ref()?;
    let face_up = face_up_prediction(&inputs.rough_zoned, stone, &stone.pose);
    let mut bands: Vec<AbsorptionBand> = Vec::new();
    for (index, share) in face_up.fractions.iter().enumerate() {
        if *share <= 0.0 {
            continue;
        }
        let Some(zone) = zoning.zone_absorption(index) else {
            continue;
        };
        bands.extend(zone.tensor.o_ray.iter().map(|band| AbsorptionBand {
            peak: band.peak * *share as f32,
            ..*band
        }));
    }
    let tensor = AbsorptionTensor::isotropic(bands);
    Some(
        body_colors(&tensor, face_up.path_mm, Illuminant::D65)
            .unpolarised
            .srgb,
    )
}

/// The colour of every stone of `layout` (already posed), in stone order.
///
/// `None` for a stone whose design placement `placement_of` does not know or whose placement is unusable (it keeps
/// the palette colour).
#[must_use]
pub fn stone_colours(
    inputs: &PreviewInputs,
    layout: &RoughLayout,
    placement_of: &dyn Fn(i64) -> Option<DesignPlacement>,
) -> Vec<Option<[u8; 3]>> {
    layout
        .stones
        .iter()
        .map(|stone| {
            let design = placement_of(stone.entry_id)?;
            stone_colour(inputs, stone, design)
        })
        .collect()
}

// ---- adopting a stone ------------------------------------------------------------------------

/// The stone a "Use colour" click adopts.
#[derive(Debug, Clone, Copy)]
pub struct AdoptTarget<'a> {
    /// The plan's name: the stem of the new material's name.
    pub rough_name: &'a str,
    /// The result the stone is in.
    pub layout: &'a RoughLayout,
    /// The result's number (its place in the plan's results, from 0).
    pub layout_index: u32,
    /// The stone's place in the result.
    pub stone_index: usize,
    /// How the stone's design sits in the planner's caliper frame.
    pub design: DesignPlacement,
    /// The design's caliper width in model units (the width of its mesh along x).
    pub design_width_units: f64,
}

/// Adopts one planned stone: the stone in the pose the cutter chose, its colour as the custom
/// material "<rough> colour" (`store::adopt_stone`).
///
/// The returned stone width is the design's real
/// width (`design_width_units` times the pose's millimetres per unit); the caller sets the editor's
/// stone width to it.
///
/// # Errors
///
/// The result has no such stone, or `store::adopt_stone` refuses (an unusable pose, a built-in
/// name, a vault write).
pub fn adopt(
    db: &Database,
    host: &OwnedHost,
    inputs: &PreviewInputs,
    target: &AdoptTarget<'_>,
) -> Result<AdoptOutcome> {
    let Some(stone) = target.layout.stones.get(target.stone_index) else {
        anyhow::bail!("This result has no stone {}", target.stone_index + 1);
    };
    let stone_index = u32::try_from(target.stone_index).unwrap_or(u32::MAX);
    let pose = posed_stone_pose(&inputs.choices, target.layout_index, stone_index, stone);
    adopt_stone(
        db,
        &AdoptRequest {
            rough_name: target.rough_name,
            rough_zoned: &inputs.rough_zoned,
            placement: StonePlacement {
                pose,
                design: target.design,
            },
            stone_width_mm: target.design_width_units * pose.mm_per_unit,
            host: host.as_host(),
        },
    )
}

// ---- the "Stone orientation" option ------------------------------------------------------------

/// The choice on the results: "Keep planner pose" or "Best colour".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PoseOption {
    /// Every stone keeps the pose the planner gave it.
    #[default]
    Keep,
    /// Every stone takes the box-symmetric pose with the most of the innermost zone face-up.
    BestColour,
}

impl PoseOption {
    /// The value of `Zoning.pose_option`.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Keep => 0,
            Self::BestColour => 1,
        }
    }

    /// The option a `Zoning.pose_option_changed` value names.
    #[must_use]
    pub const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Keep),
            1 => Some(Self::BestColour),
            _ => None,
        }
    }

    /// The option the stored choices stand for: any stone turned away from the planner's pose
    /// means "Best colour" was chosen (the vault keeps only non-canonical choices).
    #[must_use]
    pub fn of_choices(choices: &PoseChoices) -> Self {
        if choices.is_empty() {
            Self::Keep
        } else {
            Self::BestColour
        }
    }

    /// What the pose choice optimises: the share of the innermost zone (the last one, which is the
    /// core of a watermelon) face-up. A rough without shaped zones has nothing to choose.
    #[must_use]
    pub const fn goal(self, zoned: &ZonedAbsorption) -> PoseGoal {
        match self {
            Self::Keep => PoseGoal::KeepCanonical,
            Self::BestColour if zoned.zones.is_empty() => PoseGoal::KeepCanonical,
            Self::BestColour => PoseGoal::MostOfZone(zoned.zones.len()),
        }
    }
}

/// Stores the pose option for every layout of plan `plan_id` (`layouts` are the plan's results in
/// rank order, as the planner shows them) and returns the stored choices afterwards.
///
/// # Errors
///
/// A vault write or read fails.
pub fn apply_pose_option(
    db: &Database,
    plan_id: i64,
    layouts: &[RoughLayout],
    zoned: &ZonedAbsorption,
    option: PoseOption,
) -> Result<PoseChoices> {
    let goal = option.goal(zoned);
    for (index, layout) in layouts.iter().enumerate() {
        let index = u32::try_from(index).unwrap_or(u32::MAX);
        choose_and_save_poses(db, plan_id, index, layout, zoned, goal)?;
    }
    load_pose_choices(db, plan_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::rough_colour::store::save_rough_colour;
    use glam::DVec3;
    use indicatrix::optics::zoning::{Zone, ZoneAbsorption, ZoneShape};
    use indicatrix_cut_core::rough_plan::{
        Axis, CutOrder, CutPlan,
        fit::StonePose,
        zoned_plan::{RoughColour, ZONING_FORMAT_VERSION},
    };

    fn db() -> Database {
        Database::new(Some(":memory:")).expect("in-memory vault")
    }

    fn absorber(centre_nm: f32, peak: f32) -> ZoneAbsorption {
        ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
            centre_nm, 40.0, peak,
        )]))
    }

    /// A pale rind with a strongly red rod along x through `(y, z) = (-3, 0)`, 3 mm in radius.
    fn watermelon() -> ZonedAbsorption {
        let mut zoned = ZonedAbsorption::new(absorber(480.0, 0.02));
        zoned.zones.push(Zone {
            shape: ZoneShape::CoaxialCylinder {
                axis_point: DVec3::new(0.0, -3.0, 0.0),
                axis_dir: DVec3::X,
                r_in: 0.0,
                r_out: 3.0,
            },
            absorption: absorber(600.0, 0.9),
        });
        zoned
    }

    fn stone(centre_y: f64) -> PlacedStone {
        PlacedStone {
            entry_id: 1,
            piece_origin_mm: [-3.0; 3],
            piece_size_mm: [6.0; 3],
            stone_size_mm: [6.0; 3],
            table_axis: Axis::Y,
            carat: 1.0,
            volume_mm3: 100.0,
            pose: StonePose {
                center_mm: [0.0, centre_y, 0.0],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            },
        }
    }

    fn layout(stones: Vec<PlacedStone>, exact_fit: bool) -> RoughLayout {
        RoughLayout {
            cut_order: CutOrder::Xyz,
            stones,
            cut_plan: CutPlan { slabs: Vec::new() },
            total_carat: 1.0,
            total_volume_mm3: 100.0,
            yield_fraction: 0.5,
            exact_fit,
        }
    }

    fn inputs(choices: PoseChoices) -> PreviewInputs {
        PreviewInputs {
            plan_id: 1,
            rough_zoned: watermelon(),
            host: GemMaterial::diamond(),
            choices,
        }
    }

    #[test]
    fn a_plan_without_a_colour_has_no_inputs_and_one_with_a_colour_has_them() {
        let db = db();
        assert!(load_inputs(&db, 3, "Quartz").unwrap().is_none());
        save_rough_colour(
            &db,
            3,
            &RoughColour {
                zoned: watermelon(),
                fit_json: "{}".to_string(),
                version: ZONING_FORMAT_VERSION,
                created: 0,
            },
        )
        .unwrap();
        let loaded = load_inputs(&db, 3, "Quartz").unwrap().expect("a colour");
        assert_eq!(loaded.plan_id, 3);
        assert_eq!(loaded.rough_zoned.zones.len(), 1);
        assert!(loaded.choices.is_empty());
        // An unknown host leaves the previews alone.
        assert!(load_inputs(&db, 3, "Unobtainium").unwrap().is_none());
    }

    #[test]
    fn the_host_is_a_custom_material_first_then_a_built_in() {
        let db = db();
        assert!(matches!(
            resolve_host(&db, "Quartz"),
            Some(OwnedHost::BuiltIn(_))
        ));
        assert!(resolve_host(&db, "Unobtainium").is_none());
    }

    #[test]
    fn the_core_shows_red_and_the_rind_shows_pale() {
        let inputs = inputs(PoseChoices::new());
        // A stone in the core (the rod is at y = -3, the table normal is +y, the path runs from
        // the table down through the rod) against one in the pale rind far above it.
        let in_core = stone_colour(&inputs, &stone(-3.0), DesignPlacement::IDENTITY).unwrap();
        let in_rind = stone_colour(&inputs, &stone(30.0), DesignPlacement::IDENTITY).unwrap();
        // Red absorbs green and blue less than... a strong 600 nm absorber lowers the red/green
        // channels: the core is darker and differs from the rind.
        let sum = |c: [u8; 3]| u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2]);
        assert!(sum(in_core) < sum(in_rind), "{in_core:?} vs {in_rind:?}");
        assert_ne!(in_core, in_rind);
    }

    #[test]
    fn an_unusable_placement_has_no_colour() {
        let inputs = inputs(PoseChoices::new());
        let bad = DesignPlacement {
            width_dir: [0.0, 0.0],
            centre_units: [0.0; 3],
        };
        assert!(stone_colour(&inputs, &stone(0.0), bad).is_none());
        let all = stone_colours(&inputs, &layout(vec![stone(0.0)], false), &|_| None);
        assert_eq!(all, vec![None]);
    }

    #[test]
    fn colours_come_in_stone_order() {
        let inputs = inputs(PoseChoices::new());
        let layout = layout(vec![stone(-3.0), stone(30.0)], false);
        let all = stone_colours(&inputs, &layout, &|_| Some(DesignPlacement::IDENTITY));
        assert_eq!(all.len(), 2);
        assert_ne!(all[0], all[1]);
    }

    #[test]
    fn a_stored_pose_choice_turns_the_drawn_stone() {
        let base = layout(vec![stone(0.0), stone(0.0)], false);
        let mut choices = PoseChoices::new();
        choices.insert((2, 1), 1);
        let posed = posed_layout(&base, 2, &choices);
        assert_eq!(posed.stones[0].pose, base.stones[0].pose);
        assert!(
            (posed.stones[1].pose.axes[1][1] + 1.0).abs() < 1e-12,
            "stone 1 of result 2 is turned over"
        );
        // Another result keeps the planner's poses.
        let kept = posed_layout(&base, 0, &choices);
        for (a, b) in kept.stones.iter().zip(&base.stones) {
            assert_eq!(a.pose, b.pose);
        }
    }

    #[test]
    fn the_pose_option_is_stored_for_every_layout_and_read_back() {
        let db = db();
        let zoned = watermelon();
        let layouts = vec![
            layout(vec![stone(0.0), stone(0.0)], false),
            layout(vec![stone(0.0)], true),
            layout(vec![stone(0.0)], false),
        ];
        // The default stores nothing.
        let none = apply_pose_option(&db, 5, &layouts, &zoned, PoseOption::Keep).unwrap();
        assert!(none.is_empty());
        assert_eq!(PoseOption::of_choices(&none), PoseOption::Keep);
        // "Best colour" turns the stones of the layouts that may turn; an exact fit never turns.
        let best = apply_pose_option(&db, 5, &layouts, &zoned, PoseOption::BestColour).unwrap();
        assert_eq!(best.get(&(0, 0)), Some(&1));
        assert_eq!(best.get(&(0, 1)), Some(&1));
        assert_eq!(best.get(&(1, 0)), None, "an exact fit keeps its pose");
        assert_eq!(best.get(&(2, 0)), Some(&1));
        assert_eq!(PoseOption::of_choices(&best), PoseOption::BestColour);
        // The choice survives a reload of the plan (a fresh read of the vault).
        assert_eq!(load_pose_choices(&db, 5).unwrap(), best);
        // Back to the default clears every layout.
        let again = apply_pose_option(&db, 5, &layouts, &zoned, PoseOption::Keep).unwrap();
        assert!(again.is_empty());
    }

    #[test]
    fn a_rough_without_zones_has_nothing_to_turn() {
        let db = db();
        let plain = ZonedAbsorption::new(absorber(480.0, 0.1));
        let layouts = vec![layout(vec![stone(0.0)], false)];
        let stored = apply_pose_option(&db, 6, &layouts, &plain, PoseOption::BestColour).unwrap();
        assert!(stored.is_empty());
        assert_eq!(PoseOption::BestColour.goal(&plain), PoseGoal::KeepCanonical);
    }

    #[test]
    fn adopting_uses_the_chosen_pose_and_the_designs_real_width() {
        let db = db();
        let zoned = watermelon();
        let host =
            OwnedHost::BuiltIn(GemMaterial::by_name("Quartz").unwrap_or_else(GemMaterial::diamond));
        let layout = layout(vec![stone(0.0)], false);
        let target = AdoptTarget {
            rough_name: "Melon",
            layout: &layout,
            layout_index: 4,
            stone_index: 0,
            design: DesignPlacement::IDENTITY,
            design_width_units: 3.0,
        };

        // The planner's pose: the width is the design's width in the stone's scale (2 mm/unit).
        let mut plain = inputs(PoseChoices::new());
        plain.rough_zoned = zoned.clone();
        let mut scaled_layout = layout.clone();
        scaled_layout.stones[0].pose.mm_per_unit = 2.0;
        let scaled = AdoptTarget {
            layout: &scaled_layout,
            ..target
        };
        let kept = adopt(&db, &host, &plain, &scaled).unwrap();
        assert_eq!(kept.material_name, "Melon colour");
        assert!((kept.stone_width_mm - 6.0).abs() < 1e-9, "3 units x 2 mm");

        // With the stone turned over by the stored choice, the zones land in the stone differently.
        let mut turned_choices = PoseChoices::new();
        turned_choices.insert((4, 0), 1);
        let mut turned = inputs(turned_choices);
        turned.rough_zoned = zoned;
        let flipped = adopt(&db, &host, &turned, &scaled).unwrap();
        assert!(flipped.replaced, "the same name is replaced");
        assert_ne!(
            kept.material.zoning, flipped.material.zoning,
            "the pose choice reaches the adopted zones"
        );

        // A stone the result does not have is refused with a sentence.
        let missing = AdoptTarget {
            stone_index: 3,
            ..scaled
        };
        assert!(adopt(&db, &host, &plain, &missing).is_err());
    }

    #[test]
    fn the_option_values_match_the_ui() {
        assert_eq!(PoseOption::Keep.index(), 0);
        assert_eq!(PoseOption::BestColour.index(), 1);
        assert_eq!(PoseOption::from_index(1), Some(PoseOption::BestColour));
        assert_eq!(PoseOption::from_index(7), None);
        assert_eq!(PoseOption::default(), PoseOption::Keep);
    }
}
