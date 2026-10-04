//! The material editor's physics-color state: the recipe being edited, its undo stack,
//! the locks, the picked color and everything derived from them (rows, swatches, banner).
//!
//! Pure Rust, no Slint: [`PhysicsState`] is edited by the callbacks in [`super::physics_ui`] and
//! turned into plain [`PhysicsView`] data, so every rule here -- the log sliders, the
//! fractions, "impossible" items, the undo stack, the dirty test, mode switching -- is unit
//! tested without a window. Rendering always uses the recipe's stored `resolved_bands`
//! (never a silent re-resolve): an edit re-resolves with the current catalogue, which is the
//! only way the stored bands change.

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use indicatrix::{
    color::body_color::{Bodycolor, Bodycolors, Illuminant, body_colors, delta_e_2000},
    optics::{
        chromophore::{
            ChromophoreCatalogue, ChromophoreData, GlowStrength, HostData, ResolvedBands,
            SolveResult, UV365_NM, UV395_NM, UvGlow, colorRecipe, fluorescence_report,
            garnet_optics, resolve, species_element,
        },
        materials::{CrystalSystem, GemMaterial, OpticalCharacter},
    },
};
use indicatrix_cut_core::material::{
    Activecolor, RecipeHistory, built_in_specific_gravity, colorMode,
};

use super::crystal_optics::{crystal_system_to_index, optical_character_to_index};

/// Reference path (mm) when no design with a size is open.
pub const DEFAULT_PATH_MM: f32 = 5.0;
/// `reference_path_mm` defaults to this multiple of the open design's girdle diameter, a rough
/// proxy for the table-to-culet path plus return (spec 7).
pub const GIRDLE_PATH_FACTOR: f32 = 1.5;
/// An amount of 0 sits at the left end of a log slider; the smallest non-zero amount is
/// `LOG_SPAN * max` (the solver's own `c_min`).
pub const LOG_SPAN: f64 = 1e-3;
/// The slider strength bounds.
pub const STRENGTH_RANGE: (f64, f64) = (0.05, 10.0);
/// D65 delta-E above which a pick counts as "closest reachable" (`SolveResult::reachable`).
pub const REACHABLE_DELTA_E: f64 = 1.0;
/// D65 delta-E above which the mismatch is shown in the warning color.
pub const WARN_DELTA_E: f64 = 2.0;
/// color-change label threshold (spec 3.5).
pub const color_CHANGE_DELTA_E: f64 = 5.0;

/// The default reference path for a design whose girdle is `stone_width_mm` (`<= 0`: no design).
#[must_use]
pub fn default_reference_path_mm(stone_width_mm: f32) -> f32 {
    if stone_width_mm > 0.0 {
        stone_width_mm * GIRDLE_PATH_FACTOR
    } else {
        DEFAULT_PATH_MM
    }
}

/// Log-scale slider position `0..=1` of `amount` in `[0, max]`.
#[must_use]
pub fn amount_to_pos(amount: f64, max: f64) -> f64 {
    let c_min = LOG_SPAN * max;
    if max <= 0.0 || amount <= c_min {
        return 0.0;
    }
    (amount / c_min).log(max / c_min).clamp(0.0, 1.0)
}

/// Inverse of [`amount_to_pos`]; position 0 is exactly 0.
#[must_use]
pub fn pos_to_amount(pos: f64, max: f64) -> f64 {
    if max <= 0.0 || !pos.is_finite() || pos <= 0.0 {
        return 0.0;
    }
    let c_min = LOG_SPAN * max;
    (c_min * (max / c_min).powf(pos.min(1.0))).min(max)
}

/// The unit an item's amount is entered in.
#[must_use]
pub fn item_unit(host: &HostData, id: &str) -> String {
    if host.end_members.iter().any(|m| m.id == id) {
        "mol_fraction".to_string()
    } else {
        host.element_unit(id).unwrap_or_default()
    }
}

/// A human label for a concentration unit.
#[must_use]
pub fn unit_label(unit: &str) -> String {
    if let Some(oxide) = unit.strip_prefix("wt_pct_oxide:") {
        return format!("wt% {oxide}");
    }
    match unit {
        "ppm_site" => "ppm".to_string(),
        "ppma_all" => "ppma".to_string(),
        "mol_fraction" => "mol fraction".to_string(),
        other => other.to_string(),
    }
}

/// `amount` with its unit, e.g. `0.200 wt% Cr2O3`, `80 ppm`.
#[must_use]
pub fn format_amount(amount: f64, unit: &str) -> String {
    let number = if unit.starts_with("wt_pct_oxide:") || unit == "mol_fraction" {
        format!("{amount:.3}")
    } else if amount >= 100.0 {
        format!("{amount:.0}")
    } else {
        format!("{amount:.1}")
    };
    format!("{number} {}", unit_label(unit))
}

/// The elements/end members a chromophore is driven by -- the same ids
/// `ChromophoreCatalogue::selectable_elements` offers, including the compensator of an
/// intervalence pair (e.g. Mg, which binds the Ti4+ of the Fe2+-Ti4+ pair).
#[must_use]
pub fn drivers(c: &ChromophoreData) -> Vec<String> {
    let mut ids = absorbers(c);
    if c.kind == "ivct_pair"
        && let Some(comp) = &c.compensator
    {
        ids.push(comp.element.clone());
    }
    ids
}

/// The elements/end members whose presence makes a chromophore absorb: [`drivers`] without
/// a pair's compensator, which on its own creates nothing.
fn absorbers(c: &ChromophoreData) -> Vec<String> {
    match c.kind.as_str() {
        "end_member" => c.end_member.iter().cloned().collect(),
        "ivct_pair" => c
            .partners
            .iter()
            .map(|p| species_element(p).to_string())
            .collect(),
        _ if c.conc_unit == "intensity" => Vec::new(),
        _ => c.elements.clone(),
    }
}

