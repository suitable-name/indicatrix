/// A user-defined custom gemstone material, exactly as stored in the
/// `custom_gem_materials` table.
///
/// A plain data row owned by `indicatrix-vault` -- deliberately does NOT depend on
/// `indicatrix`'s richer `GemMaterial` type, so this crate stays independently
/// publishable. Callers that need a `GemMaterial` convert this row via
/// `GemMaterial::new_custom(..)`, then apply `crystal_system`/`optical_character`/
/// `biaxial_delta_beta_alpha` on top.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomMaterialRow {
    pub name: String,
    pub refractive_index: f32,
    pub dispersion: f32,
    pub birefringence: f32,
    pub absorption_rgb: [f32; 3],
    /// `indicatrix::optics::materials::CrystalSystem`'s variant name as plain text (e.g.
    /// `"Trigonal"`), or `None`. Stored as a string, not the enum, so this crate still
    /// doesn't depend on `indicatrix` -- `apps/indicatrix-cut` parses it back. `None`
    /// covers both "saved before this field existed" and "author never overrode it";
    /// either way the caller falls back to what `GemMaterial::new_custom(..)` infers
    /// from `birefringence` alone.
    pub crystal_system: Option<String>,
    /// `indicatrix::optics::materials::OpticalCharacter`'s variant name as plain text
    /// (e.g. `"UniaxialNegative"`), or `None`. Same string-not-enum reasoning and
    /// fallback contract as `crystal_system` above.
    pub optical_character: Option<String>,
    /// `indicatrix::optics::materials::GemMaterial::biaxial_delta_beta_alpha` --
    /// `n_beta - n_alpha` at the sodium D line -- carried straight through as `f32`
    /// since it's already a plain number. `None` for every isotropic/uniaxial material
    /// and for any row saved before biaxial authoring existed; only meaningful when
    /// `optical_character` is one of the two biaxial variants.
    pub biaxial_delta_beta_alpha: Option<f32>,
    /// `indicatrix`'s optional per-axis dispersion coefficients (e.g.
    /// `GemMaterial::uniaxial_extraordinary_dispersion`, a `DispersionModel`), stored as
    /// a JSON string -- same string-not-enum boundary reasoning as
    /// `crystal_system`/`optical_character`. `None` covers the same two cases those
    /// fields do; the caller falls back to the material's ordinary
    /// constant-`birefringence_delta` behaviour.
    pub per_axis_dispersion_json: Option<String>,
    /// The material's specific gravity (density relative to water), or `None` for
    /// a row saved before this field existed or an author who never typed one.
    /// With this `None`, carat-weight estimates for a custom material stay empty
    /// unless the cutter separately overrides SG per design. A caller wanting a
    /// `GemMaterial`-side fallback should treat `None`
    /// the same way `crystal_system`/`optical_character` do -- infer or leave blank,
    /// never guess a number.
    pub specific_gravity: Option<f32>,
}
