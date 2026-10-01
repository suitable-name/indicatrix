//! [`parse_gcs`]: walks the flat [`super::tokenize::RawTag`] stream into a
//! [`GcsDesign`], plus the attribute helpers each element kind needs.
//!
//! Required versus optional follows the published format (Gem Cut Studio User's
//! Manual v1.1.0 pp. 58-61): only the root, `index gear`, `tier angle` and, per
//! facet, enough to recover the plane are required.

use super::{
    design::GcsDesign,
    error::GcsParseError,
    metadata::{GcsColor, GcsIndex, GcsInfo, GcsRender},
    tier::{GcsFacet, GcsTier, GcsVertex, normal_from_index_angle, side_rule_index_angle},
    tokenize::{RawTag, tokenize},
};
use crate::encoding::decode_windows_1252_or_utf8;

/// The newest file version this reader knows (`version="1000"` in every file Gem
/// Cut Studio 1.1 writes). A newer one parses with a warning.
const KNOWN_VERSION: u32 = 1000;

const ROOT_ATTRS: &[&str] = &["version"];
const INDEX_ATTRS: &[&str] = &["gear", "base", "symmetry", "mirror"];
const TIER_ATTRS: &[&str] = &[
    "angle",
    "depth",
    "name",
    "instructions",
    "visible",
    "guide",
    "frosting",
];
const FACET_ATTRS: &[&str] = &["nx", "ny", "nz", "index_angle", "frosting"];
const VERTEX_ATTRS: &[&str] = &["x", "y", "z"];
const RENDER_ATTRS: &[&str] = &[
    "material",
    "refractive_index",
    "dispersion",
    "clarity",
    "density",
    "lighting_model",
];
const COLOR_ATTRS: &[&str] = &["r", "g", "b"];
const INFO_ATTRS: &[&str] = &[
    "title", "author", "date", "shape", "header2", "header3", "ri_min", "ri_max", "size_min",
    "size_max", "footer1", "footer2", "footer3", "footer4",
];

/// Records a warning for every attribute of `tag` not in `known`.
fn warn_unknown_attrs(tag: &RawTag, known: &[&str], warnings: &mut Vec<String>) {
    for key in tag.keys().filter(|k| !known.contains(k)) {
        warnings.push(format!(
            "line {}: unknown attribute `{key}` on <{}> ignored",
            tag.line, tag.name
        ));
    }
}

/// Records a warning for an unknown opening element (closing tags are silent).
fn warn_unknown_element(tag: &RawTag, warnings: &mut Vec<String>) {
    if !tag.closing {
        warnings.push(format!(
            "line {}: unknown element <{}> ignored",
            tag.line, tag.name
        ));
    }
}

/// Parses `raw` as a finite number, tolerating surrounding whitespace. `nan`, `inf`
/// and `infinity` parse as `f64` but poison every angle, depth, normal and vertex
/// they reach, so they read as garbled (`None`) like any other non-number.
fn number(raw: &str) -> Option<f64> {
    raw.trim()
        .parse()
        .ok()
        .filter(|value: &f64| value.is_finite())
}

fn index_f64(tag: &RawTag, key: &'static str) -> Result<Option<f64>, GcsParseError> {
    tag.attr(key)
        .map(|raw| {
            number(raw).ok_or_else(|| GcsParseError::IndexAttributeNotNumeric {
                attr: key,
                value: raw.to_string(),
            })
        })
        .transpose()
}

fn index_u32(tag: &RawTag, key: &'static str) -> Result<Option<u32>, GcsParseError> {
    // Parsed via f64 so a stray "4.0" works; bounded at `u32::MAX` because `as u32`
    // on a finite `f64` saturates rather than failing.
    let Some(value) = index_f64(tag, key)? else {
        return Ok(None);
    };
    if value.is_finite() && (0.0..=f64::from(u32::MAX)).contains(&value) {
        Ok(Some(value.round() as u32))
    } else {
        Err(GcsParseError::IndexAttributeNotNumeric {
            attr: key,
            value: tag.attr(key).unwrap_or_default().to_string(),
        })
    }
}

fn parse_index(tag: &RawTag, warnings: &mut Vec<String>) -> Result<GcsIndex, GcsParseError> {
    warn_unknown_attrs(tag, INDEX_ATTRS, warnings);
    Ok(GcsIndex {
        gear: index_u32(tag, "gear")?
            .ok_or(GcsParseError::IndexAttributeMissing { attr: "gear" })?,
        base: index_f64(tag, "base")?.unwrap_or(0.0),
        symmetry: index_u32(tag, "symmetry")?.unwrap_or(1),
        mirror: index_u32(tag, "mirror")?.unwrap_or(0),
    })
}

