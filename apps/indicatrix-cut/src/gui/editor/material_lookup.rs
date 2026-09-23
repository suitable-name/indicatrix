//! The editor-side `indicatrix_cut_core::MaterialLookup` -- built-ins plus whatever
//! custom catalogue materials the render context currently has loaded -- and the small
//! pure helpers built on top of it: a `GemMaterial` that actually reflects an RI
//! override (for the optimizer and, later, the tilt curve), and the "nearest built-in
//! to this refractive index" search the catalogue-load suggestion and the
//! linked-viewport-material fallback both need.
//!
//! # Per-axis dispersion is not wired up here
//!
//! `indicatrix_vault::model::material::CustomMaterialRow::per_axis_dispersion_json`
//! exists in the schema, but nothing in this crate has ever written a real value into
//! it -- `gui::optics::crystal_optics::save_gem_material` always saves `None` for it
//! today, and `gem_material_from_row` (the one place a `CustomMaterialRow` becomes a
//! `GemMaterial`) does not parse it either. This lookup therefore has the exact same
//! "a custom material with only a flat RI/dispersion pair renders non-dispersive"
//! limitation the live-render viewport already has -- deliberately left as a documented
//! limitation (stated in the material combo's tooltip) rather than a per-axis
//! authoring surface.

use indicatrix::optics::{dispersion::DispersionModel, materials::GemMaterial};
use indicatrix_cut_core::material::{BuiltinMaterials, MaterialLookup, MaterialSelection};

/// Built-ins plus whatever custom catalogue materials the caller hands in --
/// exactly `bridge::render_thread::context::resolve_material`'s own "custom
/// materials take priority over the built-in presets" precedence, reused here so
/// a design's material resolves the SAME way in the editor as it already does in
/// the live-render viewport.
pub(super) struct EditorMaterialLookup<'a> {
    custom: &'a [GemMaterial],
    /// Opt-in source for a CUSTOM material's specific gravity. `None` unless
    /// a caller opts in via [`Self::with_specific_gravity`]. Built-in names
    /// resolve via `BuiltinMaterials` regardless of this field; custom materials
    /// need opt-in since no caller requested SG lookup before this field existed.
    /// Shaped to match `bridge::render_thread::RenderContext::
    /// custom_material_specific_gravity` so a live context can pass it directly,
    /// or a plain parameter when a context is unavailable.
    specific_gravity_source: Option<&'a [(String, f64)]>,
}

impl<'a> EditorMaterialLookup<'a> {
    /// Builds a lookup with `custom` and no specific-gravity source -- opt in via
    /// [`Self::with_specific_gravity`].
    pub(super) const fn new(custom: &'a [GemMaterial]) -> Self {
        Self {
            custom,
            specific_gravity_source: None,
        }
    }

    /// Opts this lookup into resolving a CUSTOM material's specific gravity from
    /// `entries` -- a `(name, sg)` list matching `RenderContext::
    /// custom_material_specific_gravity`'s shape. Purely additive: no caller
    /// ever requested SG lookup before, so this changes no existing behavior.
    #[must_use]
    pub(super) const fn with_specific_gravity(mut self, entries: &'a [(String, f64)]) -> Self {
        self.specific_gravity_source = Some(entries);
        self
    }
}

impl MaterialLookup for EditorMaterialLookup<'_> {
    fn lookup(&self, name: &str) -> Option<GemMaterial> {
        self.custom
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
            .cloned()
            .or_else(|| BuiltinMaterials.lookup(name))
    }

    /// Custom-over-built-in, matching [`Self::lookup`]'s own precedence: a custom
    /// material's own recorded SG (when [`Self::with_specific_gravity`] opted this
    /// lookup into reading it) wins, falling back to the built-in table for a name
    /// that names a built-in preset instead. The case-insensitive scan matches
    /// `RenderContext::custom_specific_gravity`'s own matching convention, so the
    /// two never disagree about what counts as the same name.
    fn specific_gravity(&self, name: &str) -> Option<f64> {
        self.specific_gravity_source
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                    .map(|(_, sg)| *sg)
            })
            .or_else(|| BuiltinMaterials.specific_gravity(name))
    }
}

