//! Desktop trainer application for Indicatrix: live training metrics, charts and optical
//! gemstone diagnostics.
//!
//! The UI thread owns all Slint state. The training worker runs on its own thread (see
//! `trainer_worker.rs`) and talks to the UI through channels; the controller in `gui/` drains its
//! events on a 33 ms timer.

mod dataset;
mod gui;
mod stats;
mod trainer_worker;
mod ui_state;

use slint::ComponentHandle;

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let window = AppWindow::new()?;
    let gui = gui::Gui::install(&window);
    // The timer must outlive the event loop.
    let _timer = gui.start_timer();
    window.run()?;
    Ok(())
}
