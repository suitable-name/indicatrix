//! Constrained extremisation of the reported colours: the verified metamer spread (round 5,
//! ZC-4).
//!
//! # What the spread is
//!
//! The spread is a profile-likelihood statement about the colour of a zone. With the data
//! chi-square `chi2(x)` of the photos, the cost of the gain prior `G(x)` (calibration knowledge
//! about the exposure of each view) and the model parameters and gains `x`, define
//!
//! ```text
//! C(x) = chi2(x) + 2 G(x),      F = { x in bounds : C(x) <= C(x_data) + Delta },
//! ```
//!
//! where `x_data` minimises `C` WITHOUT the priors of the spectral model (smoothness, magnitude,
//! L1, zone pull: they are what hides the null space of the camera data) and `Delta` is 1, or
//! the Birge ratio when the noise model was too small. For every `(zone, size, illuminant)` and
//! every one of `+L*, -L*, +a*, -a*, +b*, -b*` the objective `f` is maximised over `F`
//! (`Delta = 1` is then the 68 % interval of that single colour component), and once more for
//! the CIEDE2000 distance from the fit. The spread reported is the largest CIEDE2000 from the
//! fitted colour among all end points.
//!
//! `x_data` is the optimum of the HUBER cost (the fit's loss) while the constraint uses the
//! plain chi-square. With outliers the plain chi-square is not minimal at `x_data`, so the
//! feasible set `F` is then a superset of the `Delta` level set about the true minimum of `C`:
//! the spread errs on the wide side, never on the narrow one.
//!
//! The first version walked an ellipsoid around the fit. That is wrong for three reasons: the
//! fit is not a stationary point of the data term (its gradient is balanced by the prior), so
//! `{chi2 <= chi2(fit) + 1}` does not contain the data optimum where the truth sits; a cap of 2
//! in a log-amplitude cannot reach "no absorption" from the fitted floor; and six eigen
//! directions do not span the direction from the fit to the truth.
//!
//! # Algorithm
//!
//! Every pixel pass costs about a second at full size, so the search runs on a quadratic
//! surrogate of the constraint and only the end points are checked against the true `C`.
//!
//! * **Search space.** A model may offer a parametrisation in which the data term is closer to a
//!   quadratic (`FitModel::search_space`): for the smooth basis the linear amplitudes `a`, in
//!   which the absorption is linear, "no absorption" is the box bound `a = 0`, and `exp(-tau)`
//!   with `tau` linear in `a` is nearly quadratic for `tau` below about 0.3. In the logarithm
//!   the same move is eight units long and the Gauss-Newton curvature there vanishes.
//! * **Surrogate.** With the Gauss-Newton matrix `H` and gradient `g` of the cost (half the
//!   chi-square) at a centre `c`, `T(x) = e + 2 g^T (x - c) + (x - c)^T H (x - c)`, where `e` is
//!   the true excess `C(c) - C(x_data)`. At the data optimum `e = 0` and `g = 0` on the free
//!   parameters, and `T <= Delta` is the ellipsoid `Delta chi2 <= Delta`.
//! * **Climb.** From a point inside the surrogate set: linearise `f` there (central differences
//!   in the parameters of the zone), solve `max d^T delta` over `{T(x + delta) <= Delta} cap
//!   box` exactly, and take the best of `t delta` for `t = 1, 1/2, ...` by the true `f`. The set
//!   is convex, so every `t` stays inside. Repeat. The subproblem is solved through its dual:
//!   for a multiplier `lambda` the minimiser of `lambda (delta^T H delta + 2 g^T delta) - d^T
//!   delta` over the box is found by projected Newton steps (`box_qp`), and `lambda` is
//!   bisected (on a logarithmic scale) until `T` just meets `Delta`.
//! * **Starts.** The data optimum, the fit, the structured starts of the fit and the semi-axes
//!   of the data-metric ellipsoid about the fit (`ExtremeInput::extra_starts`, round 6, R3: they
//!   lie along the weakest data directions, where the metamers are; the other starts all sit in
//!   one basin), each pulled along the line to the data optimum until it meets the surrogate
//!   set; the best by `f` wins.
//! * **Verification and relinearisation.** The end point is checked with the nonlinear `C` and
//!   the non-negativity of the absorption on a 5 nm grid; if it fails, it is pulled back along
//!   the segment from the last verified point by bisection. Then the surrogate is rebuilt at
//!   the verified point (one Gauss-Newton pass, which also gives its true excess) and the climb
//!   repeats, twice (`ROUNDS` = 3). Every returned point is verified.
//!
//! The tasks (one per objective) are independent and gathered in task order, so the result does
//! not depend on the thread count.

