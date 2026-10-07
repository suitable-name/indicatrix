//! [`UnknownFileKeys`]: the keys of a self-contained design file this build does not
//! claim, carried on the [`crate::design::Design`] from Open to Save.
//!
//! Every table of the file ([`indicatrix_formats::native::design::DesignFile`]) keeps
//! a flattened `unknown` map of the keys a newer build wrote, and a plain file round
//! trip writes them back. A [`crate::design::Design`] has no home for them, so loading
//! a file into a design used to drop them and the next Save then wrote a file without
//! them. This type is that home: [`super::design_from_file`] fills it and
//! [`super::design_to_file`] writes it back. A tier's or concave tier's keys are keyed
//! by its stable [`TierId`], so they follow the tier through add, remove, move and undo;
//! keys of a tier that is gone are simply not written.

use crate::design::TierId;
use std::collections::BTreeMap;

/// Keys a design file carried that this build does not claim, per table.
///
/// Opaque bookkeeping, not authored content: a [`crate::design::Design`] holds it in
/// an `Option<Arc<..>>` and leaves it out of its `PartialEq`.
#[derive(Debug, Clone, Default)]
pub struct UnknownFileKeys {
    /// Top-level keys.
    pub top: toml::Table,
    /// Keys of `[preform]`.
    pub preform: toml::Table,
    /// Keys of `[material]`.
    pub material: toml::Table,
    /// Keys of `[schedule]`.
    pub schedule: toml::Table,
    /// Keys of `[history]`.
    pub history: toml::Table,
    /// Keys of each `[[tiers]]` entry, by the tier's id.
    pub tiers: BTreeMap<TierId, toml::Table>,
    /// Keys of each `[[concave_tiers]]` entry, by the concave tier's id.
    pub concave_tiers: BTreeMap<TierId, toml::Table>,
}

impl UnknownFileKeys {
    /// Whether there is nothing to carry (the usual case).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.top.is_empty()
            && self.preform.is_empty()
            && self.material.is_empty()
            && self.schedule.is_empty()
            && self.history.is_empty()
            && self.tiers.values().all(toml::Table::is_empty)
            && self.concave_tiers.values().all(toml::Table::is_empty)
    }
}
