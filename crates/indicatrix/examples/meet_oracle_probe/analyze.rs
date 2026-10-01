//! The core per-design oracle measurement: for every tier, pins every other tier at
//! its real recorded mast and asks whether the tier's own recorded mast is realized
//! as a vertex of the arrangement of the other tiers' planes, plus the incremental-
//! and global-solve ceiling experiments described in this example's crate doc.

use glam::DVec3;
use indicatrix_formats::asc::{self, MeetInstruction};

use crate::{
    candidates::{enumerate_candidates, group_levels, side_and_normals},
    linalg::{rank4, solve_normal_equations, total_degeneracy},
    types::{
        AscRow, BLANK, Cand, DesignResult, EPS_INCIDENT, LEVEL_TOL, MATCH_REL, MAX_PLANES, Plane,
        TierStats,
    },
};

/// Runs the full oracle analysis for one design.
#[allow(
    clippy::too_many_lines,
    reason = "straight-line per-design analysis in a probe; splitting it would scatter the pipeline"
)]
pub fn analyze_one(row: &AscRow) -> DesignResult {
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
    #[allow(
        clippy::type_complexity,
        reason = "one-off nested tuple for a local accumulator; a named type adds nothing"
    )]
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