fn tier_f64(
    tag: &RawTag,
    tier_index: usize,
    key: &'static str,
) -> Result<Option<f64>, GcsParseError> {
    tag.attr(key)
        .map(|raw| {
            number(raw).ok_or_else(|| GcsParseError::TierAttributeNotNumeric {
                tier_index,
                attr: key,
                value: raw.to_string(),
            })
        })
        .transpose()
}

fn facet_f64(
    tag: &RawTag,
    tier_index: usize,
    key: &'static str,
) -> Result<Option<f64>, GcsParseError> {
    tag.attr(key)
        .map(|raw| {
            number(raw).ok_or_else(|| GcsParseError::FacetAttributeNotNumeric {
                tier_index,
                attr: key,
                value: raw.to_string(),
            })
        })
        .transpose()
}

/// A `<vertex>` coordinate: optional as a vertex, but a vertex that is present
/// must carry all three.
fn vertex_f64(tag: &RawTag, tier_index: usize, key: &'static str) -> Result<f64, GcsParseError> {
    let raw = tag.attr(key).ok_or(GcsParseError::VertexAttributeMissing {
        tier_index,
        attr: key,
    })?;
    number(raw).ok_or_else(|| GcsParseError::VertexAttributeNotNumeric {
        tier_index,
        attr: key,
        value: raw.to_string(),
    })
}

/// A `<render>` number: `0.0` when absent, an error when present but garbled.
fn render_f64(tag: &RawTag, key: &'static str) -> Result<f64, GcsParseError> {
    tag.attr(key).map_or(Ok(0.0), |raw| {
        number(raw).ok_or_else(|| GcsParseError::RenderAttributeNotNumeric {
            attr: key,
            value: raw.to_string(),
        })
    })
}

/// A `<color>` channel: same rule as [`render_f64`].
fn color_f64(tag: &RawTag, key: &'static str) -> Result<f64, GcsParseError> {
    tag.attr(key).map_or(Ok(0.0), |raw| {
        number(raw).ok_or_else(|| GcsParseError::ColorAttributeNotNumeric {
            attr: key,
            value: raw.to_string(),
        })
    })
}

/// The facet normal: all three components, or none (then derived from the tier
/// angle and `index_angle`). A partial normal names its first missing component.
fn facet_normal(tag: &RawTag, tier_index: usize) -> Result<Option<[f64; 3]>, GcsParseError> {
    const KEYS: [&str; 3] = ["nx", "ny", "nz"];
    let parts = [
        facet_f64(tag, tier_index, KEYS[0])?,
        facet_f64(tag, tier_index, KEYS[1])?,
        facet_f64(tag, tier_index, KEYS[2])?,
    ];
    match parts {
        [Some(x), Some(y), Some(z)] => Ok(Some([x, y, z])),
        [None, None, None] => Ok(None),
        _ => Err(GcsParseError::FacetAttributeMissing {
            tier_index,
            attr: KEYS[parts.iter().position(Option::is_none).unwrap_or(0)],
        }),
    }
}

/// Consumes tags from `tags[*pos..]` until (and including) the `</facet>` that
/// closes the facet opened at `tags[*pos]`.
fn parse_facet(
    tags: &[RawTag],
    pos: &mut usize,
    tier_index: usize,
    tier_angle: f64,
    warnings: &mut Vec<String>,
) -> Result<GcsFacet, GcsParseError> {
    let open = &tags[*pos];
    warn_unknown_attrs(open, FACET_ATTRS, warnings);
    let (normal, index_angle_deg) = match (
        facet_normal(open, tier_index)?,
        facet_f64(open, tier_index, "index_angle")?,
    ) {
        (Some(n), Some(ia)) => (n, ia),
        (Some(n), None) => (n, side_rule_index_angle(n).unwrap_or(0.0)),
        (None, Some(ia)) => (normal_from_index_angle(tier_angle, ia), ia),
        (None, None) => {
            return Err(GcsParseError::FacetAttributeMissing {
                tier_index,
                attr: "index_angle",
            });
        }
    };
    let frosting = facet_f64(open, tier_index, "frosting")?;
    *pos += 1;
    let mut vertices = Vec::new();
    if !open.self_closing {
        loop {
            let tag = tags
                .get(*pos)
                .ok_or(GcsParseError::UnterminatedElement { name: "facet" })?;
            *pos += 1;
            if tag.name == "facet" && tag.closing {
                break;
            }
            if tag.name == "vertex" && !tag.closing {
                warn_unknown_attrs(tag, VERTEX_ATTRS, warnings);
                vertices.push(GcsVertex {
                    x: vertex_f64(tag, tier_index, "x")?,
                    y: vertex_f64(tag, tier_index, "y")?,
                    z: vertex_f64(tag, tier_index, "z")?,
                });
            } else if tag.name != "vertex" {
                warn_unknown_element(tag, warnings);
            }
        }
    }
    Ok(GcsFacet {
        normal,
        index_angle_deg,
        frosting,
        vertices,
    })
}

