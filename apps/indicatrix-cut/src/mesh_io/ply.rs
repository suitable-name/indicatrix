//! PLY (Stanford polygon file): ASCII, binary little-endian and binary big-endian.
//!
//! The vertex element's `x`, `y` and `z` are read by name and every other property (normals,
//! colours, ...) is skipped by its size. The face element's list property `vertex_indices`
//! (or `vertex_index`) gives the polygons; a polygon of more than three corners is split as
//! an OBJ face is. Every other element is read through and dropped.

use super::{NotedMesh, face::triangulate_face, too_many_triangles_note};
use glam::DVec3;
use indicatrix_cut_core::rough_plan::MAX_MESH_TRIANGLES;

/// A PLY scalar type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Ty {
    fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Scalar(Ty),
    List { count: Ty, item: Ty },
}

#[derive(Debug)]
struct Property {
    name: String,
    kind: Kind,
}

#[derive(Debug)]
struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Ascii,
    Binary { big_endian: bool },
}

#[derive(Debug)]
struct Header {
    encoding: Encoding,
    elements: Vec<Element>,
    /// Where the body starts in the file.
    body: usize,
}

/// The position of the first `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_header(bytes: &[u8]) -> Result<Header, String> {
    const END: &[u8] = b"end_header";
    let end = find(bytes, END).ok_or("the PLY header has no `end_header` line")?;
    let mut body = end + END.len();
    if bytes.get(body) == Some(&b'\r') {
        body += 1;
    }
    if bytes.get(body) == Some(&b'\n') {
        body += 1;
    }
    let text = String::from_utf8_lossy(&bytes[..end]);
    if text.lines().next().map(str::trim) != Some("ply") {
        return Err("the file does not start with `ply`".to_string());
    }
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["ply"] | [] | ["comment" | "obj_info", ..] => {}
            ["format", name, ..] => {
                encoding = Some(match *name {
                    "ascii" => Encoding::Ascii,
                    "binary_little_endian" => Encoding::Binary { big_endian: false },
                    "binary_big_endian" => Encoding::Binary { big_endian: true },
                    other => {
                        return Err(format!(
                            "the PLY format `{other}` (header line {line_no}) is not supported"
                        ));
                    }
                });
            }
            ["element", name, count] => {
                let count = count.parse().map_err(|_| {
                    format!("header line {line_no}: the element count `{count}` is not a number")
                })?;
                elements.push(Element {
                    name: (*name).to_string(),
                    count,
                    properties: Vec::new(),
                });
            }
            ["property", "list", count, item, name] => {
                let kind = match (Ty::parse(count), Ty::parse(item)) {
                    (Some(count), Some(item)) => Kind::List { count, item },
                    _ => return Err(format!("header line {line_no}: unknown list type")),
                };
                push_property(&mut elements, name, kind, line_no)?;
            }
            ["property", ty, name] => {
                let kind = Ty::parse(ty).map(Kind::Scalar).ok_or_else(|| {
                    format!("header line {line_no}: unknown property type `{ty}`")
                })?;
                push_property(&mut elements, name, kind, line_no)?;
            }
            _ => return Err(format!("header line {line_no} is not understood: `{line}`")),
        }
    }
    let encoding = encoding.ok_or("the PLY header has no `format` line")?;
    Ok(Header {
        encoding,
        elements,
        body,
    })
}

fn push_property(
    elements: &mut [Element],
    name: &str,
    kind: Kind,
    line_no: usize,
) -> Result<(), String> {
    let element = elements
        .last_mut()
        .ok_or_else(|| format!("header line {line_no}: a property before any element"))?;
    element.properties.push(Property {
        name: name.to_string(),
        kind,
    });
    Ok(())
}

/// A source of PLY scalars; the error says why none could be read.
trait Reader {
    fn next(&mut self, ty: Ty) -> Result<f64, &'static str>;
}

const ENDED: &str = "the file ends";

struct AsciiReader<'a> {
    tokens: std::str::SplitAsciiWhitespace<'a>,
}

impl Reader for AsciiReader<'_> {
    fn next(&mut self, _ty: Ty) -> Result<f64, &'static str> {
        self.tokens
            .next()
            .ok_or(ENDED)?
            .parse()
            .map_err(|_| "a value is not a number")
    }
}

