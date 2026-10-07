//! Active color mode (Fantasy vs Physics), color mode storage, and recipe undo history.
//!
//! [`ColorMode`] is the one typed payload every store keeps for a custom material's color
//! (spec 5/6): the vault row's `color_recipe_json` and the native file's
//! [`indicatrix_formats::native::ColorRecipeDto::recipe_json`] both hold its JSON
//! ([`ColorMode::to_json`]), so switching modes never loses the inactive payload.

use serde::{Deserialize, Serialize};

use indicatrix::{
    color::body_color::{Illuminant, body_colors},
    optics::{
        absorption::{AbsorptionTensor, legacy_rgb_bands},
        chromophore::{
            Cancelled, ChromophoreCatalogue, ColorRecipe, SolveRequest, SolveResult,
            resolve_fluorescence, solve_fantasy_lab, solve_physics_with,
        },
        fluorescence::Fluorescence,
    },
};
use std::sync::atomic::AtomicBool;

/// The path (in model units) at which a fantasy color's three legacy bands are shown and
/// compared: the fantasy swatch convention of spec 3.5.
pub const FANTASY_PATH_UNITS: f64 = 1.0;

/// Active color mode in custom materials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ActiveColor {
    /// Historical fantasy mode: 3 Gaussian bands parameterized by an [R, G, B] triple.
    #[default]
    Fantasy,
    /// Physically based mode: absorption derived from host crystal and chromophore concentrations.
    Physics,
}

/// Material color state holding both the fantasy payload and the physical recipe payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorMode {
    /// Active color mode.
    pub active: ActiveColor,
    /// Legacy fantasy [R, G, B] absorption triple.
    pub fantasy_rgb: [f32; 3],
    /// Last authored or solved physical color recipe (kept while fantasy is active).
    pub last_recipe: Option<ColorRecipe>,
}

impl Default for ColorMode {
    fn default() -> Self {
        Self {
            active: ActiveColor::Fantasy,
            fantasy_rgb: [0.0, 0.0, 0.0],
            last_recipe: None,
        }
    }
}

/// The body color (D65, unpolarised, `Lab`) of a fantasy `rgb` triple at
/// [`FANTASY_PATH_UNITS`].
#[must_use]
pub fn fantasy_lab(rgb: [f32; 3]) -> [f64; 3] {
    let tensor = AbsorptionTensor::isotropic(legacy_rgb_bands(rgb));
    body_colors(&tensor, FANTASY_PATH_UNITS, Illuminant::D65)
        .unpolarised
        .lab
}

/// The legacy `absorption_rgb` triple closest to `target_lab`.
///
/// "Closest" is measured in D65 at [`FANTASY_PATH_UNITS`]: the fantasy solver of spec 4.6 (the
/// same grid + Levenberg-Marquardt machinery as the physics solver, in `indicatrix`) and the
/// fallback an older build shows for a physics material. Three Gaussians reach a thin set of colors, so the
/// result is the closest reachable one, not necessarily exact.
#[must_use]
pub fn nearest_legacy_rgb(target_lab: [f64; 3]) -> [f32; 3] {
    let never = AtomicBool::new(false);
    solve_fantasy_lab(target_lab, &never)
        .map_or_else(|Cancelled| [0.0; 3], |solution| solution.peaks)
}

/// Solves `host_id` for `target_lab` (D65) on the calling thread.
///
/// Stops early (`None`, never a truncated result) when `cancel` is set. The single call site of
/// the cancellable solver in this crate, so a change of its signature lands here.
#[must_use]
pub fn solve_cancellable(
    catalogue: &ChromophoreCatalogue,
    host_id: &str,
    target_lab: [f64; 3],
    reference_path_mm: f32,
    locked: &[(String, f64)],
    cancel: &AtomicBool,
) -> Option<SolveResult> {
    let request = SolveRequest {
        locked,
        ..SolveRequest::new(host_id, target_lab, reference_path_mm)
    };
    solve_physics_with(catalogue, &request, cancel).ok()
}

/// [`solve_cancellable`] that cannot be cancelled.
#[must_use]
pub fn solve_blocking(
    catalogue: &ChromophoreCatalogue,
    host_id: &str,
    target_lab: [f64; 3],
    reference_path_mm: f32,
    locked: &[(String, f64)],
) -> SolveResult {
    let request = SolveRequest {
        locked,
        ..SolveRequest::new(host_id, target_lab, reference_path_mm)
    };
    let never = AtomicBool::new(false);
    solve_physics_with(catalogue, &request, &never)
        .unwrap_or_else(|Cancelled| unreachable!("a flag nobody sets cannot cancel"))
}

