//! The bridge between the per-design stone render effects
//! ([`indicatrix_cut_core::StoneRenderEffects`], stored on the `Design` and in the native
//! file) and the live render context plus the controls that mirror it.
//!
//! The Stone tab drives the picture live (`render::material`), and a debounced commit writes
//! the controls' values into the design as one undoable `Edit::SetRenderEffects`
//! (`editor::callbacks::tier_actions::viewport_material`). The editor refresh pushes the
//! design's own effects back into the context and the controls, except while a live edit
//! waits for its commit ([`live_edit_pending`]) so a slider drag is never overwritten.
//! Nothing here touches the settings file: the effects live in the design, and the old
//! `AppSettings` fields are read-only legacy values.

use crate::{MainWindow, SettingsModel, bridge::render_thread::RenderContext, gui::optics::c_axis};
use indicatrix::render_setup::inclusion_scale;
use indicatrix_cut_core::{Design, Edit, StoneRenderEffects};
use slint::ComponentHandle;
use std::sync::atomic::{AtomicBool, Ordering};

/// A stored render-effect value (f64 in the design) at the f32 precision the render
/// context and the Slint controls use.
#[expect(
    clippy::cast_possible_truncation,
    reason = "render effects are small bounded values (angles, radii, sigma); f32 is the render and control precision"
)]
const fn to_f32(value: f64) -> f32 {
    value as f32
}

/// Set by a Stone-tab control while its change is shown but not yet written to the design.
static LIVE_EDIT_PENDING: AtomicBool = AtomicBool::new(false);

/// Serialises the tests that touch the process-wide pending flag.
#[cfg(test)]
pub(in crate::gui) static PENDING_FLAG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A control changed the picture and its commit to the design is still to come.
pub(in crate::gui) fn mark_live_edit_pending() {
    LIVE_EDIT_PENDING.store(true, Ordering::SeqCst);
}

/// The pending live edit was committed (or dropped).
pub(in crate::gui) fn clear_live_edit_pending() {
    LIVE_EDIT_PENDING.store(false, Ordering::SeqCst);
}

/// Whether a live edit waits for its commit: the editor refresh must not push the design's
/// effects over the controls meanwhile.
#[must_use]
pub(in crate::gui) fn live_edit_pending() -> bool {
    LIVE_EDIT_PENDING.load(Ordering::SeqCst)
}

/// The effects the Stone tab's controls show, as the design's own type. Read from the controls
/// (their exact `f32` values), not from the render context, so a committed value converts back
/// to exactly what the context already holds.
#[must_use]
pub(in crate::gui) fn effects_from_controls(ui: &MainWindow) -> StoneRenderEffects {
    let settings = ui.global::<SettingsModel>();
    StoneRenderEffects::from_render_values(
        settings.get_inclusion_sigma_s(),
        settings.get_c_axis_override_enabled(),
        settings.get_c_axis_tilt_deg(),
        settings.get_c_axis_azimuth_deg(),
        settings.get_girdle_frosted(),
        settings.get_edge_rounding_radius(),
    )
}

/// Whether the render context already traces with `effects`, up to the rounding the design's
/// stored values carry (six decimals), so a committed value does not look "changed" and restart
/// the render when the editor refresh compares it.
#[must_use]
pub(in crate::gui) fn context_matches(ctx: &RenderContext, effects: &StoneRenderEffects) -> bool {
    const EPS: f32 = 2e-6;
    let axis_matches = match (ctx.c_axis_override, effects.c_axis_override_enabled) {
        (None, false) => true,
        (Some(have), true) => {
            let want = c_axis::angles_to_c_axis(
                to_f32(effects.c_axis_tilt_deg),
                to_f32(effects.c_axis_azimuth_deg),
            );
            (have - want).length() < 1e-5
        }
        _ => false,
    };
    axis_matches
        && (ctx.inclusion_sigma_s - to_f32(effects.inclusion_sigma_s)).abs() <= EPS
        && ctx.girdle_frosted == effects.girdle_frosted
        && (ctx.edge_rounding_radius - to_f32(effects.edge_rounding_radius)).abs() <= EPS
}

/// Pushes `effects` into the render context (and marks it dirty so the render restarts).
pub(in crate::gui) fn apply_effects_to_context(
    ctx: &mut RenderContext,
    effects: &StoneRenderEffects,
) {
    ctx.inclusion_sigma_s = to_f32(effects.inclusion_sigma_s);
    ctx.c_axis_override = effects.c_axis_override_enabled.then(|| {
        c_axis::angles_to_c_axis(
            to_f32(effects.c_axis_tilt_deg),
            to_f32(effects.c_axis_azimuth_deg),
        )
    });
    ctx.girdle_frosted = effects.girdle_frosted;
    ctx.edge_rounding_radius = to_f32(effects.edge_rounding_radius);
    ctx.dirty = true;
}

/// Shows `effects` in the controls that mirror them. Setting a property from Rust does not
/// fire the control's own "changed" callback, so nothing is written back by this.
pub(in crate::gui) fn push_effects_to_controls(ui: &MainWindow, effects: &StoneRenderEffects) {
    let settings = ui.global::<SettingsModel>();
    let sigma_s = to_f32(effects.inclusion_sigma_s);
    settings.set_inclusion_sigma_s(sigma_s);
    settings.set_inclusion_position(inclusion_scale::sigma_s_to_position(sigma_s));
    settings.set_c_axis_override_enabled(effects.c_axis_override_enabled);
    settings.set_c_axis_tilt_deg(to_f32(effects.c_axis_tilt_deg));
    settings.set_c_axis_azimuth_deg(to_f32(effects.c_axis_azimuth_deg));
    settings.set_girdle_frosted(effects.girdle_frosted);
    settings.set_edge_rounding_radius(to_f32(effects.edge_rounding_radius));
}

