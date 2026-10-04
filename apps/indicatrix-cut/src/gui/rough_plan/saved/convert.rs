//! Conversions between the plan file's documents ([`super::dto`]) and the core types.
//!
//! Reading validates every number and every name and reports the offending field as a
//! path such as `layouts[1].stones[0].axes[2]`; it never panics. Writing is the plain
//! inverse and cannot fail.

use super::{
    dto::{
        BarDto, CutDto, MAX_DESIGNS, MAX_LOSS_MM, MAX_MIN_WIDTH_MM, MAX_SAW_ITEMS,
        MAX_STONES_PER_LAYOUT, RoughDto, SavedDesignDto, SavedLayoutDto, SettingsDto, SlabDto,
        StoneDto,
    },
    hull_base::{base_from_hull, write_hull},
};
use glam::DVec3;
use indicatrix_cut_core::rough_plan::{
    Axis, BarCut, BoxFace, CutOrder, CutPlan, PlacedStone, PlanSettings, RoughBase, RoughCut,
    RoughLayout, RoughModel, SlabCut, fit::StonePose,
};
use std::collections::BTreeSet;

/// How far a pose axis may be from unit length, and how far two axes may be from
/// perpendicular (as a dot product).
pub const AXIS_TOLERANCE: f64 = 1e-6;

/// A finite number.
fn finite(value: f64, path: &str) -> Result<f64, String> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{path} must be a finite number, found {value}"))
    }
}

/// A finite number greater than zero.
fn positive(value: f64, path: &str) -> Result<f64, String> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(format!(
            "{path} must be a finite number greater than 0, found {value}"
        ))
    }
}

/// A finite number of at least zero.
fn non_negative(value: f64, path: &str) -> Result<f64, String> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(format!(
            "{path} must be a finite number of at least 0, found {value}"
        ))
    }
}

/// A finite number of at least zero and at most `max`.
fn non_negative_up_to(value: f64, max: f64, path: &str) -> Result<f64, String> {
    let value = non_negative(value, path)?;
    if value <= max {
        Ok(value)
    } else {
        Err(format!("{path} must be at most {max}, found {value}"))
    }
}

/// A finite number greater than zero and at most `max`.
fn positive_up_to(value: f64, max: f64, path: &str) -> Result<f64, String> {
    let value = positive(value, path)?;
    if value <= max {
        Ok(value)
    } else {
        Err(format!("{path} must be at most {max}, found {value}"))
    }
}

/// Three finite numbers.
fn finite3(values: [f64; 3], path: &str) -> Result<[f64; 3], String> {
    for (i, &value) in values.iter().enumerate() {
        finite(value, &format!("{path}[{i}]"))?;
    }
    Ok(values)
}

/// Three finite numbers greater than zero.
fn positive3(values: [f64; 3], path: &str) -> Result<[f64; 3], String> {
    for (i, &value) in values.iter().enumerate() {
        positive(value, &format!("{path}[{i}]"))?;
    }
    Ok(values)
}

/// A box face from its file name.
fn parse_box_face(text: &str, path: &str) -> Result<BoxFace, String> {
    match text.trim().to_lowercase().as_str() {
        "top" => Ok(BoxFace::Top),
        "bottom" => Ok(BoxFace::Bottom),
        "right" => Ok(BoxFace::Right),
        "left" => Ok(BoxFace::Left),
        "front" => Ok(BoxFace::Front),
        "back" => Ok(BoxFace::Back),
        other => Err(format!(
            "{path} names an unknown box face '{other}' (expected top, bottom, right, left, front or back)"
        )),
    }
}

/// The file name of a box face.
const fn box_face_name(face: BoxFace) -> &'static str {
    match face {
        BoxFace::Top => "top",
        BoxFace::Bottom => "bottom",
        BoxFace::Right => "right",
        BoxFace::Left => "left",
        BoxFace::Front => "front",
        BoxFace::Back => "back",
    }
}

