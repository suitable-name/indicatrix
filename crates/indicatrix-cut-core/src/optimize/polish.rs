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
/// # The cap is hard
///
/// `evaluate` is never called more than `max_evaluations` times, so the stage's cost is
/// exactly what [`super::inclusive_max_evaluations_for`] budgets for it. The initial
/// simplex stops early when the cap is below the dimension; an expansion that cannot be
/// paid leaves the reflected point in; a contraction, or a shrink (which re-scores every
/// vertex but the best), that cannot be paid ends the loop with the simplex as it was.
/// None of this changes a run whose cap is not reached.
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
    let mut evaluations = 0usize;
    let mut vertices = initial_simplex(
        starting_point,
        starting_score,
        start_step,
        max_evaluations,
        &mut evaluate,
        &mut evaluations,
    );

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
        if !iterate(
            &mut vertices,
            &mut evaluate,
            &mut evaluations,
            max_evaluations,
        ) {
            break false;
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

/// The starting simplex of [`run_polish`]: `starting_point` itself plus one vertex per
/// axis, each scored by one call to `evaluate`. Stops adding vertices once `evaluations`
/// reaches `max_evaluations`.
fn initial_simplex(
    starting_point: &[f64],
    starting_score: f32,
    start_step: f64,
    max_evaluations: usize,
    evaluate: &mut impl FnMut(&[f64]) -> f32,
    evaluations: &mut usize,
) -> Vec<Vertex> {
    let dims = starting_point.len();
    let mut vertices: Vec<Vertex> = Vec::with_capacity(dims + 1);
    vertices.push(Vertex {
        point: starting_point.to_vec(),
        score: starting_score,
    });
    for axis in 0..dims {
        if *evaluations >= max_evaluations {
            break;
        }
        let mut point = starting_point.to_vec();
        let delta = if axis % 2 == 0 {
            start_step
        } else {
            -start_step
        };
        point[axis] += delta;
        let score = evaluate(&point);
        *evaluations += 1;
        vertices.push(Vertex { point, score });
    }
    vertices
}

/// One reflect / expand / contract / shrink step of [`run_polish`] on the simplex sorted
/// best first. `false` when the budget cannot pay for the move the step needs; the
/// simplex is then as it was and the caller stops.
fn iterate(
    vertices: &mut [Vertex],
    evaluate: &mut impl FnMut(&[f64]) -> f32,
    evaluations: &mut usize,
    max_evaluations: usize,
) -> bool {
    let worst = vertices.len() - 1;
    let best_score = vertices[0].score;
    let second_worst_score = vertices[worst - 1].score;
    let centroid = centroid_excluding(vertices, worst);

    let reflected_point = extrapolate(&centroid, &vertices[worst].point, -REFLECTION);
    let reflected_score = evaluate(&reflected_point);
    *evaluations += 1;
    let reflected = Vertex {
        point: reflected_point,
        score: reflected_score,
    };

    if reflected_score < best_score {
        let expanded = if *evaluations < max_evaluations {
            let point = extrapolate(&centroid, &reflected.point, EXPANSION);
            let score = evaluate(&point);
            *evaluations += 1;
            (score < reflected_score).then_some(Vertex { point, score })
        } else {
            None
        };
        vertices[worst] = expanded.unwrap_or(reflected);
        true
    } else if reflected_score < second_worst_score {
        vertices[worst] = reflected;
        true
    } else {
        contract(
            vertices,
            &centroid,
            &reflected,
            evaluate,
            evaluations,
            max_evaluations,
        )
    }
}

/// The contraction of [`iterate`], for a reflected point that is no better than the second
/// worst vertex: toward the reflected point when it still beats the worst vertex (outside),
/// toward the worst vertex otherwise (inside). A contraction that does not help shrinks the
/// simplex. `false` when the budget cannot pay for it.
fn contract(
    vertices: &mut [Vertex],
    centroid: &[f64],
    reflected: &Vertex,
    evaluate: &mut impl FnMut(&[f64]) -> f32,
    evaluations: &mut usize,
    max_evaluations: usize,
) -> bool {
    if *evaluations >= max_evaluations {
        return false;
    }
    let worst = vertices.len() - 1;
    let worst_score = vertices[worst].score;
    let outside = reflected.score < worst_score;
    let toward = if outside {
        &reflected.point
    } else {
        &vertices[worst].point
    };
    let point = extrapolate(centroid, toward, CONTRACTION);
    let score = evaluate(&point);
    *evaluations += 1;
    let helps = if outside {
        score <= reflected.score
    } else {
        score < worst_score
    };
    if helps {
        vertices[worst] = Vertex { point, score };
        true
    } else {
        shrink(vertices, evaluate, evaluations, max_evaluations)
    }
}

/// `base + coeff * (other - base)`, per dimension -- the single formula behind
/// reflection, expansion, and both contraction variants ([`iterate`], [`contract`]) and
/// the shrink ([`shrink`]) (only `base`, `other`, and `coeff` differ between the call
/// sites).
fn extrapolate(base: &[f64], other: &[f64], coeff: f64) -> Vec<f64> {
    base.iter()
        .zip(other)
        .map(|(&b, &o)| coeff.mul_add(o - b, b))
        .collect()
}

/// The mean of every vertex in `vertices` except index `exclude` -- always the
/// worst vertex at [`iterate`]'s call site.
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
/// operation in [`run_polish`] that touches every non-best vertex at once. `false`,
/// with the simplex untouched, when `max_evaluations` cannot pay for all of them.
fn shrink(
    vertices: &mut [Vertex],
    evaluate: &mut impl FnMut(&[f64]) -> f32,
    evaluations: &mut usize,
    max_evaluations: usize,
) -> bool {
    if *evaluations + (vertices.len() - 1) > max_evaluations {
        return false;
    }
    let best_point = vertices[0].point.clone();
    for vertex in vertices.iter_mut().skip(1) {
        vertex.point = extrapolate(&best_point, &vertex.point, SHRINK);
        vertex.score = evaluate(&vertex.point);
        *evaluations += 1;
    }
    true
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