/// The mean `n·v` over every vertex of `facets`, or `None` without vertices: the
/// manual's fallback for a tier with no `depth` (p.58).
fn mean_depth(facets: &[GcsFacet]) -> Option<f64> {
    let (sum, count) = facets
        .iter()
        .flat_map(|f| f.vertices.iter().map(move |v| (f.normal, v)))
        .fold((0.0, 0usize), |(sum, count), (n, v)| {
            (
                n[2].mul_add(v.z, n[0].mul_add(v.x, n[1] * v.y)) + sum,
                count + 1,
            )
        });
    (count > 0).then(|| sum / count as f64)
}

/// Consumes tags from `tags[*pos..]` until (and including) the `</tier>` that
/// closes the tier opened at `tags[*pos]`.
fn parse_tier(
    tags: &[RawTag],
    pos: &mut usize,
    tier_index: usize,
    warnings: &mut Vec<String>,
) -> Result<GcsTier, GcsParseError> {
    let open = &tags[*pos];
    warn_unknown_attrs(open, TIER_ATTRS, warnings);
    let angle_deg =
        tier_f64(open, tier_index, "angle")?.ok_or(GcsParseError::TierAttributeMissing {
            tier_index,
            attr: "angle",
        })?;
    let depth = tier_f64(open, tier_index, "depth")?;
    let frosting = tier_f64(open, tier_index, "frosting")?;
    *pos += 1;
    let mut facets = Vec::new();
    if !open.self_closing {
        loop {
            let tag = tags
                .get(*pos)
                .ok_or(GcsParseError::UnterminatedElement { name: "tier" })?;
            if tag.name == "tier" && tag.closing {
                *pos += 1;
                break;
            }
            if tag.name == "facet" && !tag.closing {
                facets.push(parse_facet(tags, pos, tier_index, angle_deg, warnings)?);
            } else {
                warn_unknown_element(tag, warnings);
                *pos += 1;
            }
        }
    }
    let depth = match depth {
        Some(depth) => depth,
        None => mean_depth(&facets).ok_or(GcsParseError::TierAttributeMissing {
            tier_index,
            attr: "depth",
        })?,
    };
    Ok(GcsTier {
        angle_deg,
        depth,
        name: open.attr("name").unwrap_or_default().to_string(),
        instructions: open.attr("instructions").unwrap_or_default().to_string(),
        visible: open.attr("visible") != Some("false"),
        guide: open.attr("guide") == Some("true"),
        frosting,
        facets,
    })
}

/// Consumes tags from `tags[*pos..]` until (and including) the `</render>` that
/// closes the render block opened at `tags[*pos]`. The last `<color>` wins.
fn parse_render(
    tags: &[RawTag],
    pos: &mut usize,
    warnings: &mut Vec<String>,
) -> Result<GcsRender, GcsParseError> {
    let open = &tags[*pos];
    warn_unknown_attrs(open, RENDER_ATTRS, warnings);
    let mut render = GcsRender {
        material: open.attr("material").unwrap_or_default().to_string(),
        refractive_index: render_f64(open, "refractive_index")?,
        dispersion: render_f64(open, "dispersion")?,
        clarity: render_f64(open, "clarity")?,
        density: render_f64(open, "density")?,
        lighting_model: open.attr("lighting_model").unwrap_or_default().to_string(),
        color: GcsColor::default(),
    };
    *pos += 1;
    if open.self_closing {
        return Ok(render);
    }
    loop {
        let tag = tags
            .get(*pos)
            .ok_or(GcsParseError::UnterminatedElement { name: "render" })?;
        *pos += 1;
        if tag.name == "render" && tag.closing {
            return Ok(render);
        }
        if tag.name == "color" && !tag.closing {
            warn_unknown_attrs(tag, COLOR_ATTRS, warnings);
            render.color = GcsColor {
                r: color_f64(tag, "r")?,
                g: color_f64(tag, "g")?,
                b: color_f64(tag, "b")?,
            };
        } else if tag.name != "color" {
            warn_unknown_element(tag, warnings);
        }
    }
}

