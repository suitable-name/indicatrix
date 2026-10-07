//! Inverse solver: maps a target color (CIELAB) back to elements, fractions and treatments
//! (plan §4.2).
//!
//! Deterministic, staged, linear-model search:
//! - Stage `s` (1, 2, 3) enumerates every *independent* subset of `s` free items (never
//!   supersets of an earlier winner), crossed with the valid treatment combinations (none unless
//!   the request allows them), and keeps the best candidate by `cost = dE00^2 + kappa*|S|`
//!   (`kappa = 1`; garnet end members are exempt from the item count and constrained by
//!   `sum x <= 1`). The search stops after the first stage whose best `dE00 <= 1`.
//! - Per-item unit spectra are precomputed per subset, so one forward evaluation is
//!   `alpha_m(lambda) = base + sum_i c_i * S_i,m(lambda)` plus `exp` and the XYZ weighting (IVCT
//!   pairs are evaluated through their pairing law). Items start on a coarse log grid
//!   (24 / 8x8 / 6x6x6), then a Levenberg-Marquardt refinement in log space runs (cap 20).
//! - The winner is re-scored through [`resolve`] (WYSIWYG); if the band budget moved `dE00` by
//!   more than 0.5, up to 5 further LM iterations use the budgeted tensor as the forward model.
//! - Ties (`|dcost| < 1e-9`): fewer items, then lexicographic ids. No wall clock in the decision
//!   path; the result is bit-identical for identical input on one platform.
//! - A cancellation flag is polled between evaluations; a cancelled solve returns
//!   [`Cancelled`] and never a truncated result.

#![expect(
    clippy::suboptimal_flops,
    clippy::needless_range_loop,
    clippy::many_single_char_names,
    reason = "index-parallel spectra and the textbook normal equations read better than mul_add/iterator chains"
)]

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

use super::{
    catalogue::{ChromophoreCatalogue, ChromophoreData, HostData, species_element},
    recipe::{ColorRecipe, ResolvedBands},
    resolve::{Removal, apply_treatments, effective_concentration, resolve},
};
use crate::{
    color::{
        body_color::{BodyColor, Illuminant, body_colors, delta_e_2000_residual, xyz_to_lab},
        cie1931::cie_1931_cmf,
    },
    optics::absorption::{AbsorptionBand, legacy_rgb_bands},
};

/// Maximum forward evaluations per solve before capping.
pub const MAX_EVALS: usize = 40_000;

/// Item-count penalty `kappa` of the cost `dE00^2 + kappa*|S|`.
const KAPPA: f64 = 1.0;
/// A stage is accepted (and the staging stops) at `dE00 <= ACCEPT_DE`.
const ACCEPT_DE: f64 = 1.0;
/// Levenberg-Marquardt iteration cap.
const LM_ITERS: usize = 20;
/// Further LM iterations on the budgeted forward model (§4.2 step 6).
const RESCORE_ITERS: usize = 5;
/// Coarse log-grid points per dimension for subset sizes 1, 2, 3.
const GRID_POINTS: [usize; 3] = [24, 8, 6];
/// Lower search bound relative to `conc_max`.
const C_MIN_FRAC: f64 = 1e-3;
/// Largest subset size.
const MAX_SUBSET: usize = 3;
/// Wavelength samples, 380..=780 nm at 1 nm.
const N: usize = 401;
/// Treatments combined at once by the search.
const MAX_TREATMENTS: usize = 2;
/// color temperature of the optional second target (the Incandescent preset).
const A_TEMP_K: f32 = 3200.0;

/// The solve was cancelled through its cancellation flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("solve cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Which treatments the solver may combine with the elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TreatmentPolicy<'a> {
    /// Natural stones only: no treatment is ever added (the default).
    #[default]
    NaturalOnly,
    /// The solver may add any combination (up to two) of these catalogue treatment ids whose
    /// required elements are present.
    Allow(&'a [String]),
}

/// A full solve request (see [`solve_physics_with`]).
#[derive(Debug, Clone, Copy)]
pub struct SolveRequest<'a> {
    /// Host identifier (`"corundum"`).
    pub host_id: &'a str,
    /// Target CIELAB under D65 (Lab relative to the illuminant's own white).
    pub target_d65_lab: [f64; 3],
    /// Optional second target under Planckian 3200 K (color-change gems).
    pub target_a_lab: Option<[f64; 3]>,
    /// Weight `w_A` of the second target in `dE_D65^2 + w_A*dE_A^2` (default 1).
    pub dual_weight: f64,
    /// Reference path in mm.
    pub ref_path_mm: f32,
    /// Locked entries (id, amount in the item's unit): fixed, exempt from `kappa`.
    pub locked: &'a [(String, f64)],
    /// Treatment policy.
    pub treatments: TreatmentPolicy<'a>,
}

impl<'a> SolveRequest<'a> {
    /// A natural-only, single-target request without locks.
    #[must_use]
    pub const fn new(host_id: &'a str, target_d65_lab: [f64; 3], ref_path_mm: f32) -> Self {
        Self {
            host_id,
            target_d65_lab,
            target_a_lab: None,
            dual_weight: 1.0,
            ref_path_mm,
            locked: &[],
            treatments: TreatmentPolicy::NaturalOnly,
        }
    }
}

/// Output of the inverse solver.
#[derive(Debug, Clone, PartialEq)]
pub struct SolveResult {
    /// Solved recipe with amounts (strength = 1.0).
    pub recipe: ColorRecipe,
    /// Achieved body color (D65, unpolarised) computed from the budgeted tensor.
    pub achieved: BodyColor,
    /// Final color difference Delta E 2000 under D65 from the target pick (budgeted tensor).
    pub delta_e: f64,
    /// Delta E 2000 under 3200 K from the second target, when one was given.
    pub delta_e_a: Option<f64>,
    /// Whether the target was reachable: the accept metric (`dE_D65`, or
    /// `sqrt(dE_D65^2 + w_A*dE_A^2)` with a second target) is `<= 1.0`.
    pub reachable: bool,
    /// Whether the search was truncated by the evaluation cap.
    pub capped: bool,
    /// Forward evaluations spent.
    pub evals: usize,
}

/// Solves for element amounts in `host_id` matching `target_d65_lab` (natural stones only, not
/// cancellable). See [`solve_physics_with`] for the full interface.
#[must_use]
pub fn solve_physics(
    catalogue: &ChromophoreCatalogue,
    host_id: &str,
    target_d65_lab: [f64; 3],
    target_a_lab: Option<[f64; 3]>,
    ref_path_mm: f32,
    locked_entries: &[(String, f64)],
) -> SolveResult {
    let request = SolveRequest {
        target_a_lab,
        locked: locked_entries,
        ..SolveRequest::new(host_id, target_d65_lab, ref_path_mm)
    };
    let never = AtomicBool::new(false);
    solve_physics_with(catalogue, &request, &never)
        .unwrap_or_else(|Cancelled| unreachable!("a flag that nobody sets cannot cancel"))
}

/// Staged inverse solve (§4.2). `cancel` is polled between evaluations; when it is set the solve
/// returns `Err(Cancelled)` (latest request wins: the caller sets the flag of the superseded
/// solve).
///
/// # Errors
///
/// [`Cancelled`] if `cancel` was set during the solve.
pub fn solve_physics_with(
    catalogue: &ChromophoreCatalogue,
    req: &SolveRequest<'_>,
    cancel: &AtomicBool,
) -> Result<SolveResult, Cancelled> {
    let Some(host) = catalogue.host(req.host_id) else {
        return Ok(SolveResult {
            recipe: ColorRecipe::new(req.host_id, catalogue.data_version),
            achieved: BodyColor {
                xyz: [0.0; 3],
                lab: [0.0; 3],
                srgb: [0; 3],
                out_of_gamut: false,
            },
            delta_e: 100.0,
            delta_e_a: req.target_a_lab.map(|_| 100.0),
            reachable: false,
            capped: false,
            evals: 0,
        });
    };
    let ctx = Ctx::new(
        cancel,
        req.target_d65_lab,
        req.target_a_lab,
        req.dual_weight,
        req.ref_path_mm,
    );
    Search::new(catalogue, host, req, &ctx).run()
}

