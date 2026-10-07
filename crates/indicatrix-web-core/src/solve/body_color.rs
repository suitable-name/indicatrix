//! The path-aware L*C*h body-colour solve as a Worker job (`SolveRequest::BodyColor`).
//!
//! The Design settings colour editor (`indicatrix_editor::lch_color`) solves a typed Tone /
//! Saturation / Hue for the design's reference path: a 2187-point grid plus a Levenberg-Marquardt
//! polish, far too slow for the page's UI thread on every slider release. The request carries
//! only the typed colour and the path (no design), the answer the whole [`LchSolve`] as plain
//! data, so the page shows swatches and applies the result exactly as the desktop does.

use std::sync::atomic::AtomicBool;

use indicatrix_editor::lch_color::{LchSolve, solve_lch};
use serde::{Deserialize, Serialize};

use super::SolveResponse;

/// What to solve: the typed `[L*, C*, h]` and the reference path in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BodyColorParams {
    /// The typed target `[L*, C*, h]`.
    pub lch: [f64; 3],
    /// The reference path (mm) the colour is solved for.
    pub path_mm: f64,
}

/// An [`LchSolve`] as plain data (it has no serde of its own).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyColorResultData {
    /// The typed target `[L*, C*, h]`.
    pub lch: [f64; 3],
    /// The reference path (mm) the colour was solved for.
    pub path_mm: f64,
    /// `Delta E 2000` of the solved colour from the target.
    pub delta_e: f64,
    /// Whether the solver reached the target.
    pub reachable: bool,
    /// The band rows `[centre_nm, width_nm, amplitude_per_mm]`, empty for a colourless solve.
    pub bands: Vec<[f32; 3]>,
    /// The nearest legacy triple of the solved colour.
    pub triple: [f32; 3],
    /// sRGB (0..1) swatches at the fixed paths, then the stone's own.
    pub swatches: Vec<[f32; 3]>,
}

impl From<LchSolve> for BodyColorResultData {
    fn from(solve: LchSolve) -> Self {
        Self {
            lch: solve.lch,
            path_mm: solve.path_mm,
            delta_e: solve.delta_e,
            reachable: solve.reachable,
            bands: solve.bands,
            triple: solve.triple,
            swatches: solve.swatches,
        }
    }
}

impl BodyColorResultData {
    /// The [`LchSolve`] this came from.
    #[must_use]
    pub fn into_solve(self) -> LchSolve {
        LchSolve {
            lch: self.lch,
            path_mm: self.path_mm,
            delta_e: self.delta_e,
            reachable: self.reachable,
            bands: self.bands,
            triple: self.triple,
            swatches: self.swatches,
        }
    }
}

/// Solves `params`: [`SolveResponse::BodyColor`], [`SolveResponse::Cancelled`] when `cancel`
/// was set, or [`SolveResponse::AnalysisFailed`] for a colour or path that is not a number.
#[must_use]
pub fn run_body_color(params: &BodyColorParams, cancel: &AtomicBool) -> SolveResponse {
    if !params.lch.iter().all(|v| v.is_finite())
        || !params.path_mm.is_finite()
        || params.path_mm <= 0.0
    {
        return SolveResponse::AnalysisFailed {
            message: "the colour or the path length is not a usable number".to_string(),
            missing_anchor: false,
        };
    }
    solve_lch(params.lch, params.path_mm, cancel).map_or(SolveResponse::Cancelled, |solve| {
        SolveResponse::BodyColor(solve.into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUE: [f64; 3] = [50.0, 30.0, 265.0];

    #[test]
    fn a_solve_round_trips_through_its_plain_data() {
        let params = BodyColorParams {
            lch: BLUE,
            path_mm: 5.0,
        };
        let SolveResponse::BodyColor(data) = run_body_color(&params, &AtomicBool::new(false))
        else {
            panic!("a blue solves");
        };
        assert_eq!(data.lch, BLUE);
        assert!((data.path_mm - 5.0).abs() < 1e-12);
        assert_ne!(data.bands.len(), 0);
        let direct = solve_lch(BLUE, 5.0, &AtomicBool::new(false)).expect("not cancelled");
        assert_eq!(
            data.clone().into_solve(),
            direct,
            "the worker solves what the page would"
        );
        assert_eq!(BodyColorResultData::from(direct), data);
    }

    #[test]
    fn a_cancelled_or_unusable_request_is_answered_without_a_colour() {
        let params = BodyColorParams {
            lch: BLUE,
            path_mm: 5.0,
        };
        assert_eq!(
            run_body_color(&params, &AtomicBool::new(true)),
            SolveResponse::Cancelled
        );
        for bad in [
            BodyColorParams {
                path_mm: 0.0,
                ..params
            },
            BodyColorParams {
                lch: [f64::NAN, 30.0, 265.0],
                ..params
            },
        ] {
            assert!(matches!(
                run_body_color(&bad, &AtomicBool::new(false)),
                SolveResponse::AnalysisFailed { .. }
            ));
        }
    }
}
