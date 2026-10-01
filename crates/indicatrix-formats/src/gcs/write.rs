//! [`to_gcs_string`]: `.asc` cutting instructions as a Gem Cut Studio `.gcs` file.

use super::{
    polytope::{HalfSpace, PolytopeError, polytope_faces},
    tier::normal_from_index_angle,
};
use crate::asc::{AscSchedule, AscTier};
use std::fmt;

/// Everything that can stop [`to_gcs_string`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GcsWriteError {
    /// The schedule's gear has zero teeth, so no index maps to an angle.
    ZeroGear,
    /// The schedule has no tiers.
    NoTiers,
    /// A tier's angle, mast or an index is not finite.
    NonFinite {
        /// 0-based position of the tier in [`AscSchedule::tiers`].
        tier_index: usize,
    },
    /// The facet planes do not enclose a bounded stone (for example, no girdle).
    Unbounded,
    /// The facet planes have an empty or flat intersection.
    EmptySolid,
}

impl fmt::Display for GcsWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroGear => write!(f, "the index gear has zero teeth"),
            Self::NoTiers => write!(f, "the cutting instructions have no tiers"),
            Self::NonFinite { tier_index } => {
                write!(
                    f,
                    "tier #{tier_index} has a non-finite angle, mast or index"
                )
            }
            Self::Unbounded => write!(f, "the facet planes do not enclose a bounded stone"),
            Self::EmptySolid => write!(f, "the facet planes enclose no volume"),
        }
    }
}

impl std::error::Error for GcsWriteError {}

/// Refractive index written when the schedule has no usable one (the manual's
/// own example value, p.60).
const FALLBACK_REFRACTIVE_INDEX: f64 = 1.54;

/// One facet plane to write.
struct FacetRow {
    tier_index: usize,
    normal: [f64; 3],
    index_angle: f64,
    mast: f64,
}

/// The `.gcs` polar angle of an `.asc` tier: the crown angle as is, a pavilion
/// angle (including `-0` culet and `-90` girdle) as `180 + angle`.
fn gcs_angle(tier: &AscTier) -> f64 {
    if tier.angle_deg.is_sign_negative() {
        180.0 + tier.angle_deg
    } else {
        tier.angle_deg
    }
}

/// Every facet plane of `schedule`, tier by tier. A tier with no index gets one
/// facet at tooth 0, as `.asc` does.
///
/// The `.asc` tooth `i` becomes the Gem Cut Studio tooth
/// `t = sign(gear)·(i - offset)` (mod `|gear|`), the reading `GemCAD`'s own
/// `.gem` geometry uses (`phi = 90° - 360°·(i - offset)/gear`): a negative gear
/// reverses the direction, the offset shifts it. `index_angle = 360°·t/|gear|`.
fn facet_rows(schedule: &AscSchedule, teeth: f64) -> Result<Vec<FacetRow>, GcsWriteError> {
    let direction = if schedule.gear_teeth < 0 { -1.0 } else { 1.0 };
    let offset = schedule.gear_reference_angle;
    let mut rows = Vec::new();
    for (tier_index, tier) in schedule.tiers.iter().enumerate() {
        let angle = gcs_angle(tier);
        let mast = tier.mast.abs();
        let default_index = [teeth];
        let indices = if tier.indices.is_empty() {
            &default_index[..]
        } else {
            &tier.indices[..]
        };
        if !angle.is_finite()
            || !mast.is_finite()
            || !indices.iter().all(|i| i.is_finite())
            || !offset.is_finite()
        {
            return Err(GcsWriteError::NonFinite { tier_index });
        }
        for &index in indices {
            let tooth = (direction * (index - offset)).rem_euclid(teeth);
            let index_angle = (360.0 * tooth / teeth).rem_euclid(360.0);
            rows.push(FacetRow {
                tier_index,
                normal: normal_from_index_angle(angle, index_angle),
                index_angle,
                mast,
            });
        }
    }
    Ok(rows)
}

/// Gem Cut Studio's normalisation: scale `s` so `max(|x|,|y|) = 1` and the z
/// shift `z0` that centres the z-range on 0.
struct Frame {
    scale: f64,
    z_centre: f64,
}

