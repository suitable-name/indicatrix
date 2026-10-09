//! Tests of the planner integration of rough colour: frames, pose choice, adoption, codecs and
//! the cached photos. Pure geometry and bytes; no database.

use super::*;
use crate::rough_plan::{
    Axis, PlacedStone,
    fit::StonePose,
    photometry::{PixelMask, ResampledImage, WorkingGrid, flag},
};
use glam::DVec3;
use indicatrix::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    materials::{AbsorptionUnit, GemMaterial},
    zoning::{MAX_ZONES, Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption, zone_lengths},
};

const EPS: f64 = 1e-9;

fn absorber(centre_nm: f32, peak: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        centre_nm, 40.0, peak,
    )]))
}

fn clear() -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new()))
}

fn identity_pose(centre: [f64; 3], mm_per_unit: f64) -> StonePose {
    StonePose {
        center_mm: centre,
        axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        mm_per_unit,
    }
}

/// A 6 mm cube stone centred on the origin, table facing +y.
fn cube_stone() -> PlacedStone {
    PlacedStone {
        entry_id: 1,
        piece_origin_mm: [-3.0; 3],
        piece_size_mm: [6.0; 3],
        stone_size_mm: [6.0; 3],
        table_axis: Axis::Y,
        carat: 1.0,
        volume_mm3: 100.0,
        pose: identity_pose([0.0; 3], 1.0),
    }
}

/// Zone 1 is everything above the plane `y = 1`.
fn layered() -> ZonedAbsorption {
    let mut zoned = ZonedAbsorption::new(clear());
    zoned.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::Y,
            offset: 1.0,
        },
        absorption: absorber(600.0, 0.8),
    });
    zoned
}

