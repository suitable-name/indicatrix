//! [`parse_gcs`]: walks the flat [`super::tokenize::RawTag`] stream into a
//! [`GcsDesign`], plus the attribute-parsing helpers each element kind needs.

use super::{
    design::GcsDesign,
    error::GcsParseError,
    metadata::{GcsColor, GcsIndex, GcsInfo, GcsRender},
    tier::{GcsFacet, GcsTier, GcsVertex},
    tokenize::{RawTag, tokenize},
};

fn required_index_attr<'a>(tag: &'a RawTag, key: &'static str) -> Result<&'a str, GcsParseError> {
    tag.attr(key)
        .ok_or(GcsParseError::IndexAttributeMissing { attr: key })
}

fn parse_index_u32(tag: &RawTag, key: &'static str) -> Result<u32, GcsParseError> {
    let raw = required_index_attr(tag, key)?;
    // Values are written as plain integers in every sample, but parse via f64
    // first so a stray "4.0"-style decimal (as `.asc` tolerates on its own
    // integer fields) would not need a second code path here.
    //
    // Upper-bounded at `u32::MAX`: `as u32` on a finite `f64` outside that range
    // SATURATES rather than erroring (stable Rust's documented float-to-int cast
    // behavior), so e.g. "1e20" must be rejected rather than silently becoming
    // `u32::MAX`.
    raw.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && (0.0..=f64::from(u32::MAX)).contains(v))
        .map(|v| v.round() as u32)
        .ok_or_else(|| GcsParseError::IndexAttributeNotNumeric {
            attr: key,
            value: raw.to_string(),
        })
}

fn parse_index_f64(tag: &RawTag, key: &'static str) -> Result<f64, GcsParseError> {
    let raw = required_index_attr(tag, key)?;
    raw.parse()
        .map_err(|_| GcsParseError::IndexAttributeNotNumeric {
            attr: key,
            value: raw.to_string(),
        })
}

fn parse_index(tag: &RawTag) -> Result<GcsIndex, GcsParseError> {
    Ok(GcsIndex {
        gear: parse_index_u32(tag, "gear")?,
        base: parse_index_f64(tag, "base")?,
        symmetry: parse_index_u32(tag, "symmetry")?,
        mirror: parse_index_u32(tag, "mirror")?,
    })
}

fn tier_f64(tag: &RawTag, tier_index: usize, key: &'static str) -> Result<f64, GcsParseError> {
    let raw = tag.attr(key).ok_or(GcsParseError::TierAttributeMissing {
        tier_index,
        attr: key,
    })?;
    raw.parse()
        .map_err(|_| GcsParseError::TierAttributeNotNumeric {
            tier_index,
            attr: key,
            value: raw.to_string(),
        })
}

fn facet_f64(tag: &RawTag, tier_index: usize, key: &'static str) -> Result<f64, GcsParseError> {
    let raw = tag.attr(key).unwrap_or("0");
    raw.parse()
        .map_err(|_| GcsParseError::FacetAttributeNotNumeric {
            tier_index,
            attr: key,
            value: raw.to_string(),
        })
}

fn vertex_f64(tag: &RawTag, tier_index: usize, key: &'static str) -> Result<f64, GcsParseError> {
    let raw = tag.attr(key).unwrap_or("0");
    raw.parse()
        .map_err(|_| GcsParseError::VertexAttributeNotNumeric {
            tier_index,
            attr: key,
            value: raw.to_string(),
        })
}

/// Parses one `<render>` numeric attribute: `Ok(0.0)` when `key` is absent (these
/// fields are optional -- not every real `.gcs` file's `<render>` element sets all of
/// them), but returns an error rather than silently defaulting to `0.0` when `key` IS
/// present and fails to parse. Garbage masquerading as "no data" deserves the same
/// treatment as the corruption [`tier_f64`]'s fallible path catches for `<tier>`.
fn render_f64(tag: &RawTag, key: &'static str) -> Result<f64, GcsParseError> {
    tag.attr(key).map_or(Ok(0.0), |raw| {
        raw.parse()
            .map_err(|_| GcsParseError::RenderAttributeNotNumeric {
                attr: key,
                value: raw.to_string(),
            })
    })
}

/// Consumes tags from `tags[*pos..]` until (and including) the `</facet>` that
/// closes `open`, building the facet's vertex list.
fn parse_facet(
    tags: &[RawTag],
    pos: &mut usize,
    tier_index: usize,
) -> Result<GcsFacet, GcsParseError> {
    let open = &tags[*pos];
    let normal = [
        facet_f64(open, tier_index, "nx")?,
        facet_f64(open, tier_index, "ny")?,
        facet_f64(open, tier_index, "nz")?,
    ];
    let index_angle_deg = facet_f64(open, tier_index, "index_angle")?;
    let self_closing = open.self_closing;
    *pos += 1;

    let mut vertices = Vec::new();
    if self_closing {
        // Not seen in the sampled corpus (every real facet has at least one
        // vertex), but a self-closing `<facet .../>` has no `</facet>` to look
        // for -- treat it as a facet with no vertices rather than scanning past
        // whatever tag happens to come next.
        return Ok(GcsFacet {
            normal,
            index_angle_deg,
            vertices,
        });
    }
    loop {
        let Some(tag) = tags.get(*pos) else {
            return Err(GcsParseError::UnterminatedElement { name: "facet" });
        };
        *pos += 1;
        if tag.name == "facet" && tag.closing {
            break;
        }
        if tag.name == "vertex" {
            vertices.push(GcsVertex {
                x: vertex_f64(tag, tier_index, "x")?,
                y: vertex_f64(tag, tier_index, "y")?,
                z: vertex_f64(tag, tier_index, "z")?,
            });
        }
    }
    Ok(GcsFacet {
        normal,
        index_angle_deg,
        vertices,
    })
}