/// A typical amount of `id` in its own unit, the `c_ref` of the fractions view: the largest
/// `conc_typical` upper bound of the offered chromophores it drives (converted into the item's
/// unit), or a tenth of the maximum when the data give none.
#[must_use]
pub fn typical_amount(host: &HostData, id: &str) -> f64 {
    let max = host.element_conc_max(id);
    if host.end_members.iter().any(|m| m.id == id) {
        let typical = host
            .chromophores
            .iter()
            .filter(|c| c.is_offered() && c.end_member.as_deref() == Some(id))
            .filter_map(|c| c.conc_typical.map(|t| t[1]))
            .fold(0.0_f64, f64::max);
        return if typical > 0.0 {
            typical.min(max)
        } else {
            0.25 * max
        };
    }
    let unit = item_unit(host, id);
    let n_in = host.n_site_for_unit(&unit);
    let mut best = 0.0_f64;
    if n_in > 0.0 {
        for c in &host.chromophores {
            if !c.is_offered() || !drivers(c).iter().any(|d| d == id) {
                continue;
            }
            if let Some(t) = c.conc_typical {
                best = best.max(t[1] * host.n_site_for_unit(&c.conc_unit) / n_in);
            }
        }
    }
    if best > 0.0 { best.min(max) } else { 0.1 * max }
}

/// Why `id` cannot be added to the recipe right now (`None`: it can). Derived only from the
/// catalogue: a missing usable range, a full end-member budget, a chromophore whose `requires` /
/// `excludes` name another chromophore of the host by id, or a centre only a treatment creates.
#[must_use]
pub fn impossible_reason(host: &HostData, recipe: &colorRecipe, id: &str) -> Option<String> {
    let chromos: Vec<&ChromophoreData> = host
        .chromophores
        .iter()
        .filter(|c| c.is_offered() && drivers(c).iter().any(|d| d == id))
        .collect();
    if chromos.is_empty() || host.element_conc_max(id) <= 0.0 {
        return Some("no usable concentration range in the data".to_string());
    }
    let present: BTreeSet<&str> = recipe
        .entries
        .iter()
        .filter(|e| e.amount > 0.0)
        .map(|e| e.id.as_str())
        .collect();
    if host.end_members.iter().any(|m| m.id == id) {
        let sum: f64 = host
            .end_members
            .iter()
            .filter(|m| !m.colorless)
            .map(|m| recipe.amount(&m.id))
            .sum();
        if sum >= 1.0 - 1e-9 {
            return Some("end-member fractions already add up to 100 %".to_string());
        }
    }
    let chromophore_present = |other: &ChromophoreData| -> bool {
        absorbers(other)
            .iter()
            .any(|d| present.contains(d.as_str()))
    };
    let mut first_reason: Option<String> = None;
    for c in &chromos {
        // A chromophore with no block makes the item possible (`?` returns `None`).
        let reason = chromophore_block(host, recipe, c, &chromophore_present)?;
        first_reason.get_or_insert(reason);
    }
    first_reason
}

fn chromophore_block(
    host: &HostData,
    recipe: &colorRecipe,
    c: &ChromophoreData,
    present: &dyn Fn(&ChromophoreData) -> bool,
) -> Option<String> {
    for req in &c.requires {
        if let Some(other) = host.chromophores.iter().find(|o| &o.id == req)
            && !present(other)
        {
            return Some(format!("needs {} first", other.id));
        }
    }
    for exc in &c.excludes {
        if let Some(other) = host.chromophores.iter().find(|o| &o.id == exc)
            && present(other)
        {
            return Some(format!("excluded by {}", other.id));
        }
    }
    if host.is_treatment_created(&c.id) {
        let creators: Vec<_> = host
            .treatments
            .iter()
            .filter(|t| {
                t.effects.iter().any(|e| {
                    e.effect_type == "create_centre" && e.centre_id.as_deref() == Some(&c.id)
                })
            })
            .collect();
        if !creators.iter().any(|t| recipe.treatments.contains(&t.id)) {
            let name = creators.first().map_or("a treatment", |t| t.name.as_str());
            return Some(format!("only exists after treatment: {name}"));
        }
    }
    None
}

/// Confidence ranking, worst first.
fn confidence_rank(confidence: &str) -> u8 {
    match confidence {
        "unknown" => 0,
        "estimate" => 1,
        "secondary" => 2,
        _ => 3,
    }
}

/// Per-item data confidence: the worst confidence of the offered chromophores/bands that
/// `id` drives, plus the Sources popover text.
#[must_use]
pub fn item_sources(host: &HostData, id: &str) -> (String, String) {
    let mut worst = "verified".to_string();
    let mut text = String::new();
    for c in &host.chromophores {
        if !c.is_offered() || !drivers(c).iter().any(|d| d == id) {
            continue;
        }
        let mut conf = c.confidence.clone();
        for b in c.usable_bands() {
            if let Some(bc) = &b.confidence
                && confidence_rank(bc) < confidence_rank(&conf)
            {
                conf.clone_from(bc);
            }
        }
        if confidence_rank(&conf) < confidence_rank(&worst) {
            worst.clone_from(&conf);
        }
        let _ = writeln!(text, "{} - confidence: {conf}", c.id);
        let mut seen = BTreeSet::new();
        for b in c.usable_bands() {
            if let Some(src) = &b.source
                && seen.insert(src.clone())
                && seen.len() <= 6
            {
                let _ = writeln!(text, "  \u{2022} {src}");
            }
        }
        if seen.len() > 6 {
            let _ = writeln!(text, "  \u{2026} and {} more sources", seen.len() - 6);
        }
        let skipped = c.bands.len() - c.usable_bands().count();
        if skipped > 0 {
            let _ = writeln!(
                text,
                "  {skipped} band(s) skipped (suspect or no coefficient)"
            );
        }
    }
    if text.is_empty() {
        text.push_str("No sources recorded.");
    }
    (worst, text.trim_end().to_string())
}

/// The data-confidence banner of a host: `Some` text only when some offered coefficient is not
/// `verified`/`measured`.
#[must_use]
pub fn host_banner(host: &HostData) -> Option<String> {
    let offered: Vec<&ChromophoreData> = host
        .chromophores
        .iter()
        .filter(|c| c.is_offered())
        .collect();
    let weak = offered
        .iter()
        .filter(|c| !matches!(c.confidence.as_str(), "verified" | "measured"))
        .count();
    if weak == 0 {
        return None;
    }
    let estimates = offered
        .iter()
        .filter(|c| matches!(c.confidence.as_str(), "estimate" | "unknown"))
        .count();
    let detail = if estimates > 0 {
        format!(
            "{weak} of {} chromophores are secondary-source or estimated ({estimates} estimated)",
            offered.len()
        )
    } else {
        format!(
            "{weak} of {} chromophores are secondary-source",
            offered.len()
        )
    };
    Some(format!(
        "\u{26a0} Strengths uncalibrated \u{2014} approximate. {detail}."
    ))
}

