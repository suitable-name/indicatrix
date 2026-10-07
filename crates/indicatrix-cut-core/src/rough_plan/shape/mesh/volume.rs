//! The material of a mesh inside cut planes: its volume, and its clipped surface for
//! display.
//!
//! The mesh's boundary after the cuts is its own triangles, clipped, plus one flat cap per
//! cut; the divergence theorem turns that closed surface into a volume.

use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;

use super::{
    CUT_SHIFT_FRACTION, ClippedSurface, RoughMesh, SurfaceCap,
    geometry::{area_vector, clip_halfspace},
    triangulate::triangulate,
};

/// What a cut plane is in the frame of a volume computation.
#[derive(Debug, Clone, Copy)]
struct Cut {
    /// Unit outward normal.
    n: DVec3,
    /// Offset in the frame centred on the mesh, already moved by the cut shift.
    m: f64,
    /// Position in the list the caller gave.
    index: usize,
}

impl RoughMesh {
    /// The cuts as normalised planes in the frame centred on the mesh, deduplicated: of
    /// equal normals only the tighter one is kept.
    fn effective_cuts(&self, cuts: &[(DVec3, f64)], origin: DVec3) -> Vec<Cut> {
        let shift = CUT_SHIFT_FRACTION * (self.hi - self.lo).max_element();
        let mut kept: Vec<Cut> = Vec::new();
        for (index, &(n, m)) in cuts.iter().enumerate() {
            let len = n.length();
            if !(len > 0.0 && len.is_finite() && m.is_finite()) {
                continue;
            }
            let n = n / len;
            let m = m / len - n.dot(origin) + shift;
            match kept.iter_mut().find(|c| c.n.dot(n) > 1.0 - 1e-12) {
                Some(prev) if m < prev.m => {
                    prev.m = m;
                    prev.index = index;
                }
                Some(_) => {}
                None => kept.push(Cut { n, m, index }),
            }
        }
        kept
    }

    /// The triangles (ascending) that can contribute to the material inside `cuts`, in the
    /// frame centred on `origin`, each with whether it still has to be clipped.
    ///
    /// The BVH is walked once: a node wholly outside some cut is dropped, and one wholly
    /// inside every cut is kept as it is (no clipping, which would copy its triangles
    /// unchanged). A node within [`CUT_SHIFT_FRACTION`]-sized slack of a plane counts as
    /// neither, so rounding can never change an outcome; the surviving triangles are clipped
    /// exactly as the full scan clips them and in the same order, so the volume is
    /// bit-identical. With `prune` false every triangle is returned to be clipped.
    fn clip_candidates(&self, origin: DVec3, cuts: &[Cut], prune: bool) -> Vec<(u32, bool)> {
        if !prune || self.bvh.is_empty() {
            return (0..self.tris.len() as u32).map(|t| (t, true)).collect();
        }
        let slack = 1e-9 * (self.hi - self.lo).max_element();
        let mut found: Vec<(u32, bool)> = Vec::new();
        let mut stack: Vec<(u32, bool)> = vec![(0, false)];
        while let Some((index, inside)) = stack.pop() {
            let node = &self.bvh[index as usize];
            let mut inside = inside;
            if !inside {
                let (lo, hi) = (node.min - origin, node.max - origin);
                let mut beyond = false;
                inside = true;
                for cut in cuts {
                    let positive = cut.n.cmpge(DVec3::ZERO);
                    let least = cut.n.dot(DVec3::select(positive, lo, hi));
                    let most = cut.n.dot(DVec3::select(positive, hi, lo));
                    if least > cut.m + slack {
                        beyond = true;
                        break;
                    }
                    inside &= most <= cut.m - slack;
                }
                if beyond {
                    continue;
                }
            }
            if node.count > 0 {
                let leaf = &self.order[node.first as usize..(node.first + node.count) as usize];
                found.extend(leaf.iter().map(|&t| (t, !inside)));
            } else {
                stack.push((node.first, inside));
                stack.push((node.first + 1, inside));
            }
        }
        found.sort_unstable();
        found
    }