// ---------------------------------------------------------------------------------------------
// Evaluation context and linear model
// ---------------------------------------------------------------------------------------------

/// Illuminant-weighted color-matching functions at 1 nm, normalised so a colorless stone has
/// `Y = 1` (identical to `body_color`'s integration).
struct Observer {
    w: [[f64; N]; 3],
    white: [f64; 3],
}

impl Observer {
    fn new(ill: Illuminant) -> Self {
        let mut w = [[0.0; N]; 3];
        let mut white_y = 0.0;
        for i in 0..N {
            let lambda = 380.0 + i as f64;
            let cmf = cie_1931_cmf(lambda as f32);
            let s = ill.spectral_power(lambda);
            for k in 0..3 {
                w[k][i] = s * f64::from(cmf[k]);
            }
            white_y += w[1][i];
        }
        let norm = if white_y > 1e-12 { white_y } else { 1.0 };
        let mut white = [0.0; 3];
        for k in 0..3 {
            for i in 0..N {
                w[k][i] /= norm;
                white[k] += w[k][i];
            }
        }
        white[1] = 1.0;
        Self { w, white }
    }

    fn lab(&self, xyz: [f64; 3]) -> [f64; 3] {
        xyz_to_lab(xyz, self.white)
    }
}

/// Targets, observers, evaluation counter and cancellation token of one solve.
struct Ctx<'a> {
    cancel: &'a AtomicBool,
    evals: Cell<usize>,
    d65: Observer,
    a: Option<Observer>,
    target_d65: [f64; 3],
    target_a: Option<[f64; 3]>,
    w_a: f64,
    path_mm: f64,
}

/// The residual vector of an evaluation: `|r|^2 = dE_D65^2 + w_A*dE_A^2`.
type Resid = [f64; 6];

impl<'a> Ctx<'a> {
    fn new(
        cancel: &'a AtomicBool,
        target_d65: [f64; 3],
        target_a: Option<[f64; 3]>,
        w_a: f64,
        path_mm: f32,
    ) -> Self {
        Self {
            cancel,
            evals: Cell::new(0),
            d65: Observer::new(Illuminant::D65),
            a: target_a.map(|_| Observer::new(Illuminant::Planckian(A_TEMP_K))),
            target_d65,
            target_a,
            w_a: if w_a.is_finite() { w_a.max(0.0) } else { 1.0 },
            path_mm: f64::from(path_mm),
        }
    }

    const fn evals(&self) -> usize {
        self.evals.get()
    }

    /// Counts one evaluation and polls the cancellation flag.
    fn tick(&self) -> Result<(), Cancelled> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Cancelled);
        }
        self.evals.set(self.evals.get() + 1);
        Ok(())
    }

    /// Residuals from the unpolarised Lab values under D65 (and 3200 K).
    fn residual(&self, lab_d65: [f64; 3], lab_a: Option<[f64; 3]>) -> Resid {
        let mut r = [0.0; 6];
        let d = delta_e_2000_residual(self.target_d65, lab_d65);
        r[..3].copy_from_slice(&d);
        if let (Some(t), Some(l)) = (self.target_a, lab_a) {
            let da = delta_e_2000_residual(t, l);
            let s = self.w_a.sqrt();
            for k in 0..3 {
                r[3 + k] = s * da[k];
            }
        }
        r
    }

    /// `(dE_D65, dE_A, accept metric)` of a residual.
    fn metrics(&self, r: &Resid) -> (f64, Option<f64>, f64) {
        let sq = |s: &[f64]| s.iter().map(|x| x * x).sum::<f64>();
        let de = sq(&r[..3]).sqrt();
        let de_a =
            (self.target_a.is_some() && self.w_a > 0.0).then(|| (sq(&r[3..]) / self.w_a).sqrt());
        let combined = sq(r).sqrt();
        (de, de_a.or_else(|| self.target_a.map(|_| 0.0)), combined)
    }

    /// Lab values (D65, optional 3200 K) of the unpolarised transmittance of `alpha`
    /// (`modes * N` absorption coefficients in mm^-1).
    fn labs(&self, alpha: &[f64], modes: usize) -> ([f64; 3], Option<[f64; 3]>) {
        let mut xd = [0.0; 3];
        let mut xa = [0.0; 3];
        let dual = self.a.is_some();
        let path = self.path_mm;
        for i in 0..N {
            let t0 = (-alpha[i] * path).exp();
            let t = match modes {
                1 => t0,
                2 => (2.0 * t0 + (-alpha[N + i] * path).exp()) / 3.0,
                _ => (t0 + (-alpha[N + i] * path).exp() + (-alpha[2 * N + i] * path).exp()) / 3.0,
            };
            for k in 0..3 {
                xd[k] += t * self.d65.w[k][i];
            }
            if let Some(obs) = &self.a
                && dual
            {
                for k in 0..3 {
                    xa[k] += t * obs.w[k][i];
                }
            }
        }
        (self.d65.lab(xd), self.a.as_ref().map(|obs| obs.lab(xa)))
    }
}

/// An IVCT pair chromophore whose weight follows its pairing law (evaluated per call).
struct PairTerm<'h> {
    chromo: &'h ChromophoreData,
    elements: [String; 2],
    spectrum: Vec<f64>,
}

/// One free item of a subset: its unit spectrum per mode and its bounds.
struct Item {
    id: String,
    end_member: bool,
    c_min: f64,
    c_max: f64,
    spectrum: Vec<f64>,
    /// Spectrum per unit of `c^2` (pair-enhanced `quadratic_coeff` bands), if any.
    quad: Option<Vec<f64>>,
}

/// The linear forward model of one (subset, treatment combination).
struct Model<'h> {
    host: Option<&'h HostData>,
    modes: usize,
    base: Vec<f64>,
    items: Vec<Item>,
    pairs: Vec<PairTerm<'h>>,
    /// `split`/`created` for the pair law, from the treatment combination.
    treated: Option<super::resolve::Treated>,
    /// Amounts keyed by element id (locked entries fixed, free entries updated per call).
    amounts: BTreeMap<String, f64>,
    /// Sum of the locked end-member fractions.
    locked_em_sum: f64,
    scratch: Vec<f64>,
}

impl Model<'_> {
    /// Evaluates the model at free amounts `c`.
    fn eval(&mut self, ctx: &Ctx<'_>, c: &[f64]) -> Result<Resid, Cancelled> {
        ctx.tick()?;
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.base);
        for (item, &ci) in self.items.iter().zip(c) {
            for (a, s) in self.scratch.iter_mut().zip(&item.spectrum) {
                *a += ci * s;
            }
            if let Some(q) = &item.quad {
                for (a, s) in self.scratch.iter_mut().zip(q) {
                    *a += ci * ci * s;
                }
            }
            if !self.pairs.is_empty() {
                self.amounts.insert(item.id.clone(), ci);
            }
        }
        if let (Some(host), Some(treated)) = (self.host, &self.treated) {
            for pair in &self.pairs {
                let present = |id: &str| self.amounts.get(id).is_some_and(|v| *v > 0.0);
                if !(present(&pair.elements[0]) && present(&pair.elements[1])) {
                    continue;
                }
                let w = effective_concentration(
                    host,
                    pair.chromo,
                    &self.amounts,
                    &treated.split,
                    &treated.created,
                );
                if w.is_finite() && w > 0.0 {
                    for (a, s) in self.scratch.iter_mut().zip(&pair.spectrum) {
                        *a += w * s;
                    }
                }
            }
        }
        let (d, a) = ctx.labs(&self.scratch, self.modes);
        Ok(ctx.residual(d, a))
    }
}

fn dot(r: &Resid) -> f64 {
    r.iter().map(|x| x * x).sum()
}

// ---------------------------------------------------------------------------------------------
// Levenberg-Marquardt in log space
// ---------------------------------------------------------------------------------------------