/// The optics a host (and, for garnet-style hosts, the recipe's end-member mix) proposes.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefill {
    /// Mean refractive index (sodium D).
    pub ri: f32,
    /// Fraunhofer F-C dispersion.
    pub dispersion: f32,
    /// Birefringence.
    pub birefringence: f32,
    /// Specific gravity (`0.0`: unknown).
    pub specific_gravity: f32,
    /// Crystal-system combo index.
    pub crystal_system_idx: i32,
    /// Optical-character combo index.
    pub optical_character_idx: i32,
    /// `n_beta` - `n_alpha` (biaxial).
    pub biaxial_delta: f32,
}

/// The prefill from a host's built-in material.
fn builtin_prefill(host: &HostData, b: &GemMaterial) -> Prefill {
    let sg = host
        .materials
        .iter()
        .find_map(|n| built_in_specific_gravity(n))
        .map_or(host.density_g_cm3 as f32, |s| s.representative as f32);
    Prefill {
        ri: b.dispersion.evaluate(589.3),
        dispersion: b.dispersion.evaluate(486.1) - b.dispersion.evaluate(656.3),
        birefringence: b.birefringence_delta,
        specific_gravity: sg,
        crystal_system_idx: crystal_system_to_index(b.crystal_system),
        optical_character_idx: optical_character_to_index(b.optical_character),
        biaxial_delta: b.biaxial_delta_beta_alpha.unwrap_or(0.0),
    }
}

/// The prefill for a host with no built-in material: its optical class and density.
fn generic_prefill(host: &HostData) -> Prefill {
    let oc = match host.optical.as_str() {
        "isotropic" => OpticalCharacter::Isotropic,
        "biaxial" => OpticalCharacter::BiaxialPositive,
        _ => OpticalCharacter::UniaxialPositive,
    };
    let cs = match oc {
        OpticalCharacter::Isotropic => CrystalSystem::Cubic,
        OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative => {
            CrystalSystem::Orthorhombic
        }
        _ => CrystalSystem::Hexagonal,
    };
    Prefill {
        ri: 1.6,
        dispersion: 0.015,
        birefringence: 0.0,
        specific_gravity: host.density_g_cm3 as f32,
        crystal_system_idx: crystal_system_to_index(cs),
        optical_character_idx: optical_character_to_index(oc),
        biaxial_delta: 0.0,
    }
}

/// RI/SG/crystal system prefill for `host`; end-member hosts interpolate RI and SG over the
/// recipe's fractions (`garnet_optics`), so a garnet mix is prefilled rather than locked.
#[must_use]
pub fn host_prefill(host: &HostData, recipe: Option<&colorRecipe>) -> Prefill {
    let builtin = host.materials.iter().find_map(|name| {
        GemMaterial::all_materials()
            .into_iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
    });
    let mut out = builtin
        .as_ref()
        .map_or_else(|| generic_prefill(host), |b| builtin_prefill(host, b));
    if !host.end_members.is_empty() {
        let fractions = host.end_member_fractions(|id| recipe.map_or(0.0, |r| r.amount(id)));
        if let Some(fractions) = fractions {
            let refs: Vec<(&str, f64)> = fractions.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            let (ri, sg) = garnet_optics(&refs);
            out.ri = ri as f32;
            out.specific_gravity = sg as f32;
        }
    }
    out
}

/// What a solver request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolveKind {
    /// The user picked a color.
    Pick,
    /// The first switch to physics: solve the current fantasy color into the host.
    InitialFromFantasy,
}

/// One solver job, handed to the worker (see [`super::physics_solver`]).
#[derive(Debug, Clone)]
pub struct SolveJob {
    /// Generation stamp: only the state's latest generation may write back.
    pub generation: u64,
    /// Host id to solve in.
    pub host: String,
    /// Target color (D65 `Lab`).
    pub target_lab: [f64; 3],
    /// Reference path (mm).
    pub reference_path_mm: f32,
    /// Locked `(id, amount)` entries the solver keeps.
    pub locked: Vec<(String, f64)>,
    /// Set when a newer job superseded this one.
    pub cancel: Arc<AtomicBool>,
}

/// The picked color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    /// D65 `Lab`.
    pub lab: [f64; 3],
    /// sRGB for the swatch.
    pub srgb: [u8; 3],
}

/// How the last solve ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolveSummary {
    /// Whether the search hit its evaluation cap.
    pub capped: bool,
}

/// The dialog's physics state.
#[derive(Debug)]
pub struct PhysicsState {
    /// Both color payloads and the active one.
    pub mode: colorMode,
    /// The host the recipe (or the next recipe) is for.
    pub host_id: String,
    /// Strength + fractions view (`true`) or absolute amounts.
    pub view_fractions: bool,
    /// Items whose amounts the solver keeps.
    pub locked: BTreeSet<String>,
    /// The picked color, if any.
    pub target: Option<Target>,
    /// How the last solve ended.
    pub summary: Option<SolveSummary>,
    /// A solver job is in flight.
    pub solving: bool,
    /// The recipe undo/redo stack (50 entries).
    pub history: RecipeHistory,
    /// The open design's girdle diameter (mm); `0.0` when none.
    pub stone_width_mm: f32,
    /// Latest solver generation; results of older generations are dropped.
    pub generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    solving_kind: Option<SolveKind>,
    fantasy_picked: bool,
    drag_origin: Option<colorRecipe>,
    baseline: colorMode,
    pending_prefill: Option<Prefill>,
}