/// Consumes tags from `tags[*pos..]` until (and including) the `</tier>` that
/// closes `open`, building the tier's facet list.
fn parse_tier(
    tags: &[RawTag],
    pos: &mut usize,
    tier_index: usize,
) -> Result<GcsTier, GcsParseError> {
    let open = &tags[*pos];
    let angle_deg = tier_f64(open, tier_index, "angle")?;
    let depth = tier_f64(open, tier_index, "depth")?;
    let name = open.attr("name").unwrap_or("").to_string();
    let instructions = open.attr("instructions").unwrap_or("").to_string();
    let visible = open.attr("visible") != Some("false");
    let guide = open.attr("guide") == Some("true");
    let self_closing = open.self_closing;
    *pos += 1;

    let mut facets = Vec::new();
    if self_closing {
        // Not seen in the sampled corpus, but a self-closing `<tier .../>` (a
        // tier with no facets at all) has no `</tier>` to scan for.
        return Ok(GcsTier {
            angle_deg,
            depth,
            name,
            instructions,
            visible,
            guide,
            facets,
        });
    }
    loop {
        let Some(tag) = tags.get(*pos) else {
            return Err(GcsParseError::UnterminatedElement { name: "tier" });
        };
        if tag.name == "tier" && tag.closing {
            *pos += 1;
            break;
        }
        if tag.name == "facet" && !tag.closing {
            facets.push(parse_facet(tags, pos, tier_index)?);
        } else {
            *pos += 1;
        }
    }
    Ok(GcsTier {
        angle_deg,
        depth,
        name,
        instructions,
        visible,
        guide,
        facets,
    })
}

/// Consumes tags from `tags[*pos..]` until (and including) the `</render>` that
/// closes `open`.
fn parse_render(tags: &[RawTag], pos: &mut usize) -> Result<GcsRender, GcsParseError> {
    let open = &tags[*pos];
    let material = open.attr("material").unwrap_or("").to_string();
    let refractive_index = render_f64(open, "refractive_index")?;
    let dispersion = render_f64(open, "dispersion")?;
    let clarity = render_f64(open, "clarity")?;
    let density = render_f64(open, "density")?;
    let lighting_model = open.attr("lighting_model").unwrap_or("").to_string();
    let self_closing = open.self_closing;
    *pos += 1;

    let mut color = GcsColor::default();
    if self_closing {
        // Not seen in the sampled corpus (every real `<render>` wraps a
        // `<color>`), but a self-closing `<render .../>` has no `</render>` to
        // scan for.
        return Ok(GcsRender {
            material,
            refractive_index,
            dispersion,
            clarity,
            density,
            lighting_model,
            color,
        });
    }
    loop {
        let Some(tag) = tags.get(*pos) else {
            return Err(GcsParseError::UnterminatedElement { name: "render" });
        };
        *pos += 1;
        if tag.name == "render" && tag.closing {
            break;
        }
        if tag.name == "color" {
            color = GcsColor {
                r: tag.attr("r").unwrap_or("0").parse().unwrap_or(0.0),
                g: tag.attr("g").unwrap_or("0").parse().unwrap_or(0.0),
                b: tag.attr("b").unwrap_or("0").parse().unwrap_or(0.0),
            };
        }
    }
    Ok(GcsRender {
        material,
        refractive_index,
        dispersion,
        clarity,
        density,
        lighting_model,
        color,
    })
}

fn opt_string(tag: &RawTag, key: &str) -> Option<String> {
    tag.attr(key).map(str::to_string)
}

fn parse_info(tag: &RawTag) -> GcsInfo {
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

/// Parses a Gem Cut Studio `.gcs` design.
///
/// # Errors
///
/// Returns `Err` if `content` is empty, is not well-formed enough for
/// [`tokenize`] to find matching `<`/`>` pairs and quoted attribute values, is
/// missing the `<GemCutStudio>` root or its `<index>` child, or has a tier,
/// facet, or vertex with a missing or non-numeric required attribute. Unknown
/// elements and unknown attributes are ignored rather than rejected, so a future
/// Gem Cut Studio version that adds fields this module does not know about should
/// still parse.
pub fn parse_gcs(content: &str) -> Result<GcsDesign, GcsParseError> {
    if content.trim().is_empty() {
        return Err(GcsParseError::EmptyInput);
    }
    let tags = tokenize(content)?;

    let mut pos = 0usize;
    let root = tags.first().ok_or(GcsParseError::MissingRootElement)?;
    if root.name != "GemCutStudio" || root.closing {
        return Err(GcsParseError::MissingRootElement);
    }
    let version = root.attr("version").unwrap_or("").to_string();
    pos += 1;

    let mut index = None;
    let mut tiers = Vec::new();
    let mut render = None;
    let mut info = None;

    while pos < tags.len() {
        let tag = &tags[pos];
        if tag.name == "GemCutStudio" && tag.closing {
            break;
        }
        match tag.name.as_str() {
            "index" => {
                index = Some(parse_index(tag)?);
                pos += 1;
            }
            "tier" if !tag.closing => {
                let tier_index = tiers.len();
                tiers.push(parse_tier(&tags, &mut pos, tier_index)?);
            }
            "render" if !tag.closing => {
                render = Some(parse_render(&tags, &mut pos)?);
            }
            "info" => {
                info = Some(parse_info(tag));
                pos += 1;
            }
            _ => pos += 1,
        }
    }

    Ok(GcsDesign {
        version,
        index: index.ok_or(GcsParseError::MissingIndexElement)?,
        tiers,
        render,
        info,
    })
}
