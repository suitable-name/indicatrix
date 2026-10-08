//! Shared fixtures of the saved-plan tests: a layout whose every figure follows from first
//! principles (so it passes the loader's cross-checks), and the helpers that write it.
//!
//! The layout is cut in the order Y, X, Z (`Yxz`: slabs across Y, bars across X, pieces
//! across Z) with a kerf of 0.3 mm and an allowance of 0.2 mm per side:
//!
//! | piece | slab thickness (Y) | bar width (X) | piece length (Z) | piece box `[x, y, z]` | origin |
//! |-------|-----|-----|-----|-----------------|----------------|
//! | 0     | 4.0 | 3.0 | 3.5 | `[3.0, 4.0, 3.5]` | `[0, 0, 0]`    |
//! | 1     | 4.0 | 3.0 | 3.2 | `[3.0, 4.0, 3.2]` | `[0, 0, 3.8]`  |
//! | 2     | 3.0 | 2.5 | 3.0 | `[2.5, 3.0, 3.0]` | `[0, 4.3, 0]`  |
//!
//! (piece 1 starts after piece 0's 3.5 mm and one 0.3 mm kerf, slab 1 after slab 0's 4.0 mm
//! and one kerf). A stone is its piece less 0.4 mm on every axis (0.2 mm allowance per side).
//! The volumes 10, 8 and 5 mm³ are chosen freely; everything else follows: a carat is
//! `volume * SG / 200` (1 ct is 0.2 g, SG is g/cm³, 1 cm³ is 1000 mm³), the totals are sums,
//! the yield is the total volume over the rough's volume. The pieces reach at most 7.3 mm in
//! Y and 7.0 mm in Z, inside every rough the tests use (the smallest box is the pebble's
//! 18.4 x 11 x 9.6 mm).

use super::{
    convert::CandidateSource,
    dto::SavedDesignDto,
    format::{RankedLayout, SerializeInput, serialize_plan_to_toml},
};
use indicatrix_cut_core::rough_plan::{
    Axis, BarCut, CutOrder, CutPlan, PlacedStone, PlanSettings, RoughBase, RoughCut, RoughLayout,
    RoughModel, SlabCut, fit::StonePose,
};
use indicatrix_vault::model::solid_extents::SOLID_EXTENTS_VERSION;

