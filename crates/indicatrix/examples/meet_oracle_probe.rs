//! TEMPORARY oracle probe for the vertex-incidence meet model.
//!
//! For every tier of every corpus design, this pins every *other* tier at its real
//! recorded mast and asks: is this tier's own recorded mast realized as `n . v` for a
//! vertex `v` of the arrangement of the other tiers' planes? (The "E3b" measurement
//! from the Python analysis, reproduced in Rust, in f64, deterministically -- no
//! convex-hull crate anywhere.) It also gathers the statistics the incremental solver
//! rebuild needs:
//!
//! - rank of the true vertex among candidate levels ordered by `n . v` descending
//!   (rank 0 == support-function tangency, the current solver's model);
//! - how much intersecting candidate values across a tier's symmetric index
//!   instances prunes the candidate list;
//! - whether "deepest cut that leaves every other facet alive" identifies the true
//!   level (a candidate local selection rule);
//! - E4: for stated `"Meet <names>"` tiers whose names resolve, does restricting
//!   candidates to vertices incident to the named tiers pin the true value;
//! - an incremental-reachability ceiling: starting from the design's scale anchor,
//!   can every tier's true vertex be pinned by already-reachable tiers plus the
//!   tier's own sibling instances;
//! - a global-solve ceiling: with the TRUE incidence structure known, is the whole
//!   mast vector uniquely determined by the anchors alone (one big linear system).
//!
//! Run from the workspace root:
//! ```text
//! cargo run --profile probe -p indicatrix --example meet_oracle_probe
//! ```

// Temporary measurement tooling: readability-of-the-measurement wins over the
// production lint bar here (the lints below are all style/perf-shape, not
// correctness).
#![allow(
    clippy::doc_markdown,
    clippy::suboptimal_flops,
    clippy::many_single_char_names,
    clippy::needless_range_loop,
    clippy::option_if_let_else,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::too_long_first_doc_paragraph,
    clippy::useless_let_if_seq
)]

use glam::{DMat3, DVec3};
use indicatrix_formats::asc::{self, AscSchedule, MeetInstruction};
use rusqlite::Connection;
use std::collections::HashSet;

const THREADS: usize = 16;
/// Bounding-blank half extent (matches the solver's `BLANK_HALF_EXTENT`).
const BLANK: f64 = 64.0;
/// Feasibility slack: a vertex may poke this far (absolute, masts are ~1) beyond a
/// plane before that plane's tier counts as violated.
const EPS_FEAS: f64 = 1e-5;
/// Incidence tolerance: a plane within this absolute distance of a vertex counts as
/// passing through it.
const EPS_INCIDENT: f64 = 1e-4;
/// Relative error below which a candidate value counts as matching the true mast.
const MATCH_REL: f64 = 0.005;
/// Two candidate values within this (absolute) distance belong to one "level".
const LEVEL_TOL: f64 = 1e-5;
/// Designs with more planes than this are skipped (cubic triple enumeration).
const MAX_PLANES: usize = 400;
/// Minimum |det| for a triple of unit normals to define a candidate vertex.
const MIN_DET: f64 = 1e-6;

struct AscRow {
    detail_id: i64,
    content: Vec<u8>,
}

/// One plane of the design: unit normal, offset (mast), owning tier (usize::MAX for
/// the bounding blank).
#[derive(Clone, Copy)]
struct Plane {
    n: DVec3,
    m: f64,
    owner: usize,
}

/// One candidate vertex of the arrangement: position, the single tier whose planes it
/// violates (`None` = feasible for the full solid), and the three owning tiers of the
/// planes that formed it.
struct Cand {
    v: DVec3,
    violated: Option<usize>,
    owners: [usize; 3],
}

#[derive(Default)]
struct TierStats {
    /// E3b relative error, instance 0.
    err0: f64,
    /// E3b relative error, worst instance.
    err_worst: f64,
    /// Rank (0-based level index, descending by value) of the true level, instance 0.
    /// `None` when err0 >= MATCH_REL (no matching level).
    rank: Option<usize>,
    /// Rank after intersecting candidate values across all instances.
    rank_sym: Option<usize>,
    /// Total candidate levels (instance 0).
    n_levels: usize,
    /// Tangency overshoot: (max candidate value) / true mast, instance 0.
    tangency_ratio: f64,
    /// Rank of the deepest level at which every other tier's facet keeps >= 3
    /// corner vertices ("deepest safe cut").
    deepest_safe_rank: Option<usize>,
    /// E4: relative error restricted to candidates incident to every resolved named
    /// reference. `None` when the tier has no resolved named refs.
    e4_err: Option<f64>,
    /// How many of the tier's stated meet names resolved / total stated.
    named_resolved: Option<(usize, usize)>,
    /// True if this tier was reachable in the incremental simulation.
    reachable: bool,
    /// Relative mast error from the global true-incidence linear solve. `None` when
    /// the design's global system could not be assembled/solved for this tier.
    global_err: Option<f64>,
    /// Selection-rule experiment hits (others pinned at truth): did each rule's
    /// predicted level match the true mast within MATCH_REL?
    hit_rank1: bool,
    hit_named_rank1: bool,
    hit_degree: bool,
    hit_named_degree: bool,
    /// Relative error of the named->rank1 rule's prediction (not just hit/miss).
    named_rank1_err: Option<f64>,
    /// True if the tier is meet-derived (scored at all).
    scored: bool,
}