impl Frame {
    fn fit(faces: &[Vec<[f64; 3]>]) -> Result<Self, GcsWriteError> {
        let points = faces.iter().flatten();
        let reach = points
            .clone()
            .map(|p| p[0].abs().max(p[1].abs()))
            .fold(0.0_f64, f64::max);
        let z_min = points.clone().map(|p| p[2]).fold(f64::INFINITY, f64::min);
        let z_max = points.map(|p| p[2]).fold(f64::NEG_INFINITY, f64::max);
        if reach <= 0.0 || !reach.is_finite() || z_max <= z_min {
            return Err(GcsWriteError::EmptySolid);
        }
        Ok(Self {
            scale: reach.recip(),
            z_centre: f64::midpoint(z_min, z_max),
        })
    }

    /// A plane `n·x = mast` in the normalised frame: `s·(mast - z0·nz)`.
    fn depth(&self, mast: f64, normal: [f64; 3]) -> f64 {
        self.scale * normal[2].mul_add(-self.z_centre, mast)
    }

    fn point(&self, p: [f64; 3]) -> [f64; 3] {
        [
            self.scale * p[0],
            self.scale * p[1],
            self.scale * (p[2] - self.z_centre),
        ]
    }
}

/// Escapes an attribute value: the five XML specials as named entities and every
/// non-printable or non-ASCII character as a numeric reference, so the output is
/// plain ASCII.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            ' '..='~' => out.push(c),
            _ => {
                out.push_str("&#");
                out.push_str(&u32::from(c).to_string());
                out.push(';');
            }
        }
    }
    out
}

/// Appends one CRLF-terminated line at `depth` levels of 4-space indentation.
fn line(out: &mut String, depth: usize, text: &str) {
    out.push_str(&"    ".repeat(depth));
    out.push_str(text);
    out.push_str("\r\n");
}

/// The `<info>` element from the schedule's headers and footnotes: H1 → `title`,
/// H2 → `header2`, the rest joined by `"; "` → `header3`; F1..F3 → `footer1..3`,
/// the rest joined → `footer4`.
fn info_line(schedule: &AscSchedule) -> String {
    let joined = |items: &[String]| Some(items.join("; ")).filter(|s| !s.is_empty());
    let headers = &schedule.headers;
    let footnotes = &schedule.footnotes;
    let fields = [
        ("title", Some(headers.first().cloned().unwrap_or_default())),
        ("header2", headers.get(1).cloned()),
        ("header3", joined(headers.get(2..).unwrap_or_default())),
        ("footer1", footnotes.first().cloned()),
        ("footer2", footnotes.get(1).cloned()),
        ("footer3", footnotes.get(2).cloned()),
        ("footer4", joined(footnotes.get(3..).unwrap_or_default())),
    ];
    let mut text = String::from("<info");
    for (key, value) in fields {
        if let Some(value) = value {
            text.push(' ');
            text.push_str(key);
            text.push_str("=\"");
            text.push_str(&escape(&value));
            text.push('"');
        }
    }
    text.push_str("/>");
    text
}

/// Appends the `<tier>` blocks.
fn write_tiers(
    out: &mut String,
    schedule: &AscSchedule,
    rows: &[FacetRow],
    faces: &[Vec<[f64; 3]>],
    frame: &Frame,
) {
    for (tier_index, tier) in schedule.tiers.iter().enumerate() {
        let tier_rows: Vec<usize> = (0..rows.len())
            .filter(|&k| rows[k].tier_index == tier_index)
            .collect();
        let depth = tier_rows
            .first()
            .map_or(0.0, |&k| frame.depth(rows[k].mast, rows[k].normal));
        line(
            out,
            1,
            &format!(
                "<tier angle=\"{}\" depth=\"{depth}\" name=\"{}\" instructions=\"{}\" visible=\"true\" guide=\"false\">",
                gcs_angle(tier),
                escape(&tier.name),
                escape(&tier.notes),
            ),
        );
        for &k in &tier_rows {
            let [nx, ny, nz] = rows[k].normal;
            line(
                out,
                2,
                &format!(
                    "<facet nx=\"{nx}\" ny=\"{ny}\" nz=\"{nz}\" index_angle=\"{}\">",
                    rows[k].index_angle
                ),
            );
            for &p in &faces[k] {
                let [x, y, z] = frame.point(p);
                line(out, 3, &format!("<vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>"));
            }
            line(out, 2, "</facet>");
        }
        line(out, 1, "</tier>");
    }
}