/// Solves the symmetric `n x n` system `a x = b` (`n <= 3`) by Gaussian elimination with partial
/// pivoting; `None` when singular.
fn solve_small(n: usize, mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in (col + 1)..n {
            let f = a[row][col] / a[col][col];
            for k in col..n {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; 3];
    for row in (0..n).rev() {
        let mut s = b[row];
        for k in (row + 1)..n {
            s -= a[row][k] * x[k];
        }
        x[row] = s / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Projected Levenberg-Marquardt on `u` (log amounts) with box bounds, a finite-difference
/// Jacobian and Marquardt scaling; at most `iters` iterations. Returns the best point and its
/// squared residual.
fn lm_log<F>(
    mut u: Vec<f64>,
    lo: &[f64],
    hi: &[f64],
    iters: usize,
    project: &dyn Fn(&mut [f64]),
    f: &mut F,
) -> Result<(Vec<f64>, f64), Cancelled>
where
    F: FnMut(&[f64]) -> Result<Resid, Cancelled>,
{
    const H: f64 = 1e-3;
    const MAX_STEP: f64 = 2.0;
    let n = u.len();
    let mut r = f(&u)?;
    let mut cost = dot(&r);
    let mut lambda = 1e-2;
    for _ in 0..iters {
        if cost < 1e-10 {
            break;
        }
        let mut jac = [[0.0; 6]; 3];
        for j in 0..n {
            let mut up = u.clone();
            let h = if u[j] + H > hi[j] { -H } else { H };
            up[j] += h;
            let rp = f(&up)?;
            for k in 0..6 {
                jac[j][k] = (rp[k] - r[k]) / h;
            }
        }
        let mut jtj = [[0.0; 3]; 3];
        let mut jtr = [0.0; 3];
        for i in 0..n {
            for j in 0..n {
                jtj[i][j] = (0..6).map(|k| jac[i][k] * jac[j][k]).sum();
            }
            jtr[i] = (0..6).map(|k| jac[i][k] * r[k]).sum();
        }
        let mut accepted = false;
        let mut step_norm = 0.0;
        let mut gain = 0.0;
        for _ in 0..4 {
            let mut a = jtj;
            for i in 0..n {
                a[i][i] += lambda * jtj[i][i].max(1e-9);
            }
            let rhs = [-jtr[0], -jtr[1], -jtr[2]];
            if let Some(delta) = solve_small(n, a, rhs) {
                let mut un = u.clone();
                for j in 0..n {
                    un[j] = (u[j] + delta[j].clamp(-MAX_STEP, MAX_STEP)).clamp(lo[j], hi[j]);
                }
                project(&mut un);
                for j in 0..n {
                    un[j] = un[j].clamp(lo[j], hi[j]);
                }
                let rn = f(&un)?;
                let cn = dot(&rn);
                if cn < cost {
                    step_norm = (0..n).map(|j| (un[j] - u[j]).powi(2)).sum::<f64>().sqrt();
                    gain = cost - cn;
                    u = un;
                    r = rn;
                    cost = cn;
                    lambda = (lambda / 3.0).max(1e-7);
                    accepted = true;
                    break;
                }
            }
            lambda *= 5.0;
        }
        if !accepted || step_norm < 1e-6 || gain < 1e-9 * cost.max(1e-12) {
            break;
        }
    }
    Ok((u, cost))
}

// ---------------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------------

/// A scored candidate: an item subset with amounts and a treatment combination.
#[derive(Debug, Clone)]
struct Candidate {
    ids: Vec<String>,
    amounts: Vec<f64>,
    treatments: Vec<String>,
    de: f64,
    de_a: Option<f64>,
    /// Accept metric (`dE_D65` or the weighted combination).
    metric: f64,
    /// `metric^2 + kappa*counted`.
    cost: f64,
    /// `1e-6 * sum ln(c_i / c_max,i)` (smaller concentrations preferred within one size).
    tiebreak: f64,
}

impl Candidate {
    /// Whether `self` beats `other`: lower cost; ties (1e-9) fewer items, then lexicographic ids.
    fn beats(&self, other: &Self) -> bool {
        let same_size = self.ids.len() == other.ids.len();
        let (a, b) = if same_size {
            (self.cost + self.tiebreak, other.cost + other.tiebreak)
        } else {
            (self.cost, other.cost)
        };
        if (a - b).abs() >= 1e-9 {
            return a < b;
        }
        if !same_size {
            return self.ids.len() < other.ids.len();
        }
        (&self.ids, &self.treatments) < (&other.ids, &other.treatments)
    }
}

struct Search<'a> {
    cat: &'a ChromophoreCatalogue,
    host: &'a HostData,
    req: &'a SolveRequest<'a>,
    ctx: &'a Ctx<'a>,
    modes: usize,
    /// Unit spectra of single chromophores per (treatment key, chromophore index).
    unit_cache: RefCell<BTreeMap<(String, usize), Vec<f64>>>,
    locked: BTreeMap<String, f64>,
}

impl<'a> Search<'a> {
    fn new(
        cat: &'a ChromophoreCatalogue,
        host: &'a HostData,
        req: &'a SolveRequest<'a>,
        ctx: &'a Ctx<'a>,
    ) -> Self {
        let modes = match host.optical.as_str() {
            "biaxial" => 3,
            "uniaxial" => 2,
            _ => 1,
        };
        let mut locked = BTreeMap::new();
        for (id, amount) in req.locked {
            if amount.is_finite() {
                locked.insert(id.clone(), amount.clamp(0.0, host.element_conc_max(id)));
            }
        }
        Self {
            cat,
            host,
            req,
            ctx,
            modes,
            unit_cache: RefCell::new(BTreeMap::new()),
            locked,
        }
    }

    fn run(&self) -> Result<SolveResult, Cancelled> {
        let free: Vec<String> = self
            .cat
            .selectable_elements(&self.host.id)
            .into_iter()
            .filter(|id| !self.locked.contains_key(id) && self.host.element_conc_max(id) > 0.0)
            .collect();

        let mut best = self.baseline()?;
        let mut capped = false;
        if best.metric > ACCEPT_DE {
            'stages: for size in 1..=MAX_SUBSET.min(free.len()) {
                for subset in subsets(free.len(), size) {
                    let ids: Vec<&String> = subset.iter().map(|&i| &free[i]).collect();
                    for combo in self.treatment_combos(&ids) {
                        // Never start a candidate that could overrun the cap (the worst case of
                        // one candidate plus the final re-scoring is bounded).
                        if self.ctx.evals() + candidate_bound(size) + FINALISE_RESERVE > MAX_EVALS {
                            capped = true;
                            break 'stages;
                        }
                        let cand = self.optimise(&ids, &combo)?;
                        if cand.beats(&best) {
                            best = cand;
                        }
                    }
                }
                if best.metric <= ACCEPT_DE {
                    break;
                }
            }
        }
        self.finalise(&best, capped)
    }

    /// The locks alone (size 0).
    fn baseline(&self) -> Result<Candidate, Cancelled> {
        let mut model = self.build_model(&[], &[]);
        let r = model.eval(self.ctx, &[])?;
        Ok(self.candidate(&model, &r, &[], Vec::new()))
    }

    fn candidate(
        &self,
        model: &Model<'_>,
        r: &Resid,
        c: &[f64],
        treatments: Vec<String>,
    ) -> Candidate {
        let (de, de_a, metric) = self.ctx.metrics(r);
        let counted = model.items.iter().filter(|i| !i.end_member).count();
        Candidate {
            ids: model.items.iter().map(|i| i.id.clone()).collect(),
            amounts: c.to_vec(),
            treatments,
            de,
            de_a,
            metric,
            cost: metric * metric + KAPPA * counted as f64,
            tiebreak: 1e-6
                * model
                    .items
                    .iter()
                    .zip(c)
                    .map(|(i, &x)| (x / i.c_max).max(1e-300).ln())
                    .sum::<f64>(),
        }
    }

    /// Valid treatment combinations for a subset: the empty one, plus (when allowed) every
    /// combination of up to [`MAX_TREATMENTS`] allowed treatments whose required elements are
    /// present (free subset or non-zero locks).
    fn treatment_combos(&self, ids: &[&String]) -> Vec<Vec<String>> {
        let mut combos = vec![Vec::new()];
        let TreatmentPolicy::Allow(allowed) = self.req.treatments else {
            return combos;
        };
        let mut present: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        present.extend(
            self.locked
                .iter()
                .filter(|(_, v)| **v > 0.0)
                .map(|(k, _)| k.as_str()),
        );
        let valid: Vec<String> = self
            .cat
            .selectable_treatments(&self.host.id, &present)
            .into_iter()
            .filter(|t| allowed.contains(&t.id))
            .map(|t| t.id.clone())
            .collect();
        for size in 1..=MAX_TREATMENTS.min(valid.len()) {
            for sub in subsets(valid.len(), size) {
                combos.push(sub.into_iter().map(|i| valid[i].clone()).collect());
            }
        }
        combos
    }

    /// Unit spectrum (`modes * N`, mm^-1 per unit of effective concentration) of one chromophore.
    fn unit_spectrum(&self, key: &str, idx: usize, removals: Option<&Vec<Removal>>) -> Vec<f64> {
        let cache_key = (key.to_string(), idx);
        if let Some(v) = self.unit_cache.borrow().get(&cache_key) {
            return v.clone();
        }
        let v = unit_spectrum_of(
            &self.host.chromophores[idx],
            removals,
            &self.host.optical,
            self.modes,
            false,
        );
        self.unit_cache.borrow_mut().insert(cache_key, v.clone());
        v
    }

    /// Builds the linear model of `ids` under the treatment combination `combo`.
    #[expect(
        clippy::too_many_lines,
        reason = "one pass assembling the base, pair and per-element spectra of a model"
    )]
    fn build_model(&self, ids: &[&String], combo: &[String]) -> Model<'a> {
        let host = self.host;
        let key = combo.join("+");
        let mut stub = ColorRecipe::new(&host.id, self.cat.data_version);
        stub.treatments = combo.to_vec();
        let len = self.modes * N;

        // Elements present: free subset and non-zero locks.
        let mut present: Vec<(&str, bool)> = ids.iter().map(|s| (s.as_str(), true)).collect();
        present.extend(
            self.locked
                .iter()
                .filter(|(_, v)| **v > 0.0)
                .map(|(k, _)| (k.as_str(), false)),
        );

        // Treatment state with every present element at an epsilon amount (gates `requires`).
        let eps: BTreeMap<String, f64> = present
            .iter()
            .map(|(id, _)| ((*id).to_string(), 1e-100))
            .collect();
        let mut warnings = Vec::new();
        let treated0 = apply_treatments(host, &stub, &eps, &mut warnings);

        let mut base = vec![0.0; len];
        let mut pairs = Vec::new();
        for (idx, chromo) in host.chromophores.iter().enumerate() {
            if !chromo.is_offered() {
                continue;
            }
            match chromo.kind.as_str() {
                "intrinsic" => {
                    let u = self.unit_spectrum(&key, idx, treated0.removals.get(&chromo.id));
                    add_scaled(&mut base, &u, 1.0);
                }
                "ivct_pair" if chromo.partners.len() == 2 => {
                    let elements = [
                        species_element(&chromo.partners[0]).to_string(),
                        species_element(&chromo.partners[1]).to_string(),
                    ];
                    if elements.iter().all(|e| present.iter().any(|(p, _)| p == e)) {
                        let spectrum =
                            self.unit_spectrum(&key, idx, treated0.removals.get(&chromo.id));
                        pairs.push(PairTerm {
                            chromo,
                            elements,
                            spectrum,
                        });
                    }
                }
                _ => {}
            }
        }

        // Per-element unit spectra by probing the (linear) effective concentrations.
        let mut element_spectrum: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        let mut element_quad: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        for (el, _) in &present {
            let mut probe = eps.clone();
            probe.insert((*el).to_string(), 1.0);
            let treated = apply_treatments(host, &stub, &probe, &mut warnings);
            let mut spectrum = vec![0.0; len];
            let mut quad = vec![0.0; len];
            let mut has_quad = false;
            for (idx, chromo) in host.chromophores.iter().enumerate() {
                if !chromo.is_offered() || matches!(chromo.kind.as_str(), "intrinsic" | "ivct_pair")
                {
                    continue;
                }
                let k =
                    effective_concentration(host, chromo, &probe, &treated.split, &treated.created);
                if k.is_finite() && k > 1e-12 {
                    let u = self.unit_spectrum(&key, idx, treated.removals.get(&chromo.id));
                    add_scaled(&mut spectrum, &u, k);
                    if chromo.usable_bands().any(|b| b.quadratic_coeff.is_some()) {
                        // c_eff = k * c, so the quadratic term is k^2 * c^2 * unit_quad.
                        let uq = unit_spectrum_of(
                            chromo,
                            treated.removals.get(&chromo.id),
                            &host.optical,
                            self.modes,
                            true,
                        );
                        add_scaled(&mut quad, &uq, k * k);
                        has_quad = true;
                    }
                }
            }
            element_spectrum.insert(el, spectrum);
            if has_quad {
                element_quad.insert(el, quad);
            }
        }

        let mut amounts = BTreeMap::new();
        let mut locked_em_sum = 0.0;
        for (id, &v) in &self.locked {
            if v > 0.0 {
                if let Some(s) = element_spectrum.get(id.as_str()) {
                    add_scaled(&mut base, s, v);
                }
                if let Some(q) = element_quad.get(id.as_str()) {
                    add_scaled(&mut base, q, v * v);
                }
                if host.end_members.iter().any(|m| &m.id == id && !m.colorless) {
                    locked_em_sum += v;
                }
                amounts.insert(id.clone(), v);
            }
        }

        let mut items = Vec::new();
        for id in ids {
            let c_max = host.element_conc_max(id);
            items.push(Item {
                id: (*id).clone(),
                end_member: host.end_members.iter().any(|m| &m.id == *id),
                c_min: C_MIN_FRAC * c_max,
                c_max,
                spectrum: element_spectrum
                    .remove(id.as_str())
                    .unwrap_or_else(|| vec![0.0; len]),
                quad: element_quad.remove(id.as_str()),
            });
        }

        Model {
            host: Some(host),
            modes: self.modes,
            base,
            items,
            pairs,
            treated: Some(treated0),
            amounts,
            locked_em_sum,
            scratch: Vec::with_capacity(len),
        }
    }

    /// Coarse log grid, then LM, for one (subset, treatments).
    fn optimise(&self, ids: &[&String], combo: &[String]) -> Result<Candidate, Cancelled> {
        let mut model = self.build_model(ids, combo);
        let n = model.items.len();
        let lo: Vec<f64> = model.items.iter().map(|i| i.c_min.ln()).collect();
        let hi: Vec<f64> = model.items.iter().map(|i| i.c_max.ln()).collect();
        let points = GRID_POINTS[n - 1];
        let allowed = (1.0 - model.locked_em_sum).max(0.0);

        // Coarse log grid: best cell is the single LM start.
        let mut best_u = lo.clone();
        let mut best_cost = f64::INFINITY;
        let mut idx = vec![0usize; n];
        'grid: loop {
            let u: Vec<f64> = (0..n)
                .map(|j| lo[j] + (hi[j] - lo[j]) * idx[j] as f64 / (points - 1) as f64)
                .collect();
            let em_sum: f64 = model
                .items
                .iter()
                .zip(&u)
                .filter(|(i, _)| i.end_member)
                .map(|(_, x)| x.exp())
                .sum();
            if em_sum <= allowed + 1e-12 {
                let c: Vec<f64> = u.iter().map(|x| x.exp()).collect();
                let cost = dot(&model.eval(self.ctx, &c)?);
                if cost < best_cost {
                    best_cost = cost;
                    best_u = u;
                }
            }
            for j in (0..n).rev() {
                idx[j] += 1;
                if idx[j] < points {
                    continue 'grid;
                }
                idx[j] = 0;
            }
            break;
        }

        let ctx = self.ctx;
        let (u, _) = {
            let proj_items: Vec<bool> = model.items.iter().map(|i| i.end_member).collect();
            let project = |u: &mut [f64]| {
                let sum: f64 = proj_items
                    .iter()
                    .zip(u.iter())
                    .filter(|(e, _)| **e)
                    .map(|(_, &x)| x.exp())
                    .sum();
                if sum > allowed && sum > 0.0 {
                    let shift = (allowed.max(1e-12) / sum).ln();
                    for (e, x) in proj_items.iter().zip(u.iter_mut()) {
                        if *e {
                            *x += shift;
                        }
                    }
                }
            };
            let mut f = |u: &[f64]| {
                let c: Vec<f64> = u.iter().map(|x| x.exp()).collect();
                model.eval(ctx, &c)
            };
            lm_log(best_u, &lo, &hi, LM_ITERS, &project, &mut f)?
        };
        let c: Vec<f64> = u.iter().map(|x| x.exp()).collect();
        let r = model.eval(self.ctx, &c)?;
        Ok(self.candidate(&model, &r, &c, combo.to_vec()))
    }

    fn make_recipe(&self, ids: &[String], c: &[f64], treatments: &[String]) -> ColorRecipe {
        let mut recipe = ColorRecipe::new(&self.host.id, self.cat.data_version);
        recipe.reference_path_mm = self.req.ref_path_mm;
        recipe.treatments = treatments.to_vec();
        for (id, v) in &self.locked {
            recipe.set_amount(id, *v);
        }
        for (id, &v) in ids.iter().zip(c) {
            if v > 1e-9 {
                recipe.set_amount(id, v);
            }
        }
        recipe
    }

    /// Exact residual of a recipe through `resolve` and the budgeted tensor.
    fn exact_residual(&self, recipe: &ColorRecipe) -> Option<(Resid, BodyColor)> {
        let (tensor, _) = resolve(recipe, self.cat).ok()?;
        let path = f64::from(recipe.reference_path_mm);
        let d65 = body_colors(&tensor, path, Illuminant::D65).unpolarised;
        let lab_a = self.ctx.target_a.map(|_| {
            body_colors(&tensor, path, Illuminant::Planckian(A_TEMP_K))
                .unpolarised
                .lab
        });
        Some((self.ctx.residual(d65.lab, lab_a), d65))
    }

    /// WYSIWYG re-scoring through [`resolve`] (§4.2 step 6) and result assembly.
    fn finalise(&self, best: &Candidate, capped: bool) -> Result<SolveResult, Cancelled> {
        let ids = best.ids.clone();
        let c = best.amounts.clone();
        let mut recipe = self.make_recipe(&ids, &c, &best.treatments);
        let mut exact = self.exact_residual(&recipe);

        // The band budget moved the color: refine on the budgeted forward model.
        if let Some((r, _)) = &exact
            && (self.ctx.metrics(r).2 - best.metric).abs() > 0.5
            && !ids.is_empty()
        {
            let model = self.build_model(&ids.iter().collect::<Vec<_>>(), &best.treatments);
            let lo: Vec<f64> = model.items.iter().map(|i| i.c_min.ln()).collect();
            let hi: Vec<f64> = model.items.iter().map(|i| i.c_max.ln()).collect();
            let allowed = (1.0 - model.locked_em_sum).max(0.0);
            let em: Vec<bool> = model.items.iter().map(|i| i.end_member).collect();
            let project = |u: &mut [f64]| {
                let sum: f64 = em
                    .iter()
                    .zip(u.iter())
                    .filter(|(e, _)| **e)
                    .map(|(_, &x)| x.exp())
                    .sum();
                if sum > allowed && sum > 0.0 {
                    let shift = (allowed.max(1e-12) / sum).ln();
                    for (e, x) in em.iter().zip(u.iter_mut()) {
                        if *e {
                            *x += shift;
                        }
                    }
                }
            };
            let mut f = |u: &[f64]| {
                self.ctx.tick()?;
                let c: Vec<f64> = u.iter().map(|x| x.exp()).collect();
                let rec = self.make_recipe(&ids, &c, &best.treatments);
                Ok(self.exact_residual(&rec).map_or([1e3; 6], |(r, _)| r))
            };
            let u0: Vec<f64> = c
                .iter()
                .zip(lo.iter().zip(&hi))
                .map(|(x, (l, h))| x.ln().clamp(*l, *h))
                .collect();
            let (u, _) = lm_log(u0, &lo, &hi, RESCORE_ITERS, &project, &mut f)?;
            let c2: Vec<f64> = u.iter().map(|x| x.exp()).collect();
            let rec2 = self.make_recipe(&ids, &c2, &best.treatments);
            if let Some(e2) = self.exact_residual(&rec2)
                && exact.as_ref().is_none_or(|e1| dot(&e2.0) < dot(&e1.0))
            {
                recipe = rec2;
                exact = Some(e2);
            }
        }

        let (delta_e, delta_e_a, metric, achieved) = match exact {
            Some((r, d65)) => {
                let (de, de_a, m) = self.ctx.metrics(&r);
                (de, de_a, m, d65)
            }
            None => (
                best.de,
                best.de_a,
                best.metric,
                BodyColor {
                    xyz: [0.0; 3],
                    lab: [0.0; 3],
                    srgb: [0; 3],
                    out_of_gamut: false,
                },
            ),
        };
        if let Ok((tensor, _)) = resolve(&recipe, self.cat) {
            recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
        }
        Ok(SolveResult {
            recipe,
            achieved,
            delta_e,
            delta_e_a,
            reachable: metric <= ACCEPT_DE,
            capped,
            evals: self.ctx.evals(),
        })
    }
}