struct BinaryReader<'a> {
    data: &'a [u8],
    at: usize,
    big_endian: bool,
}

impl BinaryReader<'_> {
    /// The `N` bytes of the next value, in little-endian order.
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], &'static str> {
        let slice = self.data.get(self.at..self.at + N).ok_or(ENDED)?;
        self.at += N;
        let mut array = [0_u8; N];
        array.copy_from_slice(slice);
        if self.big_endian {
            array.reverse();
        }
        Ok(array)
    }
}

impl Reader for BinaryReader<'_> {
    fn next(&mut self, ty: Ty) -> Result<f64, &'static str> {
        Ok(match ty {
            Ty::I8 => f64::from(i8::from_le_bytes(self.bytes()?)),
            Ty::U8 => f64::from(u8::from_le_bytes(self.bytes()?)),
            Ty::I16 => f64::from(i16::from_le_bytes(self.bytes()?)),
            Ty::U16 => f64::from(u16::from_le_bytes(self.bytes()?)),
            Ty::I32 => f64::from(i32::from_le_bytes(self.bytes()?)),
            Ty::U32 => f64::from(u32::from_le_bytes(self.bytes()?)),
            Ty::F32 => f64::from(f32::from_le_bytes(self.bytes()?)),
            Ty::F64 => f64::from_le_bytes(self.bytes()?),
        })
    }
}

/// The non-negative whole number `value` as a `usize`.
fn whole(value: f64) -> Option<usize> {
    (value >= 0.0 && value <= f64::from(u32::MAX) && value.fract() == 0.0).then_some(value as usize)
}

/// The points and triangles of the PLY file `bytes`.
///
/// # Errors
///
/// Returns the message for a header that is missing or not understood, a file that ends
/// inside an element (naming it), a vertex without `x`, `y` and `z`, a face index outside the
/// vertices, or a file without vertices.
pub(super) fn parse_ply(bytes: &[u8]) -> Result<NotedMesh, String> {
    let header = parse_header(bytes)?;
    let body = bytes.get(header.body..).unwrap_or_default();
    match header.encoding {
        Encoding::Ascii => {
            let text = std::str::from_utf8(body)
                .map_err(|_| "the body of the ASCII PLY file is not text".to_string())?;
            read_body(
                &header.elements,
                AsciiReader {
                    tokens: text.split_ascii_whitespace(),
                },
            )
        }
        Encoding::Binary { big_endian } => read_body(
            &header.elements,
            BinaryReader {
                data: body,
                at: 0,
                big_endian,
            },
        ),
    }
}

/// What a read of `element` ran into, naming the element and the item it was reading.
fn context(element: &Element, item: usize, why: &str) -> String {
    format!(
        "{why} inside the `{}` element (item {} of {})",
        element.name,
        item + 1,
        element.count
    )
}

/// Reads one value of `kind`, returning it when it is a scalar (a list is read through and
/// gives `None`).
fn skip_or_read(reader: &mut impl Reader, kind: Kind) -> Result<Option<f64>, &'static str> {
    match kind {
        Kind::Scalar(ty) => reader.next(ty).map(Some),
        Kind::List { count, item } => {
            let n = whole(reader.next(count)?).ok_or("a list length is not a whole number")?;
            for _ in 0..n {
                reader.next(item)?;
            }
            Ok(None)
        }
    }
}

