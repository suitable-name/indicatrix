//! Clear the catalogue's optional startup offers before recording a lesson.
//!
//! Wait for the real asynchronous scan, then click the dialog's own Not Now/Skip or
//! Close button once. No model setters or callback invocations: the pointer release is
//! the only activation, and the dialog must actually disappear. This is library setup,
//! before the lesson recording (which must not expose catalogue-specific startup text).

use crate::{
    pacing,
    support::{WindowView, app::pump},
    targets::Spot,
};
use indicatrix_cut::{BatchModel, MainWindow, harness_support};
use slint::{
    ComponentHandle, LogicalPosition,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;
use uiprobe::AccessibleRole;

/// Declines optional offers, including a tilt offer opened by closing a preview dialog.
pub fn dismiss_batch_offers(ui: &MainWindow) -> Result<(), String> {
    if !pacing::wait_real(pacing::LIBRARY_LOAD, || {
        !harness_support::startup_preview_scan_pending()
    }) {
        return Err("startup preview scan did not deliver its result before recording".into());
    }

    let batch = ui.global::<BatchModel>();
    for _ in 0..4 {
        pump(50);
        // Both have z=400; TiltBatchDialog is declared later in GlobalDialogs.
        let tilt = batch.get_tilt_visible();
        let (component, confirming, running, done, label) = if tilt {
            (
                "TiltBatchDialog",
                batch.get_tilt_confirming(),
                batch.get_tilt_batch_running(),
                batch.get_tilt_done(),
                "Not Now",
            )
        } else if batch.get_preview_visible() {
            (
                "PreviewBatchDialog",
                batch.get_preview_confirming(),
                batch.get_preview_batch_running(),
                batch.get_preview_done(),
                if batch.get_preview_offer_from_import() {
                    "Skip"
                } else {
                    "Not Now"
                },
            )
        } else {
            if batch.get_preview_batch_running() || batch.get_tilt_batch_running() {
                return Err("a catalogue batch is running during tutorial setup".into());
            }
            return Ok(());
        };
        if running || (!confirming && !done) {
            return Err(format!(
                "startup {component} is not dismissible: confirming={confirming}, running={running}, done={done}"
            ));
        }
        let label = if confirming { label } else { "Close" };
        let view = WindowView::of("MainWindow", ui);
        let dialog = view
            .root
            .query_descendants()
            .match_inherits(component)
            .find_first()
            .ok_or_else(|| format!("startup {component} is missing"))?;
        let buttons: Vec<_> = dialog
            .query_descendants()
            .match_accessible_role(AccessibleRole::Button)
            .find_all()
            .into_iter()
            .filter(|element| element.accessible_label().is_some_and(|name| name == label))
            .filter(|element| element.accessible_enabled() != Some(false))
            .map(|element| Spot::of(&element))
            .filter(|spot| spot.usable(view.size))
            .collect();
        if buttons.len() != 1 {
            return Err(format!(
                "startup {component}: expected one visible enabled {label:?} button, found {}",
                buttons.len()
            ));
        }
        let (x, y) = buttons[0].center();
        let position = LogicalPosition::new(x, y);
        ui.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        ui.window().dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        ui.window().dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        if !pacing::wait_real(Duration::from_secs(2), || {
            if tilt {
                !batch.get_tilt_visible()
            } else {
                !batch.get_preview_visible()
            }
        }) {
            return Err(format!(
                "startup {component}: clicking {label:?} did not close the dialog"
            ));
        }
    }
    Err("startup batch dialogs kept reopening after four dismissals".into())
}
