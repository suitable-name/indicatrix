//! The pure half of the path-aware L*C*h body-colour editor.
//!
//! The editor lives in Design settings > Body Color > "Custom...": solving a typed Tone /
//! Saturation / Hue for the design's reference path, the swatches of that colour at other sizes, the reachability badge, seeding the sliders
//! from a stored colour, and the one edit that stores the result.
//!
//! The solve is [`solve_body_color`] (seven-band, optical-density space: the same pick is
//! deeper in a larger stone). The stored form is the band rows
//! (`MaterialSelection::body_color_bands_override`, amplitudes per millimetre) NEXT TO the
//! nearest legacy triple (`body_color_override`), so an older build still shows a close
//! colour. The path scale is left to the renderer: with the design's girdle diameter set
//! (`Design::girdle_diameter_mm`) it scales model units to millimetres itself
//! (`render_setup::apply_material_overrides`).
//!
//! No Slint here, so the rules are unit-testable without a window.

use std::sync::atomic::AtomicBool;

use indicatrix::{
    color::body_color::{body_color_swatches, lab_to_lch, srgb_to_lab},
    optics::{
        absorption::{BODY_COLOR_BASIS_NM, body_color_bands},
        chromophore::{BodyColorTarget, solve_body_color},
    },
};
use indicatrix_cut_core::{
    MaterialSelection,
    material::color::{fantasy_lab, nearest_legacy_rgb},
};

/// The fixed paths (millimetres) every solved colour is shown at, before the stone's own.
pub const SWATCH_PATHS_MM: [f64; 3] = [3.0, 5.0, 10.0];

/// The colour a typed (or picked) L*C*h solved to, for one path.
#[derive(Debug, Clone, PartialEq)]
pub struct LchSolve {
    /// The typed target `[L*, C*, h]`.
    pub lch: [f64; 3],
    /// The reference path (mm) the colour was solved for: the stone's own.
    pub path_mm: f64,
    /// `Delta E 2000` of the solved colour from the target.
    pub delta_e: f64,
    /// Whether the solver reached the target (within its tolerance).
    pub reachable: bool,
    /// The band rows `[centre_nm, width_nm, amplitude_per_mm]` (zero bands dropped; empty
    /// for a colourless solve).
    pub bands: Vec<[f32; 3]>,
    /// The nearest legacy triple of the solved colour at the reference path.
    pub triple: [f32; 3],
    /// sRGB (0..1) of the solved colour at each of [`SWATCH_PATHS_MM`], then the stone's own path.
    pub swatches: Vec<[f32; 3]>,
}

/// Solves `lch` for `path_mm`; `None` when `cancel` was set (a newer request superseded it).
#[must_use]
pub fn solve_lch(lch: [f64; 3], path_mm: f64, cancel: &AtomicBool) -> Option<LchSolve> {
    let target = BodyColorTarget { lch, path_mm };
    let solution = solve_body_color(&target, cancel).ok()?;
    let bands = body_color_bands(solution.amplitudes_per_mm)
        .iter()
        .map(|band| [band.center_nm, band.width_nm, band.peak])
        .collect();
    let swatches = swatches_for(&solution.amplitudes_per_mm, path_mm);
    let at_stone = swatches.last().copied().unwrap_or([1.0; 3]);
    let triple = nearest_legacy_rgb(srgb_to_lab(at_stone.map(f64::from)));
    Some(LchSolve {
        lch,
        path_mm,
        delta_e: solution.delta_e,
        reachable: solution.reachable,
        bands,
        triple,
        swatches,
    })
}

/// The colour of `amplitudes` (per mm) at 3 mm, 5 mm, 10 mm and `path_mm`.
fn swatches_for(amplitudes: &[f32; 7], path_mm: f64) -> Vec<[f32; 3]> {
    let mut paths = SWATCH_PATHS_MM.to_vec();
    paths.push(path_mm);
    body_color_swatches(amplitudes, &paths)
}