pub(super) const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
pub(super) const QUARTER_TURN: [[f64; 3]; 3] = [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
pub(super) const TURN_3_4_5: [[f64; 3]; 3] = [[0.6, 0.0, -0.8], [0.0, 1.0, 0.0], [0.8, 0.0, 0.6]];
/// A right-handed frame whose table normal (`axes[1]`) is the rough's X axis.
pub(super) const FACING_X: [[f64; 3]; 3] = [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

pub(super) const MINIMAL_HEADER: &str = "format = \"indicatrix-rough-plan\"\nversion = 1\n";

/// The specific gravity of the fixture plan's material.
pub(super) const SPECIFIC_GRAVITY: f64 = 3.51;

/// The stamp the fixture plan records for its library.
pub(super) const LIBRARY_STAMP: u32 = 77;

/// One stone: the `piece` box at `origin`, 0.4 mm smaller in every axis, of `volume_mm3`.
fn stone(
    entry_id: i64,
    origin: [f64; 3],
    piece: [f64; 3],
    volume_mm3: f64,
    axes: [[f64; 3]; 3],
) -> PlacedStone {
    PlacedStone {
        entry_id,
        piece_origin_mm: origin,
        piece_size_mm: piece,
        stone_size_mm: piece.map(|side| side - 0.4),
        table_axis: Axis::Y,
        carat: volume_mm3 * SPECIFIC_GRAVITY / 200.0,
        volume_mm3,
        pose: StonePose {
            center_mm: [0, 1, 2].map(|i| 0.5f64.mul_add(piece[i], origin[i])),
            axes,
            mm_per_unit: 2.0,
        },
    }
}

/// The three stones of the fixture layout (designs 1, 2 and 1) with three different poses.
pub(super) fn sample_stones() -> Vec<PlacedStone> {
    vec![
        stone(1, [0.0, 0.0, 0.0], [3.0, 4.0, 3.5], 10.0, IDENTITY),
        stone(2, [0.0, 0.0, 3.8], [3.0, 4.0, 3.2], 8.0, QUARTER_TURN),
        stone(1, [0.0, 4.3, 0.0], [2.5, 3.0, 3.0], 5.0, TURN_3_4_5),
    ]
}

/// The fixture layout for a rough of `rough_volume_mm3`.
pub(super) fn sample_layout_with_volume(rough_volume_mm3: f64) -> RoughLayout {
    let stones = sample_stones();
    let total_volume_mm3: f64 = stones.iter().map(|s| s.volume_mm3).sum();
    let total_carat: f64 = stones.iter().map(|s| s.carat).sum();
    RoughLayout {
        cut_order: CutOrder::Yxz,
        stones,
        cut_plan: CutPlan {
            slabs: vec![
                SlabCut {
                    thickness_mm: 4.0,
                    bars: vec![BarCut {
                        width_mm: 3.0,
                        pieces_mm: vec![3.5, 3.2],
                    }],
                },
                SlabCut {
                    thickness_mm: 3.0,
                    bars: vec![BarCut {
                        width_mm: 2.5,
                        pieces_mm: vec![3.0],
                    }],
                },
            ],
        },
        total_carat,
        total_volume_mm3,
        yield_fraction: total_volume_mm3 / rough_volume_mm3,
        exact_fit: false,
    }
}

/// The volume of `model` as the loader measures it.
pub(super) fn volume_of(model: &RoughModel) -> f64 {
    model
        .measure()
        .expect("the test rough has a volume")
        .volume_mm3
}

/// The fixture layout on `model`.
pub(super) fn sample_layout_in(model: &RoughModel) -> RoughLayout {
    sample_layout_with_volume(volume_of(model))
}

/// The fixture layout on the plain block.
pub(super) fn sample_layout() -> RoughLayout {
    sample_layout_in(&plain_block())
}

/// The fixture layout on `model` with stone 1's pose axes replaced.
pub(super) fn layout_with_axes(model: &RoughModel, axes: [[f64; 3]; 3]) -> RoughLayout {
    let mut layout = sample_layout_in(model);
    layout.stones[1].pose.axes = axes;
    layout
}

/// An exact single-stone fit on `model`: one stone of 12 mm³ whose piece is its own
/// 2.0 x 3.0 x 2.5 mm box at (1, 1, 1), one slab, bar and piece of that box in the order
/// X, Y, Z (as the planner writes it).
pub(super) fn exact_fit_layout_in(model: &RoughModel) -> RoughLayout {
    let mut placed = stone(1, [1.0, 1.0, 1.0], [2.0, 3.0, 2.5], 12.0, IDENTITY);
    placed.stone_size_mm = placed.piece_size_mm;
    RoughLayout {
        cut_order: CutOrder::Xyz,
        total_carat: placed.carat,
        total_volume_mm3: placed.volume_mm3,
        yield_fraction: placed.volume_mm3 / volume_of(model),
        stones: vec![placed],
        cut_plan: CutPlan {
            slabs: vec![SlabCut {
                thickness_mm: 2.0,
                bars: vec![BarCut {
                    width_mm: 3.0,
                    pieces_mm: vec![2.5],
                }],
            }],
        },
        exact_fit: true,
    }
}

pub(super) fn settings() -> PlanSettings {
    PlanSettings {
        count: 6,
        min_count: 1,
        kerf_mm: 0.3,
        allowance_mm: 0.2,
        skin_mm: 0.0,
        min_width_mm: 1.0,
        specific_gravity: SPECIFIC_GRAVITY,
    }
}

/// The two designs the fixture layout uses (ids 1 and 2).
pub(super) fn designs() -> Vec<SavedDesignDto> {
    vec![
        SavedDesignDto {
            entry_id: 1,
            title: "Barion Oval".to_string(),
            fingerprint: [1.3912, 0.67512, 0.4123],
            width_caliper: Some(1.25),
            extents_version: SOLID_EXTENTS_VERSION,
        },
        SavedDesignDto {
            entry_id: 2,
            title: "Emerald".to_string(),
            fingerprint: [1.5, 0.6, 0.35],
            width_caliper: Some(0.8),
            extents_version: SOLID_EXTENTS_VERSION,
        },
    ]
}

/// A plan written from the pieces as they are, without the loader's checks.
pub(super) fn write_with(
    model: &RoughModel,
    layouts: &[RankedLayout<'_>],
    designs: &[SavedDesignDto],
) -> String {
    serialize_plan_to_toml(&SerializeInput {
        name: "Test plan",
        created_at: 1_790_000_000,
        library_id: Some(LIBRARY_STAMP),
        model,
        material_name: "Aquamarine",
        weighed_ct: Some(8.9),
        settings: &settings(),
        candidate_source: CandidateSource::Library,
        designs,
        layouts,
    })
    .expect("the plan serialises")
}

/// A plan of `layouts` (ranked 1, 2, ...) with the fixture designs they use (a design no
/// stone uses is refused, so none at all when there are no layouts).
pub(super) fn write(model: &RoughModel, layouts: &[RoughLayout]) -> String {
    let ranked: Vec<RankedLayout<'_>> = layouts
        .iter()
        .enumerate()
        .map(|(i, layout)| RankedLayout {
            rank: i + 1,
            layout,
        })
        .collect();
    let used: Vec<SavedDesignDto> = designs()
        .into_iter()
        .filter(|design| {
            layouts
                .iter()
                .any(|layout| layout.stones.iter().any(|s| s.entry_id == design.entry_id))
        })
        .collect();
    write_with(model, &ranked, &used)
}

/// A 20 x 12 x 10 mm block with `cuts`.
pub(super) fn block(cuts: Vec<RoughCut>) -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 20.0,
            y_mm: 12.0,
            z_mm: 10.0,
        },
        cuts,
    )
}

/// The 20 x 12 x 10 mm block (2400 mm³).
pub(super) fn plain_block() -> RoughModel {
    block(Vec::new())
}

/// The loader's refusal of `text`.
pub(super) fn expect_error(text: &str) -> String {
    super::format::parse_and_validate_plan(text).expect_err("the plan must be refused")
}
