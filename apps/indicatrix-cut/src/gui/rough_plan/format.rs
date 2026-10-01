//! Formatting rough planning results, stone groups, and cut plans for display.
//!
//! Everything here builds plain strings and numbers (`Send`, unlike Slint's models), so a
//! worker thread can format the whole result list; the UI thread only wraps it.

use super::{
    metrics::{
        DesignFacts, ModelGeometry, SavedShape, format_cut_order, format_saw_work,
        format_total_carat, format_weighed_yield, group_metrics,
    },
    run::DesignStatus,
    saved::dto::DesignShape,
};
use indicatrix_cut_core::rough_plan::{Axis, PlacedStone, PlanSettings, RoughLayout};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{solid_extents::SolidExtents, tilt_curves::TiltPerformanceCurves},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex, PoisonError},
};
use tracing::warn;

/// One design in a result: how many stones, what size, and its metric list.
pub struct GroupRow {
    /// The library entry to link to (the resolved one for a design matched by title).
    pub entry_id: i32,
    /// The design's title ("Design #id" when the library has none).
    pub name: String,
    /// How many stones of this design the layout holds.
    pub count: i32,
    /// The size line ("5.10 x 5.10 x 3.40 mm, 1.20 ct each").
    pub detail: String,
    /// The design's position in the layout's group order; it picks the palette colour.
    pub swatch_index: usize,
    /// Label and value pairs.
    pub metrics: Vec<(String, String)>,
    /// The staleness chip text ("" when there is none).
    pub status: String,
    /// 0 none, 1 info, 2 warning, 3 error.
    pub status_level: i32,
    /// Whether the design still exists in the library. Whether library links work at all
    /// (they do not for a remote library) is the window's live `links_enabled`, which the
    /// row combines with this, so switching libraries needs no new rows.
    pub linkable: bool,
}

/// One ranked layout, as plain strings and numbers.
pub struct ResultRow {
    /// The layout's position in the result list, from 1.
    pub rank: i32,
    /// The total carat weight ("3.71 ct").
    pub total_ct: String,
    /// The yield ("41.2 %").
    pub yield_pct: String,
    /// "38.9 % of 9.54 ct weighed", empty without a weighed carat.
    pub yield_weight_text: String,
    /// "5 cuts · kerf loss ≤ 0.21 ct"; empty for an exact fit (nothing is sawn) and when
    /// the rough's extents are unknown.
    pub saw_text: String,
    /// How many stones the layout holds.
    pub stone_count: i32,
    /// The designs of the layout, most stones first.
    pub groups: Vec<GroupRow>,
    /// The multi-line cut plan.
    pub cut_plan: String,
}

/// What is cached about the designs a result list uses.
#[derive(Default)]
pub struct DesignData {
    /// The measured extents.
    pub extents: BTreeMap<i64, SolidExtents>,
    /// The cached tilt curves.
    pub curves: BTreeMap<i64, TiltPerformanceCurves>,
    /// The material each design's preview and curves were made with.
    pub materials: BTreeMap<i64, String>,
}

/// Everything needed to format the rows of one result list.
pub struct RowContext<'a> {
    /// Design titles by entry id (a missing one reads "Design #id").
    pub titles: &'a BTreeMap<i64, String>,
    /// Staleness of the designs of a loaded plan (empty for a fresh plan).
    pub statuses: &'a BTreeMap<i64, DesignStatus>,
    /// The shape each design had when the layouts were planned, by the entry id the
    /// layouts use (empty when unknown).
    pub shapes: &'a BTreeMap<i64, DesignShape>,
    /// The settings the layouts were planned with.
    pub settings: &'a PlanSettings,
    /// The weighed rough, if one was entered.
    pub weighed_ct: Option<f64>,
    /// The modelled rough, if it can still be measured.
    pub model: Option<&'a ModelGeometry>,
    /// The rough's bounding box, for the saw work (none when the model is unknown).
    pub rough_extents: Option<[f64; 3]>,
    /// Extents, curves and preview materials of the designs.
    pub designs: &'a DesignData,
}