use std::sync::atomic::{AtomicBool, Ordering};

use indicatrix::color::body_color::{Illuminant, delta_e_2000};

use super::{
    FitConfig, FitError,
    lm::{LmSettings, bounds, run_lm},
    models::{FitModel, PriorContext},
    par::{inner_threads, par_map_progress},
    predict::{face_up_path_mm, zone_lab},
    problem::Problem,
};

/// Climbing iterations (linearise, solve, line search) per surrogate.
const CLIMB_ITERATIONS: usize = 6;
/// Halvings of the step in the line search.
const LINE_SEARCH_HALVINGS: usize = 6;
/// Bisection steps on the logarithm of the multiplier.
const DUAL_STEPS: usize = 40;
/// The range of `log10(lambda)` of the dual bisection.
const LOG_LAMBDA_MIN: f64 = -14.0;
const LOG_LAMBDA_MAX: f64 = 14.0;
/// Bisection steps of the pull-back to the verified set (resolution 1/64 of the step).
const PULL_BACK_STEPS: usize = 6;
/// Further bisection steps between the last feasible and infeasible fractions of the pull-back
/// (total resolution 1/1024 of the step).
const PULL_BACK_REFINE_STEPS: usize = 4;
/// Verification rounds: the surrogate is rebuilt after every round except the last (round 6,
/// R3: three, one more relinearisation at the verified point, because the surrogate of a
/// saturated stone is built far outside its validity).
const ROUNDS: usize = 3;
/// Iterations of the projected Newton method of `box_qp`.
const BOX_QP_ITERATIONS: usize = 50;
/// The share of the search's progress taken by the data-only optimum.
const OPTIMUM_SHARE: f32 = 0.2;

/// Everything the search needs.
pub(super) struct ExtremeInput<'a> {
    pub problem: &'a Problem<'a>,
    /// The fitted model.
    pub model: &'a dyn FitModel,
    /// `[parameters, log-gains]` of the fit.
    pub fit: &'a [f64],
    /// `Delta`: 1, or the Birge ratio above its threshold.
    pub delta: f64,
    /// Extra starting points, `[parameters, log-gains]` in the fitted model's own space: the
    /// candidates along the data-metric ellipsoid semi-axes. Each is mapped to the search space
    /// and pulled onto the surrogate surface like the other starts.
    pub extra_starts: &'a [Vec<f64>],
    pub config: &'a FitConfig,
    pub cancel: &'a AtomicBool,
}

/// The quadratic surrogate `T` of the constraint around `centre`.
struct Surrogate {
    centre: Vec<f64>,
    /// The true `C(centre) - C(x_data)`.
    excess: f64,
    /// Gradient of the cost at the centre.
    g: Vec<f64>,
    /// Gauss-Newton matrix of the cost (row-major `n x n`).
    h: Vec<f64>,
}

impl Surrogate {
    fn value(&self, y: &[f64]) -> f64 {
        let n = y.len();
        let d: Vec<f64> = (0..n).map(|i| y[i] - self.centre[i]).collect();
        let (mut lin, mut quad) = (0.0, 0.0);
        for i in 0..n {
            lin += self.g[i] * d[i];
            let mut row = 0.0;
            for j in 0..n {
                row += self.h[i * n + j] * d[j];
            }
            quad += d[i] * row;
        }
        self.excess + 2.0 * lin + quad
    }