/// An axis from its file name.
fn parse_axis(text: &str, path: &str) -> Result<Axis, String> {
    match text.trim().to_lowercase().as_str() {
        "x" => Ok(Axis::X),
        "y" => Ok(Axis::Y),
        "z" => Ok(Axis::Z),
        other => Err(format!(
            "{path} names an unknown axis '{other}' (expected x, y or z)"
        )),
    }
}

/// The file name of an axis.
const fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

/// A cut order from its file name.
fn parse_cut_order(text: &str, path: &str) -> Result<CutOrder, String> {
    match text.trim().to_lowercase().as_str() {
        "xyz" => Ok(CutOrder::Xyz),
        "xzy" => Ok(CutOrder::Xzy),
        "yxz" => Ok(CutOrder::Yxz),
        "yzx" => Ok(CutOrder::Yzx),
        "zxy" => Ok(CutOrder::Zxy),
        "zyx" => Ok(CutOrder::Zyx),
        other => Err(format!("{path} names an unknown cut order '{other}'")),
    }
}

/// The file name of a cut order.
const fn cut_order_name(order: CutOrder) -> &'static str {
    match order {
        CutOrder::Xyz => "xyz",
        CutOrder::Xzy => "xzy",
        CutOrder::Yxz => "yxz",
        CutOrder::Yzx => "yzx",
        CutOrder::Zxy => "zxy",
        CutOrder::Zyx => "zyx",
    }
}

/// A required optional field.
fn required<T: Copy>(value: Option<T>, path: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("{path} is missing"))
}

/// The base shape of `dto`, with its size fields checked and named.
fn base_from_dto(dto: &RoughDto) -> Result<RoughBase, String> {
    let sizes = |what: &str| -> Result<[f64; 3], String> {
        Ok([
            positive(
                required(dto.x_mm, &format!("rough.x_mm ({what})"))?,
                "rough.x_mm",
            )?,
            positive(
                required(dto.y_mm, &format!("rough.y_mm ({what})"))?,
                "rough.y_mm",
            )?,
            positive(
                required(dto.z_mm, &format!("rough.z_mm ({what})"))?,
                "rough.z_mm",
            )?,
        ])
    };
    let base = match dto.base.trim().to_lowercase().as_str() {
        "block" => {
            let [x_mm, y_mm, z_mm] = sizes("block")?;
            RoughBase::Block { x_mm, y_mm, z_mm }
        }
        "pebble" => {
            let [x_mm, y_mm, z_mm] = sizes("pebble")?;
            RoughBase::Pebble { x_mm, y_mm, z_mm }
        }
        "cylinder" => {
            let diameter_mm = positive(
                required(dto.diameter_mm, "rough.diameter_mm (cylinder)")?,
                "rough.diameter_mm",
            )?;
            let length_mm = positive(
                required(dto.length_mm, "rough.length_mm (cylinder)")?,
                "rough.length_mm",
            )?;
            let axis = dto
                .axis
                .as_deref()
                .ok_or_else(|| "rough.axis (cylinder) is missing".to_string())?;
            RoughBase::Cylinder {
                diameter_mm,
                length_mm,
                axis: parse_axis(axis, "rough.axis")?,
            }
        }
        "hull" => base_from_hull(dto)?,
        other => {
            return Err(format!(
                "rough.base names an unknown shape '{other}' (expected block, cylinder, pebble or hull)"
            ));
        }
    };
    base.validate().map_err(|e| format!("rough: {e}"))?;
    Ok(base)
}

/// The material name, specific gravity and optional weighed carat of `dto`.
///
/// # Errors
///
/// Returns a message naming the field when the name is empty, the gravity is not a
/// positive number or the weighed carat is not.
pub fn material_from_dto(dto: &RoughDto) -> Result<(String, f64, Option<f64>), String> {
    let name = dto.material.trim();
    if name.is_empty() {
        return Err("rough.material must not be empty".to_string());
    }
    let gravity = positive(dto.specific_gravity, "rough.specific_gravity")?;
    let weighed = dto
        .weighed_ct
        .map(|value| positive(value, "rough.weighed_ct"))
        .transpose()?;
    Ok((name.to_string(), gravity, weighed))
}