/// `n` as the `i32` a Slint `int` holds (saturating).
pub fn to_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// `n` with a comma between every group of three digits ("3,299").
pub fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The library titles of every design used by `layouts`, keyed by entry id. A design
/// whose record cannot be read is simply absent (its name falls back to `Design #id`).
pub fn load_titles(db: &Mutex<Database>, layouts: &[RoughLayout]) -> BTreeMap<i64, String> {
    let ids: BTreeSet<i64> = layouts
        .iter()
        .flat_map(|layout| layout.stones.iter().map(|stone| stone.entry_id))
        .collect();
    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    ids.into_iter()
        .filter_map(|id| match guard.get_diagram_full_meta(id) {
            Ok(Some(meta)) if !meta.title.trim().is_empty() => Some((id, meta.title)),
            Ok(_) => None,
            Err(e) => {
                warn!("Rough planner: could not read the title of design #{id}: {e}");
                None
            }
        })
        .collect()
}

/// A design's display name.
fn design_name(titles: &BTreeMap<i64, String>, entry_id: i64) -> String {
    titles
        .get(&entry_id)
        .cloned()
        .unwrap_or_else(|| format!("Design #{entry_id}"))
}

/// A stone's bounding box in the rough's axes: "5.10 x 5.10 x 3.40".
fn dims_text(stone: &PlacedStone) -> String {
    let [x, y, z] = stone.stone_size_mm;
    format!("{x:.2} x {y:.2} x {z:.2}")
}

/// The size line of one design's group: shared by all its stones when they are alike,
/// otherwise the carat range and the largest stone.
pub fn group_detail(stones: &[&PlacedStone]) -> String {
    let Some(&first) = stones.first() else {
        return String::new();
    };
    let each = if stones.len() > 1 { " each" } else { "" };
    let alike = stones.iter().all(|s| {
        dims_text(s) == dims_text(first)
            && format!("{:.2}", s.carat) == format!("{:.2}", first.carat)
    });
    if alike {
        return format!("{} mm, {:.2} ct{each}", dims_text(first), first.carat);
    }
    let largest = stones
        .iter()
        .copied()
        .max_by(|a, b| a.carat.total_cmp(&b.carat))
        .unwrap_or(first);
    let smallest = stones
        .iter()
        .copied()
        .min_by(|a, b| a.carat.total_cmp(&b.carat))
        .unwrap_or(first);
    format!(
        "{:.2} to {:.2} ct{each}, largest {} mm",
        smallest.carat,
        largest.carat,
        dims_text(largest)
    )
}

/// The designs of a layout as `(entry_id, stone count)`, most stones first, then by id.
/// This is the group order of a result card, so a group's index picks the same palette
/// colour in the card and in the 3D view.
#[must_use]
pub fn group_order(layout: &RoughLayout) -> Vec<(i64, usize)> {
    let mut composition = layout.composition();
    composition.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    composition
}

/// The entry whose cached data (extents, previews, curves) describes the design `entry_id`
/// of a layout: the resolved entry for a design matched by title, else the id itself.
#[must_use]
pub fn effective_id(statuses: &BTreeMap<i64, DesignStatus>, entry_id: i64) -> i64 {
    match statuses.get(&entry_id) {
        Some(DesignStatus::MatchedByTitle { resolved_entry_id }) => *resolved_entry_id,
        _ => entry_id,
    }
}

/// The staleness chip of a design: its text and level (0 none, 1 info, 2 warning, 3 error).
///
/// A design is also "matched by title" when a status elsewhere names it as the resolved
/// entry (the layout was already re-pointed at it).
fn status_of(statuses: &BTreeMap<i64, DesignStatus>, entry_id: i64) -> (&'static str, i32) {
    let matched = ("matched by title", 1);
    match statuses.get(&entry_id) {
        Some(DesignStatus::Deleted) => ("design deleted", 3),
        Some(DesignStatus::Changed) => ("design changed since saved", 2),
        Some(DesignStatus::MatchedByTitle { .. }) => matched,
        Some(DesignStatus::Unchanged) | None => {
            let resolved_here = statuses.values().any(|status| {
                matches!(status, DesignStatus::MatchedByTitle { resolved_entry_id }
                    if *resolved_entry_id == entry_id)
            });
            if resolved_here { matched } else { ("", 0) }
        }
    }
}

/// The size a planned design's shape records, if it has one.
fn saved_shape(shape: &DesignShape) -> Option<SavedShape> {
    shape.width_caliper.map(|width_caliper| SavedShape {
        width_caliper,
        fingerprint: shape.fingerprint,
    })
}

