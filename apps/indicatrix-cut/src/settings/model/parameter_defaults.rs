//! Rust-side defaults, reset functions and the one-shot stone-effects migration of the
//! parameter panels (Stone / Lighting & Environment / Render & Output).
//!
//! The panels' "Reset this tab" buttons call [`reset_tab`] (through a callback) instead of
//! assigning literals in Slint, so there is exactly one place that knows the defaults: the
//! `Default` impls of the settings structs.

use super::{
    app_settings::AppSettings, lighting_values::LightingValues, output_settings::OutputSettings,
};
use indicatrix_cut_core::StoneRenderEffects;

/// The three tabs of the Parameters dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterTab {
    /// Stone: the per-design render effects (material and geometry are undoable design
    /// edits and are never touched by a reset).
    Stone,
    /// Lighting & Environment: rig, light, exposure, backdrop, glare, head shadow, HDR map.
    Lighting,
    /// Render & Output: live quality, compute, output defaults, thumbnails.
    RenderOutput,
}

impl ParameterTab {
    /// The tab at Slint index `0`/`1`/`2`; anything else is the Stone tab.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Lighting,
            2 => Self::RenderOutput,
            _ => Self::Stone,
        }
    }

    /// Inverse of [`Self::from_index`].
    #[cfg(test)]
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Stone => 0,
            Self::Lighting => 1,
            Self::RenderOutput => 2,
        }
    }
}

/// The built-in defaults of the Stone tab's per-design effects: everything off.
#[must_use]
pub fn default_stone_effects() -> StoneRenderEffects {
    StoneRenderEffects::default()
}

/// The legacy app-level stone values of a settings file written before the effects moved
/// into the design. See [`AppSettings::legacy_stone_defaults`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegacyStoneDefaults {
    /// The old app-level effects, as the design's own type.
    pub effects: StoneRenderEffects,
    /// The old app-level stone width in millimetres, `None` when it was off (`0.0`).
    pub stone_width_mm: Option<f64>,
}

impl AppSettings {
    /// The old app-level stone effects and width, when they are worth carrying over: the
    /// migration has not run yet ([`Self::migrated_stone_effects`] is `false`) and at least
    /// one of the six legacy values is off its default. `None` otherwise.
    ///
    /// The caller applies them ONCE, as the starting values of the first design that is
    /// opened with no `[render_effects]` table and no girdle diameter (an undoable
    /// `Edit::SetRenderEffects` / `Edit::SetGirdleDiameterMm`), then calls
    /// [`Self::mark_stone_effects_migrated`]. The legacy fields themselves stay in the file
    /// untouched and are no longer read for rendering.
    #[must_use]
    pub fn legacy_stone_defaults(&self) -> Option<LegacyStoneDefaults> {
        if self.migrated_stone_effects {
            return None;
        }
        let effects = StoneRenderEffects::from_render_values(
            self.inclusion_sigma_s,
            self.c_axis_override_enabled,
            self.c_axis_tilt_deg,
            self.c_axis_azimuth_deg,
            self.girdle_frosted,
            self.edge_rounding_radius,
        );
        let width = f64::from(self.stone_width_mm);
        let stone_width_mm = (width.is_finite() && width > 0.0).then_some(width);
        (!effects.is_default() || stone_width_mm.is_some()).then_some(LegacyStoneDefaults {
            effects,
            stone_width_mm,
        })
    }

    /// Records that the legacy stone values were consumed (or that there was nothing to
    /// carry over): [`Self::legacy_stone_defaults`] returns `None` from now on.
    pub const fn mark_stone_effects_migrated(&mut self) {
        self.migrated_stone_effects = true;
    }

    /// Resets the Render & Output tab to the built-in defaults: live quality, the local
    /// preview, the compute targets, the denoise toggle, the output defaults and the
    /// library thumbnail values. The remote endpoint and the transfer settings that belong
    /// to it are never touched.
    pub fn reset_render_output_tab(&mut self) {
        let d = Self::default();
        self.ray_trace = d.ray_trace;
        self.target_samples = d.target_samples;
        self.render_width = d.render_width;
        self.render_height = d.render_height;
        self.max_bounces = d.max_bounces;
        self.local_preview_scale = d.local_preview_scale;
        self.live_compute_target = d.live_compute_target;
        self.local_compute_target = d.local_compute_target;
        self.tilt_video_compute_target = d.tilt_video_compute_target;
        self.contribute_to_final_picture = d.contribute_to_final_picture;
        self.denoise_enabled = d.denoise_enabled;
        self.preview_size = d.preview_size;
        self.thumbnail_renderer = d.thumbnail_renderer;
        self.thumbnail_backdrop = d.thumbnail_backdrop;
        self.preview_spp = d.preview_spp;
        self.export_filename_template = d.export_filename_template;
        self.output = OutputSettings::default();
    }

