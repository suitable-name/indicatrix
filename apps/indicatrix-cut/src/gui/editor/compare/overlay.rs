//! The overlay comparison's difference layer: a transparent RGBA image, the same
//! size as both solid frames, that shows where the geometry changed no matter which
//! renderer draws the base image underneath.
//!
//! It is computed from the two sides' solid-raster coverage (the rasterizer's pick
//! buffers, `0` = uncovered) rendered at the SAME pose and size, so it needs neither
//! a renderer nor a window and is a pure function of the two masks:
//!
//! * covered by BEFORE only (material removed) -> red tint;
//! * covered by AFTER only (material added) -> green tint;
//! * covered by both or by neither -> fully transparent;
//! * the AFTER silhouette -> a 2 px bright green line, centred on the boundary;
//! * the BEFORE silhouette -> a 1 px red line on its inside;
//! * the AFTER side's facet edges inside the shared area -> thin, light,
//!   semi-transparent lines, so a changed facet reads even where the two stones
//!   overlap.
//!
//! Later steps win where they meet: after outline over before outline over facet
//! edges over the tints.

/// Tint over pixels only BEFORE covers: material that is gone.
pub(super) const REMOVED_TINT: [u8; 4] = [239, 68, 68, 150];

/// Tint over pixels only AFTER covers: material that is new.
pub(super) const ADDED_TINT: [u8; 4] = [34, 197, 94, 150];

/// The 2 px line along AFTER's silhouette.
pub(super) const AFTER_OUTLINE: [u8; 4] = [74, 255, 128, 255];

/// The 1 px line along BEFORE's silhouette.
pub(super) const BEFORE_OUTLINE: [u8; 4] = [255, 82, 82, 255];

/// The faint line along AFTER's facet edges inside the shared area.
pub(super) const FACET_EDGE: [u8; 4] = [235, 240, 250, 90];

/// A coverage mask over a `width x height` grid, read from a pick buffer.
struct Coverage<'a> {
    pick: &'a [u32],
    width: usize,
    height: usize,
}

impl Coverage<'_> {
    /// Whether the in-bounds pixel `(x, y)` is covered.
    const fn at(&self, x: usize, y: usize) -> bool {
        self.pick[y * self.width + x] != 0
    }

    /// Whether any 4-neighbour of `(x, y)` has coverage `covered`; a neighbour
    /// outside the frame counts as uncovered, so a stone clipped by the frame still
    /// gets a closed outline.
    fn any_neighbour(&self, x: usize, y: usize, covered: bool) -> bool {
        let probe = |nx: Option<usize>, ny: Option<usize>| match (nx, ny) {
            (Some(nx), Some(ny)) if nx < self.width && ny < self.height => {
                self.at(nx, ny) == covered
            }
            _ => !covered,
        };
        probe(x.checked_sub(1), Some(y))
            || probe(Some(x + 1), Some(y))
            || probe(Some(x), y.checked_sub(1))
            || probe(Some(x), Some(y + 1))
    }

    /// Whether `(x, y)` is a covered pixel on the silhouette's inside edge.
    fn is_inner_boundary(&self, x: usize, y: usize) -> bool {
        self.at(x, y) && self.any_neighbour(x, y, false)
    }

    /// Whether `(x, y)` is an uncovered pixel touching the silhouette from outside.
    fn is_outer_boundary(&self, x: usize, y: usize) -> bool {
        !self.at(x, y) && self.any_neighbour(x, y, true)
    }

    /// Whether `(x, y)` is a covered pixel whose right or lower neighbour is a
    /// covered pixel of a different facet -- an interior facet edge.
    fn is_facet_edge(&self, x: usize, y: usize) -> bool {
        let here = self.pick[y * self.width + x];
        if here == 0 {
            return false;
        }
        let differs = |nx: usize, ny: usize| {
            nx < self.width && ny < self.height && {
                let other = self.pick[ny * self.width + nx];
                other != 0 && other != here
            }
        };
        differs(x + 1, y) || differs(x, y + 1)
    }
}

/// Writes `rgba` at pixel index `index` of `out`.
fn put(out: &mut [u8], index: usize, rgba: [u8; 4]) {
    out[index * 4..index * 4 + 4].copy_from_slice(&rgba);
}

/// The removed/added tints from the two coverage masks.
fn paint_tints(out: &mut [u8], before: &Coverage<'_>, after: &Coverage<'_>) {
    for y in 0..before.height {
        for x in 0..before.width {
            match (before.at(x, y), after.at(x, y)) {
                (true, false) => put(out, y * before.width + x, REMOVED_TINT),
                (false, true) => put(out, y * before.width + x, ADDED_TINT),
                _ => {}
            }
        }
    }
}

/// AFTER's facet edges, on pixels the tints left transparent (the shared area).
fn paint_facet_edges(out: &mut [u8], after: &Coverage<'_>, edges: Option<&[u8]>) {
    for y in 0..after.height {
        for x in 0..after.width {
            let index = y * after.width + x;
            if out[index * 4 + 3] != 0 || !after.at(x, y) {
                continue;
            }
            let is_edge = edges.map_or_else(|| after.is_facet_edge(x, y), |mask| mask[index] != 0);
            if is_edge {
                put(out, index, FACET_EDGE);
            }
        }
    }
}

/// The BEFORE silhouette (1 px, inside) then the AFTER silhouette (2 px, centred),
/// so AFTER's line wins where they coincide.
fn paint_outlines(out: &mut [u8], before: &Coverage<'_>, after: &Coverage<'_>) {
    for y in 0..before.height {
        for x in 0..before.width {
            if before.is_inner_boundary(x, y) {
                put(out, y * before.width + x, BEFORE_OUTLINE);
            }
        }
    }
    for y in 0..after.height {
        for x in 0..after.width {
            if after.is_inner_boundary(x, y) || after.is_outer_boundary(x, y) {
                put(out, y * after.width + x, AFTER_OUTLINE);
            }
        }
    }
}

/// The difference layer for two pick buffers rendered at the same pose and `size`
/// (`pick == 0` means uncovered): an RGBA8 buffer of `size.0 * size.1 * 4` bytes.
///
/// `after_edges` is an optional one-byte-per-pixel mask (non-zero = a facet edge of
/// the AFTER render); `None` derives the edges from `after_pick` itself, wherever
/// two neighbouring covered pixels belong to different facets.
///
/// A buffer whose length does not match `size` (never expected) yields a fully
/// transparent layer rather than a panic on the worker thread.
#[must_use]
pub(super) fn difference_overlay(
    before_pick: &[u32],
    after_pick: &[u32],
    after_edges: Option<&[u8]>,
    size: (u32, u32),
) -> Vec<u8> {
    let (width, height) = (size.0 as usize, size.1 as usize);
    let count = width * height;
    let mut out = vec![0u8; count * 4];
    let mismatched = before_pick.len() != count
        || after_pick.len() != count
        || after_edges.is_some_and(|mask| mask.len() != count);
    if mismatched {
        return out;
    }
    let before = Coverage {
        pick: before_pick,
        width,
        height,
    };
    let after = Coverage {
        pick: after_pick,
        width,
        height,
    };
    paint_tints(&mut out, &before, &after);
    paint_facet_edges(&mut out, &after, after_edges);
    paint_outlines(&mut out, &before, &after);
    out
}