/// An sRGB swatch (0..1) as the bytes the UI paints.
#[must_use]
pub fn swatch_bytes(rgb: [f32; 3]) -> [u8; 3] {
    rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// The captions under the swatches: the fixed paths, then "This stone" with its own path.
#[must_use]
pub fn swatch_labels(path_mm: f64) -> Vec<String> {
    let mut labels: Vec<String> = SWATCH_PATHS_MM
        .iter()
        .map(|mm| format!("{mm:.0} mm"))
        .collect();
    labels.push(format!("This stone {path_mm:.1} mm"));
    labels
}

/// The badge under the swatches.
#[must_use]
pub fn badge_text(delta_e: f64, reachable: bool) -> String {
    if reachable {
        format!("Reachable (Delta E {delta_e:.1})")
    } else {
        format!(
            "Not fully reachable at this size: the closest colour is Delta E {delta_e:.1} away. Try less saturation or a lighter tone."
        )
    }
}

/// The reference path as a multiple of the girdle width. A copy of the desktop's
/// `physics_state::GIRDLE_PATH_FACTOR` (the physics colour editor's convention); a desktop
/// test pins the two together.
pub const GIRDLE_PATH_FACTOR: f64 = 1.5;
/// The reference path (mm) of a design with no girdle diameter; a copy of the desktop's
/// `physics_state::DEFAULT_PATH_MM`.
pub const DEFAULT_PATH_MM: f64 = 5.0;

/// The reference path (mm) of a design whose girdle is `girdle_diameter_mm` wide: the same
/// number the physics colour editor uses (`default_reference_path_mm`).
#[must_use]
pub fn reference_path_mm(girdle_diameter_mm: Option<f64>) -> f64 {
    girdle_diameter_mm
        .filter(|mm| *mm > 0.0)
        .map_or(DEFAULT_PATH_MM, |mm| mm * GIRDLE_PATH_FACTOR)
}

/// `[L*, C*, h]` the editor opens on for a design's stored colour at `path_mm`.
///
/// From the bands when it has them, else from the triple; `None` for a design with the material's own
/// colour (the caller then keeps the editor's default).
#[must_use]
pub fn lch_from_material(material: &MaterialSelection, path_mm: f64) -> Option<[f64; 3]> {
    if let Some(bands) = material
        .body_color_bands_override
        .as_deref()
        .filter(|rows| !rows.is_empty())
    {
        let amplitudes = amplitudes_from_rows(bands);
        let swatch = *swatches_for(&amplitudes, path_mm).last()?;
        return Some(lab_to_lch(srgb_to_lab(swatch.map(f64::from))));
    }
    material
        .body_color_override
        .map(|triple| lab_to_lch(fantasy_lab(triple)))
}

/// The seven basis amplitudes of stored band rows (a basis band with no row is zero).
fn amplitudes_from_rows(rows: &[[f32; 3]]) -> [f32; 7] {
    let mut amplitudes = [0.0f32; 7];
    for (slot, (centre, _sigma)) in amplitudes.iter_mut().zip(BODY_COLOR_BASIS_NM) {
        if let Some(row) = rows.iter().find(|row| (row[0] - centre).abs() < 0.5) {
            *slot = row[2];
        }
    }
    amplitudes
}

/// The slider range of Saturation (C*): the picked colour is clamped into it.
pub const MAX_CHROMA: f64 = 130.0;

/// The L*C*h of a picked screen colour (sRGB 0..1), clamped into the sliders' ranges.
#[must_use]
pub fn lch_from_srgb(srgb: [f64; 3]) -> [f64; 3] {
    let [l, c, h] = lab_to_lch(srgb_to_lab(srgb));
    [
        l.clamp(0.0, 100.0),
        c.clamp(0.0, MAX_CHROMA),
        h.rem_euclid(360.0),
    ]
}

/// The selection with `solve` stored as the design's body colour (bands plus the nearest
/// triple), or `None` when the design already has exactly that (no undo step is added).
#[must_use]
pub fn recoloured_with_solve(
    material: &MaterialSelection,
    solve: &LchSolve,
) -> Option<MaterialSelection> {
    let bands = (!solve.bands.is_empty()).then(|| solve.bands.clone());
    let next = material
        .clone()
        .with_body_color_bands(Some(solve.triple), bands, None);
    (next != *material).then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::color::body_color::lch_to_lab;

    const BLUE: [f64; 3] = [50.0, 30.0, 265.0];

    fn solved(path_mm: f64) -> LchSolve {
        solve_lch(BLUE, path_mm, &AtomicBool::new(false)).expect("not cancelled")
    }

    fn lightness(srgb: [f32; 3]) -> f64 {
        srgb_to_lab(srgb.map(f64::from))[0]
    }

    #[test]
    fn a_solve_carries_bands_triple_and_four_swatches() {
        let solve = solved(5.0);
        assert!(!solve.bands.is_empty(), "a blue is not colourless");
        assert_eq!(solve.swatches.len(), SWATCH_PATHS_MM.len() + 1);
        assert_eq!(swatch_labels(5.0).len(), solve.swatches.len());
        assert!(solve.triple.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn the_same_colour_is_deeper_in_a_larger_stone() {
        let solve = solved(5.0);
        let [at3, at5, at10, _] = [
            solve.swatches[0],
            solve.swatches[1],
            solve.swatches[2],
            solve.swatches[3],
        ];
        assert!(lightness(at10) < lightness(at5) && lightness(at5) < lightness(at3));
    }

    #[test]
    fn the_swatch_at_the_stones_own_path_is_the_solved_colour() {
        let solve = solved(5.0);
        let at_five = solve.swatches[1];
        let at_stone = solve.swatches[3];
        for (a, b) in at_five.iter().zip(at_stone) {
            assert!((a - b).abs() < 1e-4, "{at_five:?} vs {at_stone:?}");
        }
        assert!(solve.reachable, "the typed blue is reachable at 5 mm");
    }

    #[test]
    fn a_cancelled_solve_returns_none() {
        assert!(solve_lch(BLUE, 5.0, &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn applying_stores_bands_and_triple_and_a_repeat_is_not_an_edit() {
        let solve = solved(5.0);
        let plain = MaterialSelection::none();
        let stored = recoloured_with_solve(&plain, &solve).expect("a new colour is an edit");
        assert_eq!(stored.body_color_override, Some(solve.triple));
        assert_eq!(stored.body_color_bands_override, Some(solve.bands.clone()));
        assert!(recoloured_with_solve(&stored, &solve).is_none());
    }

    #[test]
    fn a_colourless_solve_stores_no_bands() {
        let mut solve = solved(5.0);
        solve.bands.clear();
        let stored = recoloured_with_solve(&MaterialSelection::none(), &solve);
        assert_eq!(stored.and_then(|m| m.body_color_bands_override), None);
    }

    #[test]
    fn reopening_seeds_the_sliders_from_the_stored_colour() {
        let solve = solved(5.0);
        let stored = recoloured_with_solve(&MaterialSelection::none(), &solve).expect("edit");
        let [l, c, h] = lch_from_material(&stored, 5.0).expect("has a colour");
        assert!((l - BLUE[0]).abs() < 6.0, "L* {l}");
        assert!((c - BLUE[1]).abs() < 10.0, "C* {c}");
        let hue_gap = ((h - BLUE[2] + 180.0).rem_euclid(360.0) - 180.0).abs();
        assert!(hue_gap < 15.0, "h {h}");
        assert_eq!(lch_from_material(&MaterialSelection::none(), 5.0), None);
    }

    #[test]
    fn a_triple_only_design_seeds_from_the_triple() {
        let with_triple = MaterialSelection::none().with_body_color(Some([0.2, 0.4, 2.8]));
        assert!(lch_from_material(&with_triple, 5.0).is_some());
    }

    #[test]
    fn a_picked_colour_is_clamped_into_the_slider_ranges() {
        let [l, c, h] = lch_from_srgb([0.0, 0.0, 1.0]);
        assert!((0.0..=100.0).contains(&l));
        assert!((0.0..=MAX_CHROMA).contains(&c));
        assert!((0.0..360.0).contains(&h));
        let round_trip = lch_to_lab(lch_from_srgb([0.5, 0.5, 0.5]));
        assert!(round_trip[0] > 40.0 && round_trip[0] < 60.0);
    }

    #[test]
    fn swatch_bytes_clamp_and_round() {
        assert_eq!(swatch_bytes([0.0, 1.0, 0.5]), [0, 255, 128]);
        assert_eq!(swatch_bytes([-0.2, 1.7, 0.0]), [0, 255, 0]);
    }

    #[test]
    fn the_badge_names_the_distance_when_not_reachable() {
        assert!(badge_text(0.8, true).starts_with("Reachable"));
        assert!(badge_text(7.2, false).contains("7.2"));
    }

    #[test]
    fn the_reference_path_follows_the_girdle() {
        assert!(reference_path_mm(Some(10.0)) > reference_path_mm(Some(5.0)));
        assert!(reference_path_mm(None) > 0.0);
    }

    #[test]
    fn a_preset_pick_after_a_custom_one_drops_the_bands() {
        let solve = solved(5.0);
        let stored = recoloured_with_solve(&MaterialSelection::none(), &solve).expect("edit");
        let preset = stored.with_body_color(Some([0.2, 0.4, 2.8]));
        assert_eq!(preset.body_color_bands_override, None);
    }
}