/// The faces of an edge or corner cut: `N` faces on `N` different axes (which makes them
/// distinct and adjacent).
fn cut_faces<const N: usize>(dto: &CutDto, path: &str) -> Result<[BoxFace; N], String> {
    let names = dto
        .faces
        .as_deref()
        .ok_or_else(|| format!("{path}.faces is missing"))?;
    if names.len() != N {
        return Err(format!(
            "{path}.faces must list {N} faces, found {}",
            names.len()
        ));
    }
    let mut faces = [BoxFace::Top; N];
    let mut axes = BTreeSet::new();
    for (i, (slot, name)) in faces.iter_mut().zip(names).enumerate() {
        let face = parse_box_face(name, &format!("{path}.faces[{i}]"))?;
        if !axes.insert(face.axis_and_side().0) {
            return Err(format!(
                "{path}.faces[{i}] '{}' shares its axis with another face; the faces must be distinct and adjacent",
                name.trim()
            ));
        }
        *slot = face;
    }
    Ok(faces)
}

/// The setbacks of an edge or corner cut: `N` finite lengths greater than zero.
fn cut_setbacks<const N: usize>(dto: &CutDto, path: &str) -> Result<[f64; N], String> {
    let values = dto
        .setbacks_mm
        .as_deref()
        .ok_or_else(|| format!("{path}.setbacks_mm is missing"))?;
    if values.len() != N {
        return Err(format!(
            "{path}.setbacks_mm must list {N} lengths, found {}",
            values.len()
        ));
    }
    let mut setbacks = [0.0; N];
    for (i, (slot, &value)) in setbacks.iter_mut().zip(values).enumerate() {
        *slot = positive(value, &format!("{path}.setbacks_mm[{i}]"))?;
    }
    Ok(setbacks)
}

/// A face cut: any non-zero finite normal (the core normalises it) and a positive depth.
fn face_cut_from_dto(dto: &CutDto, path: &str) -> Result<RoughCut, String> {
    let normal = finite3(
        required(dto.normal, &format!("{path}.normal"))?,
        &format!("{path}.normal"),
    )?;
    if DVec3::from(normal).length() <= 1e-9 {
        return Err(format!("{path}.normal must not be the zero vector"));
    }
    let depth_mm = positive(
        required(dto.depth_mm, &format!("{path}.depth_mm"))?,
        &format!("{path}.depth_mm"),
    )?;
    Ok(RoughCut::Face { normal, depth_mm })
}

/// One cut of the rough.
fn cut_from_dto(dto: &CutDto, path: &str) -> Result<RoughCut, String> {
    match dto.kind.trim().to_lowercase().as_str() {
        "edge" => Ok(RoughCut::Edge {
            faces: cut_faces::<2>(dto, path)?,
            setbacks_mm: cut_setbacks::<2>(dto, path)?,
        }),
        "corner" => Ok(RoughCut::Corner {
            faces: cut_faces::<3>(dto, path)?,
            setbacks_mm: cut_setbacks::<3>(dto, path)?,
        }),
        "face" => face_cut_from_dto(dto, path),
        other => Err(format!(
            "{path}.kind names an unknown cut '{other}' (expected edge, corner or face)"
        )),
    }
}