impl ColorMode {
    /// Creates a fantasy color mode from an RGB triple.
    #[must_use]
    pub const fn fantasy(rgb: [f32; 3]) -> Self {
        Self {
            active: ActiveColor::Fantasy,
            fantasy_rgb: rgb,
            last_recipe: None,
        }
    }

    /// Creates a physics color mode from a recipe and a fallback legacy RGB triple.
    #[must_use]
    pub const fn physics(recipe: ColorRecipe, fallback_rgb: [f32; 3]) -> Self {
        Self {
            active: ActiveColor::Physics,
            fantasy_rgb: fallback_rgb,
            last_recipe: Some(recipe),
        }
    }

    /// Whether the material renders from its recipe: physics is active and a recipe exists.
    #[must_use]
    pub const fn is_physics(&self) -> bool {
        matches!(self.active, ActiveColor::Physics) && self.last_recipe.is_some()
    }

    /// The target color (D65 `Lab`) of the current fantasy payload.
    #[must_use]
    pub fn fantasy_target_lab(&self) -> [f64; 3] {
        fantasy_lab(self.fantasy_rgb)
    }

    /// The body color (D65, unpolarised, `Lab`) the stored recipe renders, from its
    /// `resolved_bands` (never a re-resolve) at its own reference path.
    #[must_use]
    pub fn recipe_lab(&self) -> Option<[f64; 3]> {
        let recipe = self.last_recipe.as_ref()?;
        let tensor = recipe.resolved_bands.to_tensor();
        Some(
            body_colors(
                &tensor,
                f64::from(recipe.reference_path_mm),
                Illuminant::D65,
            )
            .unpolarised
            .lab,
        )
    }

    /// The fluorescence the material renders with: the emitters of the stored recipe while
    /// physics is active (`resolve_fluorescence`, from the recipe's elements and
    /// concentrations, the same ones that give the body color), empty for a fantasy color or
    /// a recipe without emitters.
    #[must_use]
    pub fn fluorescence(&self, catalogue: &ChromophoreCatalogue) -> Fluorescence {
        match (self.is_physics(), self.last_recipe.as_ref()) {
            (true, Some(recipe)) => resolve_fluorescence(catalogue, recipe),
            _ => Fluorescence::new(Vec::new()),
        }
    }

    /// Switches mode to Fantasy without discarding the physics recipe.
    ///
    /// A fantasy payload still at its default (no color) is seeded from the physics color
    /// through the legacy solver (spec 5); a fantasy color that was ever set is never
    /// overwritten, and the recipe is always kept.
    pub fn switch_to_fantasy(&mut self) {
        self.active = ActiveColor::Fantasy;
        if self.fantasy_rgb == [0.0; 3]
            && let Some(lab) = self.recipe_lab()
        {
            self.fantasy_rgb = nearest_legacy_rgb(lab);
        }
    }

    /// Whether switching to Physics still needs a recipe solved from the fantasy color.
    #[must_use]
    pub const fn needs_initial_solve(&self) -> bool {
        self.last_recipe.is_none()
    }

    /// Switches mode to Physics without discarding the fantasy RGB triple. With no
    /// `last_recipe` the *current fantasy color* is solved into `host_id` (closest reachable),
    /// on the calling thread -- the desktop dialog instead calls
    /// [`Self::needs_initial_solve`] and solves on its worker.
    pub fn switch_to_physics(&mut self, host_id: &str, catalogue: &ChromophoreCatalogue) {
        self.active = ActiveColor::Physics;
        if self.last_recipe.is_none() {
            let res = solve_blocking(catalogue, host_id, self.fantasy_target_lab(), 5.0, &[]);
            self.last_recipe = Some(res.recipe);
        }
    }

    /// Installs `recipe` as the physics payload and makes Physics active.
    pub fn install_recipe(&mut self, recipe: ColorRecipe) {
        self.active = ActiveColor::Physics;
        self.last_recipe = Some(recipe);
    }

    /// Resolves the effective absorption tensor for this color mode, always from the stored
    /// `resolved_bands` (never a silent re-resolve).
    #[must_use]
    pub fn resolve_tensor(&self) -> AbsorptionTensor {
        match (self.active, self.last_recipe.as_ref()) {
            (ActiveColor::Physics, Some(recipe)) => recipe.resolved_bands.to_tensor(),
            _ => AbsorptionTensor::isotropic(legacy_rgb_bands(self.fantasy_rgb)),
        }
    }

