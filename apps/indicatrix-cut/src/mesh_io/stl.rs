//! STL, binary and ASCII: a soup of triangles, each with three vertices.
//!
//! The stored normals are ignored (the planner orients the mesh itself). Vertices that are
//! exactly equal are shared here, in file order, through an ordered map; the mesh check then
//! welds the ones that are merely close.

use super::{NotedMesh, too_many_triangles_note};
use glam::DVec3;
use indicatrix_cut_core::rough_plan::MAX_MESH_TRIANGLES;
use std::collections::BTreeMap;

/// The bytes before the triangles of a binary STL: an 80-byte header and the `u32` count.
const BINARY_PREFIX: usize = 84;

/// The bytes of one binary triangle: a normal and three vertices (12 `f32`) and a `u16`.
const BINARY_TRIANGLE: usize = 50;

/// The triangles, deduplicated vertices and (past the limit) the dropped faces of a file
/// being read.
struct Soup {
    points: Vec<DVec3>,
    index: BTreeMap<[u64; 3], u32>,
    triangles: Vec<[u32; 3]>,
}

impl Soup {
    const fn new() -> Self {
        Self {
            points: Vec::new(),
            index: BTreeMap::new(),
            triangles: Vec::new(),
        }
    }

    /// The index of the vertex `p`, adding it when no equal one is there yet. `-0.0` and
    /// `0.0` are the same vertex.
    fn vertex(&mut self, p: DVec3) -> Result<u32, String> {
        let p = p + DVec3::ZERO;
        let key = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
        if let Some(&id) = self.index.get(&key) {
            return Ok(id);
        }
        let id = u32::try_from(self.points.len())
            .map_err(|_| "the file has too many vertices".to_string())?;
        self.points.push(p);
        self.index.insert(key, id);
        Ok(id)
    }

    /// Ends the file: the mesh, or (past the triangle limit) the points alone with the note.
    fn finish(self, count: usize) -> NotedMesh {
        if count > MAX_MESH_TRIANGLES {
            (
                self.points,
                Vec::new(),
                Some(too_many_triangles_note(count)),
            )
        } else {
            (self.points, self.triangles, None)
        }
    }
}

/// The triangle count of `bytes` as a binary STL, when its size is exactly what the count
/// in its header implies.
fn binary_count(bytes: &[u8]) -> Option<usize> {
    let count = u32::from_le_bytes(bytes.get(80..BINARY_PREFIX)?.try_into().ok()?);
    let count = usize::try_from(count).ok()?;
    let expected = count
        .checked_mul(BINARY_TRIANGLE)?
        .checked_add(BINARY_PREFIX)?;
    (expected == bytes.len()).then_some(count)
}

/// Whether `bytes` is ASCII STL: `solid` first (after white space) and a `facet` somewhere.
fn is_ascii_solid(bytes: &[u8]) -> bool {
    let trimmed = bytes.trim_ascii_start();
    trimmed.len() >= 5
        && trimmed[..5].eq_ignore_ascii_case(b"solid")
        && trimmed.windows(5).any(|w| w.eq_ignore_ascii_case(b"facet"))
}

/// Whether `bytes` is an STL file of either kind. A binary file is recognised by its size
/// alone: it often starts with `solid` as well.
pub(super) fn looks_like_stl(bytes: &[u8]) -> bool {
    binary_count(bytes).is_some() || is_ascii_solid(bytes)
}