impl PhysicsState {
    /// The state a freshly opened dialog starts from: every piece of physics state is rebuilt
    /// (nothing leaks from a previous open). `stored_json` is the material's stored color JSON,
    /// `fantasy_rgb` its fantasy color when there is none, `material_name` the name used to
    /// preselect a host.
    #[must_use]
    pub fn open(
        stored_json: &str,
        fantasy_rgb: [f32; 3],
        material_name: &str,
        stone_width_mm: f32,
        catalogue: &ChromophoreCatalogue,
        previous_generation: u64,
    ) -> Self {
        let mode =
            colorMode::from_json(stored_json).unwrap_or_else(|| colorMode::fantasy(fantasy_rgb));
        let host_id = mode
            .last_recipe
            .as_ref()
            .map(|r| r.host.clone())
            .filter(|h| catalogue.host(h).is_some())
            .or_else(|| {
                catalogue
                    .host_for_material(material_name)
                    .map(|h| h.id.clone())
            })
            .or_else(|| catalogue.hosts.first().map(|h| h.id.clone()))
            .unwrap_or_default();
        Self {
            baseline: mode.clone(),
            mode,
            host_id,
            view_fractions: true,
            locked: BTreeSet::new(),
            target: None,
            summary: None,
            solving: false,
            history: RecipeHistory::new(),
            stone_width_mm,
            generation: previous_generation + 1,
            cancel: None,
            solving_kind: None,
            fantasy_picked: false,
            drag_origin: None,
            pending_prefill: None,
        }
    }

    /// Whether physics is the active mode.
    #[must_use]
    pub const fn is_physics(&self) -> bool {
        matches!(self.mode.active, Activecolor::Physics)
    }

    /// The recipe, if the material has one.
    #[must_use]
    pub const fn recipe(&self) -> Option<&colorRecipe> {
        self.mode.last_recipe.as_ref()
    }

    /// The reference path currently in force.
    #[must_use]
    pub fn reference_path_mm(&self) -> f32 {
        self.recipe().map_or_else(
            || default_reference_path_mm(self.stone_width_mm),
            |r| r.reference_path_mm,
        )
    }

    fn blank_recipe(&self, catalogue: &ChromophoreCatalogue) -> colorRecipe {
        let mut recipe = colorRecipe::new(self.host_id.clone(), catalogue.data_version);
        recipe.reference_path_mm = default_reference_path_mm(self.stone_width_mm);
        refresh(&mut recipe, catalogue);
        recipe
    }

    fn recipe_mut(&mut self, catalogue: &ChromophoreCatalogue) -> &mut colorRecipe {
        if self.mode.last_recipe.is_none() {
            let blank = self.blank_recipe(catalogue);
            self.mode.last_recipe = Some(blank);
        }
        self.mode
            .last_recipe
            .as_mut()
            .unwrap_or_else(|| unreachable!("a recipe was just installed"))
    }

    /// Pushes the current recipe on the undo stack (call before changing it).
    fn push_history(&mut self) {
        if let Some(r) = self.mode.last_recipe.clone() {
            self.history.push(r);
        }
    }

    // ---- mode ----

    /// Switches mode. Physics: keeps an existing recipe, else installs a blank one for the
    /// host and returns the solver job that fits the *current fantasy color*. Fantasy: keeps the
    /// recipe, seeds a default fantasy color from the physics color. Any in-flight solve is
    /// cancelled. `fantasy_rgb_from_dialog` is the dialog's selected preset color (`None` for
    /// "Custom (keep)", which keeps the stored payload).
    pub fn set_mode(
        &mut self,
        physics: bool,
        fantasy_rgb_from_dialog: Option<[f32; 3]>,
        catalogue: &ChromophoreCatalogue,
    ) -> Option<SolveJob> {
        self.cancel_solves();
        if let Some(rgb) = fantasy_rgb_from_dialog {
            self.mode.fantasy_rgb = rgb;
        }
        if !physics {
            self.mode.switch_to_fantasy();
            return None;
        }
        let needs_solve = self.mode.last_recipe.is_none();
        self.mode.active = Activecolor::Physics;
        if needs_solve {
            let blank = self.blank_recipe(catalogue);
            self.mode.last_recipe = Some(blank);
            self.pending_prefill = catalogue
                .host(&self.host_id)
                .map(|h| host_prefill(h, self.recipe()));
            let lab = self.mode.fantasy_target_lab();
            return Some(self.begin_solve(SolveKind::InitialFromFantasy, lab, None));
        }
        None
    }

    /// The fantasy picker's result: the legacy color fitted to the picked one.
    pub fn set_fantasy_pick(&mut self, rgb: [f32; 3]) {
        self.mode.fantasy_rgb = rgb;
        self.fantasy_picked = true;
    }

    // ---- host ----

    /// Chooses the host `host_id`: a fresh recipe for it (the old one goes on the undo stack),
    /// and a prefill of the optics.
    pub fn select_host(&mut self, host_id: &str, catalogue: &ChromophoreCatalogue) -> bool {
        let Some(host) = catalogue.host(host_id) else {
            return false;
        };
        self.cancel_solves();
        self.locked.clear();
        self.target = None;
        self.summary = None;
        if self.host_id != host_id || self.recipe().is_some_and(|r| r.host != host_id) {
            self.push_history();
            self.host_id = host_id.to_string();
            let blank = self.blank_recipe(catalogue);
            self.mode.last_recipe = Some(blank);
        }
        self.pending_prefill = Some(host_prefill(host, self.recipe()));
        true
    }

    /// The prefill proposed by the latest host/mix change, once.
    pub const fn take_prefill(&mut self) -> Option<Prefill> {
        self.pending_prefill.take()
    }

    // ---- recipe edits ----

    fn max_for(&self, id: &str, catalogue: &ChromophoreCatalogue) -> f64 {
        catalogue
            .host(&self.host_id)
            .map_or(0.0, |h| h.element_conc_max(id))
    }

    /// The largest amount an end member can take: what the others leave of 1.
    fn end_member_room(&self, id: &str, catalogue: &ChromophoreCatalogue) -> Option<f64> {
        let host = catalogue.host(&self.host_id)?;
        if !host.end_members.iter().any(|m| m.id == id && !m.colorless) {
            return None;
        }
        let recipe = self.recipe()?;
        let others: f64 = host
            .end_members
            .iter()
            .filter(|m| !m.colorless && m.id != id)
            .map(|m| recipe.amount(&m.id))
            .sum();
        Some((1.0 - others).max(0.0))
    }

    /// A drag step of the log slider of `id` to position `pos` (no history entry until
    /// [`Self::release`]).
    pub fn set_amount_pos(&mut self, id: &str, pos: f64, catalogue: &ChromophoreCatalogue) {
        let mut max = self.max_for(id, catalogue);
        if let Some(room) = self.end_member_room(id, catalogue) {
            max = max.min(room);
        }
        let amount = pos_to_amount(pos, self.max_for(id, catalogue)).min(max);
        self.begin_drag();
        let recipe = self.recipe_mut(catalogue);
        if recipe.set_amount(id, amount) {
            refresh(recipe, catalogue);
        }
        self.after_edit(id, catalogue);
    }