/// The modelled rough of `dto`: base and cuts, checked field by field and then as a
/// whole (the core must be able to build the solid).
///
/// # Errors
///
/// Returns a message naming the offending field, or the core's message when the cuts do
/// not fit the base.
pub fn rough_from_dto(dto: &RoughDto) -> Result<RoughModel, String> {
    let base = base_from_dto(dto)?;
    let cuts = dto
        .cuts
        .iter()
        .enumerate()
        .map(|(i, cut)| cut_from_dto(cut, &format!("rough.cuts[{i}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let model = RoughModel::new(base, cuts);
    model.halfspaces().map_err(|e| format!("rough: {e}"))?;
    Ok(model)
}

/// The planning settings of `dto` for a material of specific gravity `specific_gravity`.
///
/// # Errors
///
/// Returns a message naming the field when the count is outside `1..=99` (it is reported,
/// never clamped), a length is negative (zero for the minimum width), or a length is
/// beyond what the planner form accepts (`MAX_LOSS_MM` for kerf, allowance and skin,
/// `MAX_MIN_WIDTH_MM` for the minimum width).
pub fn settings_from_dto(dto: &SettingsDto, specific_gravity: f64) -> Result<PlanSettings, String> {
    let count = u8::try_from(dto.count)
        .ok()
        .filter(|count| (1..=99).contains(count))
        .ok_or_else(|| format!("settings.count must be 1 to 99, found {}", dto.count))?;
    Ok(PlanSettings {
        count,
        kerf_mm: non_negative_up_to(dto.kerf_mm, MAX_LOSS_MM, "settings.kerf_mm")?,
        allowance_mm: non_negative_up_to(dto.allowance_mm, MAX_LOSS_MM, "settings.allowance_mm")?,
        skin_mm: non_negative_up_to(dto.skin_mm, MAX_LOSS_MM, "settings.skin_mm")?,
        min_width_mm: positive_up_to(dto.min_width_mm, MAX_MIN_WIDTH_MM, "settings.min_width_mm")?,
        specific_gravity,
    })
}

/// Where a plan's candidate designs came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CandidateSource {
    /// The designs the library filter selected. Also what a file without the key means.
    #[default]
    Filter,
    /// Every design in the library.
    Library,
}

impl CandidateSource {
    /// The source of a run that used the library filter (`true`) or the whole library.
    #[must_use]
    pub const fn from_use_filter(use_filter: bool) -> Self {
        if use_filter {
            Self::Filter
        } else {
            Self::Library
        }
    }

    /// Whether the candidates were the library filter's designs.
    #[must_use]
    pub const fn uses_filter(self) -> bool {
        matches!(self, Self::Filter)
    }

    /// The value written to the file.
    const fn key(self) -> &'static str {
        match self {
            Self::Filter => "filter",
            Self::Library => "library",
        }
    }
}

/// The candidate source of `dto`: "filter" when the file has none.
///
/// # Errors
///
/// Returns a message naming the field when the key holds anything but "filter" or
/// "library".
pub fn candidate_source_from_dto(dto: &SettingsDto) -> Result<CandidateSource, String> {
    match dto.candidate_source.as_deref() {
        None => Ok(CandidateSource::default()),
        Some("filter") => Ok(CandidateSource::Filter),
        Some("library") => Ok(CandidateSource::Library),
        Some(other) => Err(format!(
            "settings.candidate_source must be \"filter\" or \"library\", found \"{other}\""
        )),
    }
}

/// The file section of `settings`, planned from `source`.
#[must_use]
pub fn settings_to_dto(settings: &PlanSettings, source: CandidateSource) -> SettingsDto {
    SettingsDto {
        count: i64::from(settings.count),
        kerf_mm: settings.kerf_mm,
        allowance_mm: settings.allowance_mm,
        skin_mm: settings.skin_mm,
        min_width_mm: settings.min_width_mm,
        candidate_source: Some(source.key().to_string()),
    }
}

/// Checks that a design list is no longer than a plan can use, has finite fingerprints,
/// a positive width where one is given, and no id twice.
///
/// # Errors
///
/// Returns a message naming the offending entry.
pub fn check_designs(designs: &[SavedDesignDto]) -> Result<(), String> {
    if designs.len() > MAX_DESIGNS {
        return Err(format!(
            "designs lists {} designs; a plan uses at most {MAX_DESIGNS}",
            designs.len()
        ));
    }
    let mut seen = BTreeSet::new();
    for (i, design) in designs.iter().enumerate() {
        finite3(design.fingerprint, &format!("designs[{i}].fingerprint"))?;
        if let Some(width) = design.width_caliper {
            positive(width, &format!("designs[{i}].width_caliper"))?;
        }
        if !seen.insert(design.entry_id) {
            return Err(format!(
                "designs[{i}].entry_id {} appears twice",
                design.entry_id
            ));
        }
    }
    Ok(())
}

/// The dot product of two vectors.
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    DVec3::from(a).dot(DVec3::from(b))
}