    /// The gradient of `T / 2` at `y`: `g + H (y - centre)`.
    fn half_gradient(&self, y: &[f64]) -> Vec<f64> {
        let n = y.len();
        (0..n)
            .map(|i| {
                let mut row = self.g[i];
                for j in 0..n {
                    row += self.h[i * n + j] * (y[j] - self.centre[j]);
                }
                row
            })
            .collect()
    }
}

/// What is extremised.
#[derive(Clone, Copy)]
enum Kind {
    /// `sign * Lab[component]`.
    Lab { component: usize, sign: f64 },
    /// The CIEDE2000 distance from a reference colour.
    FromFit { reference: [f64; 3] },
}

/// One reported colour: a zone, a path and an illuminant.
#[derive(Clone, Copy)]
struct Group {
    zone: usize,
    path_mm: f64,
    illuminant: Illuminant,
    /// The colour of the fit.
    fit_lab: [f64; 3],
}

struct Objective {
    group: Group,
    kind: Kind,
}

impl Objective {
    const fn directional(group: Group, index: usize) -> Self {
        Self {
            group,
            kind: Kind::Lab {
                component: index / 2,
                sign: if index & 1 == 0 { 1.0 } else { -1.0 },
            },
        }
    }

    /// The objective at the model parameters `params` (no gains) of `search`. Non-finite values
    /// count as the worst.
    fn value(&self, search: &dyn FitModel, params: &[f64]) -> f64 {
        let lab = zone_lab(
            search.as_spectral(),
            self.group.zone,
            params,
            self.group.path_mm,
            self.group.illuminant,
        );
        let v = match self.kind {
            Kind::Lab { component, sign } => sign * lab[component],
            Kind::FromFit { reference } => delta_e_2000(reference, lab),
        };
        if v.is_finite() { v } else { f64::NEG_INFINITY }
    }

    /// The gradient with respect to the search-space parameters of the objective's zone (central
    /// differences clamped at the bounds); zero for everything else, gains included.
    fn gradient(&self, search: &dyn FitModel, y: &[f64], lo: &[f64], hi: &[f64]) -> Vec<f64> {
        let np = search.n_params();
        let per = search.per_zone();
        let step = search.fd_step();
        let mut out = vec![0.0; y.len()];
        let mut moved = y[..np].to_vec();
        for p in self.group.zone * per..(self.group.zone + 1) * per {
            let up = (y[p] + step).min(hi[p]);
            let down = (y[p] - step).max(lo[p]);
            if up - down < 1e-12 {
                continue;
            }
            moved[p] = up;
            let f_up = self.value(search, &moved);
            moved[p] = down;
            let f_down = self.value(search, &moved);
            moved[p] = y[p];
            let slope = (f_up - f_down) / (up - down);
            if slope.is_finite() {
                out[p] = slope;
            }
        }
        out
    }
}

/// The read-only state shared by all tasks.
struct Env<'a> {
    problem: &'a Problem<'a>,
    np: usize,
    lo: Vec<f64>,
    hi: Vec<f64>,
    active: Vec<bool>,
    ctx: PriorContext<'a>,
    /// The data optimum without model priors, in search space.
    y0: Vec<f64>,
    /// `C(x_data)`.
    c0: f64,
    delta: f64,
    /// The surrogate around `y0`.
    s0: Surrogate,
    cancel: &'a AtomicBool,
}