    /// The legacy `absorption_rgb` an older build shows for this material: the fantasy triple
    /// itself, or -- while physics is active -- the nearest legacy color of the recipe's
    /// rendered color.
    #[must_use]
    pub fn fallback_rgb(&self) -> [f32; 3] {
        if self.is_physics() {
            self.recipe_lab()
                .map_or(self.fantasy_rgb, nearest_legacy_rgb)
        } else {
            self.fantasy_rgb
        }
    }

    /// Whether the stored recipe was built with older catalogue data than `catalogue`'s (the
    /// "color data updated" badge). Rendering still uses the stored `resolved_bands`.
    #[must_use]
    pub fn data_outdated(&self, catalogue: &ChromophoreCatalogue) -> bool {
        self.last_recipe
            .as_ref()
            .is_some_and(|r| r.data_version < catalogue.data_version)
    }

    /// The JSON both stores keep.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parses stored JSON: a [`ColorMode`], or -- for the bare recipe an interim build wrote --
    /// a [`ColorRecipe`] (read as active physics). `None` for blank or unreadable text.
    #[must_use]
    pub fn from_json(json: &str) -> Option<Self> {
        let json = json.trim();
        if json.is_empty() {
            return None;
        }
        serde_json::from_str::<Self>(json).ok().or_else(|| {
            serde_json::from_str::<ColorRecipe>(json)
                .ok()
                .map(|recipe| Self::physics(recipe, [0.0; 3]))
        })
    }
}

/// History stack for the material editor dialog (cap 50).
#[derive(Debug, Clone, Default)]
pub struct RecipeHistory {
    undo_stack: Vec<ColorRecipe>,
    redo_stack: Vec<ColorRecipe>,
}

impl RecipeHistory {
    /// Maximum capacity of the undo stack.
    pub const CAPACITY: usize = 50;

    /// Builds a new empty history stack.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pushes a recipe before modification, clearing the redo stack.
    pub fn push(&mut self, recipe: ColorRecipe) {
        if self.undo_stack.len() >= Self::CAPACITY {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(recipe);
        self.redo_stack.clear();
    }

    /// Undoes to the previous state, saving `current` to the redo stack.
    pub fn undo(&mut self, current: ColorRecipe) -> Option<ColorRecipe> {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(current);
            Some(prev)
        } else {
            None
        }
    }

