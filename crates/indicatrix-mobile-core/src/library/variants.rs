//! Worker-only wrappers over the vault's existing variant records.
use super::{LibraryHandle, handle::db_error};
use indicatrix_vault::model::design_variant::{DesignVariant, NewVariant, VariantSummary};

impl LibraryHandle {
    /// The variants of the design `uuid`.
    ///
    /// # Errors
    ///
    /// A sentence when the library cannot be read.
    pub fn list_variants(&self, uuid: &str) -> Result<Vec<VariantSummary>, String> {
        self.lock()
            .list_variants(uuid)
            .map_err(db_error("Could not list variants"))
    }

    /// Saves a new variant of the design `uuid` and returns its row ID.
    ///
    /// # Errors
    ///
    /// A sentence when the library cannot be written.
    pub fn save_variant(&self, uuid: &str, variant: &NewVariant<'_>) -> Result<i64, String> {
        let id = self
            .lock()
            .save_variant(uuid, variant)
            .map_err(db_error("Could not save variant"))?;
        self.bump();
        Ok(id)
    }

    /// Ownership is checked for loads and every mutation, including stale row IDs.
    ///
    /// # Errors
    ///
    /// A sentence when the library cannot be read, the variant was deleted, or it belongs
    /// to another design.
    pub fn load_variant(&self, uuid: &str, id: i64) -> Result<DesignVariant, String> {
        let value = self
            .lock()
            .load_variant(id)
            .map_err(db_error("Could not load variant"))?
            .ok_or_else(|| "This variant was deleted.".to_owned())?;
        if !value.summary.design_uuid.eq_ignore_ascii_case(uuid) {
            return Err("This variant belongs to another design.".into());
        }
        Ok(value)
    }

    /// Renames a variant of the design `uuid`.
    ///
    /// # Errors
    ///
    /// The same sentences as [`Self::load_variant`], or one when the library cannot be written.
    pub fn rename_variant(&self, uuid: &str, id: i64, name: &str) -> Result<(), String> {
        self.load_variant(uuid, id)?;
        let changed = self
            .lock()
            .rename_variant(id, name)
            .map_err(db_error("Could not rename variant"))?;
        self.variant_changed(changed)
    }

    /// Sets or clears the note of a variant of the design `uuid`.
    ///
    /// # Errors
    ///
    /// The same sentences as [`Self::load_variant`], or one when the library cannot be written.
    pub fn set_variant_note(&self, uuid: &str, id: i64, note: Option<&str>) -> Result<(), String> {
        self.load_variant(uuid, id)?;
        let changed = self
            .lock()
            .set_variant_note(id, note)
            .map_err(db_error("Could not update variant note"))?;
        self.variant_changed(changed)
    }

    /// Deletes a variant of the design `uuid`.
    ///
    /// # Errors
    ///
    /// The same sentences as [`Self::load_variant`], or one when the library cannot be written.
    pub fn delete_variant(&self, uuid: &str, id: i64) -> Result<(), String> {
        self.load_variant(uuid, id)?;
        let changed = self
            .lock()
            .delete_variant(id)
            .map_err(db_error("Could not delete variant"))?;
        self.variant_changed(changed)
    }

    fn variant_changed(&self, changed: usize) -> Result<(), String> {
        if changed == 0 {
            return Err("This variant was deleted.".into());
        }
        self.bump();
        Ok(())
    }
}
