//! Tests for [`super`].

use super::*;
use crate::{
    BuiltinMaterials, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Edit, ManufacturabilityWarning,
    ObjectiveFidelity, RemapRounding, check_manufacturability,
    design::cutting_order::flat_cutting_order, evaluate_objective_under,
};
use indicatrix::{
    geometry::{
        GpuFacetPlane,
        stone_metrics::{SolidStatus, StoneProportions, build_solid_mesh, measure_solid},
    },
    optics::{LightingPreset, materials::GemMaterial},
};

/// The gears the New Design dialog offers besides a free "custom" entry
/// (`indicatrix_editor::material::GEAR_PRESETS`).
const DIALOG_GEARS: [i32; 6] = [96, 80, 77, 72, 64, 120];

/// `spec` on `preform` at its own gear, as a design.
fn design_on(spec: &TemplateSpec, preform: PreformSpec) -> Design {
    Design::new(preform, spec.schedule_meta(), spec.tiers())
}

/// `spec` on its default preform, remapped to `gear` the way the gear-remap panel
/// does it: one batch of the index remap and the schedule change.
fn remapped(spec: &TemplateSpec, gear: i32) -> Design {
    let mut design = design_on(spec, spec.preform);
    design
        .apply_edit(Edit::Batch(vec![
            Edit::RemapIndices {
                from_gear: spec.gear_teeth,
                to_gear: gear,
                rounding: RemapRounding::Nearest,
            },
            Edit::SetSchedule {
                gear_teeth: gear,
                symmetry_order: spec.symmetry_order,
                mirror: spec.mirror,
            },
        ]))
        .unwrap_or_else(|e| panic!("{:?}: remap to {gear} failed: {e:?}", spec.name));
    design
}

/// What the checks below read off one solved stone.
struct Inspected {
    proportions: StoneProportions,
    warnings: Vec<ManufacturabilityWarning>,
    planes: Vec<(glam::DVec3, f64)>,
}

