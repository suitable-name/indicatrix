//! The metric list of one design group in a result card: weight, size in the stone's own
//! terms, ratios, volume, orientation, fill and optics.
//!
//! A group is all stones of one design in a layout; they can differ in scale, in the face
//! their table looks at and in how much of their piece they use, so every metric that
//! can differ shows the range (or each variant) instead of the first stone's figure.

use crate::gui::rough_plan::saved::dto::SavedDesignDto;
use glam::DVec3;
use indicatrix::geometry::stone_metrics::measure_solid;
use indicatrix_cut_core::rough_plan::{
    PlacedStone, RoughLayout, RoughMesh, RoughModel,
    shaped::{CLASS_EXTERIOR, CLASS_INTERIOR, classify_box_into},
};
use indicatrix_vault::model::{
    solid_extents::SolidExtents,
    tilt_curves::{AxisTiltCurves, TILT_CURVE_POINTS_PER_AXIS, TiltPerformanceCurves},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

/// The nearest rough faces for an orientation: name, axis label and outward normal.
const FACES: [(&str, &str, [f64; 3]); 6] = [
    ("Top", "+Y", [0.0, 1.0, 0.0]),
    ("Bottom", "-Y", [0.0, -1.0, 0.0]),
    ("Right", "+X", [1.0, 0.0, 0.0]),
    ("Left", "-X", [-1.0, 0.0, 0.0]),
    ("Front", "+Z", [0.0, 0.0, 1.0]),
    ("Back", "-Z", [0.0, 0.0, -1.0]),
];

/// What a fill percentage was measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FillBase {
    /// The sawn piece, cut down to the modelled rough.
    Piece,
    /// The whole modelled rough (an exact single-stone fit has no piece of its own).
    Model,
}

/// The modelled rough as far as the fill figure needs it.
#[derive(Debug, Clone)]
pub struct ModelGeometry {
    /// The rough's volume in mm³.
    pub volume_mm3: f64,
    /// The extents of its bounding box in mm.
    pub extents_mm: [f64; 3],
    /// Its bounding planes `(n, m)` with `n · p <= m`, in the rough frame.
    pub planes: Vec<(DVec3, f64)>,
    /// The mesh of a non-convex rough, which the pieces' volumes are measured against
    /// instead of `planes`; `None` for a convex rough.
    pub mesh: Option<MeshGeometry>,
}

/// A non-convex rough as the fill figure needs it.
#[derive(Debug, Clone)]
pub struct MeshGeometry {
    /// The rough's closed mesh, in the rough frame.
    pub mesh: Arc<RoughMesh>,
    /// The model's cuts as planes `(n, m)` with `n · p <= m` (the base's own planes left
    /// out: the mesh lies inside them).
    pub cuts: Vec<(DVec3, f64)>,
    /// The volumes already measured, by the bits of a piece's origin and size: every stone
    /// of a card and every redraw asks for its piece, and many pieces are the same box.
    volumes: PieceVolumes,
}

/// A piece box (origin and size, as bits) and the volume of the rough inside it.
type PieceVolumes = Arc<Mutex<BTreeMap<[u64; 6], Option<f64>>>>;

impl ModelGeometry {
    /// Measures `model`; `None` when it is not a valid solid.
    #[must_use]
    pub fn of(model: &RoughModel) -> Option<Self> {
        let planes = model.halfspaces().ok()?;
        let measure = model.measure().ok()?;
        let mesh = model.mesh().map(|mesh| {
            let base = model.base.to_halfspaces(false).map_or(0, |base| base.len());
            MeshGeometry {
                mesh,
                cuts: planes.get(base..).unwrap_or_default().to_vec(),
                volumes: PieceVolumes::default(),
            }
        });
        Some(Self {
            volume_mm3: measure.volume_mm3,
            extents_mm: measure.extents_mm,
            planes,
            mesh,
        })
    }
}

/// What a saved plan recorded about a design's size when it was saved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SavedShape {
    /// The design's width (the smaller caliper extent) in model units then.
    pub width_caliper: f64,
    /// Its `[L/W, H/W, V/W^3]` fingerprint then; all zero means unknown.
    pub fingerprint: [f64; 3],
}

/// The size a saved plan's design record stores. `Err` when the record has no width (a
/// file from before widths were stored): the library's extents are the only size then.
impl TryFrom<&SavedDesignDto> for SavedShape {
    type Error = ();

    fn try_from(design: &SavedDesignDto) -> Result<Self, ()> {
        design
            .width_caliper
            .map(|width_caliper| Self {
                width_caliper,
                fingerprint: design.fingerprint,
            })
            .ok_or(())
    }
}

