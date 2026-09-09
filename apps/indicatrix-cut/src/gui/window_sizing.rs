//! Initial main-window size that fits the monitor it opens on, plus the Edit sub-tab's
//! per-screen layout default that rides along with it.
//!
//! `ui/app.slint` asks for a 1600 x 900 logical window, which is the comfortable
//! working size for the editor dock plus a viewport. On a Full HD monitor at 125 %
//! scaling the usable area is only about 1536 x 830 logical pixels, so that request
//! would spill past the screen edge. This module shrinks the request to a fraction of
//! the monitor the window actually landed on, using the `winit` window behind Slint's
//! (the same accessor `render::detached_render` uses for always-on-top). Nothing here
//! sets a minimum: the window stays freely resizable, and content clips instead of
//! pushing the window around (see the sizing notes at the top of `ui/app.slint`).
//!
//! The same fit pass also decides whether the resulting window is small enough that
//! the Edit sub-tab should start with its inspector collapsed and its dock narrower --
//! see [`apply_small_screen_layout_default`].

use crate::{EditorModel, MainWindow};
use slint::{ComponentHandle, winit_030::WinitWindowAccessor};
use tracing::{debug, warn};

/// What `ui/app.slint` declares as `preferred-width` / `preferred-height`.
const PREFERRED_WIDTH: f32 = 1600.0;
const PREFERRED_HEIGHT: f32 = 900.0;

/// Fraction of the monitor's logical size the window may take on first show. The
/// vertical fraction is lower because task bars and title bars eat height, not width,
/// and `winit` exposes no cross-platform work-area query.
const MAX_WIDTH_FRACTION: f32 = 0.94;
const MAX_HEIGHT_FRACTION: f32 = 0.88;

/// Below this fitted window height, the Edit sub-tab starts with its inspector
/// collapsed and its dock narrower -- a laptop/small-monitor default so a shorter
/// combined viewport+dock column leads with the tier table, not the inspector's own
/// fixed-height panel. Applied only once (at startup) and only while the user has
/// never adjusted the layout themselves -- see [`apply_small_screen_layout_default`].
const SMALL_SCREEN_HEIGHT_THRESHOLD: f32 = 820.0;
/// The Edit sub-tab's dock width applied under [`SMALL_SCREEN_HEIGHT_THRESHOLD`] --
/// narrower than `AppSettings::DEFAULT_EDITOR_DOCK_WIDTH` (640px) so the viewport
/// still gets reasonable room on a shorter/narrower monitor.
const SMALL_SCREEN_DOCK_WIDTH: f32 = 600.0;

/// Shrinks the freshly shown main window so it fits its monitor, and (see
/// [`apply_small_screen_layout_default`]) applies the Edit sub-tab's small-screen
/// layout default if this is a small enough screen and the user has never touched the
/// layout themselves.
///
/// Must run after `show()`; the `winit` window is created lazily by the event loop, so
/// the fit is deferred to the first event-loop turn with a zero-length single-shot
/// timer. Enlarging never happens: a window that already fits is left alone, so a
/// remembered/OS-restored size is not clobbered.
///
/// `editor_layout_touched` is `gui::run_gui`'s own settings-store snapshot of
/// `AppSettings::editor_layout_touched`, threaded through rather than read from
/// `EditorModel` here -- by the time this timer fires, `EditorModel.dock_width`/
/// `inspector_collapsed` already hold whatever `editor_layout::
/// apply_editor_layout_from_settings` restored, which is exactly what this function is
/// about to overwrite for a small screen, so the "was it ever touched" question has to
/// be answered from the settings snapshot, not from the (about to change) UI state.
pub fn fit_initial_window_size(ui: &MainWindow, editor_layout_touched: bool) {
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let monitor = ui
            .window()
            .with_winit_window(|winit_window| {
                winit_window
                    .current_monitor()
                    .or_else(|| winit_window.primary_monitor())
                    .map(|monitor| {
                        let physical = monitor.size();
                        let scale = monitor.scale_factor();
                        (
                            f64::from(physical.width) / scale,
                            f64::from(physical.height) / scale,
                        )
                    })
            })
            .flatten();
        let Some((monitor_width, monitor_height)) = monitor else {
            warn!(
                "Could not determine the monitor size for the main window -- keeping the \
                 .slint preferred size; the window stays resizable."
            );
            return;
        };
        let current = ui.window().size().to_logical(ui.window().scale_factor());
        let target = fitted_size(monitor_width as f32, monitor_height as f32, current);
        let fitted_height = target.map_or(current.height, |size| size.height);
        if let Some(size) = target {
            debug!(
                monitor_width,
                monitor_height,
                width = size.width,
                height = size.height,
                "Fitting the main window to its monitor"
            );
            ui.window().set_size(size);
        }
        apply_small_screen_layout_default(&ui, editor_layout_touched, fitted_height);
    });
}