    /// Sets the share of `id` in the fractions view: the others keep their proportions and fill
    /// the rest, so the total "typical-normalised" amount stays put.
    pub fn set_fraction(&mut self, id: &str, fraction: f64, catalogue: &ChromophoreCatalogue) {
        let Some(host) = catalogue.host(&self.host_id) else {
            return;
        };
        let Some(recipe) = self.recipe() else {
            return;
        };
        let units: Vec<(String, f64)> = recipe
            .entries
            .iter()
            .map(|e| {
                (
                    e.id.clone(),
                    e.amount / typical_amount(host, &e.id).max(1e-12),
                )
            })
            .collect();
        let total: f64 = units.iter().map(|(_, u)| u).sum();
        if total <= 0.0 || units.len() < 2 {
            return;
        }
        let f = fraction.clamp(0.0, 1.0);
        let others: f64 = units.iter().filter(|(k, _)| k != id).map(|(_, u)| u).sum();
        let new_units: Vec<(String, f64)> = units
            .iter()
            .map(|(k, u)| {
                if k == id {
                    (k.clone(), f * total)
                } else if others > 0.0 {
                    (k.clone(), u / others * (1.0 - f) * total)
                } else {
                    (k.clone(), (1.0 - f) * total / (units.len() - 1) as f64)
                }
            })
            .collect();
        self.begin_drag();
        let host_id = self.host_id.clone();
        let Some(host) = catalogue.host(&host_id) else {
            return;
        };
        let caps: Vec<(String, f64, f64)> = new_units
            .iter()
            .map(|(k, u)| {
                (
                    k.clone(),
                    u * typical_amount(host, k),
                    host.element_conc_max(k),
                )
            })
            .collect();
        let recipe = self.recipe_mut(catalogue);
        for (k, amount, max) in caps {
            recipe.set_amount(&k, amount.min(max));
        }
        refresh(recipe, catalogue);
        self.after_edit(id, catalogue);
    }

    fn after_edit(&mut self, id: &str, catalogue: &ChromophoreCatalogue) {
        // A garnet-style mix re-prefills RI/SG from the end-member fractions.
        if let Some(host) = catalogue.host(&self.host_id)
            && host.end_members.iter().any(|m| m.id == id)
        {
            self.pending_prefill = Some(host_prefill(host, self.recipe()));
        }
    }

    fn begin_drag(&mut self) {
        if self.drag_origin.is_none() {
            self.drag_origin = self.recipe().cloned();
        }
    }

    /// A slider was released: commits the whole drag as one undo step.
    pub fn release(&mut self) {
        if let Some(origin) = self.drag_origin.take()
            && self.recipe().is_some_and(|r| *r != origin)
        {
            self.history.push(origin);
        }
    }

    /// Sets the strength (a drag step; see [`Self::release`]).
    pub fn set_strength(&mut self, strength: f64, catalogue: &ChromophoreCatalogue) {
        self.begin_drag();
        let recipe = self.recipe_mut(catalogue);
        if recipe.set_strength(strength.clamp(STRENGTH_RANGE.0, STRENGTH_RANGE.1)) {
            refresh(recipe, catalogue);
        }
    }

    /// Sets the reference path (a drag step; see [`Self::release`]).
    pub fn set_path(&mut self, mm: f32, catalogue: &ChromophoreCatalogue) {
        if !mm.is_finite() {
            return;
        }
        self.begin_drag();
        let recipe = self.recipe_mut(catalogue);
        recipe.reference_path_mm = mm.clamp(0.1, 100.0);
        refresh(recipe, catalogue);
    }

    /// Adds `id` at its typical amount (undoable).
    pub fn add_item(&mut self, id: &str, catalogue: &ChromophoreCatalogue) -> bool {
        let Some(host) = catalogue.host(&self.host_id) else {
            return false;
        };
        if !catalogue
            .selectable_elements(&self.host_id)
            .iter()
            .any(|e| e == id)
        {
            return false;
        }
        if self
            .recipe()
            .is_some_and(|r| impossible_reason(host, r, id).is_some())
        {
            return false;
        }
        self.push_history();
        let mut amount = typical_amount(host, id);
        if let Some(room) = self.end_member_room(id, catalogue) {
            amount = amount.min(room);
        }
        let recipe = self.recipe_mut(catalogue);
        recipe.set_amount(id, amount);
        refresh(recipe, catalogue);
        self.after_edit(id, catalogue);
        true
    }

    /// Removes `id` and any treatment that no longer has its required elements (undoable).
    pub fn remove_item(&mut self, id: &str, catalogue: &ChromophoreCatalogue) {
        if self
            .recipe()
            .is_none_or(|r| !r.entries.iter().any(|e| e.id == id))
        {
            return;
        }
        self.push_history();
        self.locked.remove(id);
        let host_id = self.host_id.clone();
        let recipe = self.recipe_mut(catalogue);
        recipe.remove_entry(id);
        let present: Vec<String> = recipe
            .entries
            .iter()
            .filter(|e| e.amount > 0.0)
            .map(|e| e.id.clone())
            .collect();
        let present_refs: Vec<&str> = present.iter().map(String::as_str).collect();
        let valid: BTreeSet<String> = catalogue
            .selectable_treatments(&host_id, &present_refs)
            .iter()
            .map(|t| t.id.clone())
            .collect();
        recipe.treatments.retain(|t| valid.contains(t));
        refresh(recipe, catalogue);
        self.after_edit(id, catalogue);
    }

    /// Toggles the solver lock of `id`.
    pub fn toggle_lock(&mut self, id: &str) {
        if !self.locked.remove(id) {
            self.locked.insert(id.to_string());
        }
    }

    /// Switches a treatment on or off (undoable).
    pub fn toggle_treatment(&mut self, id: &str, active: bool, catalogue: &ChromophoreCatalogue) {
        if self
            .recipe()
            .is_some_and(|r| r.treatments.iter().any(|t| t == id) == active)
        {
            return;
        }
        self.push_history();
        let recipe = self.recipe_mut(catalogue);
        if active {
            recipe.treatments.push(id.to_string());
        } else {
            recipe.treatments.retain(|t| t != id);
        }
        refresh(recipe, catalogue);
    }

