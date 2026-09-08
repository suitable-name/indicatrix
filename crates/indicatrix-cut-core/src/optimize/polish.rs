//! The polish stage's algorithm itself: a deterministic Nelder-Mead downhill
//! simplex, generic over the point-scoring function so it can be exercised in
//! `tests.rs` against a synthetic objective with no [`crate::design::Design`]
//! involved at all. See the parent module's "Coordinate descent stalls on diagonal
//! ridges" section for why this hand-off from the coordinate stage exists: paired
//! angles that must move together to hold a critical-angle relation form a ridge no
//! axis-aligned move can climb, and a gradient-free simplex that can move every
//! free angle at once can.
//!
//! [`super::search::run_search`]'s caller wires the real,
//! [`crate::design::Design`]-backed scoring closure (built from
//! [`super::candidate::build_free_angle_candidate`] and
//! [`super::candidate::evaluate_candidate`]) -- this module never imports
//! `Design`/`GemMaterial` itself, which is what keeps [`run_polish`] directly
//! testable against a plain synthetic function.

/// Standard Nelder-Mead coefficients (Nelder & Mead, 1965) -- algorithmic constants,
/// not user-tunable search-budget knobs like [`super::search::OptimizeConfig`]'s
/// fields.
const REFLECTION: f64 = 1.0;
const EXPANSION: f64 = 2.0;
const CONTRACTION: f64 = 0.5;
const SHRINK: f64 = 0.5;

/// One simplex vertex: a point in the free-angle vector space plus its score.
/// `f32::INFINITY` marks a point the caller's scoring closure rejected (unsolvable,
/// non-closed, a manufacturability regression, or out of bounds) -- see the module
/// doc comment.
#[derive(Clone)]
struct Vertex {
    point: Vec<f64>,
    score: f32,
}

/// What [`run_polish`] found: the best point in the simplex when it stopped, that
/// point's score, how many times the scoring closure was called, and whether
/// cancellation cut the search short. The caller decides whether `score` actually
/// improves on the coordinate stage's own result before adopting it -- this type
/// makes no such judgment itself.
pub(super) struct PolishResult {
    pub(super) point: Vec<f64>,
    pub(super) score: f32,
    pub(super) evaluations: usize,
    pub(super) cancelled: bool,
}

/// Deterministic Nelder-Mead downhill simplex, minimizing `evaluate` over
/// `starting_point`'s own dimensionality.
///
/// # Initial simplex
///
/// Vertex 0 is `starting_point` itself, scored `starting_score` (the caller already
/// has this -- e.g. the coordinate stage's own ending score -- so it costs no extra
/// call to `evaluate`). Vertex `i` (one per dimension) is `starting_point` with that
/// one axis shifted by `+start_step` on an even axis index and `-start_step` on an
/// odd one -- deterministic and needing no bounds pre-check: a shift that lands
/// out of bounds simply scores `f32::INFINITY` via `evaluate`, exactly like any
/// other rejected point during the main loop (see the module doc comment).
///
/// # Loop
///
/// Standard reflect / expand / contract / shrink Nelder-Mead, using
/// [`REFLECTION`]/[`EXPANSION`]/[`CONTRACTION`]/[`SHRINK`]. Stops when: the
/// simplex's largest single-axis spread (see [`simplex_spread`]) falls below
/// `min_spread`; `evaluations` reaches `max_evaluations`; or `is_cancelled` returns
/// `true` (polled once per iteration, the same per-decision cadence
/// [`super::search::SearchHooks`] uses in the coordinate stage).
///
/// # Determinism
///
/// No randomness anywhere: for a deterministic `evaluate` (a pure function of its
/// argument), identical inputs always produce the identical sequence of simplex
/// states and the identical [`PolishResult`].
pub(super) fn run_polish(
    starting_point: &[f64],
    starting_score: f32,
    start_step: f64,
    max_evaluations: usize,
    min_spread: f64,
    is_cancelled: &dyn Fn() -> bool,
    mut evaluate: impl FnMut(&[f64]) -> f32,
) -> PolishResult {
    let dims = starting_point.len();
    let mut evaluations = 0usize;

    let mut vertices: Vec<Vertex> = Vec::with_capacity(dims + 1);
    vertices.push(Vertex {
        point: starting_point.to_vec(),
        score: starting_score,
    });
    for axis in 0..dims {
        let mut point = starting_point.to_vec();
        let delta = if axis % 2 == 0 {
            start_step
        } else {
            -start_step
        };
        point[axis] += delta;
        let score = evaluate(&point);
        evaluations += 1;
        vertices.push(Vertex { point, score });
    }

    let cancelled = loop {
        if is_cancelled() {
            break true;
        }
        if evaluations >= max_evaluations {
            break false;
        }
        vertices.sort_by(|a, b| a.score.total_cmp(&b.score));
        if simplex_spread(&vertices) < min_spread {
            break false;
        }

        let worst = vertices.len() - 1;
        let best_score = vertices[0].score;
        let second_worst_score = vertices[worst - 1].score;
        let worst_score = vertices[worst].score;
        let centroid = centroid_excluding(&vertices, worst);

        let reflected_point = extrapolate(&centroid, &vertices[worst].point, -REFLECTION);
        let reflected_score = evaluate(&reflected_point);
        evaluations += 1;

        if reflected_score < best_score {
            let expanded_point = extrapolate(&centroid, &reflected_point, EXPANSION);
            let expanded_score = evaluate(&expanded_point);
            evaluations += 1;
            vertices[worst] = if expanded_score < reflected_score {
                Vertex {
                    point: expanded_point,
                    score: expanded_score,
                }
            } else {
                Vertex {
                    point: reflected_point,
                    score: reflected_score,
                }
            };
        } else if reflected_score < second_worst_score {
            vertices[worst] = Vertex {
                point: reflected_point,
                score: reflected_score,
            };
        } else if reflected_score < worst_score {
            let contracted_point = extrapolate(&centroid, &reflected_point, CONTRACTION);
            let contracted_score = evaluate(&contracted_point);
            evaluations += 1;
            if contracted_score <= reflected_score {
                vertices[worst] = Vertex {
                    point: contracted_point,
                    score: contracted_score,
                };
            } else {
                shrink(&mut vertices, &mut evaluate, &mut evaluations);
            }
        } else {
            let contracted_point = extrapolate(&centroid, &vertices[worst].point, CONTRACTION);
            let contracted_score = evaluate(&contracted_point);
            evaluations += 1;
            if contracted_score < worst_score {
                vertices[worst] = Vertex {
                    point: contracted_point,
                    score: contracted_score,
                };
            } else {
                shrink(&mut vertices, &mut evaluate, &mut evaluations);
            }
        }
    };

    vertices.sort_by(|a, b| a.score.total_cmp(&b.score));
    let best = vertices
        .into_iter()
        .next()
        .expect("a simplex always has at least one vertex (starting_point itself)");
    PolishResult {
        point: best.point,
        score: best.score,
        evaluations,
        cancelled,
    }
}

