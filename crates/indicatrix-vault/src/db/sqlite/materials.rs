use super::Database;
use anyhow::Result;
use rusqlite::params;

/// Bundled parameters for [`Database::save_custom_material`].
///
/// See `crates/indicatrix/src/optics/raytracer/refraction.rs`'s `RayMaterialContext` for the
/// established pattern this follows. Borrowed rather than reusing
/// [`crate::model::material::CustomMaterialRow`] outright: that type owns its strings
/// (it's what a *read* hands back), while a caller about to save a material usually
/// already holds `&str`s -- forcing an allocation just to call this would be worse than
/// the extra parameters it replaces.
pub struct CustomMaterialParams<'a> {
    pub name: &'a str,
    pub refractive_index: f32,
    pub dispersion: f32,
    pub birefringence: f32,
    pub absorption_rgb: [f32; 3],
    pub crystal_system: Option<&'a str>,
    pub optical_character: Option<&'a str>,
    pub biaxial_delta_beta_alpha: Option<f32>,
    /// See `crate::model::material::CustomMaterialRow::per_axis_dispersion_json`'s doc
    /// comment. Borrowed (`&str`) for the same reason every other field here is, per
    /// this struct's own doc comment.
    pub per_axis_dispersion_json: Option<&'a str>,
}

impl Database {
    /// Retrieves all user-defined custom gemstone materials
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or running the underlying `SELECT` query fails.
    /// A row that fails to decode is silently skipped (via `.flatten()`) rather than
    /// failing the whole call.
    pub fn get_custom_materials(&self) -> Result<Vec<crate::model::material::CustomMaterialRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, refractive_index, dispersion, birefringence, absorption_r, absorption_g, absorption_b,
                    crystal_system, optical_character, biaxial_delta_beta_alpha, per_axis_dispersion_json
             FROM custom_gem_materials
             ORDER BY name ASC"
        )?;

        let rows = stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let ri: f64 = row.get(1)?;
            let disp: f64 = row.get(2)?;
            let biref: f64 = row.get(3)?;
            let abs_r: f64 = row.get(4)?;
            let abs_g: f64 = row.get(5)?;
            let abs_b: f64 = row.get(6)?;
            let crystal_system: Option<String> = row.get(7)?;
            let optical_character: Option<String> = row.get(8)?;
            let biaxial_delta_beta_alpha: Option<f64> = row.get(9)?;
            let per_axis_dispersion_json: Option<String> = row.get(10)?;

            Ok(crate::model::material::CustomMaterialRow {
                name,
                refractive_index: ri as f32,
                dispersion: disp as f32,
                birefringence: biref as f32,
                absorption_rgb: [abs_r as f32, abs_g as f32, abs_b as f32],
                crystal_system,
                optical_character,
                biaxial_delta_beta_alpha: biaxial_delta_beta_alpha.map(|v| v as f32),
                per_axis_dispersion_json,
            })
        })?;

        let mut materials = Vec::new();
        for m in rows.flatten() {
            materials.push(m);
        }
        Ok(materials)
    }

    /// Saves or updates a custom gemstone material.
    ///
    /// `crystal_system`/`optical_character` are a `indicatrix` enum variant name (e.g.
    /// `"Trigonal"`) or `None`; `biaxial_delta_beta_alpha` is `None` for every
    /// isotropic/uniaxial material; `per_axis_dispersion_json` is `None` unless
    /// the caller authored genuine per-axis dispersion data. All four round-trip
    /// through [`Self::get_custom_materials`] as the matching `Option` fields on
    /// `CustomMaterialRow` -- see that struct's doc comments for why plain text/`f32`/
    /// JSON rather than the `indicatrix` enums (this crate must not depend on `indicatrix`)
    /// and for what `None` means to a caller that does (fall back to whatever
    /// `GemMaterial::new_custom` infers).
    ///
    /// # Errors
    ///
    /// Returns an error if preparing the upsert statement or executing it (`INSERT
    /// ... ON CONFLICT(name) DO UPDATE`) fails.
    pub fn save_custom_material(&self, material: &CustomMaterialParams<'_>) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "INSERT INTO custom_gem_materials (name, refractive_index, dispersion, birefringence, absorption_r, absorption_g, absorption_b,
                crystal_system, optical_character, biaxial_delta_beta_alpha, per_axis_dispersion_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(name) DO UPDATE SET
                refractive_index = excluded.refractive_index,
                dispersion = excluded.dispersion,
                birefringence = excluded.birefringence,
                absorption_r = excluded.absorption_r,
                absorption_g = excluded.absorption_g,
                absorption_b = excluded.absorption_b,
                crystal_system = excluded.crystal_system,
                optical_character = excluded.optical_character,
                biaxial_delta_beta_alpha = excluded.biaxial_delta_beta_alpha,
                per_axis_dispersion_json = excluded.per_axis_dispersion_json"
        )?;

        stmt.execute(params![
            material.name,
            f64::from(material.refractive_index),
            f64::from(material.dispersion),
            f64::from(material.birefringence),
            f64::from(material.absorption_rgb[0]),
            f64::from(material.absorption_rgb[1]),
            f64::from(material.absorption_rgb[2]),
            material.crystal_system,
            material.optical_character,
            material.biaxial_delta_beta_alpha.map(f64::from),
            material.per_axis_dispersion_json,
        ])?;
        Ok(())
    }

    /// Deletes a user-defined custom gemstone material
    ///
    /// # Errors
    ///
    /// Returns an error if preparing or executing the underlying `DELETE` statement
    /// fails. Deleting a `name` that doesn't exist is not an error -- it simply
    /// affects zero rows.
    pub fn delete_custom_material(&self, name: &str) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("DELETE FROM custom_gem_materials WHERE name = ?1")?;
        stmt.execute(params![name])?;
        Ok(())
    }
}