/// Checks a pose's three axes: unit length, pairwise perpendicular, right-handed.
fn check_pose_axes(axes: &[[f64; 3]; 3], path: &str) -> Result<(), String> {
    for (i, &axis) in axes.iter().enumerate() {
        finite3(axis, &format!("{path}[{i}]"))?;
        let length = dot(axis, axis).sqrt();
        if (length - 1.0).abs() > AXIS_TOLERANCE {
            return Err(format!(
                "{path}[{i}] must be a unit vector (within {AXIS_TOLERANCE}), its length is {length}"
            ));
        }
    }
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        let product = dot(axes[i], axes[j]);
        if product.abs() > AXIS_TOLERANCE {
            return Err(format!(
                "{path}[{i}] and {path}[{j}] must be perpendicular, their dot product is {product}"
            ));
        }
    }
    let x = DVec3::from(axes[0]);
    let determinant = x.dot(DVec3::from(axes[1]).cross(DVec3::from(axes[2])));
    if determinant <= 0.0 {
        return Err(format!(
            "{path} must be a right-handed frame (a left-handed one would mirror the stone), its determinant is {determinant}"
        ));
    }
    Ok(())
}

/// One placed stone.
fn stone_from_dto(dto: &StoneDto, path: &str) -> Result<PlacedStone, String> {
    let piece_origin_mm = finite3(dto.piece_origin_mm, &format!("{path}.piece_origin_mm"))?;
    let piece_size_mm = positive3(dto.piece_size_mm, &format!("{path}.piece_size_mm"))?;
    let stone_size_mm = positive3(dto.stone_size_mm, &format!("{path}.stone_size_mm"))?;
    let center_mm = finite3(dto.center_mm, &format!("{path}.center_mm"))?;
    check_pose_axes(&dto.axes, &format!("{path}.axes"))?;
    Ok(PlacedStone {
        entry_id: dto.entry_id,
        piece_origin_mm,
        piece_size_mm,
        stone_size_mm,
        table_axis: parse_axis(&dto.table_axis, &format!("{path}.table_axis"))?,
        carat: non_negative(dto.carat, &format!("{path}.carat"))?,
        volume_mm3: non_negative(dto.volume_mm3, &format!("{path}.volume_mm3"))?,
        pose: StonePose {
            center_mm,
            axes: dto.axes,
            mm_per_unit: positive(dto.mm_per_unit, &format!("{path}.mm_per_unit"))?,
        },
    })
}

/// Fails when a list of `what` at `path` holds more than `max` items.
fn check_count(len: usize, max: usize, path: &str, what: &str) -> Result<(), String> {
    if len > max {
        Err(format!(
            "{path} lists {len} {what}; a layout has at most {max}"
        ))
    } else {
        Ok(())
    }
}

/// The saw plan of a layout.
fn slabs_from_dto(slabs: &[SlabDto], path: &str) -> Result<CutPlan, String> {
    check_count(
        slabs.len(),
        MAX_SAW_ITEMS,
        &format!("{path}.slabs"),
        "slabs",
    )?;
    let mut out = Vec::with_capacity(slabs.len());
    for (i, slab) in slabs.iter().enumerate() {
        let slab_path = format!("{path}.slabs[{i}]");
        let thickness_mm = positive(slab.thickness_mm, &format!("{slab_path}.thickness_mm"))?;
        check_count(
            slab.bars.len(),
            MAX_SAW_ITEMS,
            &format!("{slab_path}.bars"),
            "bars",
        )?;
        let mut bars = Vec::with_capacity(slab.bars.len());
        for (j, bar) in slab.bars.iter().enumerate() {
            let bar_path = format!("{slab_path}.bars[{j}]");
            let width_mm = positive(bar.width_mm, &format!("{bar_path}.width_mm"))?;
            check_count(
                bar.pieces_mm.len(),
                MAX_SAW_ITEMS,
                &format!("{bar_path}.pieces_mm"),
                "pieces",
            )?;
            let mut pieces_mm = Vec::with_capacity(bar.pieces_mm.len());
            for (k, &piece) in bar.pieces_mm.iter().enumerate() {
                pieces_mm.push(positive(piece, &format!("{bar_path}.pieces_mm[{k}]"))?);
            }
            bars.push(BarCut {
                width_mm,
                pieces_mm,
            });
        }
        out.push(SlabCut { thickness_mm, bars });
    }
    Ok(CutPlan { slabs: out })
}