/// The designs of a layout, most stones first, with their metric lists.
pub fn group_rows(layout: &RoughLayout, ctx: &RowContext<'_>) -> Vec<GroupRow> {
    group_order(layout)
        .into_iter()
        .enumerate()
        .map(|(swatch_index, (entry_id, count))| {
            let stones: Vec<&PlacedStone> = layout
                .stones
                .iter()
                .filter(|stone| stone.entry_id == entry_id)
                .collect();
            let cached = effective_id(ctx.statuses, entry_id);
            let facts = DesignFacts {
                extents: ctx.designs.extents.get(&cached),
                curves: ctx.designs.curves.get(&cached),
                preview_material: ctx.designs.materials.get(&cached).map(String::as_str),
                saved: ctx.shapes.get(&entry_id).and_then(saved_shape),
            };
            let (status, status_level) = status_of(ctx.statuses, entry_id);
            GroupRow {
                entry_id: i32::try_from(cached).unwrap_or(-1),
                name: design_name(ctx.titles, entry_id),
                count: to_i32(count),
                detail: group_detail(&stones),
                swatch_index,
                metrics: group_metrics(&stones, layout, ctx.model, &facts),
                status: status.to_string(),
                status_level,
                linkable: !matches!(ctx.statuses.get(&entry_id), Some(DesignStatus::Deleted)),
            }
        })
        .collect()
}

/// `text` with its first letter upper-cased.
fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// One saw stage in words: "cut X into 3 slabs: 8.10 / 8.10 / 7.80 mm", or the
/// no-cut form when the stage leaves a single part.
fn stage_phrase(axis: Axis, noun: &str, sizes: &[f64]) -> String {
    if let [only] = sizes {
        format!("no cut along {axis} (one {noun}, {only:.2} mm)")
    } else {
        let list: Vec<String> = sizes.iter().map(|size| format!("{size:.2}")).collect();
        format!(
            "cut {axis} into {} {noun}s: {} mm",
            sizes.len(),
            list.join(" / ")
        )
    }
}

/// The 1-based `(slab, bar, piece within its bar)` of every stone, in the layout's
/// traversal order (slab by slab, bar by bar, piece by piece): the numbers the cut plan
/// text prints as "Slab S / Bar B / Piece P" and the 3D view names on hover.
///
/// Empty when the layout has no saw plan (an exact fit) or when its stones do not match
/// the cut plan's pieces one to one, so a caller never names a position that does not
/// exist.
#[must_use]
pub fn piece_positions(layout: &RoughLayout) -> Vec<(usize, usize, usize)> {
    if layout.exact_fit {
        return Vec::new();
    }
    let positions: Vec<(usize, usize, usize)> = layout
        .cut_plan
        .slabs
        .iter()
        .enumerate()
        .flat_map(|(slab_index, slab)| {
            slab.bars
                .iter()
                .enumerate()
                .flat_map(move |(bar_index, bar)| {
                    (0..bar.pieces_mm.len())
                        .map(move |piece_index| (slab_index + 1, bar_index + 1, piece_index + 1))
                })
        })
        .collect();
    if positions.len() == layout.stones.len() {
        positions
    } else {
        Vec::new()
    }
}

/// The rough's six faces with their outward normals, in the tie-break order of the
/// "Orientation" metric.
const FACE_NORMALS: [(&str, [f64; 3]); 6] = [
    ("Top", [0.0, 1.0, 0.0]),
    ("Bottom", [0.0, -1.0, 0.0]),
    ("Right", [1.0, 0.0, 0.0]),
    ("Left", [-1.0, 0.0, 0.0]),
    ("Front", [0.0, 0.0, 1.0]),
    ("Back", [0.0, 0.0, -1.0]),
];

/// How a table normal sits against the rough, in the words of the "Orientation" metric:
/// "table faces Top" when it points at a face, otherwise "table tilted 37° from Top"
/// against the nearest face.
fn table_orientation(table_normal: [f64; 3]) -> String {
    let mut best = FACE_NORMALS[0].0;
    let mut best_dot = f64::NEG_INFINITY;
    for (name, face) in FACE_NORMALS {
        let dot: f64 = table_normal.iter().zip(face).map(|(a, b)| a * b).sum();
        if dot > best_dot {
            best_dot = dot;
            best = name;
        }
    }
    let angle_deg = best_dot.clamp(-1.0, 1.0).acos().to_degrees().round() as i32;
    if angle_deg == 0 {
        format!("table faces {best}")
    } else {
        format!("table tilted {angle_deg}° from {best}")
    }
}