/// Resolves `selection` to the real [`GemMaterial`] a consumer that needs actual
/// optics (the optimizer's objective, the tilt curve) should score against -- unlike
/// [`MaterialSelection::resolve`] alone, this makes `refractive_index_override`
/// actually show up in the returned material's own dispersion, not just in the
/// caller-visible `n_d` scalar.
///
/// When no override is set, this is exactly `selection.resolve(lookup).gem` -- the
/// resolved material's own real dispersion curve, unchanged. When an override is set,
/// the resolved material's dispersion is replaced with a flat (non-dispersive)
/// `DispersionModel::Cauchy { a: n_d, b: 0.0, c: 0.0 }` fit at exactly the override
/// value, so `evaluate(_)` returns the override at every wavelength -- everything else
/// about the resolved material (crystal system, absorption, birefringence, c-axis) is
/// left untouched. Deliberate simplification, not a real dispersion curve: an override
/// is, by construction, a single typed RI with no accompanying dispersion data, so
/// "flat at the override" is the only honest thing to draw.
#[must_use]
pub(super) fn resolved_gem_material(
    selection: &MaterialSelection,
    lookup: &dyn MaterialLookup,
) -> GemMaterial {
    let resolved = selection.resolve(lookup);
    if selection.refractive_index_override.is_none() {
        return resolved.gem;
    }
    let mut gem = resolved.gem;
    gem.dispersion = DispersionModel::Cauchy {
        a: resolved.n_d as f32,
        b: 0.0,
        c: 0.0,
    };
    gem
}

/// How close a design's own refractive index must sit to a built-in preset's for that
/// preset to stand in for an unnamed material.
///
/// 0.02 is roughly the gap between neighbouring species in the built-in table, so a match
/// this close is the same stone by any practical reading, while a design sitting between
/// two presets falls through to a refusal rather than being rounded to whichever happened
/// to be nearer. Shared by `super::view::traced_material_for` (the editor's own design)
/// and [`material_for_refractive_index`] (a catalogue row), so the two can never disagree
/// about what counts as a match.
pub(in crate::gui) const MATERIAL_MATCH_TOLERANCE: f64 = 0.02;

/// The built-in material a design carrying `n_d` and NO species name should be rendered
/// as: the nearest preset within [`MATERIAL_MATCH_TOLERANCE`], resolved to a real
/// [`GemMaterial`], or `None` when nothing built in is that close.
///
/// The catalogue's own case. A library row records a refractive index but never
/// a species, so this is the only honest way to name a stone for it. Refuses rather
/// than substituting, since rendering a design as a different species gives wrong
/// optics with no indication on screen.
#[must_use]
pub(in crate::gui) fn material_for_refractive_index(n_d: f64) -> Option<(String, GemMaterial)> {
    let (name, _) = nearest_built_in_material(n_d, MATERIAL_MATCH_TOLERANCE)?;
    let gem = BuiltinMaterials.lookup(&name)?;
    Some((name, gem))
}

/// The [`GemMaterial`] a design is actually RENDERED as, given the material name
/// `super::view::traced_material_for` already resolved -- `None` when nothing in
/// this catalogue answers to that name.
///
/// Differs from [`resolved_gem_material`]: keys off the TRACED name rather than
/// `MaterialSelection::name`, and refuses instead of substituting. The render path
/// needs this because `MaterialSelection::resolve` falls back to diamond for a
/// selection with no name (every untouched import and new design). Diamond's empty
/// absorption bands rendered every such design colourless, so direct refusal prevents
/// silently mis-rendering by species. `RenderContext::material_override` beats the
/// by-name lookup and must honor the same rule.
///
/// `selection` is consulted only for its `refractive_index_override`, applied exactly as
/// [`resolved_gem_material`] applies it (flat Cauchy at the typed value, everything else
/// about the material left alone) -- see that function's own doc comment.
#[must_use]
pub(super) fn traced_gem_material(
    traced_name: &str,
    selection: &MaterialSelection,
    lookup: &dyn MaterialLookup,
) -> Option<GemMaterial> {
    let mut gem = lookup.lookup(traced_name)?;
    if let Some(n_d) = selection.refractive_index_override {
        gem.dispersion = DispersionModel::Cauchy {
            a: n_d as f32,
            b: 0.0,
            c: 0.0,
        };
    }
    Some(gem)
}