/// What is known about a design beyond its stones in the layout.
#[derive(Debug, Clone, Copy, Default)]
pub struct DesignFacts<'a> {
    /// The design's cached caliper extents.
    pub extents: Option<&'a SolidExtents>,
    /// Its cached tilt curves.
    pub curves: Option<&'a TiltPerformanceCurves>,
    /// The material the preview and the curves were made with, if recorded.
    pub preview_material: Option<&'a str>,
    /// The size the design had when the layout was planned (a loaded plan: what its file
    /// recorded; `None` when unknown, for example a file from before widths were stored).
    /// The stones' scale belongs to that size, so the Size line is computed from it, not
    /// from today's extents.
    pub saved: Option<SavedShape>,
}

/// The `(label, value)` list of one design group. `stones` are the layout's stones of the
/// design; `model` is the modelled rough (the fill line is left out without it).
#[must_use]
pub fn group_metrics(
    stones: &[&PlacedStone],
    layout: &RoughLayout,
    model: Option<&ModelGeometry>,
    design: &DesignFacts<'_>,
) -> Vec<(String, String)> {
    let mut metrics = Vec::new();
    if stones.is_empty() {
        return metrics;
    }
    let mut push = |label: &str, value: String| metrics.push((label.to_string(), value));

    push("Weight", weight_value(stones, layout));
    if let Some(size) = size_value(stones, design.extents, design.saved.as_ref()) {
        push("Size", size);
    }
    if let Some(ratios) = design.extents.and_then(ratios_value) {
        push("Ratios", ratios);
    }
    push("Volume", volume_value(stones));
    push("Orientation", orientation_value(stones));
    if let Some(fill) = model.and_then(|model| fill_value(stones, layout, model)) {
        push("Fill", fill);
    }
    push(
        "Optics",
        format_optics(design.curves, design.preview_material),
    );
    metrics
}

/// `values` at `decimals` places, and whether every value prints alike: the one figure when
/// they do, else "lowest-highest". What a reader sees decides "alike", so two weights of
/// 0.6201 and 0.6215 ct are "0.62", not the range "0.62-0.62".
fn spread_alike(values: impl Iterator<Item = f64>, decimals: usize) -> (String, bool) {
    let (low, high) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), v| {
        (low.min(v), high.max(v))
    });
    let (low, high) = (format!("{low:.decimals$}"), format!("{high:.decimals$}"));
    if low == high {
        (low, true)
    } else {
        (format!("{low}-{high}"), false)
    }
}

/// `values` at `decimals` places: the one figure when every value prints alike, else
/// "lowest-highest".
fn spread(values: impl Iterator<Item = f64>, decimals: usize) -> String {
    spread_alike(values, decimals).0
}

/// " each" for a group of several stones that share one figure, else nothing.
const fn each_suffix(count: usize, alike: bool) -> &'static str {
    if count > 1 && alike { " each" } else { "" }
}

/// The "Weight" value: one weight, a common weight, or the range, with the group's share
/// of the layout's total.
fn weight_value(stones: &[&PlacedStone], layout: &RoughLayout) -> String {
    let group_carat: f64 = stones.iter().map(|s| s.carat).sum();
    let share = if layout.total_carat > 0.0 {
        ((group_carat / layout.total_carat) * 100.0).round() as i32
    } else {
        0
    };
    let (weight, alike) = spread_alike(stones.iter().map(|s| s.carat), 2);
    let each = each_suffix(stones.len(), alike);
    format!("{weight} ct{each} · {share} % of the total")
}

/// The "Volume" value: one volume, a common volume, or the range.
fn volume_value(stones: &[&PlacedStone]) -> String {
    let (volume, alike) = spread_alike(stones.iter().map(|s| s.volume_mm3), 1);
    format!("{volume} mm³{}", each_suffix(stones.len(), alike))
}

/// The design's width and length in model units, the smaller first.
const fn width_and_length(extents: &SolidExtents) -> (f64, f64) {
    (
        extents.width_caliper.min(extents.length_caliper),
        extents.width_caliper.max(extents.length_caliper),
    )
}

/// A design's `(width, length, height)` in model units.
type Dimensions = (f64, f64, f64);

/// The dimensions the plan recorded: its width and the ratios of its fingerprint. `None`
/// when the width or the fingerprint is unknown.
fn saved_dimensions(saved: &SavedShape) -> Option<Dimensions> {
    let [length_ratio, height_ratio, _] = saved.fingerprint;
    let usable = [saved.width_caliper, length_ratio, height_ratio]
        .iter()
        .all(|figure| figure.is_finite() && *figure > 0.0);
    usable.then_some((
        saved.width_caliper,
        saved.width_caliper * length_ratio,
        saved.width_caliper * height_ratio,
    ))
}