/// The plan text of an exact fit: there is nothing to saw, only a placement.
fn exact_fit_text(layout: &RoughLayout) -> String {
    let Some(stone) = layout.stones.first() else {
        return "Exact fit, no saw plan.".to_string();
    };
    let [x, y, z] = stone.pose.center_mm;
    format!(
        "Exact fit, no saw plan: place the stone as shown in Orientation ({}), centre {x:.2}, {y:.2}, {z:.2} mm",
        table_orientation(stone.pose.axes[1])
    )
}

/// The multi-line cut plan: the saw stages slab by slab, bar by bar, with the stone that
/// comes out of every piece and which rough face its table faces. An exact fit has no
/// stages, so its text only says where the stone goes.
pub fn cut_plan_text(layout: &RoughLayout, titles: &BTreeMap<i64, String>, kerf_mm: f64) -> String {
    if layout.exact_fit {
        return exact_fit_text(layout);
    }
    let axes = layout.cut_order.axes().map(Axis::from_index);
    let mut lines = vec![
        format!(
            "Cut order: {} (each cut loses {kerf_mm:.2} mm)",
            format_cut_order(layout)
        ),
        "Stone sizes are X x Y x Z, in the rough's own axes.".to_string(),
    ];
    let slabs = &layout.cut_plan.slabs;
    let thicknesses: Vec<f64> = slabs.iter().map(|slab| slab.thickness_mm).collect();
    lines.push(capitalise(&stage_phrase(axes[0], "slab", &thicknesses)));
    let positions = piece_positions(layout);
    let mut stones = layout.stones.iter().enumerate();
    for (slab_index, slab) in slabs.iter().enumerate() {
        let widths: Vec<f64> = slab.bars.iter().map(|bar| bar.width_mm).collect();
        lines.push(format!(
            "Slab {} ({:.2} mm): {}",
            slab_index + 1,
            slab.thickness_mm,
            stage_phrase(axes[1], "bar", &widths)
        ));
        for (bar_index, bar) in slab.bars.iter().enumerate() {
            lines.push(format!(
                "  Bar {} ({:.2} mm): {}",
                bar_index + 1,
                bar.width_mm,
                stage_phrase(axes[2], "piece", &bar.pieces_mm)
            ));
            for piece_index in 0..bar.pieces_mm.len() {
                if let Some((stone_index, stone)) = stones.next() {
                    let piece = positions
                        .get(stone_index)
                        .map_or(piece_index + 1, |&(_, _, piece)| piece);
                    lines.push(format!(
                        "    Piece {piece}: {}, table faces {}, {} mm, {:.2} ct",
                        design_name(titles, stone.entry_id),
                        stone.table_axis,
                        dims_text(stone),
                        stone.carat
                    ));
                }
            }
        }
    }
    lines.join("\n")
}