/// Upper bound on the evaluations of one grid + LM candidate of `size` items.
const fn candidate_bound(size: usize) -> usize {
    GRID_POINTS[size - 1].pow(size as u32) + 2 + LM_ITERS * (size + 4)
}

/// Evaluations reserved for the final re-scoring (WYSIWYG) after the last candidate.
const FINALISE_RESERVE: usize = 1 + RESCORE_ITERS * (MAX_SUBSET + 4) + 8;

/// `dst += k * src`.
fn add_scaled(dst: &mut [f64], src: &[f64], k: f64) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d += k * s;
    }
}

/// Unit spectrum of a chromophore (`modes * N`, mm^-1 per unit effective concentration), the
/// per-band expression of `resolve::collect_bands` at `c_eff = 1`, `strength = 1`.
fn unit_spectrum_of(
    chromo: &ChromophoreData,
    removals: Option<&Vec<Removal>>,
    optical: &str,
    modes: usize,
    quad: bool,
) -> Vec<f64> {
    let mut out = vec![0.0; modes * N];
    for band in chromo.usable_bands() {
        let (Some(fwhm), Some(linear)) = (band.fwhm_cm1, band.peak_coeff) else {
            continue;
        };
        let peak_coeff = if quad {
            band.quadratic_coeff.unwrap_or(0.0)
        } else {
            linear
        };
        let mut factor = 1.0;
        if let Some(rs) = removals {
            for (bands_nm, f) in rs {
                if bands_nm.is_empty() || bands_nm.iter().any(|c| (c - band.centre_nm).abs() < 1.0)
                {
                    factor *= f;
                }
            }
        }
        let coeff = peak_coeff * factor / 10.0;
        let weight = |keys: &[&str], default: f64| -> f64 {
            keys.iter()
                .find_map(|k| band.pol.get(*k).copied())
                .unwrap_or(default)
                .max(0.0)
        };
        // (mode index, weight) pairs, in the same eigenmode order as `resolve`.
        let targets: Vec<(usize, f64)> = match optical {
            "biaxial" => vec![
                (0, weight(&["a"], 0.0)),
                (2, weight(&["b"], 0.0)),
                (1, weight(&["g"], 0.0)),
            ],
            "uniaxial" => vec![(0, weight(&["o"], 0.0)), (1, weight(&["e"], 0.0))],
            _ => vec![(0, weight(&["o"], 1.0))],
        };
        let shape = AbsorptionBand::energy(band.centre_nm as f32, fwhm as f32, 1.0);
        for (mode, w) in targets {
            if w <= 0.0 || mode >= modes {
                continue;
            }
            for i in 0..N {
                out[mode * N + i] += coeff * w * f64::from(shape.evaluate((380 + i) as f32));
            }
        }
    }
    out
}

