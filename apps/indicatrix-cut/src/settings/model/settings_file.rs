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
    /// Application settings.
    pub settings: AppSettings,
    /// Lighting presets.
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
    /// Adds any built-in preset missing from `self.presets` (matched by name) --
    /// covers a brand-new file and an older file saved before a given built-in
    /// existed -- and REFRESHES an already-present one whose own `built_in` flag is
    /// still `true` from the CURRENT [`built_in_presets`] table, rather than leaving
    /// whatever was persisted permanently stuck. Without the refresh half, a preset
    /// saved before a change to this app's own built-in definitions (e.g. the
    /// 2026-09-16 lighting rework, which redefined several) kept its stale light/
    /// camera/rig values forever, on every future load, even across an app update
    /// that changed what that preset is SUPPOSED to be.
    ///
    /// Every field is refreshed except `export_usable`: that is the one thing about
    /// a built-in this settings dialog lets a user customize per install (see
    /// [`LightingPreset::export_usable`]'s own doc comment), so a refresh must not
    /// silently undo that choice. A user preset (`built_in: false`), or one a user
    /// has renamed to no longer match any current built-in's name, is never touched
    /// here -- only an EXACT name match against a CURRENTLY `built_in: true` entry
    /// refreshes. Idempotent.
    pub fn ensure_built_in_presets(&mut self) {
        for builtin in built_in_presets() {
            if let Some(existing) = self.presets.iter_mut().find(|p| p.name == builtin.name) {
                if existing.built_in {
                    let export_usable = existing.export_usable;
                    *existing = LightingPreset {
                        export_usable,
                        ..builtin
                    };
                }
            } else {
                self.presets.push(builtin);
            }
        }
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

    /// Renames a user preset, failing when the name is taken or protected.
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

    /// Deletes a user preset by name.
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

#[cfg(test)]
mod tests {
    use super::*;

    impl SettingsFile {
        /// Looks up a lighting preset by name; the model tests' read accessor.
        #[must_use]
        pub fn find_preset(&self, name: &str) -> Option<&LightingPreset> {
            self.presets.iter().find(|p| p.name == name)
        }
    }

    #[test]
    fn ensure_built_in_presets_adds_a_missing_one() {
        let mut file = SettingsFile {
            settings: AppSettings::default(),
            presets: Vec::new(),
        };
        file.ensure_built_in_presets();
        assert_eq!(file.presets.len(), built_in_presets().len());
    }

    /// The refresh half: a built-in preset saved before this app's own definition of
    /// it changed (e.g. the 2026-09-16 lighting rework) must pick up the CURRENT
    /// definition on load, not stay stuck at whatever was persisted -- while still
    /// keeping the per-install `export_usable` choice.
    #[test]
    fn ensure_built_in_presets_refreshes_a_stale_built_in_but_keeps_export_usable() {
        let current = built_in_presets()
            .into_iter()
            .next()
            .expect("at least one built-in preset exists");
        let mut stale = current.clone();
        stale.light_yaw_deg = 999.0; // deliberately wrong -- simulates a since-changed value
        stale.export_usable = true; // the one user customization to preserve

        let mut file = SettingsFile {
            settings: AppSettings::default(),
            presets: vec![stale],
        };
        file.ensure_built_in_presets();

        let refreshed = file
            .find_preset(&current.name)
            .expect("the built-in is still present under its own name");
        assert_eq!(refreshed.light_yaw_deg, current.light_yaw_deg);
        assert!(
            refreshed.export_usable,
            "export_usable must survive the refresh"
        );
    }

    /// A USER preset (`built_in: false`) must never be overwritten by this, even if
    /// it happens to share a name with a built-in -- `upsert_user_preset` already
    /// refuses to create such a collision, but this guards the refresh path itself
    /// against ever clobbering something the user actually owns.
    #[test]
    fn ensure_built_in_presets_never_touches_a_same_named_user_preset() {
        let current = built_in_presets()
            .into_iter()
            .next()
            .expect("at least one built-in preset exists");
        let mut user_owned = current.clone();
        user_owned.built_in = false;
        user_owned.light_yaw_deg = 42.0;

        let mut file = SettingsFile {
            settings: AppSettings::default(),
            presets: vec![user_owned],
        };
        file.ensure_built_in_presets();

        let unchanged = file
            .find_preset(&current.name)
            .expect("the user preset is still present");
        assert_eq!(unchanged.light_yaw_deg, 42.0);
        assert!(!unchanged.built_in);
    }
}