/// The points and triangles of the STL file `bytes`, ASCII or binary.
///
/// # Errors
///
/// Returns the message for a file that is truncated or is neither kind, a facet that is not
/// a triangle, a vertex that is not a finite number, or a file without triangles.
pub(super) fn parse_stl(bytes: &[u8]) -> Result<NotedMesh, String> {
    if let Some(count) = binary_count(bytes) {
        return parse_binary(bytes, count);
    }
    if is_ascii_solid(bytes) {
        parse_ascii(&String::from_utf8_lossy(bytes))
    } else if bytes.len() < BINARY_PREFIX {
        Err(format!(
            "the file is {} bytes, too short for an STL file: a binary STL has an 84-byte \
             header, an ASCII one starts with `solid`",
            bytes.len()
        ))
    } else {
        let promised = u32::from_le_bytes(
            bytes
                .get(80..BINARY_PREFIX)
                .and_then(|b| b.try_into().ok())
                .unwrap_or_default(),
        );
        let needed = usize::try_from(promised)
            .unwrap_or(usize::MAX)
            .saturating_mul(BINARY_TRIANGLE)
            .saturating_add(BINARY_PREFIX);
        Err(format!(
            "the binary STL does not match its header: it promises {promised} triangles \
             ({needed} bytes), but the file has {} bytes{}",
            bytes.len(),
            if bytes.len() < needed {
                " (truncated?)"
            } else {
                ""
            }
        ))
    }
}

/// One little-endian `f32` at `offset`, as an `f64`.
fn f32_at(bytes: &[u8], offset: usize) -> Option<f64> {
    let raw: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(f64::from(f32::from_le_bytes(raw)))
}

fn parse_binary(bytes: &[u8], count: usize) -> Result<NotedMesh, String> {
    if count == 0 {
        return Err("the STL file has no triangles".to_string());
    }
    let keep = count <= MAX_MESH_TRIANGLES;
    let mut soup = Soup::new();
    for triangle in 0..count {
        let base = BINARY_PREFIX + triangle * BINARY_TRIANGLE + 12;
        let mut ids = [0_u32; 3];
        for (corner, id) in ids.iter_mut().enumerate() {
            let at = base + corner * 12;
            let truncated = || format!("the STL file ends inside triangle {}", triangle + 1);
            let p = DVec3::new(
                f32_at(bytes, at).ok_or_else(truncated)?,
                f32_at(bytes, at + 4).ok_or_else(truncated)?,
                f32_at(bytes, at + 8).ok_or_else(truncated)?,
            );
            if !p.is_finite() {
                return Err(format!(
                    "triangle {} has a vertex that is not a finite number",
                    triangle + 1
                ));
            }
            *id = soup.vertex(p)?;
        }
        if keep {
            soup.triangles.push(ids);
        }
    }
    Ok(soup.finish(count))
}

fn parse_ascii(text: &str) -> Result<NotedMesh, String> {
    let mut soup = Soup::new();
    let mut count = 0_usize;
    let mut corners: Vec<u32> = Vec::new();
    let mut in_loop = false;
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        let mut words = line.split_whitespace();
        let Some(first) = words.next() else {
            continue;
        };
        match first.to_ascii_lowercase().as_str() {
            "outer" => {
                corners.clear();
                in_loop = true;
            }
            "vertex" => {
                if !in_loop {
                    return Err(format!("line {line_no} is a vertex outside `outer loop`"));
                }
                let mut coordinate = || {
                    words
                        .next()
                        .and_then(|word| word.parse::<f64>().ok())
                        .filter(|value| value.is_finite())
                        .ok_or_else(|| format!("line {line_no} is not a vertex with three numbers"))
                };
                let p = DVec3::new(coordinate()?, coordinate()?, coordinate()?);
                corners.push(soup.vertex(p)?);
            }
            "endloop" => {
                if corners.len() != 3 {
                    return Err(format!(
                        "line {line_no} ends a facet with {} vertices; only triangles are read",
                        corners.len()
                    ));
                }
                count += 1;
                if count == MAX_MESH_TRIANGLES + 1 {
                    soup.triangles = Vec::new();
                }
                if count <= MAX_MESH_TRIANGLES {
                    soup.triangles.push([corners[0], corners[1], corners[2]]);
                }
                in_loop = false;
            }
            _ => {}
        }
    }
    if in_loop {
        return Err("the STL file ends inside a facet (no `endloop`)".to_string());
    }
    if count == 0 {
        return Err("the STL file has no facets".to_string());
    }
    Ok(soup.finish(count))
}