/// All `size`-element index subsets of `0..n` in lexicographic order.
fn subsets(n: usize, size: usize) -> Vec<Vec<usize>> {
    fn rec(start: usize, n: usize, size: usize, cur: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if cur.len() == size {
            out.push(cur.clone());
            return;
        }
        for i in start..n {
            cur.push(i);
            rec(i + 1, n, size, cur, out);
            cur.pop();
        }
    }
    let mut out = Vec::new();
    rec(0, n, size, &mut Vec::new(), &mut out);
    out
}

// ---------------------------------------------------------------------------------------------
// Fantasy mode
// ---------------------------------------------------------------------------------------------

/// Result of [`solve_fantasy_lab`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FantasySolution {
    /// Band peaks for [`legacy_rgb_bands`] (the `absorption_rgb` triple).
    pub peaks: [f32; 3],
    /// color difference Delta E 2000 (D65) of the swatch at L = 1.0 from the target.
    pub delta_e: f64,
}

/// Largest legacy band peak the fantasy solver uses.
const FANTASY_PEAK_MAX: f64 = 20.0;
/// Smallest non-zero legacy band peak (smaller values are returned as exactly 0).
const FANTASY_PEAK_MIN: f64 = 1e-4;

/// Solves the three `absorption_rgb` band peaks for the D65 color `target_lab` (§4.6).
///
/// The peaks are the amplitudes of [`legacy_rgb_bands`] per model unit (swatch path L = 1.0),
/// found with the same grid + Levenberg-Marquardt machinery as [`solve_physics`]. Three fixed
/// Gaussians reach a thin set of colors: `delta_e` reports the remainder.
///
/// # Errors
///
/// [`Cancelled`] if `cancel` was set during the solve.
pub fn solve_fantasy_lab(
    target_lab: [f64; 3],
    cancel: &AtomicBool,
) -> Result<FantasySolution, Cancelled> {
    let ctx = Ctx::new(cancel, target_lab, None, 1.0, 1.0);
    let bands = legacy_rgb_bands([1.0; 3]);
    let items: Vec<Item> = bands
        .iter()
        .zip(["b0", "b1", "b2"])
        .map(|(band, id)| {
            let mut spectrum = vec![0.0; N];
            for (i, s) in spectrum.iter_mut().enumerate() {
                *s = f64::from(band.evaluate((380 + i) as f32));
            }
            Item {
                id: id.to_string(),
                end_member: false,
                c_min: FANTASY_PEAK_MIN,
                c_max: FANTASY_PEAK_MAX,
                spectrum,
                quad: None,
            }
        })
        .collect();
    let mut model = Model {
        host: None,
        modes: 1,
        base: vec![0.0; N],
        items,
        pairs: Vec::new(),
        treated: None,
        amounts: BTreeMap::new(),
        locked_em_sum: 0.0,
        scratch: Vec::with_capacity(N),
    };
    let n = 3;
    let lo = vec![FANTASY_PEAK_MIN.ln(); n];
    let hi = vec![FANTASY_PEAK_MAX.ln(); n];
    let points: usize = 7;
    let mut best_u = lo.clone();
    let mut best_cost = f64::INFINITY;
    for a in 0..points {
        for b in 0..points {
            for c in 0..points {
                let u: Vec<f64> = [a, b, c]
                    .iter()
                    .map(|&k| lo[0] + (hi[0] - lo[0]) * k as f64 / (points - 1) as f64)
                    .collect();
                let amounts: Vec<f64> = u.iter().map(|x| x.exp()).collect();
                let cost = dot(&model.eval(&ctx, &amounts)?);
                if cost < best_cost {
                    best_cost = cost;
                    best_u = u;
                }
            }
        }
    }
    let mut f = |u: &[f64]| {
        let c: Vec<f64> = u.iter().map(|x| x.exp()).collect();
        model.eval(&ctx, &c)
    };
    let (u, _) = lm_log(best_u, &lo, &hi, LM_ITERS, &|_| {}, &mut f)?;
    let peaks: Vec<f64> = u
        .iter()
        .map(|x| {
            let p = x.exp();
            if p < 2.0 * FANTASY_PEAK_MIN { 0.0 } else { p }
        })
        .collect();
    let r = model.eval(&ctx, &peaks)?;
    Ok(FantasySolution {
        peaks: [peaks[0] as f32, peaks[1] as f32, peaks[2] as f32],
        delta_e: ctx.metrics(&r).0,
    })
}