impl Inspected {
    fn vanishing(&self) -> Vec<String> {
        self.warnings
            .iter()
            .filter_map(|w| match w {
                ManufacturabilityWarning::VanishingFacet { tier_name, .. } => {
                    Some(tier_name.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn undersized(&self) -> Vec<String> {
        self.warnings
            .iter()
            .filter_map(|w| match w {
                ManufacturabilityWarning::UndersizedFacet { tier_name, .. } => {
                    Some(tier_name.clone())
                }
                _ => None,
            })
            .collect()
    }
}

/// Solves, meshes and measures `design`; panics with `label` when it does not
/// solve or close.
fn inspect(design: &Design, label: &str) -> Inspected {
    let solved = design
        .solve()
        .unwrap_or_else(|e| panic!("{label}: failed to solve: {e:?}"));
    let planes = design.planes_from_solved(&solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("{label}: solved but is not a closed solid");
    };
    let metrics =
        measure_solid(&planes).unwrap_or_else(|| panic!("{label}: the solid has no metrics"));
    Inspected {
        proportions: StoneProportions::from_solid(&metrics, &mesh, &planes),
        warnings: check_manufacturability(design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2),
        planes,
    }
}

/// The shapes whose facets are all held to the strict standard (alive and big enough
/// to cut). The standard round brilliant's culet is a pin-prick by design and the
/// teaching variants have known dead facets, so they are only held to "closed".
fn is_strict(spec: &TemplateSpec) -> bool {
    spec.featured && spec.family != ShapeFamily::Round
}

/// Every template in the gallery must build a non-empty tier table, and --
/// the point of this module's whole "`ScaleReference`-only" restriction --
/// solve and close as a real solid, the same standard this crate already
/// holds `ConstraintTier::standard_round_brilliant` to
/// (`standard_round_brilliant_template_solves_and_closes`, `tier.rs`): on the
/// preform the New Design dialog gives it AND on the 1.5 x 1.0 x 1.5 cylinder
/// the web app and the worked-example guide use for every template.
#[test]
fn every_template_solves_and_closes() {
    for spec in TEMPLATES {
        let tiers = spec.tiers();
        assert!(
            !tiers.is_empty(),
            "template {:?} built an empty tier table",
            spec.name
        );
        for (label, preform) in [
            ("its own preform", spec.preform),
            (
                "the legacy cylinder",
                PreformSpec::cylinder(spec.gear_teeth.unsigned_abs() as usize, 1.5, 1.0, 1.5),
            ),
        ] {
            let design = design_on(spec, preform);
            design.solve().unwrap_or_else(|e| {
                panic!("template {:?} failed to solve on {label}: {e:?}", spec.name)
            });
            assert!(
                design.is_closed(),
                "template {:?} solved but is not a closed solid on {label}",
                spec.name
            );
        }
    }
}

/// Girdle alive, a table, and the outline's length-to-width ratio within 5 % of
/// the preform's, for every template on its default preform.
#[test]
fn every_template_has_a_girdle_a_table_and_its_intended_outline() {
    for spec in TEMPLATES {
        let stone = inspect(&design_on(spec, spec.preform), spec.name);
        let girdle = stone.proportions.girdle_thickness;
        assert!(
            girdle.is_some_and(|g| g > 0.0),
            "{:?}: no live girdle ({girdle:?})",
            spec.name
        );
        assert!(
            stone.proportions.table_percent.is_some(),
            "{:?}: no table facet",
            spec.name
        );
        let wanted = spec.preform.length_over_width;
        let got = stone.proportions.length_to_width.expect("a positive width");
        assert!(
            (got / wanted - 1.0).abs() <= 0.05,
            "{:?}: length/width {got:.3} is not within 5 % of {wanted:.3}",
            spec.name
        );
    }
}

/// The four shapes added after the round brilliant: every facet reaches the
/// surface, none is too small to cut, the table is a reasonable share of the
/// width, the girdle is neither missing nor thick, and the preform does not touch
/// the stone (its planes are not part of any ring).
#[test]
fn the_new_shapes_have_no_vanishing_or_undersized_facets() {
    for spec in TEMPLATES.iter().filter(|spec| is_strict(spec)) {
        let design = design_on(spec, spec.preform);
        let stone = inspect(&design, spec.name);
        assert_eq!(
            stone.vanishing(),
            Vec::<String>::new(),
            "{:?}: facets that never reach the surface",
            spec.name
        );
        assert_eq!(
            stone.undersized(),
            Vec::<String>::new(),
            "{:?}: facets below the cuttable size",
            spec.name
        );
        let table = stone.proportions.table_percent.expect("a table facet");
        assert!(
            (35.0..=80.0).contains(&table),
            "{:?}: table is {table:.1} % of the width",
            spec.name
        );
        let girdle = stone
            .proportions
            .girdle_to_width_percent
            .expect("a live girdle");
        assert!(
            (1.0..=9.0).contains(&girdle),
            "{:?}: girdle is {girdle:.1} % of the width",
            spec.name
        );
        let depth = stone.proportions.total_depth;
        assert!(
            depth < spec.preform.depth,
            "{:?}: the stone ({depth:.2}) does not fit the {:.2} tall preform",
            spec.name,
            spec.preform.depth
        );
    }
}

/// No catastrophic windowing: the fast table-up figures of each featured shape
/// in its own default material stay sane. A broad band, not a quality score --
/// it exists to catch a pavilion that leaks most of the light.
#[test]
fn featured_shapes_do_not_window_catastrophically_in_their_default_material() {
    for spec in TEMPLATES.iter().filter(|spec| spec.featured) {
        let stone = inspect(&design_on(spec, spec.preform), spec.name);
        let gem = GemMaterial::by_name(spec.default_material)
            .unwrap_or_else(|| panic!("{:?}: unknown material", spec.default_material));
        let gpu: Vec<GpuFacetPlane> = stone
            .planes
            .iter()
            .map(|&(n, m)| GpuFacetPlane::new(n.as_vec3(), -m as f32))
            .collect();
        let figures = evaluate_objective_under(
            &gpu,
            &gem,
            ObjectiveFidelity::Fast,
            LightingPreset::RingLights,
        );
        assert!(
            figures.windowing_pct.is_finite() && figures.windowing_pct < 60.0,
            "{:?}: windowing {:.1} %",
            spec.name,
            figures.windowing_pct
        );
        assert!(
            figures.tilt_brilliance_pct > 5.0,
            "{:?}: brilliance {:.1} %",
            spec.name,
            figures.tilt_brilliance_pct
        );
    }
}

/// Each template's design index is the index of its default material, and the
/// material exists.
#[test]
fn the_design_index_is_the_default_materials_index() {
    for spec in TEMPLATES {
        let resolved = spec.default_material_selection().resolve(&BuiltinMaterials);
        assert!(
            GemMaterial::by_name(spec.default_material).is_some(),
            "{:?}: {:?} is not a built-in material",
            spec.name,
            spec.default_material
        );
        assert!(
            (resolved.n_d - spec.design_ri).abs() < 0.005,
            "{:?}: designed for n {} but {} has n {}",
            spec.name,
            spec.design_ri,
            spec.default_material,
            resolved.n_d
        );
    }
}

/// The featured group is the top five shapes, in the order the dialog shows them,
/// and everything else is the second group.
#[test]
fn the_featured_shapes_are_the_top_five_in_order() {
    let featured: Vec<(&str, ShapeFamily)> = TEMPLATES
        .iter()
        .filter(|spec| spec.featured)
        .map(|spec| (spec.name, spec.family))
        .collect();
    assert_eq!(
        featured,
        vec![
            ("Standard Round Brilliant", ShapeFamily::Round),
            ("Oval Brilliant", ShapeFamily::Oval),
            ("Cushion Brilliant", ShapeFamily::Cushion),
            ("Emerald Step Cut", ShapeFamily::Emerald),
            ("Princess Cut", ShapeFamily::Princess),
        ]
    );
    assert_eq!(TEMPLATES.iter().filter(|spec| !spec.featured).count(), 4);
    // The templates that existed before the new shapes keep their positions.
    let first_five: Vec<&str> = TEMPLATES.iter().take(5).map(|spec| spec.name).collect();
    assert_eq!(
        first_five,
        [
            "Standard Round Brilliant",
            "Round Brilliant -- Shallow Pavilion",
            "Round Brilliant -- Deep Pavilion",
            "Simple Teaching Design",
            "Rich Teaching Design",
        ]
    );
}

/// The template's own gear is always allowed, and so is every gear the symmetry
/// rule accepts -- the dialog offers exactly these.
#[test]
fn allowed_gears_follow_the_symmetry_rule() {
    for spec in TEMPLATES {
        assert!(spec.allows_gear(spec.gear_teeth), "{:?}", spec.name);
        let allowed = spec.allowed_gears_among(&DIALOG_GEARS);
        let expected: Vec<i32> = if spec.family == ShapeFamily::Emerald {
            // Only multiples of 12 teeth are used, so 72 and 120 land on whole teeth too.
            vec![96, 80, 72, 64, 120]
        } else {
            vec![96, 80, 64]
        };
        assert_eq!(allowed, expected, "{:?}", spec.name);
        assert_eq!(allowed.first(), Some(&96));
        // 77 is not a multiple of any symmetry order above 1.
        assert!(!spec.allows_gear(77), "{:?}", spec.name);
    }
}

/// Tie and collapse rules, on the oval: 72 and 120 put the star positions half way
/// between two teeth; 48 puts the break positions there; 24 and 32 make neighbouring
/// positions of one tier land on the same tooth; 0 and negative counts are refused.
#[test]
fn a_gear_that_breaks_the_index_layout_is_refused() {
    let oval = TEMPLATES
        .iter()
        .find(|spec| spec.family == ShapeFamily::Oval)
        .expect("an oval template");
    for gear in [72, 120, 48, 24, 32, 0, -96, 97] {
        assert!(!oval.allows_gear(gear), "gear {gear}");
    }
    for gear in [96, 80, 64] {
        assert!(oval.allows_gear(gear), "gear {gear}");
    }
    // Candidates are filtered in the order given, the template's gear first.
    assert_eq!(oval.allowed_gears_among(&[64, 120, 80]), vec![96, 64, 80]);
    assert_eq!(oval.allowed_gears_among(&[]), vec![96]);
}

/// Remapped onto each gear the dialog offers, every template still solves and
/// closes with an unchanged outline; the new shapes keep every facet, big enough to
/// cut. This is what makes "allowed" a promise rather than a guess.
#[test]
fn every_allowed_gear_keeps_the_stone_intact() {
    for spec in TEMPLATES {
        for gear in spec.allowed_gears_among(&DIALOG_GEARS) {
            let label = format!("{} on gear {gear}", spec.name);
            let design = remapped(spec, gear);
            assert_eq!(design.meta.gear_teeth, gear, "{label}");
            let stone = inspect(&design, &label);
            let wanted = spec.preform.length_over_width;
            let got = stone.proportions.length_to_width.expect("a positive width");
            assert!(
                (got / wanted - 1.0).abs() <= 0.05,
                "{label}: length/width {got:.3} is not within 5 % of {wanted:.3}"
            );
            if is_strict(spec) {
                assert_eq!(
                    stone.vanishing(),
                    Vec::<String>::new(),
                    "{label}: facets that never reach the surface"
                );
                assert_eq!(
                    stone.undersized(),
                    Vec::<String>::new(),
                    "{label}: facets below the cuttable size"
                );
            }
        }
    }
}

/// The fresh-design spec carries the template's own schedule and preform.
#[test]
fn the_fresh_spec_is_the_templates_own_schedule() {
    for spec in TEMPLATES {
        let fresh = spec.fresh_spec(spec.default_material_selection());
        assert_eq!(fresh.gear_teeth, spec.gear_teeth);
        assert_eq!(fresh.symmetry_order, spec.symmetry_order);
        assert_eq!(fresh.mirror, spec.mirror);
        assert_eq!(fresh.preform, spec.preform);
        assert_eq!(
            fresh.material.name.as_deref(),
            Some(spec.default_material),
            "{:?}",
            spec.name
        );
    }
}

/// Every template is stored in the order it is cut, so its first tier is the first step a
/// cutter takes: the pavilion mains first, the table last. The emerald step cut keeps its
/// authored order inside each section, which `cutting_order` already reads pavilion first.
#[test]
fn every_template_is_stored_in_its_cutting_order_except_the_emerald() {
    for spec in TEMPLATES {
        let tiers = spec.tiers();
        let order = flat_cutting_order(&tiers);
        if spec.family == ShapeFamily::Emerald {
            // Stored: the table, nine crown tiers, three girdle tiers, nine pavilion tiers.
            let expected: Vec<usize> = (10..22).chain(1..10).chain([0]).collect();
            assert_eq!(order, expected, "{:?}", spec.name);
        } else {
            let stored: Vec<usize> = (0..tiers.len()).collect();
            assert_eq!(
                order, stored,
                "{:?} is not stored in cutting order",
                spec.name
            );
        }
    }
}

/// The reordered round brilliant lists the same tiers, angles, indices and masts as the
/// `standard_round_brilliant` fixture; only the order differs. Its codes in stored order read
/// P1, G1, P2, Culet, C1, C2, C3, T.
#[test]
fn the_standard_template_has_the_fixtures_tiers_in_cutting_order() {
    let template = TEMPLATES[0].tiers();
    let fixture = ConstraintTier::standard_round_brilliant();
    assert_eq!(template.len(), fixture.len());
    for tier in &fixture {
        let same = template
            .iter()
            .find(|candidate| candidate.name == tier.name)
            .unwrap_or_else(|| panic!("the template has no {:?}", tier.name));
        assert_eq!(same, tier, "{:?} differs from the fixture", tier.name);
    }
    let names: Vec<&str> = template.iter().map(|tier| tier.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Pavilion Main",
            "Girdle",
            "Lower Girdle",
            "Culet",
            "Crown Main",
            "Star",
            "Upper Girdle",
            "Table"
        ]
    );
    let labels = crate::compute_tier_labels(&template);
    let codes: Vec<&str> = labels.iter().map(|label| label.code.as_str()).collect();
    assert_eq!(codes, ["P1", "G1", "P2", "Culet", "C1", "C2", "C3", "T"]);
}

/// The two teaching designs and the other round brilliants follow the same order.
#[test]
fn the_teaching_designs_are_stored_pavilion_girdle_crown_table() {
    let names = |index: usize| -> Vec<String> {
        TEMPLATES[index]
            .tiers()
            .into_iter()
            .map(|tier| tier.name)
            .collect()
    };
    assert_eq!(names(3), ["Pavilion Main", "Girdle", "Table"]);
    assert_eq!(names(4), ["Pavilion Main", "Girdle", "Crown Main", "Table"]);
    for index in [1, 2] {
        assert_eq!(
            names(index),
            [
                "Pavilion Main",
                "Girdle",
                "Lower Girdle",
                "Culet",
                "Crown Main",
                "Star",
                "Upper Girdle",
                "Table"
            ],
            "{}",
            TEMPLATES[index].name
        );
    }
}

/// The gallery must not silently shrink to nothing, and every name must be
/// unique -- a UI index-to-template mapping (the still-open Rust
/// integration step this module's own top doc comment describes) depends
/// on stable, distinct names.
#[test]
fn template_names_are_unique_and_gallery_is_non_empty() {
    // `TEMPLATES` is the `const` array literal declared above --
    // `.is_empty()` on it is compile-time-decidable (clippy::const_is_empty);
    // the uniqueness scan below is what actually earns this test its keep.
    for (i, a) in TEMPLATES.iter().enumerate() {
        for b in &TEMPLATES[i + 1..] {
            assert_ne!(a.name, b.name, "duplicate template name {:?}", a.name);
        }
    }
}
