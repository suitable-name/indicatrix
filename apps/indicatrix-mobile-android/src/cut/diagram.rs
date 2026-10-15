//! The Cut page's diagram: the three-panel faceting diagram with the selected row's tier
//! highlighted, drawn on a worker thread and shown on Medium and larger windows.
//!
//! A new picture is asked for only when the design's generation or the highlighted tier
//! changed; a picture that a newer request has overtaken is dropped.

use std::cell::RefCell;

use indicatrix_mobile_core::{
    app_state::AppState,
    cutting::{self, DiagramJob},
    tasks::Generation,
};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};

use crate::{AppWindow, CutModel};

/// The picture's size in pixels: the diagram's three panels are 2.5 times as wide as tall.
const WIDTH: u32 = 1000;
const HEIGHT: u32 = 400;

/// What the page currently shows, or is waiting for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    generation: u64,
    tier: Option<usize>,
}

thread_local! {
    static REQUESTED: RefCell<Option<Key>> = const { RefCell::new(None) };
    static LATEST: Generation = Generation::default();
}

/// Makes the page's diagram match `state` with `tier` highlighted: nothing when the design
/// has no current solve, otherwise a picture drawn in the background (unless that exact
/// picture is already shown or on its way).
pub(super) fn push(window: &AppWindow, state: &AppState, tier: Option<usize>) {
    let key = cutting::plan(state).ok().map(|_| Key {
        generation: state.session.current_generation(),
        tier,
    });
    if REQUESTED.with(|requested| *requested.borrow() == key) {
        return;
    }
    REQUESTED.with(|requested| requested.borrow_mut().clone_from(&key));
    let latest = LATEST.with(Clone::clone);
    let token = latest.advance();
    let Some(key) = key else {
        window.global::<CutModel>().set_has_diagram(false);
        return;
    };
    let Ok(job) = cutting::diagram_job(state, key.tier, WIDTH, HEIGHT) else {
        return;
    };
    let weak = window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("cut-diagram".to_owned())
        .spawn(move || {
            let buffer = draw(&job);
            if !latest.is_current(token) {
                return;
            }
            let delivered = weak.upgrade_in_event_loop(move |window| {
                if !latest.is_current(token) {
                    return;
                }
                let model = window.global::<CutModel>();
                match buffer {
                    Some(buffer) => {
                        model.set_diagram(Image::from_rgba8(buffer));
                        model.set_has_diagram(true);
                    }
                    None => model.set_has_diagram(false),
                }
            });
            if let Err(error) = delivered {
                tracing::debug!(%error, "the cut diagram was dropped: the UI thread is gone");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the cut diagram could not be drawn: no thread");
    }
}

/// Draws the diagram into a pixel buffer; `None` when the design does not close to a solid.
fn draw(job: &DiagramJob) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let frame = job.render()?;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(frame.width, frame.height);
    let bytes = buffer.make_mut_bytes();
    if bytes.len() != frame.color.len() {
        return None;
    }
    bytes.copy_from_slice(&frame.color);
    Some(buffer)
}