    /// Resets the Lighting & Environment tab to the built-in defaults: the rig, the light
    /// direction, exposure, backdrop, surface glare, head shadow and the HDR map (cleared:
    /// the studio rig comes back). The camera pose is not lighting and is not touched.
    pub fn reset_lighting_tab(&mut self) {
        LightingValues::default().write_into(self);
    }
}

/// Resets one tab of the settings. Returns the per-design effects the caller must apply to
/// the open design for the Stone tab (as an undoable `Edit::SetRenderEffects`); `None` for
/// the two tabs that live entirely in the settings file.
#[must_use]
pub fn reset_tab(settings: &mut AppSettings, tab: ParameterTab) -> Option<StoneRenderEffects> {
    match tab {
        ParameterTab::Stone => Some(default_stone_effects()),
        ParameterTab::Lighting => {
            settings.reset_lighting_tab();
            None
        }
        ParameterTab::RenderOutput => {
            settings.reset_render_output_tab();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::{SettingsFile, TiltVideoCompute};

    #[test]
    fn an_old_file_with_default_stone_values_has_nothing_to_migrate() {
        let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").expect("parse");
        assert!(!parsed.settings.migrated_stone_effects);
        assert_eq!(parsed.settings.legacy_stone_defaults(), None);
    }

    #[test]
    fn an_old_file_with_stone_values_offers_them_once() {
        let text =
            "[settings]\ninclusion_sigma_s = 0.5\ngirdle_frosted = true\nstone_width_mm = 7.5\n";
        let mut parsed: SettingsFile = toml::from_str(text).expect("parse");
        let legacy = parsed.settings.legacy_stone_defaults().expect("offered");
        assert!(legacy.effects.girdle_frosted);
        assert_eq!(legacy.effects.inclusion_sigma_s.to_bits(), 0.5f64.to_bits());
        assert_eq!(legacy.stone_width_mm, Some(7.5));
        parsed.settings.mark_stone_effects_migrated();
        assert_eq!(parsed.settings.legacy_stone_defaults(), None);
        // The legacy fields stay in the file untouched.
        assert!(parsed.settings.girdle_frosted);
    }

    #[test]
    fn a_new_install_has_nothing_to_migrate() {
        assert!(AppSettings::default().migrated_stone_effects);
        assert_eq!(AppSettings::default().legacy_stone_defaults(), None);
    }

    /// A settings file from before the dock, the output defaults and the migration flag: none
    /// of the new keys is there, and each reads its documented default.
    #[test]
    fn an_old_file_without_the_dock_output_or_migration_keys_loads_the_defaults() {
        let parsed: SettingsFile =
            toml::from_str("[settings]\ntarget_samples = 64\nexposure = 1.5\n").expect("parse");
        let s = parsed.settings;
        assert_eq!(s.target_samples, 64);
        assert!(!s.parameters_dock_open, "the dock starts folded");
        assert_eq!(s.parameters_dock_tab, 0, "on the Stone tab");
        assert_eq!(s.output, OutputSettings::default());
        assert!(
            !s.migrated_stone_effects,
            "an old file still has values to offer"
        );
    }

    /// What the dock and the output defaults write comes back as written.
    #[test]
    fn the_dock_state_and_the_output_defaults_survive_a_round_trip() {
        let mut settings = AppSettings {
            parameters_dock_open: true,
            parameters_dock_tab: 2,
            ..AppSettings::default()
        };
        settings.output.still.sample_exponent = 12;
        settings.output.still.bounces = 64;
        settings.output.tilt_video.filename_template = "mine_{design}".to_string();
        let file = SettingsFile {
            settings,
            ..SettingsFile::default()
        };
        let text = toml::to_string_pretty(&file).expect("serialises");
        let back: SettingsFile = toml::from_str(&text).expect("parses");
        assert!(back.settings.parameters_dock_open);
        assert_eq!(back.settings.parameters_dock_tab, 2);
        assert_eq!(back.settings.output, file.settings.output);
        assert!(back.settings.migrated_stone_effects);
    }

    #[test]
    fn the_tab_indices_round_trip_and_anything_else_is_the_stone_tab() {
        for tab in [
            ParameterTab::Stone,
            ParameterTab::Lighting,
            ParameterTab::RenderOutput,
        ] {
            assert_eq!(ParameterTab::from_index(tab.index()), tab);
        }
        assert_eq!(ParameterTab::from_index(-1), ParameterTab::Stone);
        assert_eq!(ParameterTab::from_index(3), ParameterTab::Stone);
    }

    /// The Stone tab's "defaults" are the `Default` of the design's effects, and they are the
    /// very values the app-level settings default to, so a reset and a fresh install agree.
    #[test]
    fn the_stone_defaults_are_the_default_impl_and_the_settings_defaults() {
        assert_eq!(default_stone_effects(), StoneRenderEffects::default());
        assert!(default_stone_effects().is_default());
        let d = AppSettings::default();
        assert!(
            StoneRenderEffects::from_render_values(
                d.inclusion_sigma_s,
                d.c_axis_override_enabled,
                d.c_axis_tilt_deg,
                d.c_axis_azimuth_deg,
                d.girdle_frosted,
                d.edge_rounding_radius,
            )
            .is_default()
        );
    }

    #[test]
    fn resetting_the_stone_tab_hands_back_the_default_effects_and_changes_no_setting() {
        let mut s = AppSettings {
            inclusion_sigma_s: 0.5,
            girdle_frosted: true,
            stone_width_mm: 9.0,
            exposure: 2.0,
            ..AppSettings::default()
        };
        let before = s.clone();
        assert_eq!(
            reset_tab(&mut s, ParameterTab::Stone),
            Some(default_stone_effects())
        );
        assert_eq!(s, before, "the effects live in the design, not here");
    }

    /// Every field the Render & Output tab owns goes back to what `Default` says, and nothing
    /// else (not the lighting, the camera, the material or the dock) moves.
    #[test]
    fn resetting_the_render_tab_equals_the_default_for_every_field_it_owns() {
        use crate::settings::model::{LiveComputeTarget, LocalComputeTarget, LocalPreviewScale};
        let mut s = AppSettings::default();
        s.target_samples = 8;
        s.render_width = 320;
        s.render_height = 200;
        s.max_bounces = 4;
        s.local_preview_scale = LocalPreviewScale::Half;
        s.live_compute_target = LiveComputeTarget::LocalOnly;
        s.local_compute_target = LocalComputeTarget::Cpu;
        s.tilt_video_compute_target = TiltVideoCompute::Local;
        s.contribute_to_final_picture = !s.contribute_to_final_picture;
        s.denoise_enabled = !s.denoise_enabled;
        s.preview_size = 96;
        s.preview_spp = 32;
        s.export_filename_template = "mine".to_string();
        s.output.still.sample_exponent = 3;
        s.output.still.compute = TiltVideoCompute::Local;
        s.output.tilt_video.filename_template = "mine".to_string();
        // Not this tab's.
        s.exposure = 3.0;
        s.selected_material = "Ruby".to_string();
        s.camera_yaw = 0.9;
        s.parameters_dock_tab = 2;

        assert!(reset_tab(&mut s, ParameterTab::RenderOutput).is_none());
        let expected = AppSettings {
            exposure: 3.0,
            selected_material: "Ruby".to_string(),
            camera_yaw: 0.9,
            parameters_dock_tab: 2,
            ..AppSettings::default()
        };
        assert_eq!(s, expected);
    }

    /// The same for the Lighting & Environment tab: the whole look (a loaded HDR map included)
    /// goes back to `LightingValues::default()`, and the camera pose is not lighting.
    #[test]
    fn resetting_the_lighting_tab_equals_the_default_look_and_touches_nothing_else() {
        use crate::settings::model::Backdrop;
        let mut s = AppSettings {
            lighting_rig: "Incandescent (3200K)".to_string(),
            light_yaw_deg: 10.0,
            light_pitch_deg: 30.0,
            exposure: 2.0,
            surface_glare: 0.5,
            backdrop: Backdrop::White,
            head_shadow_deg: 0.0,
            env_map_path: "C:/env/a.hdr".to_string(),
            camera_yaw: 0.9,
            target_samples: 8,
            ..AppSettings::default()
        };

        assert!(reset_tab(&mut s, ParameterTab::Lighting).is_none());
        assert_eq!(
            LightingValues::from_app_settings(&s),
            LightingValues::default()
        );
        assert_eq!(s.env_map_path, "", "the HDR map is dropped");
        let expected = AppSettings {
            camera_yaw: 0.9,
            target_samples: 8,
            ..AppSettings::default()
        };
        assert_eq!(s, expected);
    }

    #[test]
    fn resetting_a_tab_restores_its_defaults_and_nothing_else() {
        let mut s = AppSettings {
            target_samples: 8,
            exposure: 3.0,
            light_yaw_deg: 10.0,
            ..AppSettings::default()
        };
        assert!(reset_tab(&mut s, ParameterTab::RenderOutput).is_none());
        assert_eq!(s.target_samples, AppSettings::default().target_samples);
        assert_eq!(s.exposure.to_bits(), 3.0f32.to_bits(), "lighting untouched");
        assert!(reset_tab(&mut s, ParameterTab::Lighting).is_none());
        assert_eq!(
            s.exposure.to_bits(),
            AppSettings::default().exposure.to_bits()
        );
        assert_eq!(
            s.light_yaw_deg.to_bits(),
            AppSettings::default().light_yaw_deg.to_bits()
        );
        assert_eq!(
            reset_tab(&mut s, ParameterTab::Stone),
            Some(StoneRenderEffects::default())
        );
    }
}