/// Applies the Edit sub-tab's small-screen layout default -- inspector collapsed,
/// dock narrowed to [`SMALL_SCREEN_DOCK_WIDTH`] -- when `fitted_height` is below
/// [`SMALL_SCREEN_HEIGHT_THRESHOLD`] and `editor_layout_touched` is `false`. A no-op
/// once the user has ever dragged a split handle or toggled a section themselves
/// (`editor_layout_touched`), so this never fights a layout the user chose.
fn apply_small_screen_layout_default(
    ui: &MainWindow,
    editor_layout_touched: bool,
    fitted_height: f32,
) {
    if editor_layout_touched || fitted_height >= SMALL_SCREEN_HEIGHT_THRESHOLD {
        return;
    }
    let editor = ui.global::<EditorModel>();
    editor.set_inspector_collapsed(true);
    editor.set_dock_width(SMALL_SCREEN_DOCK_WIDTH);
}

/// Picks the initial logical size: the .slint preferred size capped at a fraction of
/// the monitor, and never larger than what the window currently has (so an OS that
/// already clamped it is respected). `None` when no change is needed.
fn fitted_size(
    monitor_width: f32,
    monitor_height: f32,
    current: slint::LogicalSize,
) -> Option<slint::LogicalSize> {
    if monitor_width <= 0.0 || monitor_height <= 0.0 {
        return None;
    }
    let width = PREFERRED_WIDTH
        .min(monitor_width * MAX_WIDTH_FRACTION)
        .min(current.width.max(1.0));
    let height = PREFERRED_HEIGHT
        .min(monitor_height * MAX_HEIGHT_FRACTION)
        .min(current.height.max(1.0));
    let changed = (width - current.width).abs() > 0.5 || (height - current.height).abs() > 0.5;
    changed.then(|| slint::LogicalSize::new(width.floor(), height.floor()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_a_window_that_already_fits_alone() {
        let current = slint::LogicalSize::new(1600.0, 900.0);
        assert!(fitted_size(2560.0, 1440.0, current).is_none());
    }

    #[test]
    fn shrinks_to_a_scaled_full_hd_monitor() {
        let current = slint::LogicalSize::new(1600.0, 900.0);
        let size = fitted_size(1536.0, 864.0, current).expect("must shrink");
        assert_eq!(size.width, (1536.0f32 * MAX_WIDTH_FRACTION).floor());
        assert_eq!(size.height, (864.0f32 * MAX_HEIGHT_FRACTION).floor());
    }

    #[test]
    fn never_grows_a_window_the_os_already_clamped() {
        let current = slint::LogicalSize::new(1400.0, 800.0);
        assert!(fitted_size(3840.0, 2160.0, current).is_none());
    }

    #[test]
    fn ignores_a_degenerate_monitor() {
        let current = slint::LogicalSize::new(1600.0, 900.0);
        assert!(fitted_size(0.0, 0.0, current).is_none());
    }
}