/// The built-in species [`nearest_built_in_material`] guesses for a nameless
/// design, purely from its refractive index.
///
/// Deliberately only thirteen common species (not the full thirty-two built-ins),
/// because an exhaustive search regressed `material_for_refractive_index_names_the_catalogues_common_stones`:
/// at `n_d` 1.76, Benitoite sits closer than Sapphire (see its Sellmeier entry),
/// so all-thirty-two search silently misnamed the owner's Sapphire designs. Picking
/// a name by an explicit choice (right) differs from guessing one from a bare number
/// (wrong). This curated list restricts guessing to common, well-separated species a
/// cutter is likely to hold, never synthetic/lab species whose RIs happen nearby.
const RI_INFERENCE_CANDIDATES: [&str; 13] = [
    "Diamond",
    "Sapphire",
    "Ruby",
    "Emerald",
    "Zircon",
    "Alexandrite",
    "Topaz",
    "Spinel",
    "Quartz",
    "Tourmaline",
    "Tanzanite",
    "Synthetic Moissanite",
    "Cubic Zirconia",
];

/// The built-in preset (from [`RI_INFERENCE_CANDIDATES`]) whose own `n_D` sits
/// closest to `n_d`, when that distance is within `tolerance` -- `None` when
/// nothing built in is close enough. Ties keep the first (list-order) match, so
/// this is deterministic regardless of floating-point noise between two
/// equally-close presets, consistent with this crate's "no `HashMap`/`HashSet` in
/// a decision path" rule (a plain `Vec`/array scan here, not that this feeds the
/// solver at all).
///
/// Shared by the catalogue-load "suggest a material for this schedule's RI" toast
/// (`super::loading::suggest_material_for_schedule_ri`) and, in principle, any future
/// "best built-in name for a display string" need -- there is only one such search in
/// this crate today.
#[must_use]
pub(in crate::gui) fn nearest_built_in_material(n_d: f64, tolerance: f64) -> Option<(String, f64)> {
    RI_INFERENCE_CANDIDATES
        .iter()
        .filter_map(|&name| {
            indicatrix_cut_core::built_in_refractive_index(name).map(|ri| (name, ri))
        })
        .map(|(name, ri)| {
            let diff = (ri - n_d).abs();
            (name, ri, diff)
        })
        .filter(|&(_, _, diff)| diff <= tolerance)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(name, ri, _)| (name.to_string(), ri))
}

