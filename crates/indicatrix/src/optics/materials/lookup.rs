//! Name-based lookup ([`GemMaterial::by_name`]) and the built-in convenience
//! accessors (`diamond`/`ruby`/`sapphire`/`emerald`).

use super::GemMaterial;

impl GemMaterial {
    /// Looks a material up by name, tolerating extra surrounding words in the query
    /// (e.g. a diagram title like "Fine Blue Sapphire").
    ///
    /// An exact match always wins outright. The substring fallback then prefers the
    /// LONGEST matching material name: "Zircon" is a substring of "Cubic Zirconia" and
    /// is listed earlier, so a naive first-match search silently returned Zircon —
    /// a completely different stone (`n_d` 1.92 vs 2.15) — for `by_name("Cubic Zirconia")`.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        let all = Self::all_materials();
        if let Some(m) = all.iter().find(|m| m.name.eq_ignore_ascii_case(name)) {
            return Some(m.clone());
        }
        let needle = name.to_lowercase();
        all.into_iter()
            .filter(|m| needle.contains(&m.name.to_lowercase()))
            .max_by_key(|m| m.name.len())
    }

    /// Convenience accessor for the built-in Diamond material.
    ///
    /// # Panics
    ///
    /// Panics if `"Diamond"` is ever removed from the built-in list returned by
    /// [`all_materials`](Self::all_materials) -- this is an internal consistency
    /// invariant of this module (every name used by a convenience accessor below must
    /// have a matching entry there), not something a caller can trigger.
    #[must_use]
    pub fn diamond() -> Self {
        Self::by_name("Diamond").expect("\"Diamond\" must be present in all_materials()")
    }

    /// Convenience accessor for the built-in Ruby material.
    ///
    /// # Panics
    ///
    /// Panics if `"Ruby"` is ever removed from the built-in list returned by
    /// [`all_materials`](Self::all_materials) -- see [`diamond`](Self::diamond) for why.
    #[must_use]
    pub fn ruby() -> Self {
        Self::by_name("Ruby").expect("\"Ruby\" must be present in all_materials()")
    }

    /// Convenience accessor for the built-in Sapphire material.
    ///
    /// # Panics
    ///
    /// Panics if `"Sapphire"` is ever removed from the built-in list returned by
    /// [`all_materials`](Self::all_materials) -- see [`diamond`](Self::diamond) for why.
    #[must_use]
    pub fn sapphire() -> Self {
        Self::by_name("Sapphire").expect("\"Sapphire\" must be present in all_materials()")
    }

    /// Convenience accessor for the built-in Emerald material.
    ///
    /// # Panics
    ///
    /// Panics if `"Emerald"` is ever removed from the built-in list returned by
    /// [`all_materials`](Self::all_materials) -- see [`diamond`](Self::diamond) for why.
    #[must_use]
    pub fn emerald() -> Self {
        Self::by_name("Emerald").expect("\"Emerald\" must be present in all_materials()")
    }
}