/// `base + coeff * (other - base)`, per dimension -- the single formula behind
/// reflection, expansion, and both contraction variants in [`run_polish`] (only
/// `base`, `other`, and `coeff` differ between its call sites).
fn extrapolate(base: &[f64], other: &[f64], coeff: f64) -> Vec<f64> {
    base.iter()
        .zip(other)
        .map(|(&b, &o)| coeff.mul_add(o - b, b))
        .collect()
}

/// The mean of every vertex in `vertices` except index `exclude` -- always the
/// worst vertex at [`run_polish`]'s own call site.
fn centroid_excluding(vertices: &[Vertex], exclude: usize) -> Vec<f64> {
    let dims = vertices[0].point.len();
    let count = (vertices.len() - 1) as f64;
    let mut centroid = vec![0.0f64; dims];
    for (i, vertex) in vertices.iter().enumerate() {
        if i == exclude {
            continue;
        }
        for (c, &p) in centroid.iter_mut().zip(&vertex.point) {
            *c += p;
        }
    }
    for c in &mut centroid {
        *c /= count;
    }
    centroid
}

/// Moves every vertex except the best (vertices are kept sorted, so that is always
/// index 0) halfway toward it, re-scoring each -- Nelder-Mead's shrink step, the one
/// operation in [`run_polish`] that touches every non-best vertex at once.
fn shrink(
    vertices: &mut [Vertex],
    evaluate: &mut impl FnMut(&[f64]) -> f32,
    evaluations: &mut usize,
) {
    let best_point = vertices[0].point.clone();
    for vertex in vertices.iter_mut().skip(1) {
        vertex.point = extrapolate(&best_point, &vertex.point, SHRINK);
        vertex.score = evaluate(&vertex.point);
        *evaluations += 1;
    }
}

/// The largest single-axis range (max - min across all vertices, over all axes) in
/// `vertices` -- [`run_polish`]'s termination measure (the parent module's finding
/// note calls this the simplex's "max angle spread").
fn simplex_spread(vertices: &[Vertex]) -> f64 {
    let Some(dims) = vertices.first().map(|v| v.point.len()) else {
        return 0.0;
    };
    let mut spread = 0.0f64;
    for d in 0..dims {
        let mut max = f64::MIN;
        let mut min = f64::MAX;
        for vertex in vertices {
            let v = vertex.point[d];
            max = max.max(v);
            min = min.min(v);
        }
        spread = spread.max(max - min);
    }
    spread
}
