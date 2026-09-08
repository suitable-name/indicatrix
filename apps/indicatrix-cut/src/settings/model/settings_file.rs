//! [`SettingsFile`]: the full on-disk document, plus its lighting-preset CRUD.

use super::{
    app_settings::AppSettings,
    lighting_preset::{LightingPreset, built_in_presets},
};
use serde::{Deserialize, Serialize};

/// The full on-disk document: current settings plus the lighting-preset library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsFile {
    pub settings: AppSettings,
    pub presets: Vec<LightingPreset>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            settings: AppSettings::default(),
            presets: built_in_presets(),
        }
    }
}

impl SettingsFile {
    /// Adds any built-in preset missing from `self.presets` (matched by name), without
    /// touching presets the user already has -- covers both a brand-new file and an
    /// older file saved before a given built-in existed. Idempotent.
    pub fn ensure_built_in_presets(&mut self) {
        for builtin in built_in_presets() {
            if !self.presets.iter().any(|p| p.name == builtin.name) {
                self.presets.push(builtin);
            }
        }
    }

    // Not currently called from `gui::mod`, but kept as part of this type's read API
    // -- symmetric with `upsert_user_preset`/`rename_preset`/`delete_preset` -- and
    // exercised directly by tests below.
    #[allow(
        dead_code,
        reason = "only called from this module's own #[cfg(test)] tests"
    )]
    #[must_use]
    pub fn find_preset(&self, name: &str) -> Option<&LightingPreset> {
        self.presets.iter().find(|p| p.name == name)
    }

    /// Creates a new user preset, or overwrites an existing user (non-built-in) preset
    /// of the same name. Refuses to shadow a built-in.
    pub fn upsert_user_preset(&mut self, preset: LightingPreset) -> Result<(), String> {
        let trimmed = preset.name.trim();
        if trimmed.is_empty() {
            return Err("Preset name cannot be empty.".to_string());
        }
        if let Some(existing) = self.presets.iter().position(|p| p.name == trimmed) {
            if self.presets[existing].built_in {
                return Err(format!(
                    "'{trimmed}' is a built-in preset and cannot be overwritten."
                ));
            }
            self.presets[existing] = preset;
        } else {
            self.presets.push(preset);
        }
        Ok(())
    }

    pub fn rename_preset(&mut self, old_name: &str, new_name: &str) -> Result<(), String> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err("Preset name cannot be empty.".to_string());
        }
        if self
            .presets
            .iter()
            .any(|p| p.name == new_name && p.name != old_name)
        {
            return Err(format!("A preset named '{new_name}' already exists."));
        }
        let preset = self
            .presets
            .iter_mut()
            .find(|p| p.name == old_name)
            .ok_or_else(|| format!("Preset '{old_name}' not found."))?;
        if preset.built_in {
            return Err(format!(
                "'{old_name}' is a built-in preset and cannot be renamed."
            ));
        }
        preset.name = new_name.to_string();
        Ok(())
    }

    /// Flips a preset's `export_usable` flag -- the settings dialog's checkbox next to
    /// each preset row. Unlike `rename_preset`/`delete_preset`, this is NOT refused
    /// for a built-in: its name/existence is protected, but whether it's useful
    /// enough for an export fan-out is purely a matter of taste.
    pub fn set_preset_export_usable(&mut self, name: &str, usable: bool) -> Result<(), String> {
        let preset = self
            .presets
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("Preset '{name}' not found."))?;
        preset.export_usable = usable;
        Ok(())
    }

    pub fn delete_preset(&mut self, name: &str) -> Result<(), String> {
        let idx = self
            .presets
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| format!("Preset '{name}' not found."))?;
        if self.presets[idx].built_in {
            return Err(format!(
                "'{name}' is a built-in preset and cannot be deleted."
            ));
        }
        self.presets.remove(idx);
        Ok(())
    }
}