    /// Re-resolves the recipe with the current catalogue data (the "update to current data"
    /// action; undoable, never silent).
    pub fn update_data(&mut self, catalogue: &ChromophoreCatalogue) {
        self.push_history();
        let recipe = self.recipe_mut(catalogue);
        refresh(recipe, catalogue);
    }

    /// Undo (Ctrl-Z): restores the previous recipe exactly.
    pub fn undo(&mut self) -> bool {
        let Some(current) = self.mode.last_recipe.clone() else {
            return false;
        };
        self.cancel_solves();
        self.drag_origin = None;
        let Some(prev) = self.history.undo(current) else {
            return false;
        };
        self.host_id.clone_from(&prev.host);
        self.mode.last_recipe = Some(prev);
        true
    }

    /// Redo (Ctrl-Y).
    pub fn redo(&mut self) -> bool {
        let Some(current) = self.mode.last_recipe.clone() else {
            return false;
        };
        self.cancel_solves();
        self.drag_origin = None;
        let Some(next) = self.history.redo(current) else {
            return false;
        };
        self.host_id.clone_from(&next.host);
        self.mode.last_recipe = Some(next);
        true
    }

    // ---- solver ----

    /// Cancels the in-flight solve (if any) and bumps the generation, so a late result is dropped.
    pub fn cancel_solves(&mut self) {
        if let Some(token) = self.cancel.take() {
            token.store(true, Ordering::SeqCst);
        }
        self.generation += 1;
        self.solving = false;
        self.solving_kind = None;
    }

    fn begin_solve(&mut self, kind: SolveKind, lab: [f64; 3], srgb: Option<[u8; 3]>) -> SolveJob {
        self.cancel_solves();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(Arc::clone(&cancel));
        self.solving = true;
        self.solving_kind = Some(kind);
        if let Some(srgb) = srgb {
            self.target = Some(Target { lab, srgb });
        }
        let locked: Vec<(String, f64)> = self
            .recipe()
            .map(|r| {
                r.entries
                    .iter()
                    .filter(|e| self.locked.contains(&e.id))
                    .map(|e| (e.id.clone(), e.amount))
                    .collect()
            })
            .unwrap_or_default();
        SolveJob {
            generation: self.generation,
            host: self.host_id.clone(),
            target_lab: lab,
            reference_path_mm: self.reference_path_mm(),
            locked,
            cancel,
        }
    }

    /// The user picked `srgb` as the target color: the job that solves for it.
    pub fn pick(&mut self, srgb: [u8; 3]) -> SolveJob {
        let lab = indicatrix::color::body_color::srgb_to_lab(srgb.map(|c| f64::from(c) / 255.0));
        self.begin_solve(SolveKind::Pick, lab, Some(srgb))
    }

    /// Applies a finished solve. Stale generations are dropped (`false`); a current one pushes
    /// the old recipe on the undo stack first, then writes the solver's recipe (locked entries
    /// are part of it).
    pub fn finish_solve(
        &mut self,
        generation: u64,
        result: SolveResult,
        catalogue: &ChromophoreCatalogue,
    ) -> bool {
        if generation != self.generation || !self.solving {
            return false;
        }
        self.solving = false;
        self.solving_kind = None;
        self.cancel = None;
        let keep_path = self.reference_path_mm();
        let mut recipe = result.recipe;
        recipe.reference_path_mm = keep_path;
        refresh(&mut recipe, catalogue);
        self.push_history();
        self.host_id.clone_from(&recipe.host);
        // A solved end-member mix re-prefills RI/SG (garnet); other hosts keep what the
        // user has in the optics fields.
        self.pending_prefill = catalogue
            .host(&recipe.host)
            .filter(|h| !h.end_members.is_empty())
            .map(|h| host_prefill(h, Some(&recipe)));
        self.mode.last_recipe = Some(recipe);
        self.summary = Some(SolveSummary {
            capped: result.capped,
        });
        true
    }

    // ---- save / dirty ----

    /// Whether the physics state differs from what the dialog opened with: the mode, or the
    /// recipe (a pristine auto-created blank recipe on a material that had none does not count).
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        if self.fantasy_picked {
            return true;
        }
        if self.mode.active != self.baseline.active {
            return true;
        }
        match (&self.mode.last_recipe, &self.baseline.last_recipe) {
            (Some(a), Some(b)) => a != b,
            (None, None) => false,
            (Some(a), None) => !(a.entries.is_empty() && a.treatments.is_empty()),
            (None, Some(_)) => true,
        }
    }

    /// The JSON the Save button hands over: both payloads, `""` for a material that never had a
    /// recipe and is still plain fantasy. Fantasy-active saves keep the recipe.
    #[must_use]
    pub fn mode_json(&self) -> String {
        let blank = |r: &colorRecipe| r.entries.is_empty() && r.treatments.is_empty();
        match (&self.mode.last_recipe, self.is_physics()) {
            // A free fantasy pick lives only in the payload: it must be saved.
            _ if self.fantasy_picked => self.mode.to_json(),
            (None, false) => String::new(),
            (Some(r), false) if self.baseline.last_recipe.is_none() && blank(r) => String::new(),
            _ => self.mode.to_json(),
        }
    }

    // ---- view ----

    /// Everything the dialog shows, derived from the state (and the stored resolved bands).
    #[must_use]
    pub fn view(&self, catalogue: &ChromophoreCatalogue) -> PhysicsView {
        let host = catalogue.host(&self.host_id);
        let recipe = self.recipe();
        let mut view = PhysicsView {
            active: self.is_physics(),
            dirty: self.is_dirty(),
            mode_json: self.mode_json(),
            host_names: catalogue.hosts.iter().map(|h| h.name.clone()).collect(),
            host_index: catalogue
                .hosts
                .iter()
                .position(|h| h.id == self.host_id)
                .unwrap_or(0),
            banner: host.and_then(host_banner),
            data_updated: self.mode.data_outdated(catalogue),
            view_fractions: self.view_fractions,
            strength: recipe.map_or(1.0, |r| r.strength),
            reference_path_mm: self.reference_path_mm(),
            path_hint: path_hint(self.stone_width_mm),
            solving: self.solving,
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            ..PhysicsView::default()
        };
        let (Some(host), Some(recipe)) = (host, recipe) else {
            return view;
        };
        view.optics_locked = true;
        self.fill_rows(&mut view, catalogue, host, recipe);
        self.fill_colors(&mut view, recipe);
        fill_glow(&mut view, catalogue, recipe);
        if self.solving_kind == Some(SolveKind::InitialFromFantasy) {
            view.note_text =
                "Fitting your current fantasy color into this host\u{2026}".to_string();
        } else if self.is_physics() && recipe.entries.is_empty() && !self.solving {
            view.note_text = "Pure host: add elements or pick a color.".to_string();
        }
        view
    }
}