#[derive(Default)]
struct DesignResult {
    parsed: bool,
    skipped_too_big: bool,
    degenerate: bool,
    tiers: Vec<TierStats>,
    all_reachable: bool,
    any_scored: bool,
    solver_median_err: Option<f64>,
    degeneracy_truth: Option<f64>,
    degeneracy_solved: Option<f64>,
}

fn side_and_normals(schedule: &AscSchedule) -> (Vec<bool>, Vec<Vec<DVec3>>) {
    let gear = f64::from(schedule.gear_teeth_abs().max(1));
    let mut is_crown = Vec::with_capacity(schedule.tiers.len());
    let mut normals = Vec::with_capacity(schedule.tiers.len());
    let mut last_crown = true;
    for tier in &schedule.tiers {
        let crown = if tier.angle_deg == 0.0 {
            if tier.angle_deg.is_sign_negative() {
                false
            } else {
                last_crown
            }
        } else {
            tier.angle_deg > 0.0
        };
        last_crown = crown;
        is_crown.push(crown);
        let theta = tier.angle_deg.abs().to_radians();
        let (st, ct) = (theta.sin(), theta.cos());
        let y = if crown { ct } else { -ct };
        let ns: Vec<DVec3> = if tier.indices.is_empty() {
            vec![DVec3::new(0.0, y, st)]
        } else {
            tier.indices
                .iter()
                .map(|&idx| {
                    let phi = 2.0 * std::f64::consts::PI * idx / gear;
                    DVec3::new(st * phi.cos(), y, st * phi.sin())
                })
                .collect()
        };
        normals.push(ns);
    }
    (is_crown, normals)
}

/// Enumerates every feasible-or-one-tier-violating triple-intersection vertex of the
/// real plane arrangement. Deterministic: plain nested loops over a fixed plane order.
fn enumerate_candidates(planes: &[Plane], n_real_start: usize) -> Vec<Cand> {
    let p = planes.len();
    let mut out = Vec::new();
    for a in n_real_start..p {
        for b in (a + 1)..p {
            for c in (b + 1)..p {
                let (pa, pb, pc) = (planes[a], planes[b], planes[c]);
                let m = DMat3::from_cols(pa.n, pb.n, pc.n).transpose();
                let det = m.determinant();
                if det.abs() < MIN_DET {
                    continue;
                }
                let v = m.inverse() * DVec3::new(pa.m, pb.m, pc.m);
                if v.x.abs() > BLANK + 1.0 || v.y.abs() > BLANK + 1.0 || v.z.abs() > BLANK + 1.0 {
                    continue;
                }
                // Feasibility: collect violated owner tiers, early-exit at 2 distinct.
                let mut violated: Option<usize> = None;
                let mut dead = false;
                for q in planes {
                    let d = q.n.dot(v) - q.m;
                    if d > EPS_FEAS {
                        if q.owner == usize::MAX {
                            dead = true; // outside the bounding blank
                            break;
                        }
                        match violated {
                            None => violated = Some(q.owner),
                            Some(t) if t == q.owner => {}
                            Some(_) => {
                                dead = true;
                                break;
                            }
                        }
                    }
                }
                if dead {
                    continue;
                }
                out.push(Cand {
                    v,
                    violated,
                    owners: [pa.owner, pb.owner, pc.owner],
                });
            }
        }
    }
    out
}

/// Groups a descending-sorted value list into levels (values within LEVEL_TOL of the
/// level head belong to it). Returns the level head values, descending.
fn group_levels(mut vals: Vec<f64>) -> Vec<f64> {
    vals.sort_by(|x, y| y.partial_cmp(x).unwrap());
    let mut levels: Vec<f64> = Vec::new();
    for v in vals {
        match levels.last() {
            Some(&head) if (head - v).abs() <= LEVEL_TOL => {}
            _ => levels.push(v),
        }
    }
    levels
}

