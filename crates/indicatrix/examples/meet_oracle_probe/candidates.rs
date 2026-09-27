//! Candidate-vertex enumeration for `analyze_one`: per-tier facet normals from an
//! `.asc` schedule, the triple-intersection candidate-vertex enumeration over the
//! real plane arrangement, and grouping a descending value list into distinct
//! levels.

use glam::{DMat3, DVec3};
use indicatrix_formats::asc::AscSchedule;

use crate::types::{BLANK, Cand, EPS_FEAS, LEVEL_TOL, MIN_DET, Plane};

pub fn side_and_normals(schedule: &AscSchedule) -> (Vec<bool>, Vec<Vec<DVec3>>) {
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
pub fn enumerate_candidates(planes: &[Plane], n_real_start: usize) -> Vec<Cand> {
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
pub fn group_levels(mut vals: Vec<f64>) -> Vec<f64> {
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