/// Every [`RI_INFERENCE_CANDIDATES`] preset within `tolerance` of `n_d`,
/// nearest first -- same search as [`nearest_built_in_material`], but returns
/// every close match instead of only the winner. The guess badge's tooltip lists
/// these so a cutter can see what else was close before trusting the single
/// nearest name. Uses the same candidate list and tolerance, just reports every
/// match instead of one.
#[must_use]
pub(in crate::gui) fn material_guess_candidates(n_d: f64, tolerance: f64) -> Vec<(String, f64)> {
    let mut matches: Vec<(&'static str, f64, f64)> = RI_INFERENCE_CANDIDATES
        .iter()
        .filter_map(|&name| {
            indicatrix_cut_core::built_in_refractive_index(name).map(|ri| (name, ri))
        })
        .map(|(name, ri)| (name, ri, (ri - n_d).abs()))
        .filter(|&(_, _, diff)| diff <= tolerance)
        .collect();
    matches.sort_by(|a, b| a.2.total_cmp(&b.2));
    matches
        .into_iter()
        .map(|(name, ri, _)| (name.to_string(), ri))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- EditorMaterialLookup::specific_gravity ---

    #[test]
    fn specific_gravity_is_none_when_not_opted_in_and_the_name_is_not_a_builtin() {
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        assert_eq!(lookup.specific_gravity("My Garnet"), None);
    }

    #[test]
    fn specific_gravity_resolves_a_builtin_name_even_without_opting_in() {
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        assert_eq!(lookup.specific_gravity("Diamond"), Some(3.52));
    }

    #[test]
    fn specific_gravity_resolves_a_custom_material_once_opted_in() {
        let custom_list: [GemMaterial; 0] = [];
        let sg_entries = [("My Garnet".to_string(), 3.90)];
        let lookup = EditorMaterialLookup::new(&custom_list).with_specific_gravity(&sg_entries);
        assert_eq!(lookup.specific_gravity("my garnet"), Some(3.90));
    }

    /// A custom material's own recorded SG wins over a same-named built-in's table
    /// figure -- the same custom-over-built-in precedence [`EditorMaterialLookup::
    /// lookup`] already applies for the material itself.
    #[test]
    fn specific_gravity_prefers_a_custom_entry_over_a_same_named_builtin() {
        let custom_list: [GemMaterial; 0] = [];
        let sg_entries = [("Diamond".to_string(), 3.515)];
        let lookup = EditorMaterialLookup::new(&custom_list).with_specific_gravity(&sg_entries);
        assert_eq!(lookup.specific_gravity("Diamond"), Some(3.515));
    }

    #[test]
    fn specific_gravity_falls_back_to_a_builtin_when_opted_in_but_the_name_is_not_custom() {
        let custom_list: [GemMaterial; 0] = [];
        let sg_entries = [("My Garnet".to_string(), 3.90)];
        let lookup = EditorMaterialLookup::new(&custom_list).with_specific_gravity(&sg_entries);
        assert_eq!(lookup.specific_gravity("Quartz"), Some(2.65));
        assert_eq!(lookup.specific_gravity("Not A Real Material"), None);
    }

    #[test]
    fn lookup_prefers_a_custom_material_over_a_same_named_built_in() {
        let mut custom = GemMaterial::diamond();
        custom.name = "Diamond".to_string();
        custom.dispersion = DispersionModel::Cauchy {
            a: 9.0,
            b: 0.0,
            c: 0.0,
        };
        let custom_list = [custom];
        let lookup = EditorMaterialLookup::new(&custom_list);
        let found = lookup.lookup("Diamond").unwrap();
        assert!((f64::from(found.dispersion.evaluate(589.3)) - 9.0).abs() < 1e-6);
    }

    #[test]
    fn lookup_falls_back_to_a_built_in_when_no_custom_material_matches() {
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        assert!(lookup.lookup("Quartz").is_some());
        assert!(lookup.lookup("Not A Real Material").is_none());
    }

    /// The catalogue's own case: a library row records a refractive index and no
    /// species, so this is what names the stone for its preview. The four values are the
    /// owner's actual catalogue's four most common (1,789 / 571 / 195 / 115 of 3,098
    /// designs), so a regression here would mis-render most of the library.
    #[test]
    fn material_for_refractive_index_names_the_catalogues_common_stones() {
        for (n_d, expected) in [
            (1.54, "Quartz"),
            (1.76, "Sapphire"),
            (1.72, "Spinel"),
            (2.16, "Cubic Zirconia"),
        ] {
            let (name, gem) = material_for_refractive_index(n_d)
                .unwrap_or_else(|| panic!("{n_d} must resolve to a built-in"));
            assert_eq!(name, expected, "n_d {n_d}");
            assert_eq!(gem.name, expected, "n_d {n_d}: the resolved material too");
        }
    }

    /// Refuses rather than rounding to the nearest preset. 1.90 sits between
    /// Cubic Zirconia (2.16) and Tanzanite/Zircon, further than
    /// `MATERIAL_MATCH_TOLERANCE` from any.
    #[test]
    fn material_for_refractive_index_refuses_when_nothing_is_close() {
        assert!(material_for_refractive_index(1.90).is_none());
    }

    /// A design with no material name (every untouched import) must trace as the
    /// material `view::traced_material_for` picked, not as diamond (the fallback for
    /// unnamed selections). Diamond's empty absorption bands caused every imported
    /// design to render colourless regardless of its RI.
    #[test]
    fn traced_gem_material_honours_the_traced_name_for_an_unnamed_selection() {
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        let gem = traced_gem_material("Sapphire", &MaterialSelection::none(), &lookup)
            .expect("Sapphire is a built-in");
        let expected = GemMaterial::by_name("Sapphire").expect("Sapphire is a built-in");
        assert_eq!(gem.name, expected.name);
        assert_eq!(gem.absorption, expected.absorption);
        assert!(
            !gem.absorption.o_ray.is_empty(),
            "test premise: Sapphire must carry real absorption bands, or this test              cannot tell it apart from diamond"
        );
    }

    /// An RI override still flattens the dispersion -- the traced name decides WHICH
    /// material, the override decides its index, exactly as `resolved_gem_material` does.
    #[test]
    fn traced_gem_material_applies_the_refractive_index_override() {
        let selection = MaterialSelection {
            name: None,
            specific_gravity_override: None,
            refractive_index_override: Some(1.66),
        };
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        let gem = traced_gem_material("Quartz", &selection, &lookup).expect("Quartz is a built-in");
        assert!((f64::from(gem.dispersion.evaluate(400.0)) - 1.66).abs() < 1e-6);
        assert!((f64::from(gem.dispersion.evaluate(700.0)) - 1.66).abs() < 1e-6);
    }

    /// Refuses rather than substituting: an unresolvable name leaves
    /// `RenderContext::material_override` unset, so the by-name lookup in
    /// `bridge::render_thread::context::resolve_material` decides on its own instead of
    /// being overridden by a material nobody asked for.
    #[test]
    fn traced_gem_material_refuses_an_unknown_name() {
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        assert!(traced_gem_material("Kryptonite", &MaterialSelection::none(), &lookup).is_none());
    }

    #[test]
    fn resolved_gem_material_uses_the_real_dispersion_curve_with_no_override() {
        let selection = MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        let gem = resolved_gem_material(&selection, &lookup);
        let expected = GemMaterial::by_name("Quartz").unwrap();
        assert_eq!(gem.dispersion, expected.dispersion);
    }

    #[test]
    fn resolved_gem_material_flattens_dispersion_to_the_override_value() {
        let selection = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: Some(1.70),
        };
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        let gem = resolved_gem_material(&selection, &lookup);
        // Evaluated at several wavelengths -- a flat fit must agree everywhere,
        // not just at the sodium D line the override was authored against.
        for lambda in [450.0f32, 589.3, 650.0] {
            assert!((f64::from(gem.dispersion.evaluate(lambda)) - 1.70).abs() < 1e-4);
        }
        // Every other field (crystal system, absorption, c-axis) stays whatever
        // the resolved base material ("Diamond") already had -- only dispersion
        // is replaced.
        let base = GemMaterial::diamond();
        assert_eq!(gem.crystal_system, base.crystal_system);
        assert_eq!(gem.absorption, base.absorption);
    }

    /// At sapphire's own RI, the nearest-first list must start with Sapphire
    /// itself (distance 0) -- the guess badge's own headline candidate.
    #[test]
    fn material_guess_candidates_starts_with_the_exact_match() {
        let sapphire_n_d = indicatrix_cut_core::built_in_refractive_index("Sapphire").unwrap();
        let candidates = material_guess_candidates(sapphire_n_d, MATERIAL_MATCH_TOLERANCE);
        assert_eq!(
            candidates.first().map(|(name, _)| name.as_str()),
            Some("Sapphire")
        );
    }

    /// An RI with nothing close enough returns an empty list, not a forced
    /// match -- same refusal-over-substitution rule as
    /// [`material_for_refractive_index_refuses_when_nothing_is_close`].
    #[test]
    fn material_guess_candidates_is_empty_when_nothing_is_close() {
        assert_eq!(
            material_guess_candidates(1.90, MATERIAL_MATCH_TOLERANCE),
            Vec::<(String, f64)>::new()
        );
    }

    #[test]
    fn nearest_built_in_material_finds_quartz_for_its_own_ri() {
        let quartz_n_d = indicatrix_cut_core::built_in_refractive_index("Quartz").unwrap();
        let (name, ri) = nearest_built_in_material(quartz_n_d, 0.01).unwrap();
        assert_eq!(name, "Quartz");
        assert!((ri - quartz_n_d).abs() < 1e-9);
    }

    #[test]
    fn nearest_built_in_material_returns_none_outside_tolerance() {
        // 1.9999 sits roughly halfway between Spinel (~1.727) and Zircon
        // (~1.923)/Tanzanite (~1.69) -- nothing in the table is genuinely close,
        // so a tight tolerance must find nothing rather than force a match.
        assert_eq!(nearest_built_in_material(1.9999, 0.001), None);
    }

    #[test]
    fn nearest_built_in_material_picks_the_closest_when_two_are_within_tolerance() {
        // Diamond (~2.417) and Cubic Zirconia (~2.15-2.2ish) may both fall within
        // a generous tolerance of some intermediate n_d -- the closer one must
        // win, not merely list order.
        let diamond_n_d = indicatrix_cut_core::built_in_refractive_index("Diamond").unwrap();
        let (name, _) = nearest_built_in_material(diamond_n_d - 0.02, 0.5).unwrap();
        assert_eq!(name, "Diamond");
    }

    // --- Selection round trip: every `MaterialCatalogue` entry must resolve to
    // a `GemMaterial` of the SAME name and RI on the render side ---

    /// `EditorMaterialLookup::lookup` and `bridge::render_thread::context::
    /// resolve_material` implement the EXACT same precedence: a custom material by
    /// case-insensitive name first, then `GemMaterial::by_name`/a linear built-in
    /// scan, unconditionally (see `EditorMaterialLookup::lookup`'s own doc comment
    /// and `resolve_material`'s body) -- there is no difference in behaviour for a
    /// name `MaterialCatalogue` actually lists to exercise here that a direct call
    /// into `bridge::render_thread` would catch and this wouldn't. This is
    /// therefore the same round trip a design's `MaterialSelection::name` takes
    /// from "listed in a picker" to "traced in Live Render".
    #[test]
    fn every_catalogue_entry_resolves_to_a_gem_material_of_its_own_name_and_ri() {
        let mut custom = GemMaterial::diamond();
        custom.name = "My Custom Garnet".to_string();
        custom.dispersion = DispersionModel::Cauchy {
            a: 1.74,
            b: 0.0,
            c: 0.0,
        };
        let custom_list = [custom];
        let catalogue = indicatrix_cut_core::MaterialCatalogue::build(&custom_list);
        assert!(
            catalogue.len() > 30,
            "premise: the catalogue must cover every built-in, not just thirteen \
             common ones -- {} entries found",
            catalogue.len()
        );
        let lookup = EditorMaterialLookup::new(&custom_list);

        for entry in catalogue.entries() {
            let selection = MaterialSelection {
                name: Some(entry.name.clone()),
                specific_gravity_override: None,
                refractive_index_override: None,
            };
            let resolved = resolved_gem_material(&selection, &lookup);
            assert_eq!(
                resolved.name, entry.name,
                "catalogue entry {:?} resolved to the wrong material",
                entry.name
            );
            let resolved_ri = f64::from(resolved.dispersion.evaluate(589.3));
            assert!(
                (resolved_ri - entry.ri_d).abs() < 1e-6,
                "{}: catalogue said RI {}, resolved material's own dispersion says {}",
                entry.name,
                entry.ri_d,
                resolved_ri
            );
        }
    }

    /// The render-path counterpart of the round trip above: `traced_gem_material`
    /// (what `view::sync_viewport_material_link` actually calls) must agree too,
    /// for every catalogue entry, with no RI override in play.
    #[test]
    fn every_catalogue_entry_traces_as_a_gem_material_of_its_own_name() {
        let catalogue = indicatrix_cut_core::MaterialCatalogue::build(&[]);
        let lookup = EditorMaterialLookup::new(&[]);
        for entry in catalogue.entries() {
            let gem = traced_gem_material(&entry.name, &MaterialSelection::none(), &lookup)
                .unwrap_or_else(|| panic!("{} must resolve via traced_gem_material", entry.name));
            assert_eq!(gem.name, entry.name);
        }
    }
}
