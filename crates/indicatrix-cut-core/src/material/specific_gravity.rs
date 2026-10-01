//! The built-in specific-gravity table, keyed by [`indicatrix::optics::materials::
//! GemMaterial::name`] -- see [`crate::material`]'s module doc comment for provenance
//! and the note on why Zircon/Tourmaline carry a real cited range.

/// One material's specific gravity.
///
/// A representative point figure for [`crate::yield_metrics::carat_weight`], plus the
/// real low/high bounds it simplifies away for a species with a genuine compositional
/// range (see this module's doc comment). `range` equals `(representative,
/// representative)` when published sources agree to within rounding, not because no
/// natural variation exists at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecificGravity {
    /// The single number [`crate::yield_metrics::carat_weight`] multiplies by,
    /// absent a per-design override.
    pub representative: f64,
    /// `(low, high)` -- the cited real-world spread this representative figure
    /// simplifies. Always `low <= representative <= high`.
    pub range: (f64, f64),
}

/// Looks up a built-in specific gravity by the name
/// `indicatrix::optics::materials::GemMaterial::name` uses for that preset.
///
/// Matched EXACTLY (case-insensitively only -- e.g. `"diamond"` still resolves,
/// but "My Blue Sapphire" does not, unlike
/// [`indicatrix::optics::materials::GemMaterial::by_name`]'s own substring
/// fallback; see [`super::built_in_material_by_exact_name`]'s doc comment for the
/// same distinction on the refractive-index side).
///
/// See this module's doc comment for the full covered list and provenance. `None`
/// for any name outside that list, notably "Garnet", which has no built-in preset.
#[must_use]
pub fn built_in_specific_gravity(material_name: &str) -> Option<SpecificGravity> {
    // Sources: Handbook of Mineralogy / IGS (see module doc comment), except where
    // noted. Zircon/Tourmaline carry a real compositional range -- see "Ranges"
    // above; representative there is a deliberate midpoint choice, not an average.
    const TABLE: &[(&str, f64, (f64, f64))] = &[
        ("Diamond", 3.52, (3.50, 3.53)),
        // Sapphire & Ruby: same corundum host lattice, same SG.
        ("Sapphire", 4.00, (3.99, 4.10)),
        ("Ruby", 4.00, (3.99, 4.10)),
        // Emerald-specific beryl figure (beryl overall runs wider, 2.63-2.92).
        ("Emerald", 2.76, (2.67, 2.78)),
        // Representative leans toward high (crystalline) zircon (4.65) to match
        // this crate's Zircon optical entry (gem-trade "starlite" pairing).
        ("Zircon", 4.65, (3.90, 4.73)),
        ("Alexandrite", 3.73, (3.68, 3.78)),
        // OH-rich/F-rich ends of the solid solution shift SG slightly.
        ("Topaz", 3.53, (3.49, 3.57)),
        ("Spinel", 3.60, (3.58, 3.61)),
        ("Quartz", 2.65, (2.65, 2.66)),
        // Representative (3.06) is elbaite's own midpoint (the species modeled
        // optically); the cited range spans the whole dravite/schorl family since a
        // cutter may not know which tourmaline species their rough is.
        ("Tourmaline", 3.06, (2.82, 3.32)),
        ("Tanzanite", 3.35, (3.15, 3.36)),
        ("Synthetic Moissanite", 3.22, (3.21, 3.22)),
        // Synthetic; SG varies with stabilizer content.
        ("Cubic Zirconia", 5.80, (5.60, 6.00)),
        // Synthetic rutile: the standard reference value.
        ("Rutile", 4.25, (4.25, 4.25)),
    ];
    TABLE
        .iter()
        .find(|(name, ..)| name.eq_ignore_ascii_case(material_name))
        .map(|&(_, representative, range)| SpecificGravity {
            representative,
            range,
        })
}