    /// The triangles of the mesh clipped by `cuts`, in the frame centred on `origin`.
    fn clipped_polygons(&self, origin: DVec3, cuts: &[Cut], prune: bool) -> Vec<Vec<DVec3>> {
        let mut polygons = Vec::new();
        let mut current: Vec<DVec3> = Vec::with_capacity(8);
        let mut next: Vec<DVec3> = Vec::with_capacity(8);
        for (t, clip) in self.clip_candidates(origin, cuts, prune) {
            current.clear();
            current.extend(self.corners(t).map(|p| p - origin));
            if clip {
                for cut in cuts {
                    clip_halfspace(&current, cut.n, cut.m, &mut next);
                    std::mem::swap(&mut current, &mut next);
                    if current.len() < 3 {
                        break;
                    }
                }
            }
            if current.len() >= 3 {
                polygons.push(current.clone());
            }
        }
        polygons
    }

    /// The triangles (ascending) that can have vertices on both sides of `cut`'s plane: the
    /// BVH discards every node wholly on one side, with the slack of
    /// [`clip_candidates`](Self::clip_candidates). With `prune` false, all of them.
    fn crossing_triangles(&self, origin: DVec3, cut: &Cut, prune: bool) -> Vec<u32> {
        if !prune || self.bvh.is_empty() {
            return (0..self.tris.len() as u32).collect();
        }
        let slack = 1e-9 * (self.hi - self.lo).max_element();
        let positive = cut.n.cmpge(DVec3::ZERO);
        let mut found = Vec::new();
        self.walk(
            |node| {
                let (lo, hi) = (node.min - origin, node.max - origin);
                let least = cut.n.dot(DVec3::select(positive, lo, hi));
                let most = cut.n.dot(DVec3::select(positive, hi, lo));
                least <= cut.m + slack && most > cut.m - slack
            },
            |t| {
                found.push(t);
                true
            },
        );
        found.sort_unstable();
        found
    }