impl PhysicsState {
    /// The recipe rows, the addable items and the valid treatments.
    fn fill_rows(
        &self,
        view: &mut PhysicsView,
        catalogue: &ChromophoreCatalogue,
        host: &HostData,
        recipe: &colorRecipe,
    ) {
        // Rows.
        let units: Vec<(String, f64)> = recipe
            .entries
            .iter()
            .map(|e| {
                (
                    e.id.clone(),
                    e.amount / typical_amount(host, &e.id).max(1e-12),
                )
            })
            .collect();
        let total: f64 = units.iter().map(|(_, u)| u).sum();
        for e in &recipe.entries {
            let unit = item_unit(host, &e.id);
            let max = host.element_conc_max(&e.id);
            let (confidence, sources) = item_sources(host, &e.id);
            let share = units
                .iter()
                .find(|(k, _)| *k == e.id)
                .map_or(0.0, |(_, u)| if total > 0.0 { u / total } else { 0.0 });
            let shown = if self.view_fractions {
                e.amount * recipe.strength
            } else {
                e.amount
            };
            view.rows.push(RowView {
                id: e.id.clone(),
                label: catalogue
                    .end_member(&e.id)
                    .map_or_else(|| e.id.clone(), |m| m.name.clone()),
                amount_text: format_amount(shown, &unit),
                unit: unit_label(&unit),
                slider: amount_to_pos(e.amount, max),
                fraction: share,
                fraction_text: format!("{:.0} %", share * 100.0),
                locked: self.locked.contains(&e.id),
                estimated: matches!(confidence.as_str(), "estimate" | "unknown"),
                confidence,
                sources,
            });
        }
        // Add options: every offered id not in the recipe; impossible ones greyed.
        for id in catalogue.selectable_elements(&self.host_id) {
            if recipe.entries.iter().any(|e| e.id == id) {
                continue;
            }
            let reason = impossible_reason(host, recipe, &id);
            view.add_options.push(AddOptionView {
                label: catalogue
                    .end_member(&id)
                    .map_or_else(|| id.clone(), |m| m.name.clone()),
                id,
                enabled: reason.is_none(),
                reason: reason.unwrap_or_default(),
            });
        }
        // Treatments valid for the present elements.
        let present: Vec<&str> = recipe
            .entries
            .iter()
            .filter(|e| e.amount > 0.0)
            .map(|e| e.id.as_str())
            .collect();
        for t in catalogue.selectable_treatments(&self.host_id, &present) {
            view.treatments.push(TreatmentView {
                id: t.id.clone(),
                name: t.name.clone(),
                conditions: t.conditions.clone(),
                active: recipe.treatments.contains(&t.id),
            });
        }
    }

    /// The swatches (from the stored bands) and the readout against the picked color.
    fn fill_colors(&self, view: &mut PhysicsView, recipe: &colorRecipe) {
        // Swatches from the stored bands.
        let tensor = recipe.resolved_bands.to_tensor();
        let path = f64::from(recipe.reference_path_mm);
        let d65 = body_colors(&tensor, path, Illuminant::D65);
        let a = body_colors(&tensor, path, Illuminant::Planckian(3200.0));
        let delta_cc = delta_e_2000(d65.unpolarised.lab, a.unpolarised.lab);
        view.color_change_text = if delta_cc > color_CHANGE_DELTA_E {
            format!("color change \u{394}E {delta_cc:.1}")
        } else {
            format!("\u{394}E {delta_cc:.1} between D65 and 3200 K (no color change)")
        };
        view.has_beta = d65.beta_ray.is_some();
        view.d65 = Some(mode_swatches(&d65));
        view.a = Some(mode_swatches(&a));
        let stone_path = f64::from(default_reference_path_mm(self.stone_width_mm));
        if self.stone_width_mm > 0.0 && (stone_path - path).abs() > 0.05 {
            view.stone = Some((
                body_colors(&tensor, stone_path, Illuminant::D65)
                    .unpolarised
                    .srgb,
                body_colors(&tensor, stone_path, Illuminant::Planckian(3200.0))
                    .unpolarised
                    .srgb,
            ));
            view.stone_text = format!("At this stone's size ({stone_path:.1} mm)");
        }
        // The pick.
        if let Some(target) = self.target {
            let de = delta_e_2000(target.lab, d65.unpolarised.lab);
            view.target = Some(target.srgb);
            view.match_warn = de > WARN_DELTA_E;
            view.match_text = if de <= REACHABLE_DELTA_E {
                format!("\u{394}E {de:.1} from your pick (D65)")
            } else {
                format!("\u{394}E {de:.1} from your pick (D65) \u{b7} closest reachable shown")
            };
            if self.summary.is_some_and(|s| s.capped) {
                view.match_text.push_str(" \u{b7} search capped");
            }
        }
    }
}

/// The fluorescence readout and the two analytic UV glow swatches (fluorescence plan section 5),
/// from the recipe's emitters (`fluorescence_report`) at its reference path: nothing is traced.
fn fill_glow(view: &mut PhysicsView, catalogue: &ChromophoreCatalogue, recipe: &colorRecipe) {
    let report = fluorescence_report(catalogue, recipe);
    let path = recipe.reference_path_mm;
    let (glow365, glow395) = (report.glow(UV365_NM, path), report.glow(UV395_NM, path));
    let swatch = |glow: &UvGlow| GlowSwatch {
        srgb: glow.srgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
        level: (glow.photon_yield / GLOW_LEVEL_FULL).clamp(0.0, 1.0),
        text: glow_caption(glow),
    };
    // The readout names the stronger of the two lamps.
    let best = if glow395.photon_yield > glow365.photon_yield {
        &glow395
    } else {
        &glow365
    };
    view.fluorescence_text = match (best.strength(), best.color_name()) {
        (GlowStrength::None, _) | (_, None) => "Fluorescence: none".to_string(),
        (GlowStrength::Weak, Some(c)) => format!("Fluorescence: weak {c}"),
        (GlowStrength::Strong, Some(c)) => format!("Fluorescence: strong {c}"),
    };
    view.quench_text = report
        .dominant_quencher()
        .map(|by| format!("quenched by {by}"))
        .unwrap_or_default();
    view.glow = (!report.fluorescence.is_empty()).then(|| [swatch(&glow365), swatch(&glow395)]);
}