/// Solves the legacy `absorption_rgb` triple for a target sRGB color in `[0, 1]`.
///
/// See [`solve_fantasy_lab`]. The result keeps fantasy materials in their existing format: it is
/// the amplitude triple of [`legacy_rgb_bands`], not a transmittance.
#[must_use]
pub fn solve_fantasy(target_rgb: [f32; 3]) -> [f32; 3] {
    let lab = crate::color::body_color::srgb_to_lab(target_rgb.map(f64::from));
    let never = AtomicBool::new(false);
    solve_fantasy_lab(lab, &never)
        .unwrap_or_else(|Cancelled| unreachable!("a flag that nobody sets cannot cancel"))
        .peaks
}

// ---------------------------------------------------------------------------------------------
// Path-aware L*C*h body colour (seven-band basis)
// ---------------------------------------------------------------------------------------------

/// The colour a path-aware body-colour solve aims at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyColorTarget {
    /// CIE L*C*h(ab) under D65: `[L*, C*, h in degrees]`.
    pub lch: [f64; 3],
    /// The light path (mm) the colour is seen through: the stone's size.
    pub path_mm: f64,
}

/// Result of [`solve_body_color`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyColorSolution {
    /// Peak absorption coefficient per millimetre of each `BODY_COLOR_BASIS_NM` band
    /// (zero for a band the solution does not use), for `body_color_bands`.
    pub amplitudes_per_mm: [f32; 7],
    /// Delta E 2000 (D65) of the solution at `path_mm` from the target.
    pub delta_e: f64,
    /// `delta_e <= 1`: the target is reachable with this basis at this path.
    pub reachable: bool,
    /// Forward evaluations spent.
    pub evals: usize,
}

const BODY_BANDS: usize = 7;
/// Optical-density bounds (amplitude x path) of one basis band in the solve.
const BODY_OD_MIN: f64 = 1e-3;
const BODY_OD_MAX: f64 = 8.0;
/// Weight of the smoothness residual (`0.01 x` the optical-density step between neighbouring
/// bands): picks the smoothest of the many spectra that make the same colour, with a
/// negligible effect on the colour itself.
const BODY_REG: f64 = 0.01;
const BODY_LM_ITERS: usize = 60;
/// Grid points kept as Levenberg-Marquardt starts.
const BODY_SEEDS: usize = 3;

type BodyResid = [f64; 9];

/// The seven-band forward model in optical-density space `od_b = amplitude_b x path`, so the
/// solve does not depend on the path at all: only the final conversion to per-millimetre
/// amplitudes does (Beer-Lambert depends on the product).
struct BodyModel<'a, 'c> {
    ctx: &'a Ctx<'c>,
    /// `BODY_BANDS` rows of `N` samples (on the heap: the table is too big for the stack).
    spectra: Vec<[f64; N]>,
}

impl<'a, 'c> BodyModel<'a, 'c> {
    fn new(ctx: &'a Ctx<'c>) -> Self {
        let mut spectra = vec![[0.0; N]; BODY_BANDS];
        for (row, &(centre, width)) in spectra
            .iter_mut()
            .zip(crate::optics::absorption::BODY_COLOR_BASIS_NM.iter())
        {
            let band = AbsorptionBand::new(centre, width, 1.0);
            for (i, s) in row.iter_mut().enumerate() {
                *s = f64::from(band.evaluate((380 + i) as f32));
            }
        }
        Self { ctx, spectra }
    }

    fn lab(&self, od: &[f64; BODY_BANDS]) -> [f64; 3] {
        let mut xyz = [0.0; 3];
        for i in 0..N {
            let mut a = 0.0;
            for b in 0..BODY_BANDS {
                a += od[b] * self.spectra[b][i];
            }
            let t = (-a).exp();
            for k in 0..3 {
                xyz[k] += t * self.ctx.d65.w[k][i];
            }
        }
        self.ctx.d65.lab(xyz)
    }