/// The number of pieces a saw plan cuts.
fn piece_count(plan: &CutPlan) -> usize {
    plan.slabs
        .iter()
        .flat_map(|slab| &slab.bars)
        .map(|bar| bar.pieces_mm.len())
        .sum()
}

/// Checks that the stones and the saw plan's pieces belong together: one to one for a
/// sawn layout; for an exact fit exactly one stone, and at most the one piece that is its
/// own bounding box (an exact fit is not sawn, so a writer may leave the plan empty).
fn check_stones_and_pieces(dto: &SavedLayoutDto, pieces: usize, path: &str) -> Result<(), String> {
    let stones = dto.stones.len();
    if dto.exact_fit {
        if stones != 1 {
            return Err(format!(
                "{path}.exact_fit is set but {path}.stones lists {stones} stones; an exact fit holds exactly one"
            ));
        }
        if pieces > 1 {
            return Err(format!(
                "{path}.exact_fit is set but the saw plan cuts {pieces} pieces; an exact fit has no saw stages"
            ));
        }
    } else if pieces != stones {
        return Err(format!(
            "{path}.stones lists {stones} stones but the saw plan cuts {pieces} pieces"
        ));
    }
    Ok(())
}

/// One saved layout: saw plan, stones with poses and totals. `path` is the layout's own
/// path (`layouts[2]`).
///
/// # Errors
///
/// Returns a message naming the offending field. The stones must match the pieces of the
/// saw plan one to one, as the planner produces them.
pub fn layout_from_dto(dto: &SavedLayoutDto, path: &str) -> Result<RoughLayout, String> {
    if dto.rank < 1 {
        return Err(format!(
            "{path}.rank must be at least 1, found {}",
            dto.rank
        ));
    }
    check_count(
        dto.stones.len(),
        MAX_STONES_PER_LAYOUT,
        &format!("{path}.stones"),
        "stones",
    )?;
    let cut_order = parse_cut_order(&dto.cut_order, &format!("{path}.cut_order"))?;
    let total_carat = non_negative(dto.total_carat, &format!("{path}.total_carat"))?;
    let total_volume_mm3 = non_negative(dto.total_volume_mm3, &format!("{path}.total_volume_mm3"))?;
    let yield_fraction = non_negative(dto.yield_fraction, &format!("{path}.yield_fraction"))?;
    let cut_plan = slabs_from_dto(&dto.slabs, path)?;
    let stones = dto
        .stones
        .iter()
        .enumerate()
        .map(|(i, stone)| stone_from_dto(stone, &format!("{path}.stones[{i}]")))
        .collect::<Result<Vec<_>, _>>()?;
    check_stones_and_pieces(dto, piece_count(&cut_plan), path)?;
    Ok(RoughLayout {
        cut_order,
        stones,
        cut_plan,
        total_carat,
        total_volume_mm3,
        yield_fraction,
        exact_fit: dto.exact_fit,
    })
}