/// The undoable edit that sets `design`'s render effects to `effects`, or `None` when they
/// already are exactly that (so an unchanged control writes no history entry).
#[must_use]
pub(in crate::gui) fn effects_edit(design: &Design, effects: StoneRenderEffects) -> Option<Edit> {
    let effects = effects.sanitised();
    (design.render_effects != effects).then_some(Edit::SetRenderEffects { effects })
}

/// The edits that carry the legacy app-level stone values onto a design that has neither a
/// `[render_effects]` table nor a girdle diameter (see
/// `AppSettings::legacy_stone_defaults`): one `SetRenderEffects` when the legacy effects are
/// not the default, one `SetGirdleDiameterMm` when a legacy width was set. Empty for a
/// design that already has its own values.
#[must_use]
pub(in crate::gui) fn legacy_migration_edits(
    design: &Design,
    legacy: &crate::settings::model::LegacyStoneDefaults,
) -> Vec<Edit> {
    let mut edits = Vec::new();
    if design.render_effects.is_default() && !legacy.effects.is_default() {
        edits.push(Edit::SetRenderEffects {
            effects: legacy.effects,
        });
    }
    if design.girdle_diameter_mm.is_none() && legacy.stone_width_mm.is_some() {
        edits.push(Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: legacy.stone_width_mm,
        });
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::{AppSettings, SettingsFile};
    use indicatrix_editor::EditorSession;

    fn haze() -> StoneRenderEffects {
        StoneRenderEffects::from_render_values(0.5, true, 30.0, 120.0, true, 0.01)
    }

    #[test]
    fn a_context_set_from_effects_matches_them_exactly() {
        let mut ctx = RenderContext::default();
        assert!(context_matches(&ctx, &StoneRenderEffects::default()));
        assert!(!context_matches(&ctx, &haze()));
        apply_effects_to_context(&mut ctx, &haze());
        assert!(context_matches(&ctx, &haze()));
    }

    /// A settings file written before the effects and the width moved into the design: all six
    /// legacy effect keys and the width are off their defaults.
    const OLD_FILE: &str = "[settings]\ninclusion_sigma_s = 0.5\ngirdle_frosted = true\n\
        c_axis_override_enabled = true\nc_axis_tilt_deg = 30.0\nc_axis_azimuth_deg = 120.0\n\
        edge_rounding_radius = 0.01\nstone_width_mm = 7.5\n";

    fn old_settings() -> AppSettings {
        toml::from_str::<SettingsFile>(OLD_FILE)
            .expect("an old settings file parses")
            .settings
    }

    #[test]
    fn the_old_app_level_values_are_applied_to_the_first_design_once_and_then_ignored() {
        let mut settings = old_settings();
        let mut first = EditorSession::fresh();
        assert!(first.design.render_effects.is_default());
        assert_eq!(first.design.girdle_diameter_mm, None);

        let legacy = settings
            .legacy_stone_defaults()
            .expect("the old values are offered");
        let edits = legacy_migration_edits(&first.design, &legacy);
        assert_eq!(edits.len(), 2, "the effects and the width");
        first
            .apply(Edit::Batch(edits))
            .expect("the migration applies");
        assert_eq!(first.design.girdle_diameter_mm, Some(7.5));
        let effects = first.design.render_effects;
        assert_eq!(effects.inclusion_sigma_s.to_bits(), 0.5f64.to_bits());
        assert!(effects.girdle_frosted && effects.c_axis_override_enabled);
        assert!(effects.edge_rounding_radius > 0.0);

        // The whole migration is one undo step.
        first.undo().expect("undoes").expect("something to undo");
        assert!(first.design.render_effects.is_default());
        assert_eq!(first.design.girdle_diameter_mm, None);

        // Once the flag is set the settings offer nothing, so a second design starts clean.
        settings.mark_stone_effects_migrated();
        assert_eq!(settings.legacy_stone_defaults(), None);
        let second = EditorSession::fresh();
        let later = settings
            .legacy_stone_defaults()
            .map(|legacy| legacy_migration_edits(&second.design, &legacy))
            .unwrap_or_default();
        assert_eq!(later.len(), 0);
        // The legacy keys stay in the file, untouched and unread.
        assert!(settings.girdle_frosted);
        assert_eq!(settings.stone_width_mm.to_bits(), 7.5f32.to_bits());
    }

    #[test]
    fn the_migration_never_overwrites_values_a_design_already_has() {
        let legacy = old_settings()
            .legacy_stone_defaults()
            .expect("the old values are offered");
        let mut session = EditorSession::fresh();
        session
            .apply(Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(6.0),
            })
            .expect("the width applies");
        let edits = legacy_migration_edits(&session.design, &legacy);
        assert_eq!(edits.len(), 1, "only the effects: the design has its width");
        assert!(matches!(edits[0], Edit::SetRenderEffects { .. }));
        session
            .apply(Edit::SetRenderEffects { effects: haze() })
            .expect("the effects apply");
        assert_eq!(legacy_migration_edits(&session.design, &legacy).len(), 0);
        assert_eq!(session.design.girdle_diameter_mm, Some(6.0));
    }

    #[test]
    fn an_unchanged_control_writes_no_history_entry_and_a_changed_one_is_undoable() {
        let mut session = EditorSession::fresh();
        assert!(effects_edit(&session.design, StoneRenderEffects::default()).is_none());
        let edit = effects_edit(&session.design, haze()).expect("a change is an edit");
        session.apply(edit).expect("the effects apply");
        assert_eq!(session.design.render_effects, haze().sanitised());
        assert!(effects_edit(&session.design, haze()).is_none());
        session.undo().expect("undoes").expect("something to undo");
        assert!(session.design.render_effects.is_default());
    }
}