/// A watermelon: a rod of red core (zone 1) along x through `(y, z) = (-3, 0)`, 3 mm in radius,
/// inside a pale rind.
fn watermelon() -> ZonedAbsorption {
    let mut zoned = ZonedAbsorption::new(absorber(480.0, 0.05));
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

fn close(a: DVec3, b: DVec3) -> bool {
    (a - b).length() < 1e-9
}

// ---- frames ----------------------------------------------------------------------------------

#[test]
fn the_stone_frame_maps_a_known_pose_and_design_placement() {
    // axes x = +z, y = +y, z = -x (right handed), scale 2, centre (10, 0, 0).
    let pose = StonePose {
        center_mm: [10.0, 0.0, 0.0],
        axes: [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
        mm_per_unit: 2.0,
    };
    // The design is turned about the vertical by width_dir = (0, 1) -- model (px, py, pz) becomes
    // (pz, py, -px) -- and centred by (0.5, 0.25, -0.5).
    let design = DesignPlacement {
        width_dir: [0.0, 1.0],
        centre_units: [0.5, 0.25, -0.5],
    };
    let frame = rough_to_stone_frame(&pose, &design).expect("a proper pose");
    // The model point (1, 2, 3) is (3, 2, -1) in the turned frame, (2.5, 1.75, -0.5) after
    // centring, (0.5, 1.75, 2.5) * 2 + centre = (11, 3.5, 5) in the rough, and 2 * (1, 2, 3)
    // = (2, 4, 6) mm in the stone frame.
    let stone = frame.point(DVec3::new(11.0, 3.5, 5.0));
    assert!(close(stone, DVec3::new(2.0, 4.0, 6.0)), "{stone:?}");
    // A rotation, not a reflection: lengths and the handedness are kept.
    let a = frame.direction(DVec3::X);
    let b = frame.direction(DVec3::Y);
    let c = frame.direction(DVec3::Z);
    assert!((a.length() - 1.0).abs() < EPS);
    assert!(close(a.cross(b), c));
}

#[test]
fn an_identity_placement_gives_a_pure_translation() {
    let pose = identity_pose([4.0, 5.0, 6.0], 3.0);
    let frame = rough_to_stone_frame(&pose, &DesignPlacement::IDENTITY).unwrap();
    assert!(close(frame.point(DVec3::new(4.0, 5.0, 6.0)), DVec3::ZERO));
    assert!(close(
        frame.point(DVec3::new(5.0, 5.0, 6.0)),
        DVec3::new(1.0, 0.0, 0.0)
    ));
}

#[test]
fn zones_moved_into_the_stone_frame_keep_every_segment_length() {
    let rough = watermelon();
    let pose = StonePose {
        center_mm: [1.0, -2.0, 0.5],
        axes: [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
        mm_per_unit: 1.5,
    };
    let design = DesignPlacement {
        width_dir: [0.6, 0.8],
        centre_units: [0.1, -0.2, 0.3],
    };
    let stone_zones = zones_in_stone_frame(&rough, &pose, &design).expect("usable placement");
    let frame = rough_to_stone_frame(&pose, &design).unwrap();
    for (from, to) in [
        (DVec3::new(-3.0, 3.0, 0.0), DVec3::new(2.0, -4.0, 1.0)),
        (DVec3::new(0.0, 0.0, -3.0), DVec3::new(0.5, -5.0, 2.0)),
        (DVec3::new(-6.0, -1.0, 1.0), DVec3::new(6.0, -1.0, 1.0)),
    ] {
        let in_rough = zone_lengths(&rough, from, to);
        let in_stone = zone_lengths(&stone_zones, frame.point(from), frame.point(to));
        for zone in 0..=MAX_ZONES {
            assert!(
                (in_rough[zone] - in_stone[zone]).abs() < 1e-9,
                "zone {zone}: {in_rough:?} against {in_stone:?}"
            );
        }
    }
    // The absorptions are untouched.
    assert_eq!(stone_zones.base, rough.base);
    assert_eq!(stone_zones.zones[0].absorption, rough.zones[0].absorption);
}

#[test]
fn an_unusable_pose_or_placement_gives_no_frame() {
    let design = DesignPlacement::IDENTITY;
    let mut mirrored = identity_pose([0.0; 3], 1.0);
    mirrored.axes[2] = [0.0, 0.0, -1.0];
    assert!(rough_to_stone_frame(&mirrored, &design).is_none());
    assert!(rough_to_stone_frame(&identity_pose([0.0; 3], 0.0), &design).is_none());
    assert!(rough_to_stone_frame(&identity_pose([f64::NAN, 0.0, 0.0], 1.0), &design).is_none());
    let skewed = StonePose {
        axes: [[1.0, 0.1, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        ..identity_pose([0.0; 3], 1.0)
    };
    assert!(rough_to_stone_frame(&skewed, &design).is_none());
    let bad_design = DesignPlacement {
        width_dir: [2.0, 0.0],
        ..DesignPlacement::IDENTITY
    };
    assert!(rough_to_stone_frame(&identity_pose([0.0; 3], 1.0), &bad_design).is_none());
}

#[test]
fn the_four_poses_are_proper_rotations_of_the_same_box() {
    let pose = StonePose {
        center_mm: [1.0, 2.0, 3.0],
        axes: [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
        mm_per_unit: 1.0,
    };
    let all = candidate_poses(&pose, false);
    assert_eq!(all.len(), usize::from(POSE_COUNT));
    assert_eq!(all[0], (0, pose));
    for (index, variant) in &all {
        assert_eq!(variant.center_mm, pose.center_mm);
        assert!((variant.mm_per_unit - pose.mm_per_unit).abs() < EPS);
        let [x, y, z] = variant.axes.map(DVec3::from_array);
        assert!(close(x.cross(y), z), "pose {index} is not right handed");
        // The axes are the planner's, up to sign: the same lines in space.
        for (a, b) in variant.axes.iter().zip(&pose.axes) {
            let dot = DVec3::from_array(*a).dot(DVec3::from_array(*b));
            assert!((dot.abs() - 1.0).abs() < EPS);
        }
    }
    // Each variant turns the table normal differently: poses 1 and 3 turn the stone over.
    let ups: Vec<f64> = all
        .iter()
        .map(|(_, p)| DVec3::from_array(p.axes[1]).dot(DVec3::from_array(pose.axes[1])))
        .collect();
    assert_eq!(ups, vec![1.0, -1.0, 1.0, -1.0]);
    assert_eq!(pose_variant(&pose, POSE_COUNT), None);
    // An exact single-stone fit has one pose.
    assert_eq!(candidate_poses(&pose, true).len(), 1);
}

// ---- pose choice -----------------------------------------------------------------------------

#[test]
fn the_face_up_prediction_splits_the_path_between_the_zones() {
    let stone = cube_stone();
    let layers = layered();
    let up = face_up_prediction(&layers, &stone, &stone.pose);
    // Table up: the 1 mm above y = 1 ... 3 mm of the 6 mm centre line is zone 1 only from y = 3
    // to y = 1 (2 mm); each ring line is 3.75 mm long and also has 2 mm there. 18 of 36 mm.
    assert!((up.fractions[1] - 0.5).abs() < EPS, "{:?}", up.fractions);
    assert!((up.fractions[0] - 0.5).abs() < EPS);
    assert!((up.fractions.iter().sum::<f64>() - 1.0).abs() < EPS);
    assert!(up.path_mm > 0.0, "a face-up path in mm");

    let flipped = pose_variant(&stone.pose, 1).unwrap();
    let down = face_up_prediction(&layers, &stone, &flipped);
    // Table down: only the centre line reaches y = 1 at all (2 of 36 mm).
    assert!(
        (down.fractions[1] - 2.0 / 36.0).abs() < EPS,
        "{:?}",
        down.fractions
    );
}

#[test]
fn best_colour_turns_a_watermelon_stone_so_the_core_is_face_up() {
    let stone = cube_stone();
    let rough = watermelon();
    let canonical = face_up_prediction(&rough, &stone, &stone.pose).fractions[1];
    let flipped =
        face_up_prediction(&rough, &stone, &pose_variant(&stone.pose, 1).unwrap()).fractions[1];
    assert!(
        flipped > canonical + 0.05,
        "the core lies below the centre, so turning the stone over shows more of it: \
         {flipped} against {canonical}"
    );

    // The default keeps the planner's pose.
    assert_eq!(
        choose_pose(&rough, &stone, false, PoseGoal::KeepCanonical),
        0
    );
    // "Best colour": the most core face-up is the lowest-index pose that turns the stone over.
    assert_eq!(
        choose_pose(&rough, &stone, false, PoseGoal::MostOfZone(1)),
        1
    );
    // The opposite wish keeps the canonical pose, which already shows the least core.
    assert_eq!(
        choose_pose(&rough, &stone, false, PoseGoal::LeastOfZone(1)),
        0
    );
    // An exact single-stone fit has a unique pose.
    assert_eq!(
        choose_pose(&rough, &stone, true, PoseGoal::MostOfZone(1)),
        0
    );
}

#[test]
fn a_symmetric_zoning_keeps_the_canonical_pose_on_a_tie() {
    let stone = cube_stone();
    // Zone 1 is a vertical rod through the stone's centre: every pose sees the same thing.
    let mut rough = ZonedAbsorption::new(clear());
    rough.zones.push(Zone {
        shape: ZoneShape::CoaxialCylinder {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Y,
            r_in: 0.0,
            r_out: 1.0,
        },
        absorption: absorber(600.0, 0.9),
    });
    assert_eq!(
        choose_pose(&rough, &stone, false, PoseGoal::MostOfZone(1)),
        0
    );
    assert_eq!(
        choose_pose(&rough, &stone, false, PoseGoal::MostOfZone(7)),
        0
    );
}

#[test]
fn a_layout_gets_one_pose_per_stone() {
    use crate::rough_plan::{CutOrder, CutPlan, RoughLayout};
    let layout = RoughLayout {
        cut_order: CutOrder::Xyz,
        stones: vec![cube_stone(), cube_stone()],
        cut_plan: CutPlan { slabs: Vec::new() },
        total_carat: 2.0,
        total_volume_mm3: 200.0,
        yield_fraction: 0.5,
        exact_fit: false,
    };
    let poses = choose_poses(&watermelon(), &layout, PoseGoal::MostOfZone(1));
    assert_eq!(poses, vec![1, 1]);
    let single = RoughLayout {
        exact_fit: true,
        ..layout
    };
    assert_eq!(
        choose_poses(&watermelon(), &single, PoseGoal::MostOfZone(1)),
        vec![0, 0]
    );
}

// ---- adoption --------------------------------------------------------------------------------

#[test]
fn the_adopted_material_name_is_cleaned() {
    assert_eq!(
        adopted_material_name("Mozambique rough"),
        "Mozambique rough colour"
    );
    assert_eq!(adopted_material_name("  Tourmaline\n"), "Tourmaline colour");
    assert_eq!(adopted_material_name("   "), "Rough colour");
    let long = "x".repeat(200);
    assert_eq!(
        adopted_material_name(&long).chars().count(),
        60 + ADOPTED_SUFFIX.chars().count()
    );
}

#[test]
fn adopting_a_stone_keeps_the_base_zone_as_band_rows_and_the_zones_in_the_stone_frame() {
    let rough = watermelon();
    let placement = StonePlacement {
        pose: identity_pose([0.0, 0.0, 0.0], 2.0),
        design: DesignPlacement::IDENTITY,
    };
    let adopted = adopt_colour("Melon", &rough, &placement, 7.0).expect("adoptable");
    assert_eq!(adopted.material_name, "Melon colour");
    assert!((adopted.stone_width_mm - 7.0).abs() < EPS);
    assert_eq!(adopted.base_bands, vec![[480.0, 40.0, 0.05]]);
    assert_eq!(adopted.zoning.zones.len(), 1);
    adopted.zoning.validate().unwrap();

    // A bad width, or an unusable pose, adopts nothing.
    assert!(adopt_colour("Melon", &rough, &placement, 0.0).is_none());
    assert!(adopt_colour("Melon", &rough, &placement, f64::NAN).is_none());
    let broken = StonePlacement {
        pose: identity_pose([0.0; 3], 0.0),
        ..placement
    };
    assert!(adopt_colour("Melon", &rough, &broken, 7.0).is_none());

    // The material renders the base zone where zones are ignored, and is per millimetre.
    let host = GemMaterial::by_name("Quartz").unwrap_or_else(GemMaterial::diamond);
    let material = adopted_material(&host, &adopted, 2.0);
    assert_eq!(material.name, "Melon colour");
    assert_eq!(material.absorption_unit, AbsorptionUnit::PerMm);
    assert_eq!(material.absorption, rough.base.tensor);
    assert_eq!(material.zoning.as_ref(), Some(&adopted.zoning));
    assert!((material.absorption_path_scale - 2.0).abs() < 1e-6);
}

#[test]
fn a_clear_base_zone_gives_no_band_rows_and_unusable_bands_are_dropped() {
    assert_eq!(base_zone_bands(&clear()), [] as [[f32; 3]; 0]);
    let odd = ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![
        AbsorptionBand::new(500.0, 30.0, 0.2),
        AbsorptionBand::new(600.0, 0.0, 0.2),
        AbsorptionBand::new(650.0, 30.0, 0.0),
        AbsorptionBand::new(f32::NAN, 30.0, 0.2),
    ]));
    assert_eq!(base_zone_bands(&odd), vec![[500.0, 30.0, 0.2]]);
}

#[test]
fn a_planned_stone_previews_with_the_hosts_optics_and_the_ropes_zones() {
    let rough = layered();
    let placement = StonePlacement {
        pose: identity_pose([0.0, 1.0, 0.0], 1.0),
        design: DesignPlacement::IDENTITY,
    };
    let host = GemMaterial::diamond();
    let material = stone_preview_material(&host, &rough, &placement).expect("usable");
    let zoning = material.zoning.as_ref().expect("zones installed");
    // The plane y = 1 of the rough is the plane y = 0 of a stone centred at y = 1.
    let frame = rough_to_stone_frame(&placement.pose, &placement.design).unwrap();
    let lengths = zone_lengths(
        zoning,
        frame.point(DVec3::new(0.0, 0.0, 0.0)),
        frame.point(DVec3::new(0.0, 2.0, 0.0)),
    );
    assert!((lengths[0] - 1.0).abs() < 1e-9 && (lengths[1] - 1.0).abs() < 1e-9);
    assert_eq!(material.birefringence_delta, host.birefringence_delta);
    assert_eq!(material.absorption_unit, AbsorptionUnit::PerMm);
}

#[test]
fn relative_zoning_scales_with_the_stone_width_and_back() {
    let mm = watermelon();
    let unit = make_relative(&mm, 6.0).expect("scalable");
    match &unit.zones[0].shape {
        ZoneShape::CoaxialCylinder {
            r_out, axis_point, ..
        } => {
            assert!((r_out - 0.5).abs() < EPS);
            assert!(close(*axis_point, DVec3::new(0.0, -0.5, 0.0)));
        }
        other => panic!("{other:?}"),
    }
    let at_twelve = resolve_relative(&unit, 12.0).expect("scalable");
    match &at_twelve.zones[0].shape {
        ZoneShape::CoaxialCylinder { r_out, .. } => assert!((r_out - 6.0).abs() < EPS),
        other => panic!("{other:?}"),
    }
    assert!(resolve_relative(&unit, 0.0).is_none());
    assert!(make_relative(&mm, f64::INFINITY).is_none());
}

// ---- codecs ----------------------------------------------------------------------------------

#[test]
fn zones_round_trip_through_json() {
    let mut zoned = watermelon();
    zoned.boundary_softness_mm = 0.25;
    let text = encode_zoned(&zoned).expect("valid zones");
    assert_eq!(decode_zoned(&text).expect("reads back"), zoned);
}

#[test]
fn invalid_or_hostile_zone_text_is_refused() {
    let mut too_many = ZonedAbsorption::new(clear());
    for _ in 0..=MAX_ZONES {
        too_many.zones.push(Zone {
            shape: ZoneShape::HalfSpace {
                normal: DVec3::Y,
                offset: 0.0,
            },
            absorption: clear(),
        });
    }
    assert!(matches!(
        encode_zoned(&too_many),
        Err(CodecError::Invalid(_))
    ));
    // The same zones sneaked in as text are refused on the way back.
    let text = serde_json::to_string(&too_many).unwrap();
    assert!(matches!(decode_zoned(&text), Err(CodecError::Invalid(_))));
    assert!(matches!(
        decode_zoned("{\"not\":\"zones\"}"),
        Err(CodecError::Json(_))
    ));
    assert!(matches!(decode_zoned(""), Err(CodecError::Json(_))));
    assert!(check_version(ZONING_FORMAT_VERSION).is_ok());
    assert_eq!(
        check_version(ZONING_FORMAT_VERSION + 1),
        Err(CodecError::UnsupportedVersion(ZONING_FORMAT_VERSION + 1))
    );
}

// ---- cached photos ---------------------------------------------------------------------------

#[test]
fn half_floats_round_trip_the_values_a_transmittance_takes() {
    for value in [
        0.0_f32,
        1.0,
        0.5,
        0.25,
        0.125,
        2.0,
        65504.0,
        -1.0,
        1.5,
        1365.0 / 4096.0,
    ] {
        let back = f16_round_trip(value);
        assert_eq!(back, value, "{value} is exactly representable");
    }
    assert!(f16_round_trip(f32::INFINITY).is_infinite());
    assert!(f16_round_trip(f32::NAN).is_nan());
    assert!(
        f16_round_trip(1.0e6).is_infinite(),
        "overflow gives infinity"
    );
    // Any value in [0, 1] is off by at most half a unit in the eleventh bit.
    for step in 0..=1000 {
        let value = step as f32 / 1000.0;
        let back = f16_round_trip(value);
        assert!(
            (back - value).abs() <= value.max(6.0e-8) * 5.0e-4,
            "{value} -> {back}"
        );
    }
    // Subnormal halves and the round-to-even tie.
    assert_eq!(
        f16_round_trip(f32::from_bits(0x3380_0000)),
        f32::from_bits(0x3380_0000)
    );
    assert_eq!(
        f16_round_trip(1.0 + 1.0 / 2048.0),
        1.0,
        "a tie rounds to even"
    );
    assert_eq!(f16_round_trip(1.0 + 3.0 / 2048.0), 1.0 + 4.0 / 2048.0);
}

fn f16_round_trip(value: f32) -> f32 {
    photo::f16_bits_to_f32(photo::f32_to_f16_bits(value))
}

fn small_view() -> ResampledImage {
    let grid = WorkingGrid {
        width: 3,
        height: 2,
        origin: [10.25, 20.5],
        scale: 4.0,
    };
    let mut mask = PixelMask::new(3, 2);
    mask.set(1, 0, flag::SATURATED);
    mask.set(2, 1, flag::OUTSIDE_OUTLINE | flag::EDGE_BAND);
    ResampledImage {
        grid,
        values: vec![
            [0.0, 0.5, 1.0],
            [0.25, 0.25, 0.25],
            [0.125, 0.75, 0.5],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            [0.5, 0.5, 0.5],
        ],
        variance: vec![
            [1e-4, 2e-4, 3e-4],
            [f32::INFINITY; 3],
            [1e-5; 3],
            [1e-3; 3],
            [0.1; 3],
            [1e-6; 3],
        ],
        coverage: vec![1.0, 0.5, 1.0, 1.0, 0.0, 0.25],
        mask,
    }
}

#[test]
fn a_view_survives_packing_into_blobs_and_back() {
    let image = small_view();
    let blobs = blobs_from_resampled(&image).expect("packs");
    assert_eq!(blobs.len(), 5);
    let kinds: Vec<&str> = blobs.iter().map(|b| b.kind.as_str()).collect();
    assert_eq!(
        kinds,
        ["transmittance", "variance", "coverage", "mask", "grid"]
    );
    let transmittance = &blobs[0];
    assert_eq!((transmittance.width, transmittance.height), (3, 2));
    assert_eq!(
        transmittance.data.len(),
        3 * 2 * 3 * 2,
        "f16, three channels"
    );
    assert_eq!(
        PhotoEncoding::parse(blobs[1].encoding.as_str()),
        Some(PhotoEncoding::F32)
    );

    let back = resampled_from_blobs(&blobs).expect("unpacks");
    assert_eq!(back.grid, image.grid);
    assert_eq!(back.values, image.values, "these values are exact halves");
    assert_eq!(
        back.variance, image.variance,
        "variance is stored in full precision"
    );
    assert_eq!(back.coverage, image.coverage);
    assert_eq!(back.mask.bits(), image.mask.bits());
}

#[test]
fn a_damaged_blob_set_is_refused_with_a_reason() {
    let image = small_view();
    let blobs = blobs_from_resampled(&image).unwrap();

    let mut missing = blobs.clone();
    missing.retain(|b| b.kind != "mask");
    assert_eq!(
        resampled_from_blobs(&missing),
        Err(PhotoCodecError::Missing("mask"))
    );

    let mut short = blobs.clone();
    short[0].data.pop();
    assert!(matches!(
        resampled_from_blobs(&short),
        Err(PhotoCodecError::Size {
            kind: "transmittance",
            ..
        })
    ));

    let mut wrong_encoding = blobs.clone();
    wrong_encoding[2].encoding = PhotoEncoding::F32;
    assert_eq!(
        resampled_from_blobs(&wrong_encoding),
        Err(PhotoCodecError::Encoding("coverage"))
    );

    let mut bad_grid = blobs;
    bad_grid[4].data = b"{\"width\":3}".to_vec();
    assert!(matches!(
        resampled_from_blobs(&bad_grid),
        Err(PhotoCodecError::Grid(_))
    ));

    // An image whose arrays do not match its grid cannot be packed.
    let mut inconsistent = small_view();
    inconsistent.coverage.pop();
    assert!(matches!(
        blobs_from_resampled(&inconsistent),
        Err(PhotoCodecError::Size {
            kind: "coverage",
            ..
        })
    ));
}