/// Whether two widths differ by more than a millionth of the larger.
fn widths_differ(a: f64, b: f64) -> bool {
    (a - b).abs() > 1e-6 * a.abs().max(b.abs())
}

/// The "Size" value in the stone's own terms: length along the caliper z, width along x,
/// depth the total height. Stones of unlike scale show the range of each figure.
///
/// A stone's scale (`mm_per_unit`) was fitted to the design as it was when the plan was
/// made, so the figures are that design's times the scale: from the plan's recorded size
/// when it has one, else from the library's extents now. When both are known and the widths
/// differ the design was redrawn since, and the line says so (the stones themselves are
/// still what was planned). `None` when neither is known.
fn size_value(
    stones: &[&PlacedStone],
    extents: Option<&SolidExtents>,
    saved: Option<&SavedShape>,
) -> Option<String> {
    let current: Option<Dimensions> = extents.map(|extents| {
        let (width, length) = width_and_length(extents);
        (width, length, extents.height)
    });
    let recorded = saved.and_then(saved_dimensions);
    let (width, length, height) = recorded.or(current)?;
    let scaled = |unit: f64| spread(stones.iter().map(|s| s.pose.mm_per_unit * unit), 2);
    let mut text = format!(
        "L x W x D = {} x {} x {} mm",
        scaled(length),
        scaled(width),
        scaled(height)
    );
    if let (Some((was, ..)), Some((now, ..))) = (recorded, current)
        && widths_differ(was, now)
    {
        text.push_str(" (design rescaled)");
    }
    Some(text)
}

/// The "Ratios" value. A stone is its design scaled uniformly, so the ratios belong to
/// the design and are the same for every stone of the group.
fn ratios_value(extents: &SolidExtents) -> Option<String> {
    let (width, length) = width_and_length(extents);
    (width > 0.0).then(|| {
        format!(
            "L/W {:.2} · D/W {:.0} %",
            length / width,
            extents.height / width * 100.0
        )
    })
}

/// How a table normal sits against the rough: "faces Top (+Y)" when it points at a face,
/// otherwise "tilted 23° from Top" against the nearest face.
fn orientation_phrase(table_normal: [f64; 3]) -> String {
    let mut best = FACES[0];
    let mut best_dot = f64::NEG_INFINITY;
    for face in FACES {
        let dot = table_normal[0].mul_add(
            face.2[0],
            table_normal[1].mul_add(face.2[1], table_normal[2] * face.2[2]),
        );
        if dot > best_dot {
            best_dot = dot;
            best = face;
        }
    }
    let angle_deg = best_dot.clamp(-1.0, 1.0).acos().to_degrees().round() as i32;
    if angle_deg == 0 {
        format!("faces {} ({})", best.0, best.1)
    } else {
        format!("tilted {angle_deg}° from {}", best.0)
    }
}

/// The "Orientation" value: the one orientation of the group, or each orientation with
/// its stone count, most stones first.
fn orientation_value(stones: &[&PlacedStone]) -> String {
    let mut variants: Vec<(String, usize)> = Vec::new();
    for stone in stones {
        let phrase = orientation_phrase(stone.pose.axes[1]);
        match variants.iter_mut().find(|(seen, _)| *seen == phrase) {
            Some((_, count)) => *count += 1,
            None => variants.push((phrase, 1)),
        }
    }
    if let [(phrase, _)] = variants.as_slice() {
        return format!("table {phrase}");
    }
    variants.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let parts: Vec<String> = variants
        .iter()
        .map(|(phrase, count)| format!("{count} × {phrase}"))
        .collect();
    format!("table: {}", parts.join(", "))
}