    /// The closed outlines the mesh's surface makes on the plane of `cuts[which]`, as
    /// polygons wound counter-clockwise about the cut's normal (holes clockwise), clipped
    /// by every other cut.
    ///
    /// A triangle with vertices on both sides of the plane contributes one segment between
    /// the points where its edges cross; those points are keyed by the edge, so the two
    /// triangles on an edge agree on them exactly and the segments chain into closed
    /// loops whatever the rounding. A vertex exactly on the plane counts as inside (the
    /// cut shift makes that the generic case).
    #[expect(
        clippy::many_single_char_names,
        reason = "entry and exit crossings, loop and vertex names are short by convention"
    )]
    fn cap_polygons(
        &self,
        origin: DVec3,
        cuts: &[Cut],
        which: usize,
        prune: bool,
    ) -> Vec<Vec<DVec3>> {
        let cut = cuts[which];
        let mut next: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
        let mut points: BTreeMap<(u32, u32), DVec3> = BTreeMap::new();
        for t in self.crossing_triangles(origin, &cut, prune) {
            let tri = &self.tris[t as usize];
            let v = tri.map(|k| self.verts[k as usize] - origin);
            let s = v.map(|p| cut.n.dot(p) - cut.m);
            let inside = s.map(|x| x <= 0.0);
            let mut entry = None;
            let mut exit = None;
            for k in 0..3 {
                let l = (k + 1) % 3;
                if inside[k] == inside[l] {
                    continue;
                }
                let (a, b) = if tri[k] < tri[l] { (k, l) } else { (l, k) };
                let key = (tri[a], tri[b]);
                let point = v[a] + (v[b] - v[a]) * (s[a] / (s[a] - s[b]));
                points.entry(key).or_insert(point);
                if inside[k] {
                    exit = Some(key);
                } else {
                    entry = Some(key);
                }
            }
            // The clipped surface leaves the plane's inside at `exit` and returns at
            // `entry`; the cap, which closes it, runs the other way.
            if let (Some(entry), Some(exit)) = (entry, exit) {
                next.insert(entry, exit);
            }
        }

        let mut loops = Vec::new();
        let mut done: BTreeSet<(u32, u32)> = BTreeSet::new();
        for &start in next.keys() {
            if done.contains(&start) {
                continue;
            }
            let mut outline = Vec::new();
            let mut at = start;
            while done.insert(at) {
                outline.push(points[&at]);
                match next.get(&at) {
                    Some(&following) => at = following,
                    None => break,
                }
            }
            loops.push(outline);
        }

        let mut scratch = Vec::new();
        for (j, other) in cuts.iter().enumerate() {
            if j == which {
                continue;
            }
            for outline in &mut loops {
                clip_halfspace(outline, other.n, other.m, &mut scratch);
                std::mem::swap(outline, &mut scratch);
            }
            loops.retain(|outline| outline.len() >= 3);
        }
        loops
    }

    /// The volume of the material that satisfies `n . p <= m` for every `(n, m)` of `cuts`,
    /// in mm^3 (exact up to rounding).
    ///
    /// The material's boundary after the cuts is the mesh's own triangles, clipped, and one
    /// flat cap per cut; the divergence theorem gives `V = sum (1/3) p . n dA` over it. A
    /// cap is the cross-section of the mesh with the cut plane (closed loops chained from
    /// the triangles that cross it) clipped by the other cuts, and its term is
    /// `m A / 3`. Cuts with equal normals are reduced to the tighter one.
    ///
    /// With no cuts this is [`volume`](Self::volume). The cuts of a rough model are its
    /// own planes, which a face cut may position anywhere: a cut through a notch wall or
    /// along a face of the mesh gives the correct volume too.
    #[must_use]
    pub fn volume_within(&self, cuts: &[(DVec3, f64)]) -> f64 {
        self.volume_within_pruned(cuts, true)
    }

    /// [`volume_within`](Self::volume_within), with the BVH pruning of the triangles
    /// switched by `prune`: the result is bit-identical either way, only the work differs.
    pub(super) fn volume_within_pruned(&self, cuts: &[(DVec3, f64)], prune: bool) -> f64 {
        if cuts.is_empty() {
            return self.volume;
        }
        let origin = (self.lo + self.hi) * 0.5;
        let cuts = self.effective_cuts(cuts, origin);
        let mut total = 0.0;
        for polygon in self.clipped_polygons(origin, &cuts, prune) {
            let first = polygon[0];
            for pair in polygon[1..].windows(2) {
                total += first.dot(pair[0].cross(pair[1])) / 6.0;
            }
        }
        for (which, cut) in cuts.iter().enumerate() {
            let area: f64 = self
                .cap_polygons(origin, &cuts, which, prune)
                .iter()
                .map(|outline| cut.n.dot(area_vector(outline)))
                .sum();
            total += cut.m * area / 3.0;
        }
        total.max(0.0)
    }

    /// The mesh's surface inside `cuts` and the caps the cuts leave, for display.
    ///
    /// The triangles keep the mesh's own winding. Cuts with equal normals are reduced to
    /// the tighter one, so a cap names the index of the cut that is kept.
    #[must_use]
    pub fn clipped_surface(&self, cuts: &[(DVec3, f64)]) -> ClippedSurface {
        let origin = (self.lo + self.hi) * 0.5;
        let cuts = self.effective_cuts(cuts, origin);
        let min_area = 1e-12 * (self.hi - self.lo).length_squared();
        let mut triangles = Vec::new();
        for polygon in self.clipped_polygons(origin, &cuts, true) {
            let first = polygon[0];
            for pair in polygon[1..].windows(2) {
                let area2 = (pair[0] - first).cross(pair[1] - first).length();
                if area2 > min_area {
                    triangles.push([first + origin, pair[0] + origin, pair[1] + origin]);
                }
            }
        }
        let mut caps = Vec::new();
        for (which, cut) in cuts.iter().enumerate() {
            let loops = self.cap_polygons(origin, &cuts, which, true);
            let cap_triangles = triangulate(&loops, cut.n, min_area);
            if !cap_triangles.is_empty() {
                caps.push(SurfaceCap {
                    cut: cut.index,
                    normal: cut.n,
                    triangles: cap_triangles
                        .into_iter()
                        .map(|tri| tri.map(|p| p + origin))
                        .collect(),
                });
            }
        }
        ClippedSurface { triangles, caps }
    }
}