#[doc = "Experimental: writes `schedule` as a Gem Cut Studio `.gcs` file."]
///
/// Not yet loaded into Gem Cut Studio itself. The index winding rule was measured
/// on a corpus with no chiral design, so a chiral design (crown and pavilion
/// twisted against each other) may come out mirrored on one side: check such an
/// export visually in Gem Cut Studio before relying on it.
///
/// Format per the Gem Cut Studio User's Manual v1.1.0 pp. 58-61; conventions per
/// the files Gem Cut Studio 1.1 writes.
///
/// The output has `version="1000"`, CRLF line endings, no XML declaration and only
/// ASCII (anything else as `&#N;`). Geometry is solved here: every facet polygon is
/// a face of the convex polytope `∩ {n_k·x <= |mast_k|}`, and the stone is then
/// normalised as GCS does (scaled so `max(|x|,|y|) = 1`, z-range centred on 0), with
/// each tier's `depth` in that frame. `index_angle` follows the side rule (crown
/// `90° + phi`, pavilion and girdle `270° - phi`), and a negative gear or a gear
/// offset is folded into the tooth as `GemCAD`'s `.gem` geometry does.
///
/// `<index>` gets `base="0"`, `symmetry` = the schedule's symmetry order (at least
/// 1) and `mirror="0"` (UI state only); `<render>` the schedule's refractive index
/// (1.54 when it has none) with the manual's example defaults; `<info>` the headers
/// and footnotes.
///
/// # Errors
///
/// [`GcsWriteError`] for a zero gear, no tiers, a non-finite value, or planes that
/// do not enclose a bounded, non-empty stone.
pub fn to_gcs_string(schedule: &AscSchedule) -> Result<String, GcsWriteError> {
    let gear = schedule.gear_teeth_abs();
    if gear == 0 {
        return Err(GcsWriteError::ZeroGear);
    }
    if schedule.tiers.is_empty() {
        return Err(GcsWriteError::NoTiers);
    }
    let rows = facet_rows(schedule, f64::from(gear))?;
    let half_spaces: Vec<HalfSpace> = rows
        .iter()
        .map(|r| HalfSpace {
            normal: r.normal,
            offset: r.mast,
        })
        .collect();
    let faces = polytope_faces(&half_spaces).map_err(|e| match e {
        PolytopeError::Unbounded => GcsWriteError::Unbounded,
        PolytopeError::Empty => GcsWriteError::EmptySolid,
    })?;
    let frame = Frame::fit(&faces)?;
    let refractive_index = Some(schedule.refractive_index)
        .filter(|ri| ri.is_finite() && *ri > 0.0)
        .unwrap_or(FALLBACK_REFRACTIVE_INDEX);

    let mut out = String::new();
    line(&mut out, 0, "<GemCutStudio version=\"1000\">");
    line(
        &mut out,
        1,
        &format!(
            "<index gear=\"{gear}\" base=\"0\" symmetry=\"{}\" mirror=\"0\"/>",
            schedule.symmetry_order.max(1)
        ),
    );
    write_tiers(&mut out, schedule, &rows, &faces, &frame);
    line(
        &mut out,
        1,
        &format!(
            "<render material=\"(from file)\" refractive_index=\"{refractive_index}\" dispersion=\"0\" clarity=\"100\" density=\"1\" lighting_model=\"Random\">"
        ),
    );
    line(&mut out, 2, "<color r=\"1\" g=\"1\" b=\"1\"/>");
    line(&mut out, 1, "</render>");
    line(&mut out, 1, &info_line(schedule));
    line(&mut out, 0, "</GemCutStudio>");
    Ok(out)
}