    /// Redoes to the next state, saving `current` to the undo stack.
    pub fn redo(&mut self, current: ColorRecipe) -> Option<ColorRecipe> {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(current);
            Some(next)
        } else {
            None
        }
    }

    /// Whether an undo operation is available.
    #[must_use]
    pub const fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Whether a redo operation is available.
    #[must_use]
    pub const fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{color::body_color::delta_e_2000, optics::chromophore::resolve};

    fn ruby() -> ColorRecipe {
        let cat = ChromophoreCatalogue::global();
        let mut recipe = ColorRecipe::new("corundum", cat.data_version);
        recipe.set_amount("Cr", 0.3);
        let (tensor, _) = resolve(&recipe, cat).expect("ruby resolves");
        recipe.resolved_bands =
            indicatrix::optics::chromophore::ResolvedBands::from_tensor(&tensor);
        recipe
    }

    #[test]
    fn mode_switching_preserves_both_payloads() {
        let mut mode = ColorMode::fantasy([0.1, 0.2, 0.3]);
        assert_eq!(mode.active, ActiveColor::Fantasy);
        assert_eq!(mode.fantasy_rgb, [0.1, 0.2, 0.3]);

        let cat = ChromophoreCatalogue::global();
        mode.switch_to_physics("corundum", cat);
        assert_eq!(mode.active, ActiveColor::Physics);
        assert!(mode.last_recipe.is_some());
        assert_eq!(mode.fantasy_rgb, [0.1, 0.2, 0.3]);

        mode.switch_to_fantasy();
        assert_eq!(mode.active, ActiveColor::Fantasy);
        assert_eq!(mode.fantasy_rgb, [0.1, 0.2, 0.3]);
        assert!(
            mode.last_recipe.is_some(),
            "the recipe survives a switch to fantasy"
        );
    }

    /// The first switch to physics solves the *current fantasy color*, not a constant:
    /// two different fantasy colors must give different recipes.
    #[test]
    fn first_switch_to_physics_solves_the_fantasy_color() {
        let cat = ChromophoreCatalogue::global();
        let mut red = ColorMode::fantasy([0.2, 2.8, 2.4]);
        let mut blue = ColorMode::fantasy([2.8, 1.2, 0.1]);
        red.switch_to_physics("corundum", cat);
        blue.switch_to_physics("corundum", cat);
        assert_ne!(red.last_recipe, blue.last_recipe);
    }

    /// Saving while fantasy is active keeps the recipe in the JSON both stores hold.
    #[test]
    fn json_round_trip_keeps_both_payloads_in_fantasy_mode() {
        let mut mode = ColorMode::physics(ruby(), [0.4, 0.9, 1.8]);
        mode.switch_to_fantasy();
        let back = ColorMode::from_json(&mode.to_json()).expect("parses");
        assert_eq!(back, mode);
        assert_eq!(back.active, ActiveColor::Fantasy);
        assert!(back.last_recipe.is_some());
        assert_eq!(ColorMode::from_json("  "), None);
        assert_eq!(ColorMode::from_json("{not json"), None);
    }

    /// An interim build wrote a bare recipe; it still reads (as active physics).
    #[test]
    fn a_bare_recipe_json_reads_as_physics() {
        let recipe = ruby();
        let json = serde_json::to_string(&recipe).expect("serialises");
        let mode = ColorMode::from_json(&json).expect("parses");
        assert!(mode.is_physics());
        assert_eq!(mode.last_recipe, Some(recipe));
    }

    #[test]
    fn data_outdated_compares_the_recipe_version_with_the_catalogue() {
        let cat = ChromophoreCatalogue::global();
        let mut recipe = ruby();
        let mode = ColorMode::physics(recipe.clone(), [0.0; 3]);
        assert!(!mode.data_outdated(cat));
        recipe.data_version = cat.data_version.saturating_sub(1);
        let old = ColorMode::physics(recipe, [0.0; 3]);
        assert!(old.data_outdated(cat));
        assert!(!ColorMode::fantasy([0.0; 3]).data_outdated(cat));
    }

    /// An older build reads only the top-level `absorption_rgb` (the fallback): the color it
    /// shows must be near the physics color (spec 11.9, `DeltaE00` <= 15).
    #[test]
    fn the_fallback_rgb_is_close_to_the_physics_color() {
        let cat = ChromophoreCatalogue::global();
        let mut recipes = vec![ruby()];
        for (host, id, amount) in [("corundum", "Fe", 1000.0), ("beryl", "Cr", 0.2)] {
            let mut r = ColorRecipe::new(host, cat.data_version);
            r.set_amount(id, amount);
            if host == "corundum" {
                r.set_amount("Ti", 200.0);
            }
            let (t, _) = resolve(&r, cat).expect("resolves");
            r.resolved_bands = indicatrix::optics::chromophore::ResolvedBands::from_tensor(&t);
            recipes.push(r);
        }
        for recipe in recipes {
            let host = recipe.host.clone();
            let mode = ColorMode::physics(recipe, [0.0; 3]);
            let fallback = mode.fallback_rgb();
            let physics = mode.recipe_lab().expect("has a recipe");
            let de = delta_e_2000(physics, fantasy_lab(fallback));
            assert!(
                de <= 15.0,
                "{host}: fallback {fallback:?} is DeltaE {de:.1} from the recipe"
            );
        }
    }

    /// A fantasy color that was ever set is never overwritten by a switch; a default one is
    /// seeded from the physics color.
    #[test]
    fn switching_to_fantasy_seeds_only_a_default_payload() {
        let mut fresh = ColorMode::physics(ruby(), [0.0; 3]);
        fresh.switch_to_fantasy();
        assert_ne!(fresh.fantasy_rgb, [0.0; 3]);
        let mut set = ColorMode::physics(ruby(), [0.1, 0.2, 0.3]);
        set.switch_to_fantasy();
        assert_eq!(set.fantasy_rgb, [0.1, 0.2, 0.3]);
    }

    #[test]
    fn recipe_history_undo_redo_and_capacity() {
        let mut history = RecipeHistory::new();
        assert!(!history.can_undo());
        assert!(!history.can_redo());

        // Push 60 recipes to test capacity cap of 50
        for i in 0..60 {
            let mut r = ColorRecipe::new("corundum", 1);
            r.strength = f64::from(i);
            history.push(r);
        }

        assert!(history.can_undo());
        let mut current = ColorRecipe::new("corundum", 1);
        current.strength = 100.0;
        let undone = history.undo(current).expect("undo available");
        // Oldest 10 dropped, so top of undo was 59
        assert_eq!(undone.strength, 59.0);
        assert!(history.can_redo());

        let redone = history.redo(undone).expect("redo available");
        assert_eq!(redone.strength, 100.0); // original current passed into undo
    }
}