impl Env<'_> {
    fn check_cancel(&self) -> Result<(), FitError> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(FitError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Whether the absorption of every zone is non-negative on a 5 nm grid from 400 to 720 nm.
fn physical(search: &dyn FitModel, params: &[f64]) -> bool {
    (0..search.zone_count()).all(|zone| {
        (0..=64).all(|i| search.alpha(zone, 400.0 + 5.0 * f64::from(i), params) >= -1e-9)
    })
}

/// Whether `y` satisfies the nonlinear constraint and the absorption is physical.
fn feasible(env: &Env<'_>, search: &dyn FitModel, y: &[f64]) -> Result<bool, FitError> {
    if !physical(search, &y[..env.np]) {
        return Ok(false);
    }
    Ok(env.problem.constraint_value(search, y)? <= env.c0 + env.delta)
}

/// The point of the segment `ok -> bad` with the largest bisected fraction that is feasible
/// (`ok` itself when none is). In the first round `ok` is the data optimum, so an overshooting
/// climb is scaled back along the direction it chose; [`PULL_BACK_STEPS`] bisections locate the
/// feasible fraction to 1/64 and [`PULL_BACK_REFINE_STEPS`] more refine the bracket between the
/// last feasible and infeasible fractions to 1/1024, so a weak direction is not given up to the
/// start.
fn pull_back(
    env: &Env<'_>,
    search: &dyn FitModel,
    ok: &[f64],
    bad: &[f64],
) -> Result<Vec<f64>, FitError> {
    let mut best = ok.to_vec();
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    for _ in 0..PULL_BACK_STEPS + PULL_BACK_REFINE_STEPS {
        env.check_cancel()?;
        let middle = f64::midpoint(low, high);
        let point: Vec<f64> = ok
            .iter()
            .zip(bad)
            .map(|(a, b)| a + middle * (b - a))
            .collect();
        if feasible(env, search, &point)? {
            low = middle;
            best = point;
        } else {
            high = middle;
        }
    }
    Ok(best)
}

/// The point `centre + t (start - centre)` with the largest `t` in `[0, 1]` that satisfies
/// `T <= limit` (the root of the quadratic along the ray).
fn pull_into(s: &Surrogate, start: &[f64], limit: f64) -> Vec<f64> {
    let n = start.len();
    let d: Vec<f64> = (0..n).map(|i| start[i] - s.centre[i]).collect();
    let (mut a, mut b) = (0.0, 0.0);
    for i in 0..n {
        b += 2.0 * s.g[i] * d[i];
        let mut row = 0.0;
        for j in 0..n {
            row += s.h[i * n + j] * d[j];
        }
        a += d[i] * row;
    }
    let c = s.excess - limit;
    let t = if a <= 1e-300 {
        if b > 0.0 { -c / b } else { 1.0 }
    } else {
        let discriminant = (b * b - 4.0 * a * c).max(0.0);
        (-b + discriminant.sqrt()) / (2.0 * a)
    };
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (0..n).map(|i| s.centre[i] + t * d[i]).collect()
}

/// Minimises `1/2 x^T A x + b^T x` over the box `l <= x <= u` (with `l <= 0 <= u`) by projected
/// Newton steps from `x = 0`: the variables held at a bound by the gradient are fixed, the rest
/// take a (slightly ridged) Newton step that is clamped to the box and halved until the
/// objective falls.
fn box_qp(a: &[f64], b: &[f64], l: &[f64], u: &[f64]) -> Vec<f64> {
    let n = b.len();
    let objective = |x: &[f64]| -> f64 {
        let mut value = 0.0;
        for i in 0..n {
            let mut row = 0.0;
            for j in 0..n {
                row += a[i * n + j] * x[j];
            }
            value += x[i] * (0.5 * row + b[i]);
        }
        value
    };
    let scale = (0..n).map(|i| a[i * n + i]).fold(1e-12_f64, f64::max);
    let mut x = vec![0.0; n];
    for _ in 0..BOX_QP_ITERATIONS {
        let gradient: Vec<f64> = (0..n)
            .map(|i| {
                let mut row = b[i];
                for j in 0..n {
                    row += a[i * n + j] * x[j];
                }
                row
            })
            .collect();
        let free: Vec<usize> = (0..n)
            .filter(|&i| {
                !((x[i] <= l[i] && gradient[i] > 0.0) || (x[i] >= u[i] && gradient[i] < 0.0))
            })
            .collect();
        if free.is_empty() {
            break;
        }
        let m = free.len();
        let mut step = None;
        let mut ridge = 1e-10 * scale;
        for _ in 0..5 {
            let mut matrix = vec![0.0; m * m];
            let mut rhs = vec![0.0; m];
            for (ri, &i) in free.iter().enumerate() {
                rhs[ri] = -gradient[i];
                for (ci, &j) in free.iter().enumerate() {
                    matrix[ri * m + ci] = a[i * n + j];
                }
                matrix[ri * m + ri] += ridge;
            }
            if let Some(s) = super::linalg::solve_spd(&matrix, m, &rhs) {
                step = Some(s);
                break;
            }
            ridge *= 100.0;
        }
        let Some(step) = step else { break };
        let current = objective(&x);
        let mut t = 1.0;
        let mut improved = false;
        for _ in 0..30 {
            let mut trial = x.clone();
            for (ri, &i) in free.iter().enumerate() {
                trial[i] = (x[i] + t * step[ri]).clamp(l[i], u[i]);
            }
            if objective(&trial) < current {
                x = trial;
                improved = true;
                break;
            }
            t *= 0.5;
        }
        if !improved {
            break;
        }
    }
    x
}

/// Solves `max d^T delta` s.t. `T(y + delta) <= limit`, `lo <= y + delta <= hi` through the dual
/// (see the module notes). `None` when even the smallest step violates the constraint.
fn linear_step(env: &Env<'_>, s: &Surrogate, y: &[f64], d: &[f64]) -> Option<Vec<f64>> {
    let n = y.len();
    let gc = s.half_gradient(y);
    let c0 = s.value(y);
    let l: Vec<f64> = (0..n).map(|i| env.lo[i] - y[i]).collect();
    let u: Vec<f64> = (0..n).map(|i| env.hi[i] - y[i]).collect();
    let evaluate = |log_lambda: f64| -> (Vec<f64>, f64) {
        let lambda = 10.0_f64.powf(log_lambda);
        let a: Vec<f64> = s.h.iter().map(|v| 2.0 * lambda * v).collect();
        let b: Vec<f64> = (0..n).map(|i| 2.0 * lambda * gc[i] - d[i]).collect();
        let delta = box_qp(&a, &b, &l, &u);
        let (mut lin, mut quad) = (0.0, 0.0);
        for i in 0..n {
            lin += gc[i] * delta[i];
            let mut row = 0.0;
            for j in 0..n {
                row += s.h[i * n + j] * delta[j];
            }
            quad += delta[i] * row;
        }
        (delta, c0 + 2.0 * lin + quad)
    };
    let (small, value_small) = evaluate(LOG_LAMBDA_MIN);
    if value_small <= env.delta {
        return Some(small);
    }
    let (mut best, value_large) = evaluate(LOG_LAMBDA_MAX);
    if value_large > env.delta {
        return None;
    }
    let (mut low, mut high) = (LOG_LAMBDA_MIN, LOG_LAMBDA_MAX);
    for _ in 0..DUAL_STEPS {
        let middle = f64::midpoint(low, high);
        let (delta, value) = evaluate(middle);
        if value <= env.delta {
            high = middle;
            best = delta;
        } else {
            low = middle;
        }
    }
    Some(best)
}

/// Climbs the objective from `start` (inside the surrogate set) over the surrogate set.
fn climb(
    env: &Env<'_>,
    search: &dyn FitModel,
    objective: &Objective,
    s: &Surrogate,
    start: Vec<f64>,
) -> Result<Vec<f64>, FitError> {
    let np = env.np;
    let mut y = start;
    let mut f = objective.value(search, &y[..np]);
    for _ in 0..CLIMB_ITERATIONS {
        env.check_cancel()?;
        let d = objective.gradient(search, &y, &env.lo, &env.hi);
        if d.iter().all(|v| *v == 0.0) {
            break;
        }
        let Some(delta) = linear_step(env, s, &y, &d) else {
            break;
        };
        let mut t = 1.0;
        let mut moved = false;
        for _ in 0..LINE_SEARCH_HALVINGS {
            let candidate: Vec<f64> = (0..y.len())
                .map(|i| (y[i] + t * delta[i]).clamp(env.lo[i], env.hi[i]))
                .collect();
            let value = objective.value(search, &candidate[..np]);
            if value > f {
                y = candidate;
                f = value;
                moved = true;
                break;
            }
            t *= 0.5;
        }
        if !moved {
            break;
        }
    }
    Ok(y)
}

/// Where an extremisation starts.
#[derive(Clone, Copy)]
enum Start<'a> {
    /// From several starting points, pulled into the surrogate set around the data optimum.
    Fresh(&'a [Vec<f64>]),
    /// From a verified point, with the surrogate rebuilt there first.
    From(&'a [f64]),
}

/// Extremises `objective` over the verified set; the result is verified feasible.
fn extremise(
    env: &Env<'_>,
    search: &dyn FitModel,
    objective: &Objective,
    start: Start<'_>,
    normal_threads: usize,
) -> Result<Vec<f64>, FitError> {
    let np = env.np;
    let value = |y: &[f64]| objective.value(search, &y[..np]);
    let mut ok = env.y0.clone();
    let mut candidate: Option<Vec<f64>> = None;
    match start {
        Start::Fresh(starts) => {
            let mut best: Option<(f64, Vec<f64>)> = None;
            for point in starts {
                env.check_cancel()?;
                let inside = pull_into(&env.s0, point, env.delta);
                let y = climb(env, search, objective, &env.s0, inside)?;
                let f = value(&y);
                if best.as_ref().is_none_or(|(b, _)| f > *b) {
                    best = Some((f, y));
                }
            }
            candidate = best.map(|(_, y)| y);
        }
        Start::From(point) => ok = point.to_vec(),
    }
    for round in 0..ROUNDS {
        env.check_cancel()?;
        if let Some(c) = candidate.take() {
            let verified = if feasible(env, search, &c)? {
                c
            } else {
                pull_back(env, search, &ok, &c)?
            };
            if value(&verified) > value(&ok) {
                ok = verified;
            }
        }
        if round + 1 == ROUNDS {
            break;
        }
        let normal = env
            .problem
            .normal(search, &ok, &env.active, &env.ctx, normal_threads)?;
        let surrogate = Surrogate {
            centre: ok.clone(),
            excess: normal.chi2 + 2.0 * normal.prior.gain - env.c0,
            g: normal.g,
            h: normal.h,
        };
        let climbed = climb(env, search, objective, &surrogate, ok.clone())?;
        if climbed == ok {
            break;
        }
        candidate = Some(climbed);
    }
    Ok(ok)
}

/// The reported colours: every zone, size and illuminant of `config.prediction`, in the order of
/// `predict::predictions`.
fn groups(input: &ExtremeInput<'_>) -> Vec<Group> {
    let mut out = Vec::new();
    let params = &input.fit[..input.model.n_params()];
    for zone in 0..input.model.zone_count() {
        for &size_mm in &input.config.prediction.sizes() {
            let path_mm = face_up_path_mm(size_mm, input.config.prediction.model_width_units);
            for &illuminant in &input.config.prediction.illuminants {
                out.push(Group {
                    zone,
                    path_mm,
                    illuminant,
                    fit_lab: zone_lab(input.model.as_spectral(), zone, params, path_mm, illuminant),
                });
            }
        }
    }
    out
}

fn make_search(model: &dyn FitModel) -> Box<dyn FitModel> {
    model.search_space().unwrap_or_else(|| model.duplicate())
}

fn gather<T>(results: Vec<Option<Result<T, FitError>>>) -> Result<Vec<T>, FitError> {
    let mut out = Vec::with_capacity(results.len());
    for result in results {
        match result {
            None => return Err(FitError::Cancelled),
            Some(Err(e)) => return Err(e),
            Some(Ok(value)) => out.push(value),
        }
    }
    Ok(out)
}

/// The verified end points of the constrained extremisation, as full model parameter vectors
/// (`[parameters, log-gains]` in the fitted model's own parameters). Includes the data optimum.
///
/// `progress` receives the finished share of the whole search, non-decreasing in `0.0..=1.0`:
/// the data-only optimum is the first 20 %, the objectives of both phases share the rest in
/// proportion to their number (reported from the calling thread as tasks finish).
///
/// # Errors
///
/// [`FitError::Cancelled`] (also from inside a climb) and the forward evaluator's errors.
pub(super) fn constrained_extremes(
    input: &ExtremeInput<'_>,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<Vec<f64>>, FitError> {
    let problem = input.problem;
    let model = input.model;
    let config = input.config;
    let cancel = input.cancel;
    let np = model.n_params();
    let nv = problem.view_count();
    let search = make_search(model);
    let to_search = |x: &[f64]| -> Vec<f64> {
        let mut v = model.to_search_space(&x[..np]);
        v.extend_from_slice(&x[np..]);
        v
    };
    let from_search = |y: &[f64]| -> Vec<f64> {
        let mut v = model.from_search_space(&y[..np]);
        v.extend_from_slice(&y[np..]);
        v
    };
    let (lo, hi) = bounds(&*search, nv);
    let active = vec![true; nv];
    let ones = vec![1.0; np];
    let ctx = PriorContext {
        config,
        l1_weights: &ones,
        pin_common_gain: false,
        model_priors: false,
    };
    let fit_search = to_search(input.fit);
    let optimum = run_lm(
        problem,
        &*search,
        &fit_search,
        &active,
        &ctx,
        &LmSettings::new(config.max_iterations, config.rel_cost_tol)
            .with_inner_threads(inner_threads(config.threads, 1)),
        cancel,
    )?;
    progress(OPTIMUM_SHARE);
    let c0 = optimum.normal.chi2 + 2.0 * optimum.normal.prior.gain;
    let y0 = optimum.x;
    let s0 = Surrogate {
        centre: y0.clone(),
        excess: 0.0,
        g: optimum.normal.g,
        h: optimum.normal.h,
    };
    let env = Env {
        problem,
        np,
        lo,
        hi,
        active,
        ctx,
        y0: y0.clone(),
        c0,
        delta: input.delta,
        s0,
        cancel,
    };

    // The starts: the data optimum, the fit and the structured starts of the fit.
    let clamp = |mut y: Vec<f64>| -> Vec<f64> {
        for (i, v) in y.iter_mut().enumerate() {
            *v = v.clamp(env.lo[i], env.hi[i]);
        }
        y
    };
    let mut starts = vec![y0.clone(), clamp(fit_search)];
    for structured in model.structured_starts(&input.fit[..np]) {
        let mut full = structured;
        full.extend_from_slice(&input.fit[np..]);
        starts.push(clamp(to_search(&full)));
    }
    for extra in input.extra_starts {
        starts.push(clamp(to_search(extra)));
    }

    let groups = groups(input);
    let total_tasks = (groups.len() * 7).max(1);
    let share = |finished: usize| -> f32 {
        OPTIMUM_SHARE + (1.0 - OPTIMUM_SHARE) * (finished as f32 / total_tasks as f32)
    };
    let phase_one = {
        let inner = inner_threads(config.threads, groups.len() * 6);
        let work = |t: usize| -> Result<Vec<f64>, FitError> {
            let objective = Objective::directional(groups[t / 6], t % 6);
            let copy = make_search(model);
            extremise(&env, &*copy, &objective, Start::Fresh(&starts), inner)
        };
        gather(par_map_progress(
            groups.len() * 6,
            config.threads,
            cancel,
            &work,
            &mut |finished| progress(share(finished)),
        ))?
    };

    // The distance from the fit, started at the farthest directional end point of each group.
    let farthest: Vec<&[f64]> = groups
        .iter()
        .enumerate()
        .map(|(g, group)| {
            let mut best = (f64::NEG_INFINITY, &phase_one[g * 6][..]);
            for point in &phase_one[g * 6..g * 6 + 6] {
                let lab = zone_lab(
                    search.as_spectral(),
                    group.zone,
                    &point[..np],
                    group.path_mm,
                    group.illuminant,
                );
                let de = delta_e_2000(group.fit_lab, lab);
                if de.is_finite() && de > best.0 {
                    best = (de, &point[..]);
                }
            }
            best.1
        })
        .collect();
    let phase_two = {
        let inner = inner_threads(config.threads, groups.len());
        let work = |g: usize| -> Result<Vec<f64>, FitError> {
            let objective = Objective {
                group: groups[g],
                kind: Kind::FromFit {
                    reference: groups[g].fit_lab,
                },
            };
            let copy = make_search(model);
            extremise(&env, &*copy, &objective, Start::From(farthest[g]), inner)
        };
        let offset = groups.len() * 6;
        gather(par_map_progress(
            groups.len(),
            config.threads,
            cancel,
            &work,
            &mut |finished| progress(share(offset + finished)),
        ))?
    };
    progress(1.0);

    let mut out = vec![from_search(&y0)];
    out.extend(phase_one.iter().map(|y| from_search(y)));
    out.extend(phase_two.iter().map(|y| from_search(y)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_qp_finds_the_interior_and_the_bound_minimum() {
        // min 1/2 (x0^2 + 2 x1^2) - 3 x0 - 0.5 x1 on [-1, 1]^2: x0 hits the bound 1, x1 = 0.25.
        let a = [1.0, 0.0, 0.0, 2.0];
        let b = [-3.0, -0.5];
        let x = box_qp(&a, &b, &[-1.0, -1.0], &[1.0, 1.0]);
        assert!((x[0] - 1.0).abs() < 1e-9, "{x:?}");
        assert!((x[1] - 0.25).abs() < 1e-6, "{x:?}");
    }

    #[test]
    fn the_box_qp_sends_a_flat_direction_to_the_bound() {
        // No curvature in x1: the linear term alone drives it to the upper bound.
        let a = [1.0, 0.0, 0.0, 0.0];
        let b = [-0.5, -1.0];
        let x = box_qp(&a, &b, &[-2.0, -2.0], &[2.0, 3.0]);
        assert!((x[0] - 0.5).abs() < 1e-6, "{x:?}");
        assert!((x[1] - 3.0).abs() < 1e-6, "{x:?}");
    }

    #[test]
    fn a_start_outside_the_ellipsoid_is_pulled_to_its_surface() {
        // T = x^T x (H = I, g = 0, centre 0, excess 0); limit 4: the surface is |x| = 2.
        let s = Surrogate {
            centre: vec![0.0, 0.0],
            excess: 0.0,
            g: vec![0.0, 0.0],
            h: vec![1.0, 0.0, 0.0, 1.0],
        };
        let p = pull_into(&s, &[6.0, 8.0], 4.0);
        assert!((s.value(&p) - 4.0).abs() < 1e-9, "{p:?}");
        // The direction is kept.
        assert!((p[0] / p[1] - 0.75).abs() < 1e-12);
        // A start already inside is not moved.
        assert_eq!(pull_into(&s, &[0.5, 0.5], 4.0), vec![0.5, 0.5]);
    }
}