/// The file form of a cut.
impl From<&RoughCut> for CutDto {
    fn from(cut: &RoughCut) -> Self {
        let empty = Self {
            kind: String::new(),
            faces: None,
            setbacks_mm: None,
            normal: None,
            depth_mm: None,
        };
        match cut {
            RoughCut::Edge { faces, setbacks_mm } => Self {
                kind: "edge".to_string(),
                faces: Some(
                    faces
                        .iter()
                        .map(|f| box_face_name(*f).to_string())
                        .collect(),
                ),
                setbacks_mm: Some(setbacks_mm.to_vec()),
                ..empty
            },
            RoughCut::Corner { faces, setbacks_mm } => Self {
                kind: "corner".to_string(),
                faces: Some(
                    faces
                        .iter()
                        .map(|f| box_face_name(*f).to_string())
                        .collect(),
                ),
                setbacks_mm: Some(setbacks_mm.to_vec()),
                ..empty
            },
            RoughCut::Face { normal, depth_mm } => Self {
                kind: "face".to_string(),
                normal: Some(*normal),
                depth_mm: Some(*depth_mm),
                ..empty
            },
        }
    }
}

/// The file form of a stone.
impl From<&PlacedStone> for StoneDto {
    fn from(stone: &PlacedStone) -> Self {
        Self {
            entry_id: stone.entry_id,
            piece_origin_mm: stone.piece_origin_mm,
            piece_size_mm: stone.piece_size_mm,
            stone_size_mm: stone.stone_size_mm,
            table_axis: axis_name(stone.table_axis).to_string(),
            carat: stone.carat,
            volume_mm3: stone.volume_mm3,
            center_mm: stone.pose.center_mm,
            axes: stone.pose.axes,
            mm_per_unit: stone.pose.mm_per_unit,
        }
    }
}

/// The file form of a layout that had `rank` (1-based) in the plan it came from.
#[must_use]
pub fn layout_to_dto(layout: &RoughLayout, rank: usize) -> SavedLayoutDto {
    let slabs = layout
        .cut_plan
        .slabs
        .iter()
        .map(|slab| SlabDto {
            thickness_mm: slab.thickness_mm,
            bars: slab
                .bars
                .iter()
                .map(|bar| BarDto {
                    width_mm: bar.width_mm,
                    pieces_mm: bar.pieces_mm.clone(),
                })
                .collect(),
        })
        .collect();
    SavedLayoutDto {
        rank: i64::try_from(rank).unwrap_or(i64::MAX),
        cut_order: cut_order_name(layout.cut_order).to_string(),
        total_carat: layout.total_carat,
        total_volume_mm3: layout.total_volume_mm3,
        yield_fraction: layout.yield_fraction,
        slabs,
        stones: layout.stones.iter().map(StoneDto::from).collect(),
        exact_fit: layout.exact_fit,
    }
}

/// The file form of the rough: base, material and cuts.
#[must_use]
pub fn rough_to_dto(
    model: &RoughModel,
    material_name: &str,
    specific_gravity: f64,
    weighed_ct: Option<f64>,
) -> RoughDto {
    let mut dto = RoughDto {
        base: String::new(),
        x_mm: None,
        y_mm: None,
        z_mm: None,
        diameter_mm: None,
        length_mm: None,
        axis: None,
        hull: Vec::new(),
        mesh: None,
        material: material_name.to_string(),
        specific_gravity,
        weighed_ct,
        cuts: model.cuts.iter().map(CutDto::from).collect(),
    };
    if write_hull(&mut dto, &model.base) {
        return dto;
    }
    match model.base {
        RoughBase::Block { x_mm, y_mm, z_mm } => {
            dto.base = "block".to_string();
            (dto.x_mm, dto.y_mm, dto.z_mm) = (Some(x_mm), Some(y_mm), Some(z_mm));
        }
        RoughBase::Pebble { x_mm, y_mm, z_mm } => {
            dto.base = "pebble".to_string();
            (dto.x_mm, dto.y_mm, dto.z_mm) = (Some(x_mm), Some(y_mm), Some(z_mm));
        }
        RoughBase::Cylinder {
            diameter_mm,
            length_mm,
            axis,
        } => {
            dto.base = "cylinder".to_string();
            dto.diameter_mm = Some(diameter_mm);
            dto.length_mm = Some(length_mm);
            dto.axis = Some(axis_name(axis).to_string());
        }
        RoughBase::Hull { .. } => {}
    }
    dto
}