fn opt_string(tag: &RawTag, key: &str) -> Option<String> {
    tag.attr(key).map(str::to_string)
}

fn parse_info(tag: &RawTag, warnings: &mut Vec<String>) -> GcsInfo {
    warn_unknown_attrs(tag, INFO_ATTRS, warnings);
    GcsInfo {
        title: opt_string(tag, "title"),
        author: opt_string(tag, "author"),
        date: opt_string(tag, "date"),
        shape: opt_string(tag, "shape"),
        header2: opt_string(tag, "header2"),
        header3: opt_string(tag, "header3"),
        ri_min: opt_string(tag, "ri_min"),
        ri_max: opt_string(tag, "ri_max"),
        size_min: opt_string(tag, "size_min"),
        size_max: opt_string(tag, "size_max"),
        footer1: opt_string(tag, "footer1"),
        footer2: opt_string(tag, "footer2"),
        footer3: opt_string(tag, "footer3"),
        footer4: opt_string(tag, "footer4"),
    }
}

/// Decodes raw `.gcs` bytes and parses them with [`parse_gcs`].
///
/// The format has no encoding declaration and Gem Cut Studio 1.1 writes
/// Windows-1252 (real file id 2213 holds byte `0xBA`, `º`). A UTF-8 byte-order
/// mark is stripped, valid UTF-8 is read as UTF-8, anything else as Windows-1252.
/// Read files through this, not `read_to_string`.
///
/// # Errors
///
/// Exactly [`parse_gcs`]'s errors; decoding cannot fail.
pub fn parse_gcs_bytes(bytes: &[u8]) -> Result<GcsDesign, GcsParseError> {
    parse_gcs(&decode_windows_1252_or_utf8(bytes))
}

/// Parses a Gem Cut Studio `.gcs` design from text.
///
/// Unknown elements and attributes are tolerated and listed in
/// [`GcsDesign::warnings`], as is a `version` above 1000. Missing optional values
/// are derived as the manual describes: a facet normal from the tier angle and
/// `index_angle` (side rule, see [`super::side_rule_index_angle`]), an
/// `index_angle` from the normal, a tier `depth` from its vertices.
///
/// # Errors
///
/// Returns `Err` if `content` is empty, a tag or quoted value is unterminated, the
/// `<GemCutStudio>` root or its `<index gear>` is missing, a tier has no `angle`,
/// a facet has neither a normal nor an `index_angle` (or only part of a normal), a
/// tier has neither `depth` nor vertices, a present vertex lacks a coordinate, or a
/// present numeric attribute does not parse as a finite number (`nan` and `inf`
/// are rejected).
pub fn parse_gcs(content: &str) -> Result<GcsDesign, GcsParseError> {
    if content.trim().is_empty() {
        return Err(GcsParseError::EmptyInput);
    }
    let tags = tokenize(content)?;
    let root = tags.first().ok_or(GcsParseError::MissingRootElement)?;
    if root.name != "GemCutStudio" || root.closing {
        return Err(GcsParseError::MissingRootElement);
    }
    let mut warnings = Vec::new();
    warn_unknown_attrs(root, ROOT_ATTRS, &mut warnings);
    let version = root.attr("version").unwrap_or_default().to_string();
    if version
        .trim()
        .parse::<u32>()
        .is_ok_and(|v| v > KNOWN_VERSION)
    {
        warnings.push(format!(
            "file version {version} is newer than {KNOWN_VERSION}, the newest this reader knows"
        ));
    }
    let (mut index, mut tiers, mut render, mut info) = (None, Vec::new(), None, None);
    let mut pos = 1usize;
    while let Some(tag) = tags.get(pos) {
        if tag.name == "GemCutStudio" && tag.closing {
            break;
        }
        match (tag.name.as_str(), tag.closing) {
            (_, true) => pos += 1,
            ("index", false) => {
                index = Some(parse_index(tag, &mut warnings)?);
                pos += 1;
            }
            ("tier", false) => {
                let tier_index = tiers.len();
                tiers.push(parse_tier(&tags, &mut pos, tier_index, &mut warnings)?);
            }
            ("render", false) => render = Some(parse_render(&tags, &mut pos, &mut warnings)?),
            ("info", false) => {
                info = Some(parse_info(tag, &mut warnings));
                pos += 1;
            }
            _ => {
                warn_unknown_element(tag, &mut warnings);
                pos += 1;
            }
        }
    }
    Ok(GcsDesign {
        version,
        index: index.ok_or(GcsParseError::MissingIndexElement)?,
        tiers,
        render,
        info,
        warnings,
    })
}