fn read_body(elements: &[Element], mut reader: impl Reader) -> Result<NotedMesh, String> {
    let mut points: Vec<DVec3> = Vec::new();
    // Polygons as corner indices and their 1-based number, checked once the vertex count
    // is known (a face element may come first).
    let mut faces: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut triangle_count = 0_usize;
    for element in elements {
        match element.name.as_str() {
            "vertex" => read_vertices(element, &mut reader, &mut points)?,
            "face" => read_faces(element, &mut reader, &mut faces, &mut triangle_count)?,
            _ => {
                for item in 0..element.count {
                    for property in &element.properties {
                        skip_or_read(&mut reader, property.kind)
                            .map_err(|why| context(element, item, why))?;
                    }
                }
            }
        }
    }
    if points.is_empty() {
        return Err("the PLY file has no vertices".to_string());
    }
    if triangle_count > MAX_MESH_TRIANGLES {
        return Ok((
            points,
            Vec::new(),
            Some(too_many_triangles_note(triangle_count)),
        ));
    }
    let total = points.len();
    let mut triangles = Vec::new();
    let mut note = None;
    for (number, corners) in faces {
        let mut resolved = Vec::with_capacity(corners.len());
        for corner in corners {
            if corner >= total {
                return Err(format!(
                    "face {number} names vertex {corner} (0-based), but the file has {total} vertices"
                ));
            }
            resolved.push(
                u32::try_from(corner).map_err(|_| "the file has too many vertices".to_string())?,
            );
        }
        match triangulate_face(&points, &resolved) {
            Some(split) => triangles.extend(split),
            None => {
                note.get_or_insert_with(|| {
                    format!(
                        "Face {number} is a polygon that is not convex and could not be split \
                         into triangles, so the mesh is not used and its convex hull is the rough."
                    )
                });
            }
        }
    }
    if note.is_some() {
        triangles.clear();
    }
    Ok((points, triangles, note))
}

fn read_vertices(
    element: &Element,
    reader: &mut impl Reader,
    points: &mut Vec<DVec3>,
) -> Result<(), String> {
    // For each property: which of x, y, z it is, if any.
    let slots: Vec<Option<usize>> = element
        .properties
        .iter()
        .map(|p| match (p.name.as_str(), p.kind) {
            ("x", Kind::Scalar(_)) => Some(0),
            ("y", Kind::Scalar(_)) => Some(1),
            ("z", Kind::Scalar(_)) => Some(2),
            _ => None,
        })
        .collect();
    if (0..3).any(|axis| !slots.contains(&Some(axis))) {
        return Err("the PLY vertex element has no x, y and z properties".to_string());
    }
    // A header may promise far more than the file holds; the reads stop at its end.
    points.reserve(element.count.min(1 << 20));
    for item in 0..element.count {
        let mut xyz = [0.0_f64; 3];
        for (property, slot) in element.properties.iter().zip(&slots) {
            let value =
                skip_or_read(reader, property.kind).map_err(|why| context(element, item, why))?;
            if let (Some(slot), Some(value)) = (slot, value) {
                xyz[*slot] = value;
            }
        }
        let p = DVec3::from_array(xyz);
        if !p.is_finite() {
            return Err(format!(
                "vertex {} is not a finite number in x, y and z",
                item + 1
            ));
        }
        points.push(p);
    }
    Ok(())
}

fn read_faces(
    element: &Element,
    reader: &mut impl Reader,
    faces: &mut Vec<(usize, Vec<usize>)>,
    triangle_count: &mut usize,
) -> Result<(), String> {
    let has_indices = element.properties.iter().any(is_index_list);
    if !has_indices && element.count > 0 {
        return Err(
            "the PLY face element has no `vertex_indices` list, so there are no triangles"
                .to_string(),
        );
    }
    for item in 0..element.count {
        for property in &element.properties {
            match property.kind {
                Kind::List { count, item: ty } if is_index_list(property) => {
                    let corners = read_corners(reader, count, ty)
                        .map_err(|why| context(element, item, why))?;
                    if corners.len() < 3 {
                        return Err(format!("face {} has fewer than three vertices", item + 1));
                    }
                    *triangle_count += corners.len() - 2;
                    if *triangle_count > MAX_MESH_TRIANGLES {
                        // Not used past the limit: drop what is held and only count the rest.
                        *faces = Vec::new();
                    } else {
                        faces.push((item + 1, corners));
                    }
                }
                kind => {
                    skip_or_read(reader, kind).map_err(|why| context(element, item, why))?;
                }
            }
        }
    }
    Ok(())
}

fn is_index_list(property: &Property) -> bool {
    matches!(property.name.as_str(), "vertex_indices" | "vertex_index")
        && matches!(property.kind, Kind::List { .. })
}

fn read_corners(reader: &mut impl Reader, count: Ty, item: Ty) -> Result<Vec<usize>, &'static str> {
    let n = whole(reader.next(count)?).ok_or("a list length is not a whole number")?;
    let mut corners = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        corners.push(whole(reader.next(item)?).ok_or("a vertex index is negative")?);
    }
    Ok(corners)
}