#[allow(clippy::too_many_lines)]
fn analyze_one(row: &AscRow) -> DesignResult {
    let mut out = DesignResult::default();
    let text = String::from_utf8_lossy(&row.content);
    let Ok(schedule) = asc::parse_asc(&text) else {
        return out;
    };
    out.parsed = true;
    let nt = schedule.tiers.len();
    if nt == 0 {
        out.degenerate = true;
        return out;
    }

    let (_is_crown, normals) = side_and_normals(&schedule);
    let masts: Vec<f64> = schedule.tiers.iter().map(|t| t.mast.abs()).collect();

    // Plane list: 6 blank planes first, then every tier instance.
    let mut planes: Vec<Plane> = [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ]
    .into_iter()
    .map(|n| Plane {
        n,
        m: BLANK,
        owner: usize::MAX,
    })
    .collect();
    for (i, ns) in normals.iter().enumerate() {
        for &n in ns {
            planes.push(Plane {
                n,
                m: masts[i],
                owner: i,
            });
        }
    }
    if planes.len() > MAX_PLANES {
        out.skipped_too_big = true;
        return out;
    }

    let cands = enumerate_candidates(&planes, 6);
    if cands.is_empty() {
        out.degenerate = true;
        return out;
    }

    // Which tiers are scale anchors (not meet-derived). Mirrors
    // meet_tier_inputs_from_asc's classification, plus the validation harness's
    // tier-0 bootstrap when no stated anchor exists.
    let mut is_anchor: Vec<bool> = schedule
        .tiers
        .iter()
        .map(|t| {
            matches!(
                t.meet_instruction(),
                Some(MeetInstruction::ScaleReference | MeetInstruction::LevelGirdle)
            )
        })
        .collect();
    // Per-block anchors: the crown block (normal.y > 0), pavilion block
    // (normal.y < 0), and girdle block (normal.y ~ 0) each float as a coherent unit
    // (verified: a uniform y-shift of one block preserves every vertex incidence),
    // so each block present in the design needs one stated dimension. Bootstrap the
    // first tier of any block that has no stated anchor -- the exact analog of a
    // printed diagram's stated C/W, P/W, and girdle-size numbers.
    for class_of in [
        |y: f64| y > 1e-6,        // crown
        |y: f64| y < -1e-6,       // pavilion
        |y: f64| y.abs() <= 1e-6, // girdle
    ] {
        let members: Vec<usize> = (0..nt).filter(|&i| class_of(normals[i][0].y)).collect();
        if !members.is_empty() && !members.iter().any(|&i| is_anchor[i]) {
            is_anchor[members[0]] = true;
        }
    }

    // Name resolution (exact match only, first tier wins).
    let resolve = |name: &str| -> Option<usize> {
        schedule
            .tiers
            .iter()
            .position(|t| t.names().contains(&name))
    };

    // Per-tier realizing vertices for the reachability simulation: (position,
    // incident (tier, instance) pairs).
    #[allow(clippy::type_complexity)]
    let mut realizing_vertices: Vec<Vec<(DVec3, Vec<(usize, usize)>)>> = vec![Vec::new(); nt];

    let mut tier_stats: Vec<TierStats> = Vec::new();
    for i in 0..nt {
        let mut st = TierStats::default();
        let m = masts[i];
        if is_anchor[i] || m < 1e-6 {
            tier_stats.push(st);
            continue;
        }
        st.scored = true;

        // Candidates usable for tier i: none of the forming planes owned by i, and
        // the vertex violates at most tier i.
        let usable: Vec<&Cand> = cands
            .iter()
            .filter(|c| !c.owners.contains(&i) && (c.violated.is_none() || c.violated == Some(i)))
            .collect();
        if usable.is_empty() {
            st.err0 = f64::INFINITY;
            st.err_worst = f64::INFINITY;
            tier_stats.push(st);
            continue;
        }

        // Per-instance value lists.
        let inst_vals: Vec<Vec<f64>> = normals[i]
            .iter()
            .map(|&n| usable.iter().map(|c| n.dot(c.v)).collect())
            .collect();

        // E3b, instance 0 and worst instance.
        let err_of = |vals: &[f64]| -> f64 {
            vals.iter()
                .map(|&d| (d - m).abs() / m)
                .fold(f64::INFINITY, f64::min)
        };
        st.err0 = err_of(&inst_vals[0]);
        st.err_worst = inst_vals
            .iter()
            .map(|vals| err_of(vals))
            .fold(0.0_f64, f64::max);

        // Levels and rank, instance 0.
        let levels0 = group_levels(inst_vals[0].clone());
        st.n_levels = levels0.len();
        st.tangency_ratio = levels0.first().copied().unwrap_or(f64::NAN) / m;
        let true_level = levels0.iter().position(|&d| (d - m).abs() / m < MATCH_REL);
        st.rank = true_level;

        // Symmetry-intersected rank: keep instance-0 levels that appear (within
        // LEVEL_TOL + matching tolerance) in every other instance's value list.
        if normals[i].len() > 1 {
            let common: Vec<f64> = levels0
                .iter()
                .copied()
                .filter(|&d| {
                    inst_vals[1..]
                        .iter()
                        .all(|vals| vals.iter().any(|&x| (x - d).abs() <= LEVEL_TOL * 4.0))
                })
                .collect();
            st.rank_sym = common.iter().position(|&d| (d - m).abs() / m < MATCH_REL);
        } else {
            st.rank_sym = st.rank;
        }

        // Deepest-safe rank: cutting all of tier i's instances at level d removes
        // every candidate vertex with n_ij . v > d for any j. Another tier t's facet
        // keeps a corner vertex v (incident to t, feasible for the full others-solid)
        // iff v survives. Deepest safe level = last level (descending) at which every
        // other scored tier keeps >= 3 corners.
        {
            // Corner vertices per other tier: indices into `usable`.
            let mut corners: Vec<Vec<usize>> = vec![Vec::new(); nt];
            for (ci, c) in usable.iter().enumerate() {
                if c.violated.is_some() {
                    continue; // not a vertex of the others-solid interior region
                }
                for t in 0..nt {
                    if t == i {
                        continue;
                    }
                    let inc = normals[t]
                        .iter()
                        .any(|&n| (n.dot(c.v) - masts[t]).abs() < EPS_INCIDENT);
                    if inc {
                        corners[t].push(ci);
                    }
                }
            }
            let survives = |ci: usize, d: f64| -> bool {
                let v = usable[ci].v;
                normals[i].iter().all(|&n| n.dot(v) <= d + LEVEL_TOL)
            };
            let mut deepest: Option<usize> = None;
            for (li, &d) in levels0.iter().enumerate() {
                let safe = (0..nt).all(|t| {
                    if t == i || corners[t].is_empty() {
                        return true;
                    }
                    corners[t].iter().filter(|&&ci| survives(ci, d)).count() >= 3
                });
                if safe {
                    deepest = Some(li);
                } else {
                    break; // annihilation is monotone in depth
                }
            }
            st.deepest_safe_rank = deepest;
        }

        // E4: stated meet names.
        let mut resolved_refs: Vec<usize> = Vec::new();
        if let Some(MeetInstruction::Meet(names)) = schedule.tiers[i].meet_instruction() {
            let total = names.len();
            let refs: Vec<usize> = {
                let mut r: Vec<usize> = names
                    .iter()
                    .filter_map(|nm| resolve(nm))
                    .filter(|&t| t != i)
                    .collect();
                r.sort_unstable();
                r.dedup();
                r
            };
            st.named_resolved = Some((refs.len(), total));
            if !refs.is_empty() {
                let n0 = normals[i][0];
                let e4 = usable
                    .iter()
                    .filter(|c| {
                        refs.iter().all(|&t| {
                            normals[t]
                                .iter()
                                .any(|&n| (n.dot(c.v) - masts[t]).abs() < EPS_INCIDENT)
                        })
                    })
                    .map(|c| (n0.dot(c.v) - m).abs() / m)
                    .fold(f64::INFINITY, f64::min);
                st.e4_err = Some(e4);
            }
            resolved_refs = refs;
        }

        // ------------------------------------------------------------------
        // Selection-rule experiments (all other tiers at truth). Each rule picks a
        // predicted mast; a hit is a prediction within MATCH_REL of the true mast.
        // ------------------------------------------------------------------
        {
            let hit = |pred: Option<f64>| pred.is_some_and(|p| (p - m).abs() / m < MATCH_REL);

            // Per-candidate degeneracy degree: number of distinct OTHER tiers with a
            // plane through the candidate vertex.
            let degrees: Vec<usize> = usable
                .iter()
                .map(|c| {
                    (0..nt)
                        .filter(|&t| {
                            t != i
                                && normals[t]
                                    .iter()
                                    .any(|&n| (n.dot(c.v) - masts[t]).abs() < EPS_INCIDENT)
                        })
                        .count()
                })
                .collect();
            // Per-level max degree, over the first few levels.
            let level_degree: Vec<usize> = levels0
                .iter()
                .map(|&head| {
                    usable
                        .iter()
                        .enumerate()
                        .filter(|(ci, _)| (inst_vals[0][*ci] - head).abs() <= LEVEL_TOL)
                        .map(|(ci, _)| degrees[ci])
                        .max()
                        .unwrap_or(0)
                })
                .collect();

            // Rule "rank1": always pick level 1 (fall back to level 0 if only one).
            let rank1_pred = levels0.get(1).or_else(|| levels0.first()).copied();
            st.hit_rank1 = hit(rank1_pred);

            // Named override: min-|delta|... no -- at solve time truth is unknown, so
            // the named rule must be "shallowest level with a candidate incident to
            // every resolved ref".
            let named_pred: Option<f64> = if resolved_refs.is_empty() {
                None
            } else {
                levels0
                    .iter()
                    .find(|&&head| {
                        usable
                            .iter()
                            .enumerate()
                            .filter(|(ci, _)| (inst_vals[0][*ci] - head).abs() <= LEVEL_TOL)
                            .any(|(_, c)| {
                                resolved_refs.iter().all(|&t| {
                                    normals[t]
                                        .iter()
                                        .any(|&n| (n.dot(c.v) - masts[t]).abs() < EPS_INCIDENT)
                                })
                            })
                    })
                    .copied()
            };
            st.hit_named_rank1 = hit(named_pred.or(rank1_pred));
            st.named_rank1_err = named_pred.or(rank1_pred).map(|p| (p - m).abs() / m);

            // Rule "degree": among the first 4 levels, pick the one with the highest
            // degeneracy degree; ties break toward the shallower level. Skip level 0
            // only when a deeper level strictly beats it (level 0 with max degree is
            // legitimate first-touch-at-a-meet-point).
            let degree_pred: Option<f64> = {
                let span = levels0.len().min(4);
                (0..span)
                    .max_by_key(|&li| (level_degree[li], std::cmp::Reverse(li)))
                    .map(|li| levels0[li])
            };
            st.hit_degree = hit(degree_pred);
            st.hit_named_degree = hit(named_pred.or(degree_pred));
        }

        // Realizing vertices for reachability: any usable candidate whose instance-0
        // value matches the true mast. For each, record every incident plane as
        // (tier, instance) pairs -- including tier i's own sibling instances, which
        // share the unknown mast and still help pin the vertex down. Dedup vertices
        // by position.
        for (ci, c) in usable.iter().enumerate() {
            if (inst_vals[0][ci] - m).abs() / m < MATCH_REL {
                let dup = realizing_vertices[i]
                    .iter()
                    .any(|(v, _)| (*v - c.v).abs().max_element() < 1e-6);
                if dup {
                    continue;
                }
                let mut incident: Vec<(usize, usize)> = Vec::new();
                for (t, ns) in normals.iter().enumerate() {
                    for (j, &n) in ns.iter().enumerate() {
                        if (n.dot(c.v) - masts[t]).abs() < EPS_INCIDENT {
                            incident.push((t, j));
                        }
                    }
                }
                realizing_vertices[i].push((c.v, incident));
            }
        }

        tier_stats.push(st);
    }

    // Reachability simulation: known = anchors; a tier becomes known when some
    // realizing vertex is pinned down by planes of known tiers plus the tier's own
    // instances (which share the one unknown mast). "Pinned down" = the stacked
    // system over unknowns (v, m) -- rows [n, 0] for a known tier's incident plane,
    // [n, -1] for one of tier i's own incident instances -- has rank 4.
    let mut known: Vec<bool> = is_anchor.clone();
    loop {
        let mut progress = false;
        for i in 0..nt {
            if known[i] || !tier_stats[i].scored {
                continue;
            }
            let ok = realizing_vertices[i].iter().any(|(_, incident)| {
                let mut rows: Vec<[f64; 4]> = Vec::with_capacity(incident.len());
                for &(t, j) in incident {
                    let n = normals[t][j];
                    if t == i {
                        rows.push([n.x, n.y, n.z, -1.0]);
                    } else if known[t] {
                        rows.push([n.x, n.y, n.z, 0.0]);
                    }
                }
                rank4(&mut rows) == 4
            });
            if ok {
                known[i] = true;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    let mut all_reachable = true;
    for i in 0..nt {
        if tier_stats[i].scored {
            out.any_scored = true;
            tier_stats[i].reachable = known[i];
            if !known[i] {
                all_reachable = false;
            }
        }
    }
    out.all_reachable = all_reachable && out.any_scored;

    // ------------------------------------------------------------------
    // Global-solve ceiling: with the TRUE incidence structure known (each scored
    // tier's realizing vertex and the planes through it), is the whole design's
    // mast vector uniquely determined by the anchors alone? Unknowns: one 3-vector
    // per realizing vertex plus one mast per scored tier. Rows: every incident
    // plane of every realizing vertex. Anchor masts (and near-zero masts) go to
    // the right-hand side. Solved by regularized normal equations; a rank-deficient
    // system collapses to minimum-norm and shows up as mast error.
    // ------------------------------------------------------------------
    {
        let scored: Vec<usize> = (0..nt).filter(|&i| tier_stats[i].scored).collect();
        // Best realizing vertex per scored tier (min instance-0 error).
        let best_vertex: Vec<Option<usize>> = (0..nt)
            .map(|i| {
                if !tier_stats[i].scored {
                    return None;
                }
                let n0 = normals[i][0];
                realizing_vertices[i]
                    .iter()
                    .enumerate()
                    .min_by(|(_, (va, _)), (_, (vb, _))| {
                        let ea = (n0.dot(*va) - masts[i]).abs();
                        let eb = (n0.dot(*vb) - masts[i]).abs();
                        ea.partial_cmp(&eb).unwrap()
                    })
                    .map(|(idx, _)| idx)
            })
            .collect();

        let mut mast_col: Vec<Option<usize>> = vec![None; nt];
        let mut n_unknowns = 0usize;
        for &i in &scored {
            mast_col[i] = Some(n_unknowns);
            n_unknowns += 1;
        }
        let mut vert_col: Vec<Option<usize>> = vec![None; nt];
        for &i in &scored {
            if best_vertex[i].is_some() {
                vert_col[i] = Some(n_unknowns);
                n_unknowns += 3;
            }
        }

        let mut rows: Vec<Vec<f64>> = Vec::new();
        let mut rhs: Vec<f64> = Vec::new();
        for &i in &scored {
            let Some(bv) = best_vertex[i] else { continue };
            let vc = vert_col[i].expect("vertex column allocated above");
            let (_, incident) = &realizing_vertices[i][bv];
            for &(t, j) in incident {
                let n = normals[t][j];
                let mut row = vec![0.0f64; n_unknowns];
                row[vc] = n.x;
                row[vc + 1] = n.y;
                row[vc + 2] = n.z;
                let b = if let Some(mc) = mast_col[t] {
                    row[mc] = -1.0;
                    0.0
                } else {
                    masts[t] // anchor or near-zero tier: known constant
                };
                rows.push(row);
                rhs.push(b);
            }
        }

        let sol = solve_normal_equations(&rows, &rhs, n_unknowns);
        if let Some(sol) = sol {
            for &i in &scored {
                let mc = mast_col[i].expect("scored tier has a mast column");
                let got = sol[mc];
                let rel = (got - masts[i]).abs() / masts[i];
                tier_stats[i].global_err = Some(rel);
            }
        }
    }

    // ------------------------------------------------------------------
    // Degeneracy signal: does total vertex degeneracy separate the true mast
    // vector from the solver's (possibly wrong but self-consistent) output?
    // D = sum over feasible vertices of (incident plane count - 3), computed on
    // deduplicated vertices. Also record the solver's per-design median error for
    // correlation.
    // ------------------------------------------------------------------
    {
        let mut inputs = indicatrix::geometry::meet_solver::meet_tier_inputs_from_asc(&schedule);
        for (i, anchored) in is_anchor.iter().enumerate() {
            if *anchored {
                inputs[i].constraint =
                    indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(
                        schedule.tiers[i].mast,
                    );
            }
        }
        let solved = indicatrix::geometry::meet_solver::solve_meet_points(
            schedule.gear_teeth_abs(),
            &inputs,
        );
        let solved_masts: Vec<f64> = solved.iter().map(|s| s.mast.abs()).collect();
        let mut errs: Vec<f64> = (0..nt)
            .filter(|&i| !is_anchor[i] && masts[i] > 1e-6)
            .map(|i| (solved_masts[i] - masts[i]).abs() / masts[i])
            .collect();
        errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        out.solver_median_err = errs.get(errs.len() / 2).copied();

        out.degeneracy_truth = Some(total_degeneracy(&normals, &masts));
        out.degeneracy_solved = Some(total_degeneracy(&normals, &solved_masts));
    }

    out.tiers = tier_stats;
    out
}

/// Total degeneracy of the arrangement at the given masts: enumerate feasible
/// vertices (violated-set empty), dedup by position, and sum `incident - 3` over
/// vertices, where `incident` counts planes within EPS_INCIDENT.
fn total_degeneracy(normals: &[Vec<DVec3>], masts: &[f64]) -> f64 {
    let mut planes: Vec<Plane> = [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ]
    .into_iter()
    .map(|n| Plane {
        n,
        m: BLANK,
        owner: usize::MAX,
    })
    .collect();
    for (i, ns) in normals.iter().enumerate() {
        for &n in ns {
            planes.push(Plane {
                n,
                m: masts[i],
                owner: i,
            });
        }
    }
    if planes.len() > MAX_PLANES {
        return f64::NAN;
    }
    let cands = enumerate_candidates(&planes, 6);
    let mut verts: Vec<DVec3> = Vec::new();
    for c in &cands {
        if c.violated.is_some() {
            continue;
        }
        if !verts.iter().any(|v| (*v - c.v).abs().max_element() < 1e-6) {
            verts.push(c.v);
        }
    }
    let mut d = 0.0;
    for v in &verts {
        let mut inc = 0usize;
        for (t, ns) in normals.iter().enumerate() {
            for &n in ns {
                if (n.dot(*v) - masts[t]).abs() < EPS_INCIDENT {
                    inc += 1;
                }
            }
            let _ = t;
        }
        if inc > 3 {
            d += (inc - 3) as f64;
        }
    }
    d
}

/// Numeric rank (pivot threshold 1e-9) of a set of 4-column rows, via Gaussian
/// elimination with partial pivoting. Mutates `rows` in place.
fn rank4(rows: &mut [[f64; 4]]) -> usize {
    let mut rank = 0usize;
    for col in 0..4 {
        let Some(pivot) = (rank..rows.len())
            .max_by(|&a, &b| rows[a][col].abs().partial_cmp(&rows[b][col].abs()).unwrap())
        else {
            break;
        };
        if rows[pivot][col].abs() < 1e-9 {
            continue;
        }
        rows.swap(rank, pivot);
        let head = rows[rank];
        for r in rows.iter_mut().skip(rank + 1) {
            let f = r[col] / head[col];
            for c in col..4 {
                r[c] -= f * head[c];
            }
        }
        rank += 1;
        if rank == rows.len() {
            break;
        }
    }
    rank
}

/// Least squares via regularized normal equations; returns `None` on an empty or
/// singular system.
fn solve_normal_equations(rows: &[Vec<f64>], rhs: &[f64], k: usize) -> Option<Vec<f64>> {
    if rows.is_empty() || k == 0 {
        return None;
    }
    let mut ata = vec![vec![0.0f64; k]; k];
    let mut atb = vec![0.0f64; k];
    for (row, &b) in rows.iter().zip(rhs) {
        for i in 0..k {
            if row[i] == 0.0 {
                continue;
            }
            atb[i] += row[i] * b;
            for j in 0..k {
                ata[i][j] += row[i] * row[j];
            }
        }
    }
    let scale = (0..k).map(|i| ata[i][i]).fold(1e-12, f64::max);
    let reg = 1e-10 * scale;
    for (i, row) in ata.iter_mut().enumerate() {
        row[i] += reg;
    }
    // Gaussian elimination with partial pivoting.
    let mut a = ata;
    let mut b = atb;
    for col in 0..k {
        let pivot =
            (col..k).max_by(|&x, &y| a[x][col].abs().partial_cmp(&a[y][col].abs()).unwrap())?;
        if a[pivot][col].abs() < 1e-14 * scale.max(1.0) {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for r in (col + 1)..k {
            let f = a[r][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            for c in col..k {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = vec![0.0f64; k];
    for i in (0..k).rev() {
        let mut s = b[i];
        for j in (i + 1)..k {
            s -= a[i][j] * x[j];
        }
        x[i] = s / a[i][i];
    }
    Some(x)
}

fn main() {
    let db_path = find_db_path();
    println!("Using database: {db_path}");
    let conn = Connection::open(&db_path).expect("open facet_diagrams.sqlite");
    let rows = load_asc_rows(&conn);
    let mut seen: HashSet<i64> = HashSet::new();
    let unique_rows: Vec<AscRow> = rows
        .into_iter()
        .filter(|r| seen.insert(r.detail_id))
        .collect();
    println!("{} unique designs.", unique_rows.len());

    let start = std::time::Instant::now();
    let chunk_len = unique_rows.len().div_ceil(THREADS).max(1);
    let chunks: Vec<&[AscRow]> = unique_rows.chunks(chunk_len).collect();
    let mut results: Vec<DesignResult> = Vec::with_capacity(unique_rows.len());
    std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| s.spawn(move || chunk.iter().map(analyze_one).collect::<Vec<_>>()))
            .collect();
        for h in handles {
            results.extend(h.join().expect("worker thread panicked"));
        }
    });
    println!("Analyzed in {:.2?}.", start.elapsed());

    let parsed = results.iter().filter(|d| d.parsed).count();
    let skipped = results.iter().filter(|d| d.skipped_too_big).count();
    println!("parsed={parsed} skipped_too_big={skipped}");

    let all_tiers: Vec<&TierStats> = results
        .iter()
        .flat_map(|d| d.tiers.iter())
        .filter(|t| t.scored)
        .collect();
    println!("scored meet-derived tiers: {}", all_tiers.len());

    // E3b
    let err0: Vec<f64> = all_tiers.iter().map(|t| t.err0).collect();
    let errw: Vec<f64> = all_tiers.iter().map(|t| t.err_worst).collect();
    print_dist("E3b err (instance 0)", &err0);
    print_dist("E3b err (worst instance)", &errw);
    println!(
        "E3b frac < 0.5% (inst 0): {:.1}%   (worst inst): {:.1}%",
        frac_below(&err0, MATCH_REL) * 100.0,
        frac_below(&errw, MATCH_REL) * 100.0
    );

    // Tangency
    let tang: Vec<f64> = all_tiers
        .iter()
        .filter(|t| t.tangency_ratio.is_finite())
        .map(|t| t.tangency_ratio)
        .collect();
    print_dist("tangency ratio (max cand / true)", &tang);

    // Ranks
    let matched: Vec<&TierStats> = all_tiers
        .iter()
        .copied()
        .filter(|t| t.rank.is_some())
        .collect();
    println!(
        "\ntiers with a matching level (rank defined): {}",
        matched.len()
    );
    print_rank_hist(
        "rank (inst 0)",
        &matched.iter().map(|t| t.rank.unwrap()).collect::<Vec<_>>(),
    );
    let sym_matched: Vec<usize> = all_tiers.iter().filter_map(|t| t.rank_sym).collect();
    print_rank_hist("rank_sym (symmetry-intersected)", &sym_matched);

    // Deepest-safe rule
    let with_safe: Vec<&TierStats> = matched
        .iter()
        .copied()
        .filter(|t| t.deepest_safe_rank.is_some())
        .collect();
    let safe_hits = with_safe
        .iter()
        .filter(|t| t.deepest_safe_rank == t.rank)
        .count();
    let safe_off1 = with_safe
        .iter()
        .filter(|t| {
            let (r, s) = (t.rank.unwrap(), t.deepest_safe_rank.unwrap());
            r.abs_diff(s) <= 1
        })
        .count();
    println!(
        "\ndeepest-safe rule: true==deepest_safe {}/{} ({:.1}%), within 1 level {:.1}%",
        safe_hits,
        with_safe.len(),
        pct(safe_hits, with_safe.len()),
        pct(safe_off1, with_safe.len())
    );

    // E4
    let e4_all: Vec<f64> = all_tiers.iter().filter_map(|t| t.e4_err).collect();
    let e4_full: Vec<f64> = all_tiers
        .iter()
        .filter(|t| {
            t.named_resolved
                .is_some_and(|(res, tot)| res == tot && tot > 0)
        })
        .filter_map(|t| t.e4_err)
        .collect();
    print_dist("E4 err (>=1 resolved named ref)", &e4_all);
    println!(
        "E4 frac < 0.5%: {:.1}% (n={})",
        frac_below(&e4_all, MATCH_REL) * 100.0,
        e4_all.len()
    );
    print_dist("E4 err (all names resolved)", &e4_full);
    println!(
        "E4-fully-resolved frac < 0.5%: {:.1}% (n={})",
        frac_below(&e4_full, MATCH_REL) * 100.0,
        e4_full.len()
    );
    let named_tiers = all_tiers
        .iter()
        .filter(|t| t.named_resolved.is_some())
        .count();
    let named_some = all_tiers
        .iter()
        .filter(|t| t.named_resolved.is_some_and(|(r, _)| r > 0))
        .count();
    println!("MeetNamed tiers: {named_tiers}, with >=1 exact-resolved ref: {named_some}");

    // Reachability
    let reachable = all_tiers.iter().filter(|t| t.reachable).count();
    println!(
        "\nincremental reachability ceiling: {}/{} tiers ({:.1}%)",
        reachable,
        all_tiers.len(),
        pct(reachable, all_tiers.len())
    );
    let designs_scored = results.iter().filter(|d| d.any_scored).count();
    let designs_all = results.iter().filter(|d| d.all_reachable).count();
    println!(
        "designs with every scored tier reachable: {}/{} ({:.1}%)",
        designs_all,
        designs_scored,
        pct(designs_all, designs_scored)
    );

    // Selection-rule experiment
    let n_scored = all_tiers.len();
    let hits = |f: &dyn Fn(&TierStats) -> bool| all_tiers.iter().filter(|t| f(t)).count();
    println!("\nselection rules (others at truth), hit = pred within 0.5% of true:");
    for (name, f) in [
        (
            "rank1          ",
            &(|t: &TierStats| t.hit_rank1) as &dyn Fn(&TierStats) -> bool,
        ),
        ("named->rank1   ", &|t: &TierStats| t.hit_named_rank1),
        ("degree         ", &|t: &TierStats| t.hit_degree),
        ("named->degree  ", &|t: &TierStats| t.hit_named_degree),
    ] {
        let h = hits(f);
        println!("  {name}: {h}/{n_scored} ({:.1}%)", pct(h, n_scored));
    }
    let nr_err: Vec<f64> = all_tiers.iter().filter_map(|t| t.named_rank1_err).collect();
    print_dist("  named->rank1 pred rel err", &nr_err);
    println!(
        "  named->rank1 err <=10%: {:.1}%",
        frac_below(&nr_err, 0.10) * 100.0
    );
    // Per-design: every scored tier within 10% / hit, under named->rank1.
    let mut d_hit = 0usize;
    let mut d_10 = 0usize;
    let mut d_tot = 0usize;
    for d in &results {
        let scored: Vec<&TierStats> = d.tiers.iter().filter(|t| t.scored).collect();
        if scored.is_empty() {
            continue;
        }
        d_tot += 1;
        if scored.iter().all(|t| t.hit_named_rank1) {
            d_hit += 1;
        }
        if scored
            .iter()
            .all(|t| t.named_rank1_err.is_some_and(|e| e <= 0.10))
        {
            d_10 += 1;
        }
    }
    println!(
        "  designs all-hit under named->rank1: {d_hit}/{d_tot} ({:.1}%); all within 10%: {d_10} ({:.1}%)",
        pct(d_hit, d_tot),
        pct(d_10, d_tot)
    );

    // Global-solve ceiling
    let gerr: Vec<f64> = all_tiers.iter().filter_map(|t| t.global_err).collect();
    print_dist("\nglobal true-incidence solve err", &gerr);
    println!(
        "global-solve frac < 0.5%: {:.1}%  < 5%: {:.1}%  (n={}, of {} scored)",
        frac_below(&gerr, MATCH_REL) * 100.0,
        frac_below(&gerr, 0.05) * 100.0,
        gerr.len(),
        all_tiers.len()
    );
    // Per-design: every scored tier under 10% via the global solve.
    let mut designs_global_ok = 0usize;
    let mut designs_global_tot = 0usize;
    for d in &results {
        let scored: Vec<&TierStats> = d.tiers.iter().filter(|t| t.scored).collect();
        if scored.is_empty() {
            continue;
        }
        designs_global_tot += 1;
        if scored
            .iter()
            .all(|t| t.global_err.is_some_and(|e| e <= 0.10))
        {
            designs_global_ok += 1;
        }
    }
    println!(
        "designs with every scored tier within 10% via global solve: {}/{} ({:.1}%)",
        designs_global_ok,
        designs_global_tot,
        pct(designs_global_ok, designs_global_tot)
    );

    // Degeneracy signal
    let mut ratios: Vec<f64> = Vec::new();
    let mut sep_ok = 0usize;
    let mut sep_tot = 0usize;
    for d in &results {
        let (Some(dt), Some(ds), Some(err)) =
            (d.degeneracy_truth, d.degeneracy_solved, d.solver_median_err)
        else {
            continue;
        };
        if !dt.is_finite() || !ds.is_finite() {
            continue;
        }
        // Only designs where the solver is meaningfully wrong are informative.
        if err > 0.05 {
            sep_tot += 1;
            if dt > ds {
                sep_ok += 1;
            }
            ratios.push(if dt > 0.0 { ds / dt } else { f64::NAN });
        }
    }
    print_dist(
        "\nD(solved)/D(truth) on designs where solver median err > 5%",
        &ratios,
    );
    println!(
        "degeneracy separates (D(truth) > D(solved)) on {}/{} such designs ({:.1}%)",
        sep_ok,
        sep_tot,
        pct(sep_ok, sep_tot)
    );
}

fn print_dist(label: &str, vals: &[f64]) {
    let mut v: Vec<f64> = vals.iter().copied().filter(|x| x.is_finite()).collect();
    let inf = vals.len() - v.len();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        println!("{label}: no data");
        return;
    }
    let at = |p: f64| v[((v.len() as f64 * p) as usize).min(v.len() - 1)];
    println!(
        "{label}: n={} (+{inf} non-finite) p10={:.4} p25={:.4} med={:.4} p75={:.4} p90={:.4}",
        v.len(),
        at(0.10),
        at(0.25),
        at(0.50),
        at(0.75),
        at(0.90)
    );
}

fn print_rank_hist(label: &str, ranks: &[usize]) {
    let n = ranks.len();
    if n == 0 {
        println!("{label}: no data");
        return;
    }
    let count = |pred: &dyn Fn(usize) -> bool| ranks.iter().filter(|&&r| pred(r)).count();
    let buckets: [(&str, Box<dyn Fn(usize) -> bool>); 7] = [
        ("0", Box::new(|r| r == 0)),
        ("1", Box::new(|r| r == 1)),
        ("2", Box::new(|r| r == 2)),
        ("3", Box::new(|r| r == 3)),
        ("4", Box::new(|r| r == 4)),
        ("5", Box::new(|r| r == 5)),
        (">5", Box::new(|r| r > 5)),
    ];
    print!("{label} (n={n}): ");
    for (name, pred) in &buckets {
        let c = count(pred.as_ref());
        print!("{name}:{c}({:.1}%) ", pct(c, n));
    }
    println!();
}

fn frac_below(vals: &[f64], thresh: f64) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    vals.iter().filter(|&&v| v < thresh).count() as f64 / vals.len() as f64
}

fn pct(n: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * n as f64 / total as f64
    }
}

fn load_asc_rows(conn: &Connection) -> Vec<AscRow> {
    let mut stmt = conn
        .prepare("SELECT detail_id, content FROM attached_files WHERE name LIKE '%.asc' ORDER BY detail_id, id")
        .expect("prepare attached_files query");
    let rows = stmt
        .query_map([], |row| {
            Ok(AscRow {
                detail_id: row.get(0)?,
                content: row.get(1)?,
            })
        })
        .expect("query attached_files");
    rows.filter_map(Result::ok).collect()
}

fn find_db_path() -> String {
    for candidate in [
        "facet_diagrams.sqlite",
        "../../facet_diagrams.sqlite",
        "../facet_diagrams.sqlite",
    ] {
        if std::path::Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "facet_diagrams.sqlite".to_string()
}