/// The volume of the sawn piece `origin..origin + size` inside the modelled rough. A piece
/// fully inside is its box; one that pokes out is measured as the polytope of the box and
/// the planes it crosses; with a mesh, as the exact volume of the mesh inside the box and
/// the cuts. `None` for a degenerate piece or one outside the rough.
fn piece_model_volume(model: &ModelGeometry, origin: [f64; 3], size: [f64; 3]) -> Option<f64> {
    let planes = &model.planes;
    let box_volume = size[0] * size[1] * size[2];
    if !box_volume.is_finite() || box_volume <= 0.0 {
        return None;
    }
    let far = [0, 1, 2].map(|i| origin[i] + size[i]);
    if let Some(rough) = &model.mesh {
        let key = [0, 1, 2, 3, 4, 5].map(|i| if i < 3 { origin[i] } else { size[i - 3] }.to_bits());
        if let Some(&known) = rough
            .volumes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return known;
        }
        let mut region = rough.cuts.clone();
        for (axis, normal) in [DVec3::X, DVec3::Y, DVec3::Z].into_iter().enumerate() {
            region.push((normal, far[axis]));
            region.push((-normal, -origin[axis]));
        }
        let volume = Some(rough.mesh.volume_within(&region)).filter(|volume| *volume > 0.0);
        rough
            .volumes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, volume);
        return volume;
    }
    let mut crossed = Vec::new();
    match classify_box_into(origin, far, planes, &mut crossed) {
        CLASS_INTERIOR => Some(box_volume),
        CLASS_EXTERIOR => None,
        _ => {
            let half = DVec3::from(size) * 0.5;
            let centre = DVec3::from(origin) + half;
            let mut region: Vec<(DVec3, f64)> = crossed
                .iter()
                .map(|&i| (planes[i].0, planes[i].1 - planes[i].0.dot(centre)))
                .collect();
            for (axis, reach) in [(DVec3::X, half.x), (DVec3::Y, half.y), (DVec3::Z, half.z)] {
                region.push((axis, reach));
                region.push((-axis, reach));
            }
            measure_solid(&region)
                .map(|metrics| metrics.volume)
                .filter(|volume| *volume > 0.0)
        }
    }
}

/// How much of its piece (or, for a single fit, of the rough) `stone` uses, in percent.
fn stone_fill(
    stone: &PlacedStone,
    layout: &RoughLayout,
    model: &ModelGeometry,
) -> Option<(f64, FillBase)> {
    if layout.exact_fit {
        if model.volume_mm3 <= 0.0 {
            return None;
        }
        return Some((stone.volume_mm3 / model.volume_mm3 * 100.0, FillBase::Model));
    }
    let piece = piece_model_volume(model, stone.piece_origin_mm, stone.piece_size_mm)?;
    Some((stone.volume_mm3 / piece * 100.0, FillBase::Piece))
}

/// The "Fill" value: the stone's volume over the volume of its piece box inside the
/// modelled rough (a single fit: over the rough). A group shows the range.
fn fill_value(
    stones: &[&PlacedStone],
    layout: &RoughLayout,
    model: &ModelGeometry,
) -> Option<String> {
    let fills: Vec<(f64, FillBase)> = stones
        .iter()
        .filter_map(|stone| stone_fill(stone, layout, model))
        .collect();
    let (_, base) = *fills.first()?;
    let percent = spread(fills.iter().map(|(percent, _)| *percent), 0);
    Some(match (base, stones.len()) {
        (FillBase::Model, _) => format!("stone uses {percent} % of the model"),
        (FillBase::Piece, 1) => format!("stone uses {percent} % of its piece"),
        (FillBase::Piece, _) => format!("stones use {percent} % of their pieces"),
    })
}

/// The sample index of the table-up (face-up) tilt: the middle of the 181-point sweep
/// (index 0 is the edge-on extreme at -90 degrees).
const FACE_UP: usize = TILT_CURVE_POINTS_PER_AXIS / 2;

/// Face-up optical metrics from the cached tilt curves: the mean over the axes at the
/// table-up sample, and the material the curves were made with when it is known.
///
/// A sample that is not a finite number (a curve that was stored broken) is left out of
/// its mean, so one bad axis neither shows "NaN" nor reads as zero; a metric with no finite
/// sample at all reads "n/a", and curves with none for any metric read "not generated yet".
fn format_optics(curves: Option<&TiltPerformanceCurves>, preview_material: Option<&str>) -> String {
    let Some(curves) = curves else {
        return "not generated yet".to_string();
    };
    let mean = |pick: fn(&AxisTiltCurves) -> f32| -> Option<i32> {
        let samples: Vec<f64> = curves
            .axes
            .iter()
            .map(|axis| f64::from(pick(axis)))
            .filter(|sample| sample.is_finite())
            .collect();
        (!samples.is_empty())
            .then(|| (samples.iter().sum::<f64>() / samples.len() as f64).round() as i32)
    };
    let figures = [
        mean(|axis| axis.brilliance_pct[FACE_UP]),
        mean(|axis| axis.extinction_pct[FACE_UP]),
        mean(|axis| axis.windowing_pct[FACE_UP]),
    ];
    if figures.iter().all(Option::is_none) {
        return "not generated yet".to_string();
    }
    let [brilliance, extinction, windowing] = figures
        .map(|figure| figure.map_or_else(|| "n/a".to_string(), |value| format!("{value} %")));
    let material = preview_material
        .filter(|name| !name.trim().is_empty())
        .map_or_else(String::new, |name| format!(" (preview material: {name})"));
    format!("brilliance {brilliance} · extinction {extinction} · windowing {windowing}{material}")
}

#[cfg(test)]
mod tests;