/// Photon yield (emitted per lamp photon) shown as a full brightness bar.
const GLOW_LEVEL_FULL: f32 = 0.5;

/// "relative brightness 42 %" caption of a glow swatch.
fn glow_caption(glow: &UvGlow) -> String {
    format!(
        "{:.0} %",
        (glow.photon_yield / GLOW_LEVEL_FULL).clamp(0.0, 1.0) * 100.0
    )
}

fn path_hint(stone_width_mm: f32) -> String {
    if stone_width_mm > 0.0 {
        format!(
            "equivalent path (default {GIRDLE_PATH_FACTOR} \u{d7} girdle {stone_width_mm:.1} mm)"
        )
    } else {
        "equivalent path (no design size: default 5 mm)".to_string()
    }
}

/// Re-resolves `recipe` with `catalogue` and stores the budgeted bands: amounts are clamped
/// to the host's range first, `data_version` is the catalogue's. A failing resolve (an
/// impossible end-member sum) keeps the previous bands.
pub fn refresh(recipe: &mut colorRecipe, catalogue: &ChromophoreCatalogue) {
    recipe.clamp_amounts(catalogue);
    recipe.data_version = catalogue.data_version;
    if let Ok((tensor, _)) = resolve(recipe, catalogue) {
        recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
    }
}

/// One swatch row: unpolarised, o-ray, e-ray and (biaxial) beta-ray sRGB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeSwatches {
    /// Unpolarised color.
    pub unpol: [u8; 3],
    /// Ordinary ray.
    pub o: [u8; 3],
    /// Extraordinary ray.
    pub e: [u8; 3],
    /// Beta ray of a biaxial host.
    pub beta: Option<[u8; 3]>,
}

fn mode_swatches(c: &Bodycolors) -> ModeSwatches {
    let rgb = |b: &Bodycolor| b.srgb;
    ModeSwatches {
        unpol: rgb(&c.unpolarised),
        o: rgb(&c.o_ray),
        e: rgb(&c.e_ray),
        beta: c.beta_ray.as_ref().map(rgb),
    }
}

/// One UV-lamp glow swatch: the emitted color, its relative brightness and a caption.
#[derive(Debug, Clone, PartialEq)]
pub struct GlowSwatch {
    /// Display color (the glow's color, darker for a dim glow).
    pub srgb: [u8; 3],
    /// Relative brightness `0..=1` (photon yield against a bright glow).
    pub level: f32,
    /// "42 %" caption.
    pub text: String,
}

/// One recipe row.
#[derive(Debug, Clone, PartialEq)]
pub struct RowView {
    /// Selectable id.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Amount with unit.
    pub amount_text: String,
    /// Unit label.
    pub unit: String,
    /// Log slider position.
    pub slider: f64,
    /// Share in the fractions view.
    pub fraction: f64,
    /// Share text.
    pub fraction_text: String,
    /// Locked for the solver.
    pub locked: bool,
    /// The data are an estimate or unknown.
    pub estimated: bool,
    /// Worst confidence behind the item.
    pub confidence: String,
    /// Sources popover text.
    pub sources: String,
}

/// One "+ Add" entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddOptionView {
    /// Selectable id.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Whether it can be added now.
    pub enabled: bool,
    /// Why not.
    pub reason: String,
}

/// One treatment checkbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreatmentView {
    /// Treatment id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Conditions text.
    pub conditions: String,
    /// Whether it is on.
    pub active: bool,
}

/// Everything the physics section shows, as plain data.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhysicsView {
    /// Physics is the active mode.
    pub active: bool,
    /// The state differs from what the dialog opened with.
    pub dirty: bool,
    /// The JSON Save hands over.
    pub mode_json: String,
    /// Host display names (catalogue order).
    pub host_names: Vec<String>,
    /// Selected host.
    pub host_index: usize,
    /// The data-confidence banner, if the host has non-verified coefficients.
    pub banner: Option<String>,
    /// The recipe predates the catalogue's data version.
    pub data_updated: bool,
    /// Recipe rows.
    pub rows: Vec<RowView>,
    /// Addable items.
    pub add_options: Vec<AddOptionView>,
    /// Valid treatments.
    pub treatments: Vec<TreatmentView>,
    /// Strength + fractions view.
    pub view_fractions: bool,
    /// Recipe strength.
    pub strength: f64,
    /// Reference path.
    pub reference_path_mm: f32,
    /// Path label/hint.
    pub path_hint: String,
    /// D65 swatches.
    pub d65: Option<ModeSwatches>,
    /// 3200 K swatches.
    pub a: Option<ModeSwatches>,
    /// A biaxial host (beta swatches shown).
    pub has_beta: bool,
    /// "At this stone's size" D65 and 3200 K colors.
    pub stone: Option<([u8; 3], [u8; 3])>,
    /// Label of the stone-size row.
    pub stone_text: String,
    /// color-change readout.
    pub color_change_text: String,
    /// The UV 365 nm and UV 395 nm glow swatches; `None` for a recipe without emitters.
    pub glow: Option<[GlowSwatch; 2]>,
    /// "Fluorescence: none / weak / strong <color>".
    pub fluorescence_text: String,
    /// "quenched by Fe" when a quencher took more than half of the yield, else empty.
    pub quench_text: String,
    /// The picked color.
    pub target: Option<[u8; 3]>,
    /// "dE x from your pick ..." text.
    pub match_text: String,
    /// dE above the warning threshold.
    pub match_warn: bool,
    /// A solve is running.
    pub solving: bool,
    /// Short note.
    pub note_text: String,
    /// Undo available.
    pub can_undo: bool,
    /// Redo available.
    pub can_redo: bool,
    /// The host fixes crystal system and optical character.
    pub optics_locked: bool,
}

#[cfg(test)]
mod tests;