/// One ranked layout as a result row.
pub fn result_row(rank: usize, layout: &RoughLayout, ctx: &RowContext<'_>) -> ResultRow {
    ResultRow {
        rank: to_i32(rank),
        total_ct: format_total_carat(layout.total_carat),
        yield_pct: format!("{:.1} %", layout.yield_fraction * 100.0),
        yield_weight_text: format_weighed_yield(layout.total_carat, ctx.weighed_ct),
        saw_text: ctx.rough_extents.map_or_else(String::new, |extents| {
            format_saw_work(
                layout,
                extents,
                ctx.settings.kerf_mm,
                ctx.settings.specific_gravity,
            )
        }),
        stone_count: to_i32(layout.stone_count()),
        groups: group_rows(layout, ctx),
        cut_plan: cut_plan_text(layout, ctx.titles, ctx.settings.kerf_mm),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::rough_plan::metrics::fixtures;
    use indicatrix_cut_core::rough_plan::{BarCut, CutOrder, CutPlan, SlabCut, StonePose};

    fn stone(entry_id: i64, carat: f64) -> PlacedStone {
        PlacedStone {
            entry_id,
            piece_origin_mm: [0.0; 3],
            piece_size_mm: [5.0; 3],
            stone_size_mm: [4.6; 3],
            table_axis: Axis::Y,
            carat,
            volume_mm3: carat * 20.0,
            pose: StonePose {
                center_mm: [0.0; 3],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            },
        }
    }

    /// A layout whose stones sit in one slab and one bar, one 5 mm piece each: the cut plan
    /// matches the stones one to one.
    fn layout(stones: Vec<PlacedStone>) -> RoughLayout {
        let total: f64 = stones.iter().map(|s| s.carat).sum();
        let cut_plan = CutPlan {
            slabs: vec![SlabCut {
                thickness_mm: 5.0,
                bars: vec![BarCut {
                    width_mm: 5.0,
                    pieces_mm: vec![5.0; stones.len()],
                }],
            }],
        };
        RoughLayout {
            cut_order: CutOrder::Yxz,
            stones,
            cut_plan,
            total_carat: total,
            total_volume_mm3: total * 20.0,
            yield_fraction: 0.412,
            exact_fit: false,
        }
    }

    /// Rows of a plan whose design shapes are unknown.
    static NO_SHAPES: BTreeMap<i64, DesignShape> = BTreeMap::new();

    fn context<'a>(
        titles: &'a BTreeMap<i64, String>,
        statuses: &'a BTreeMap<i64, DesignStatus>,
        settings: &'a PlanSettings,
        designs: &'a DesignData,
    ) -> RowContext<'a> {
        RowContext {
            titles,
            statuses,
            shapes: &NO_SHAPES,
            settings,
            weighed_ct: Some(9.54),
            model: None,
            rough_extents: Some([10.0, 8.0, 6.0]),
            designs,
        }
    }

    #[test]
    fn thousands_are_grouped_by_three() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(3299), "3,299");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn a_multi_part_stage_lists_its_sizes() {
        assert_eq!(
            stage_phrase(Axis::X, "slab", &[8.1, 8.1, 7.8]),
            "cut X into 3 slabs: 8.10 / 8.10 / 7.80 mm"
        );
    }

    #[test]
    fn a_single_part_stage_says_no_cut() {
        assert_eq!(
            capitalise(&stage_phrase(Axis::Y, "bar", &[12.0])),
            "No cut along Y (one bar, 12.00 mm)"
        );
    }

    #[test]
    fn the_group_order_is_most_stones_first_then_entry_id() {
        let layout = layout(vec![
            stone(9, 1.0),
            stone(3, 1.0),
            stone(5, 1.0),
            stone(5, 1.0),
            stone(3, 1.0),
            stone(7, 1.0),
        ]);
        // Two each of 3 and 5 (lower id first), then the singles 7 and 9.
        assert_eq!(group_order(&layout), vec![(3, 2), (5, 2), (7, 1), (9, 1)]);
    }

    #[test]
    fn a_fresh_plan_has_no_chips_and_every_group_is_linkable() {
        let titles = BTreeMap::from([(3, "Emerald cut".to_string())]);
        let (statuses, designs) = (BTreeMap::new(), DesignData::default());
        let settings = PlanSettings::default();
        let layout = layout(vec![stone(3, 1.0), stone(3, 1.0), stone(4, 2.0)]);
        let rows = group_rows(&layout, &context(&titles, &statuses, &settings, &designs));
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Emerald cut", "Design #4"]);
        assert_eq!(
            rows.iter().map(|r| r.swatch_index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(
            rows.iter()
                .all(|r| r.status.is_empty() && r.status_level == 0)
        );
        assert!(rows.iter().all(|r| r.linkable));
        assert_eq!(rows[0].count, 2);
        assert_eq!(rows[0].entry_id, 3);
    }

    #[test]
    fn statuses_become_chips_and_a_deleted_design_is_not_linkable() {
        let titles = BTreeMap::new();
        let statuses = BTreeMap::from([
            (1, DesignStatus::Deleted),
            (2, DesignStatus::Changed),
            (
                3,
                DesignStatus::MatchedByTitle {
                    resolved_entry_id: 30,
                },
            ),
            (4, DesignStatus::Unchanged),
        ]);
        let (designs, settings) = (DesignData::default(), PlanSettings::default());
        let layout = layout(vec![
            stone(1, 4.0),
            stone(2, 3.0),
            stone(3, 2.0),
            stone(4, 1.0),
        ]);
        let rows = group_rows(&layout, &context(&titles, &statuses, &settings, &designs));
        let chip = |id: i32| {
            let row = rows.iter().find(|r| r.name.ends_with(&format!("#{id}")));
            row.map(|r| (r.status.clone(), r.status_level, r.linkable, r.entry_id))
        };
        assert_eq!(chip(1), Some(("design deleted".to_string(), 3, false, 1)));
        assert_eq!(
            chip(2),
            Some(("design changed since saved".to_string(), 2, true, 2))
        );
        // A design matched by title links to the entry it resolved to.
        assert_eq!(chip(3), Some(("matched by title".to_string(), 1, true, 30)));
        assert_eq!(chip(4), Some((String::new(), 0, true, 4)));
    }

    #[test]
    fn a_layout_already_pointed_at_the_resolved_entry_still_shows_the_chip() {
        let statuses = BTreeMap::from([(
            3,
            DesignStatus::MatchedByTitle {
                resolved_entry_id: 30,
            },
        )]);
        assert_eq!(status_of(&statuses, 30), ("matched by title", 1));
        assert_eq!(status_of(&statuses, 31), ("", 0));
        assert_eq!(effective_id(&statuses, 3), 30);
        assert_eq!(effective_id(&statuses, 30), 30);
    }

    #[test]
    fn a_row_carries_the_layout_figures_and_the_weighed_yield() {
        let titles = BTreeMap::new();
        let (statuses, designs) = (BTreeMap::new(), DesignData::default());
        let settings = PlanSettings::default();
        let mut ctx = context(&titles, &statuses, &settings, &designs);
        let layout = layout(vec![stone(3, 3.714)]);
        let row = result_row(4, &layout, &ctx);
        assert_eq!(row.rank, 4);
        assert_eq!(row.total_ct, "3.71 ct");
        assert_eq!(row.yield_pct, "41.2 %");
        assert_eq!(row.yield_weight_text, "38.9 % of 9.54 ct weighed");
        assert_eq!(row.saw_text, "0 cuts · kerf loss ≤ 0.00 ct");
        assert_eq!(row.stone_count, 1);
        assert!(row.cut_plan.starts_with(
            "Cut order: slabs across Y, bars across X, pieces across Z (each cut loses 0.30 mm)"
        ));

        ctx.weighed_ct = None;
        assert_eq!(result_row(1, &layout, &ctx).yield_weight_text, "");
        ctx.rough_extents = None;
        assert_eq!(result_row(1, &layout, &ctx).saw_text, "");
    }

    /// Five stones (entry ids 11 to 15) in a plan of two slabs: the first has two bars
    /// (two pieces, then one), the second one bar of two pieces.
    fn five_piece_layout() -> RoughLayout {
        let bar = |width_mm: f64, pieces_mm: &[f64]| BarCut {
            width_mm,
            pieces_mm: pieces_mm.to_vec(),
        };
        let mut layout = layout((11..=15).map(|id| stone(id, 1.0)).collect());
        layout.cut_order = CutOrder::Xyz;
        layout.cut_plan = CutPlan {
            slabs: vec![
                SlabCut {
                    thickness_mm: 4.0,
                    bars: vec![bar(3.0, &[2.0, 3.0]), bar(4.0, &[5.0])],
                },
                SlabCut {
                    thickness_mm: 5.0,
                    bars: vec![bar(8.0, &[6.0, 1.0])],
                },
            ],
        };
        layout
    }

    #[test]
    fn piece_positions_count_slab_bar_and_piece_from_one() {
        // Slab 1 / bar 1 holds pieces 1 and 2, bar 2 piece 1; slab 2 / bar 1 pieces 1, 2.
        assert_eq!(
            piece_positions(&five_piece_layout()),
            [(1, 1, 1), (1, 1, 2), (1, 2, 1), (2, 1, 1), (2, 1, 2)]
        );
    }

    #[test]
    fn piece_positions_are_empty_when_they_would_be_wrong() {
        // Four stones for five pieces: no stone can be named by a position.
        let none = Vec::<(usize, usize, usize)>::new();
        let mut short = five_piece_layout();
        short.stones.pop();
        assert_eq!(piece_positions(&short), none);
        // An exact fit has a cut plan of one piece but no saw plan to point into.
        assert_eq!(piece_positions(&fixtures::exact_fit_layout()), none);
    }

    /// The `(slab, bar, piece, design name)` of every "Piece" line of a plan text, read
    /// from the "Slab S" and "  Bar B" headers above it.
    fn piece_lines(text: &str) -> Vec<(usize, usize, usize, String)> {
        let number = |rest: &str| {
            rest.split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|digits| digits.parse::<usize>().ok())
                .expect("a number follows the keyword")
        };
        let (mut slab, mut bar) = (0, 0);
        let mut found = Vec::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Slab ") {
                slab = number(rest);
            } else if let Some(rest) = line.strip_prefix("  Bar ") {
                bar = number(rest);
            } else if let Some(rest) = line.strip_prefix("    Piece ") {
                let name = rest
                    .split_once(": ")
                    .and_then(|(_, tail)| tail.split(',').next())
                    .expect("a piece line names its design");
                found.push((slab, bar, number(rest), name.to_string()));
            }
        }
        found
    }

    #[test]
    fn the_plan_text_numbers_each_stone_as_its_position_says() {
        let layout = five_piece_layout();
        let titles: BTreeMap<i64, String> = (11..=15)
            .map(|id| (id, format!("Stone {}", id - 10)))
            .collect();
        let lines = piece_lines(&cut_plan_text(&layout, &titles, 0.3));
        let positions = piece_positions(&layout);
        assert_eq!(lines.len(), 5);
        for (index, (slab, bar, piece, name)) in lines.iter().enumerate() {
            // Stone `index` was given the title "Stone {index + 1}".
            assert_eq!(name, &format!("Stone {}", index + 1));
            assert_eq!((*slab, *bar, *piece), positions[index], "stone {index}");
        }
    }

    #[test]
    fn an_exact_fit_row_has_no_saw_plan_and_says_where_to_put_the_stone() {
        let fit = fixtures::exact_fit_layout();
        let titles = BTreeMap::new();
        let (statuses, designs) = (BTreeMap::new(), DesignData::default());
        let settings = PlanSettings::default();
        let ctx = context(&titles, &statuses, &settings, &designs);
        let row = result_row(1, &fit, &ctx);
        // Table normal (0.6, 0.8, 0): acos(0.8) = 36.87 degrees from Top, shown as 37;
        // the pose centre is (5, 4, 3) mm.
        assert_eq!(
            row.cut_plan,
            "Exact fit, no saw plan: place the stone as shown in Orientation \
             (table tilted 37° from Top), centre 5.00, 4.00, 3.00 mm"
        );
        assert_eq!(row.saw_text, "");
        assert_eq!(row.stone_count, 1);
        for forbidden in ["Cut order", "Slab", "Bar", "Piece"] {
            assert!(!row.cut_plan.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn an_exact_fit_with_its_table_square_to_a_face_says_it_faces_that_face() {
        let mut fit = fixtures::exact_fit_layout();
        fit.stones[0].pose.axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let text = cut_plan_text(&fit, &BTreeMap::new(), 0.3);
        assert!(text.contains("(table faces Top)"), "{text}");
        // Pointing along -X the nearest face is Left.
        fit.stones[0].pose.axes[1] = [-1.0, 0.0, 0.0];
        let text = cut_plan_text(&fit, &BTreeMap::new(), 0.3);
        assert!(text.contains("(table faces Left)"), "{text}");
    }

    #[test]
    fn a_planned_layout_row_lists_one_piece_line_per_stone() {
        // Four 5 mm cubes fill a 10 x 10 x 5 mm block, so the planner returns four
        // stones; a three-stage plan for four pieces needs three cuts whatever the order.
        let layout = fixtures::planned_four_stone_layout();
        let positions = piece_positions(&layout);
        assert_eq!(positions.len(), 4);
        let plan = &layout.cut_plan.slabs;
        for &(slab, bar, piece) in &positions {
            assert!(piece <= plan[slab - 1].bars[bar - 1].pieces_mm.len());
        }
        let titles = BTreeMap::new();
        let (statuses, designs) = (BTreeMap::new(), DesignData::default());
        let settings = PlanSettings {
            kerf_mm: 0.0,
            ..PlanSettings::default()
        };
        let mut ctx = context(&titles, &statuses, &settings, &designs);
        ctx.rough_extents = Some([10.0, 10.0, 5.0]);
        let row = result_row(1, &layout, &ctx);
        assert_eq!(row.stone_count, 4);
        assert_eq!(row.saw_text, "3 cuts · kerf loss ≤ 0.00 ct");
        assert!(row.cut_plan.starts_with("Cut order: slabs across"));
        let lines = piece_lines(&row.cut_plan);
        assert_eq!(lines.len(), 4);
        for ((slab, bar, piece, _), expected) in lines.iter().zip(&positions) {
            assert_eq!((*slab, *bar, *piece), *expected);
        }
    }
}