    fn resid(&self, od: &[f64; BODY_BANDS]) -> Result<BodyResid, Cancelled> {
        self.ctx.tick()?;
        let d = delta_e_2000_residual(self.ctx.target_d65, self.lab(od));
        let mut r = [0.0; 9];
        r[..3].copy_from_slice(&d);
        for i in 0..BODY_BANDS - 1 {
            r[3 + i] = BODY_REG * (od[i] - od[i + 1]);
        }
        Ok(r)
    }
}

fn body_od(u: &[f64; BODY_BANDS]) -> [f64; BODY_BANDS] {
    u.map(f64::exp)
}

/// Solves the dense `M x M` system `a x = b` by Gaussian elimination with partial pivoting;
/// `None` when singular.
fn solve_dense<const M: usize>(mut a: [[f64; M]; M], mut b: [f64; M]) -> Option<[f64; M]> {
    for col in 0..M {
        let piv = (col..M).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in (col + 1)..M {
            let f = a[row][col] / a[col][col];
            for k in col..M {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; M];
    for row in (0..M).rev() {
        let mut s = b[row];
        for k in (row + 1)..M {
            s -= a[row][k] * x[k];
        }
        x[row] = s / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Projected Levenberg-Marquardt on the log optical densities `u` (the seven-band twin of
/// [`lm_log`], which is limited to three parameters). Returns the best point and its squared
/// residual.
fn lm_body(
    mut u: [f64; BODY_BANDS],
    lo: f64,
    hi: f64,
    model: &BodyModel<'_, '_>,
) -> Result<([f64; BODY_BANDS], f64), Cancelled> {
    const H: f64 = 1e-3;
    const MAX_STEP: f64 = 2.0;
    let sumsq = |r: &BodyResid| r.iter().map(|x| x * x).sum::<f64>();
    let mut r = model.resid(&body_od(&u))?;
    let mut cost = sumsq(&r);
    let mut lambda = 1e-2;
    for _ in 0..BODY_LM_ITERS {
        if cost < 1e-10 {
            break;
        }
        let mut jac = [[0.0; 9]; BODY_BANDS];
        for j in 0..BODY_BANDS {
            let mut up = u;
            let h = if u[j] + H > hi { -H } else { H };
            up[j] += h;
            let rp = model.resid(&body_od(&up))?;
            for k in 0..9 {
                jac[j][k] = (rp[k] - r[k]) / h;
            }
        }
        let mut jtj = [[0.0; BODY_BANDS]; BODY_BANDS];
        let mut jtr = [0.0; BODY_BANDS];
        for i in 0..BODY_BANDS {
            for j in 0..BODY_BANDS {
                jtj[i][j] = (0..9).map(|k| jac[i][k] * jac[j][k]).sum();
            }
            jtr[i] = (0..9).map(|k| jac[i][k] * r[k]).sum();
        }
        let mut accepted = false;
        let mut step_norm = 0.0;
        let mut gain = 0.0;
        for _ in 0..4 {
            let mut a = jtj;
            for i in 0..BODY_BANDS {
                a[i][i] += lambda * jtj[i][i].max(1e-9);
            }
            let rhs = jtr.map(|v| -v);
            if let Some(delta) = solve_dense(a, rhs) {
                let mut un = u;
                for j in 0..BODY_BANDS {
                    un[j] = (u[j] + delta[j].clamp(-MAX_STEP, MAX_STEP)).clamp(lo, hi);
                }
                let rn = model.resid(&body_od(&un))?;
                let cn = sumsq(&rn);
                if cn < cost {
                    step_norm = (0..BODY_BANDS)
                        .map(|j| (un[j] - u[j]).powi(2))
                        .sum::<f64>()
                        .sqrt();
                    gain = cost - cn;
                    u = un;
                    r = rn;
                    cost = cn;
                    lambda = (lambda / 3.0).max(1e-7);
                    accepted = true;
                    break;
                }
            }
            lambda *= 5.0;
        }
        if !accepted || step_norm < 1e-6 || gain < 1e-9 * cost.max(1e-12) {
            break;
        }
    }
    Ok((u, cost))
}

/// Solves the seven per-millimetre amplitudes of the `BODY_COLOR_BASIS_NM` bands that make a
/// stone with light path `target.path_mm` show the L*C*h colour `target.lch` under D65.
///
/// Optical density is `amplitude x path`, so the same colour at twice the path needs half the
/// amplitudes (the solve itself runs in density space and only the last step divides by the
/// path). Seven Gaussians against a three-number target leave many equivalent spectra: a
/// small smoothness term ([`BODY_REG`]) picks the smoothest. The search is a fixed 3^7 grid
/// (about 2 200 evaluations, a few tens of milliseconds) whose three best points each start a
/// Levenberg-Marquardt refinement in log space; the best refined point wins, ties to the
/// earlier one. Deterministic: no threads, fixed iteration order, bit-identical for identical
/// input on one platform. A band below twice the minimum density comes back as exactly 0 (a
/// colourless target gives seven zeros).
///
/// # Errors
///
/// [`Cancelled`] if `cancel` was set during the solve.
pub fn solve_body_color(
    target: &BodyColorTarget,
    cancel: &AtomicBool,
) -> Result<BodyColorSolution, Cancelled> {
    let path = if target.path_mm.is_finite() && target.path_mm > 0.0 {
        target.path_mm
    } else {
        1.0
    };
    let lab = crate::color::body_color::lch_to_lab(target.lch);
    let ctx = Ctx::new(cancel, lab, None, 1.0, path as f32);
    let model = BodyModel::new(&ctx);
    let lo = BODY_OD_MIN.ln();
    let hi = BODY_OD_MAX.ln();
    let levels = [lo, 0.5_f64.ln(), 4.0_f64.ln()];

    // Coarse grid, first digit slowest; the best BODY_SEEDS points (earlier wins ties).
    let mut seeds: Vec<(f64, [f64; BODY_BANDS])> = Vec::with_capacity(BODY_SEEDS + 1);
    let total = 3_usize.pow(BODY_BANDS as u32);
    for code in 0..total {
        let mut u = [0.0; BODY_BANDS];
        let mut rest = code;
        for slot in u.iter_mut().rev() {
            *slot = levels[rest % 3];
            rest /= 3;
        }
        let r = model.resid(&body_od(&u))?;
        let cost = r.iter().map(|x| x * x).sum::<f64>();
        if seeds.len() < BODY_SEEDS || cost < seeds[seeds.len() - 1].0 {
            let at = seeds.iter().position(|s| cost < s.0).unwrap_or(seeds.len());
            seeds.insert(at, (cost, u));
            seeds.truncate(BODY_SEEDS);
        }
    }

    let mut best: Option<(f64, [f64; BODY_BANDS])> = None;
    for (_, start) in seeds {
        let (u, _) = lm_body(start, lo, hi, &model)?;
        let mut od = body_od(&u);
        for o in &mut od {
            if *o < 2.0 * BODY_OD_MIN {
                *o = 0.0;
            }
        }
        let de = crate::color::body_color::delta_e_2000(lab, model.lab(&od));
        if best.as_ref().is_none_or(|b| de < b.0) {
            best = Some((de, od));
        }
    }
    let (delta_e, od) = best.unwrap_or((f64::INFINITY, [0.0; BODY_BANDS]));
    Ok(BodyColorSolution {
        amplitudes_per_mm: od.map(|o| (o / path) as f32),
        delta_e,
        reachable: delta_e <= 1.0,
        evals: ctx.evals(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::body_color::{body_color, delta_e_2000};

    fn cat() -> &'static ChromophoreCatalogue {
        ChromophoreCatalogue::global()
    }

    fn forward(recipe: &ColorRecipe) -> [f64; 3] {
        let (t, _) = resolve(recipe, cat()).expect("resolves");
        body_colors(&t, f64::from(recipe.reference_path_mm), Illuminant::D65)
            .unpolarised
            .lab
    }

    #[test]
    fn subsets_are_lexicographic_and_independent() {
        assert_eq!(subsets(3, 2), vec![vec![0, 1], vec![0, 2], vec![1, 2]]);
        assert_eq!(subsets(4, 1).len(), 4);
        assert_eq!(subsets(5, 3).len(), 10);
    }

    #[test]
    fn linear_model_matches_resolve_for_ions_and_pairs() {
        // Fe + Ti (pair law) + Cr + V in corundum at several amounts.
        let ctx_cancel = AtomicBool::new(false);
        let lab_target = [60.0, 10.0, -10.0];
        let req = SolveRequest::new("corundum", lab_target, 5.0);
        let ctx = Ctx::new(&ctx_cancel, lab_target, None, 1.0, 5.0);
        let host = cat().host("corundum").expect("corundum");
        let search = Search::new(cat(), host, &req, &ctx);
        let names = ["Cr", "Fe", "Ti", "V"].map(String::from);
        let ids: Vec<&String> = names.iter().collect();
        let mut model = search.build_model(&ids, &[]);
        let amounts = [0.2, 800.0, 150.0, 120.0];
        let r = model.eval(&ctx, &amounts).expect("eval");
        let mut recipe = ColorRecipe::new("corundum", cat().data_version);
        for (n, a) in names.iter().zip(amounts) {
            recipe.set_amount(n, a);
        }
        // The linear model is the pre-budget spectrum; the budgeted (merged) tensor differs by
        // the merge dE00 that `resolve` reports and the solver's final re-score absorbs.
        let t = super::super::resolve::resolve_unbudgeted(&recipe, cat()).expect("resolves");
        let exact = body_colors(&t, 5.0, Illuminant::D65).unpolarised.lab;
        let de_model = ctx.metrics(&r).0;
        let de_exact = delta_e_2000(lab_target, exact);
        assert!(
            (de_model - de_exact).abs() < 0.01,
            "linear model {de_model} vs resolve {de_exact}"
        );
    }

    #[test]
    fn cr_only_target_selects_exactly_cr() {
        let mut r = ColorRecipe::new("corundum", cat().data_version);
        r.set_amount("Cr", 0.3);
        let res = solve_physics(cat(), "corundum", forward(&r), None, 5.0, &[]);
        assert!(res.reachable, "dE {}", res.delta_e);
        let ids: Vec<&str> = res.recipe.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["Cr"]);
        assert!(!res.capped);
    }

    #[test]
    fn solve_is_deterministic_and_cancellable() {
        let mut r = ColorRecipe::new("corundum", cat().data_version);
        r.set_amount("Cr", 0.1);
        r.set_amount("V", 300.0);
        let lab = forward(&r);
        let a = solve_physics(cat(), "corundum", lab, None, 5.0, &[]);
        let b = solve_physics(cat(), "corundum", lab, None, 5.0, &[]);
        assert_eq!(a, b);

        let flag = AtomicBool::new(true);
        let req = SolveRequest::new("corundum", lab, 5.0);
        assert_eq!(solve_physics_with(cat(), &req, &flag), Err(Cancelled));
    }

    #[test]
    fn locks_are_kept_exactly() {
        let lab = [45.0, 40.0, 20.0];
        let locked = [("Cr".to_string(), 0.05)];
        let res = solve_physics(cat(), "corundum", lab, None, 5.0, &locked);
        assert!((res.recipe.amount("Cr") - 0.05).abs() < 1e-12);
    }

    #[test]
    fn colorless_target_returns_the_empty_recipe() {
        let res = solve_physics(cat(), "corundum", [100.0, 0.0, 0.0], None, 5.0, &[]);
        assert!(res.reachable);
        assert_eq!(res.recipe.entries.len(), 0, "no entries");
        assert_eq!(res.evals, 1);
    }

    #[test]
    fn garnet_end_members_are_found_and_constrained() {
        let mut r = ColorRecipe::new("garnet_pyralspite", cat().data_version);
        r.set_amount("almandine", 0.4);
        let res = solve_physics(cat(), "garnet_pyralspite", forward(&r), None, 5.0, &[]);
        assert!(res.reachable, "dE {}", res.delta_e);
        let sum: f64 = res.recipe.entries.iter().map(|e| e.amount).sum();
        assert!(sum <= 1.0 + 1e-9, "end-member sum {sum}");
    }

    #[test]
    fn fantasy_solver_reproduces_a_legacy_band_color() {
        let peaks = [0.4_f32, 1.8, 0.9];
        let bands = legacy_rgb_bands(peaks);
        let col = body_color(
            |l| bands.iter().map(|b| f64::from(b.evaluate(l as f32))).sum(),
            1.0,
            Illuminant::D65,
        );
        let never = AtomicBool::new(false);
        let sol = solve_fantasy_lab(col.lab, &never).expect("not cancelled");
        assert!(sol.delta_e <= 1.0, "dE {}", sol.delta_e);
        // A colorless target needs no absorption at all.
        assert_eq!(solve_fantasy([1.0, 1.0, 1.0]), [0.0, 0.0, 0.0]);
        // A red pick absorbs green/blue much more than red.
        let red = solve_fantasy([0.8, 0.1, 0.1]);
        assert!(red[0] < red[1] && red[0] < red[2], "{red:?}");
    }

    fn body_target(lch: [f64; 3], path_mm: f64) -> BodyColorTarget {
        BodyColorTarget { lch, path_mm }
    }

    fn solve_body(lch: [f64; 3], path_mm: f64) -> BodyColorSolution {
        let never = AtomicBool::new(false);
        solve_body_color(&body_target(lch, path_mm), &never).expect("not cancelled")
    }

    /// The Lab of `amplitudes` at `path_mm` through the public forward model.
    fn body_lab(amplitudes: [f32; 7], path_mm: f64) -> [f64; 3] {
        let bands = crate::optics::absorption::body_color_bands(amplitudes);
        body_color(
            |l| bands.iter().map(|b| f64::from(b.evaluate(l as f32))).sum(),
            path_mm,
            Illuminant::D65,
        )
        .lab
    }

    #[test]
    fn body_color_round_trips_a_blue_at_5_mm() {
        // A known blue: absorbs yellow through red.
        let lab = body_lab([0.0, 0.0, 0.0, 0.05, 0.15, 0.3, 0.2], 5.0);
        let lch = crate::color::body_color::lab_to_lch(lab);
        let sol = solve_body(lch, 5.0);
        assert!(sol.reachable && sol.delta_e < 1.0, "dE {}", sol.delta_e);
        // The solution, rebuilt through the public bands, lands on the target.
        let back = body_lab(sol.amplitudes_per_mm, 5.0);
        assert!(delta_e_2000(lab, back) < 1.0, "{back:?} vs {lab:?}");
        // A directly typed mid-saturation blue is reachable too.
        let typed = solve_body([50.0, 30.0, 265.0], 5.0);
        assert!(typed.reachable, "dE {}", typed.delta_e);
    }

    #[test]
    fn body_color_clear_is_all_zero() {
        let sol = solve_body([100.0, 0.0, 0.0], 5.0);
        assert_eq!(sol.amplitudes_per_mm, [0.0; 7]);
        assert!(sol.reachable);
    }

    #[test]
    fn body_color_doubling_the_path_halves_the_amplitudes() {
        let a = solve_body([55.0, 35.0, 140.0], 5.0);
        let b = solve_body([55.0, 35.0, 140.0], 10.0);
        for (x, y) in a.amplitudes_per_mm.iter().zip(b.amplitudes_per_mm) {
            assert!(
                (x - 2.0 * y).abs() <= 1e-4 * x.abs().max(1e-3),
                "{x} vs 2*{y}"
            );
        }
        assert!((a.delta_e - b.delta_e).abs() < 1e-6);
    }

    #[test]
    fn body_color_solve_is_deterministic() {
        let a = solve_body([60.0, 45.0, 30.0], 7.5);
        let b = solve_body([60.0, 45.0, 30.0], 7.5);
        assert_eq!(
            a.amplitudes_per_mm.map(f32::to_bits),
            b.amplitudes_per_mm.map(f32::to_bits)
        );
        assert_eq!(a.delta_e.to_bits(), b.delta_e.to_bits());
        assert_eq!(a.evals, b.evals);
    }

    #[test]
    fn body_color_solve_honours_cancellation() {
        let stop = AtomicBool::new(true);
        assert_eq!(
            solve_body_color(&body_target([50.0, 20.0, 90.0], 5.0), &stop),
            Err(Cancelled)
        );
    }
}
