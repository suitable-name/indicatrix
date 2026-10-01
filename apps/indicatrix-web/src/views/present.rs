//! Copies a finished frame into `SolidModel`: the image (the pipeline's RGBA bytes
//! into a Slint pixel buffer, the web's `to_pixel_buffer`), the status line, and
//! the pick buffers/tables hover and click read afterwards.

use super::state::ViewsState;
use crate::{
    AppWindow, SolidModel,
    app::state::{SolveState, WebApp},
};
use indicatrix_solid::preview::RenderedFrame;
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};
use std::time::Duration;

/// An RGBA8 buffer of `width x height` pixels as a Slint image.
fn image_from_rgba(rgba: &[u8], width: u32, height: u32) -> Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let bytes = buffer.make_mut_bytes();
    if bytes.len() == rgba.len() {
        bytes.copy_from_slice(rgba);
    }
    Image::from_rgba8(buffer)
}

/// Keeps `frame`'s pick buffers and tables in `views` and shows its image and
/// status in the view `view_mode` names (0 Solid, 3 Diagram).
pub fn show_frame(ui: &AppWindow, views: &mut ViewsState, frame: RenderedFrame, view_mode: u8) {
    let model = ui.global::<SolidModel>();
    views.pick = Some(frame.pick);
    views.hover_text = frame.hover_text;
    views.facet_tier = frame.facet_tier;
    views.bounding_radius = frame.mesh_bounding_radius;
    views.geometry = frame.geometry;
    views.frame_generation = frame.generation;
    if view_mode == 3 {
        views.diagram = frame.diagram;
        match &views.diagram {
            Some(diagram) => {
                model.set_diagram_frame(image_from_rgba(
                    &diagram.color,
                    diagram.width,
                    diagram.height,
                ));
                model.set_has_diagram(true);
            }
            None => model.set_has_diagram(false),
        }
    } else {
        let (width, height) = views.pipeline.size();
        model.set_frame(image_from_rgba(views.pipeline.solid_rgba(), width, height));
        model.set_has_solid(frame.has_solid);
    }
    model.set_status(frame.status.into());
    model.set_stale(frame.stale);
}

/// The status while the views wait for a solve they cannot run here (the last
/// frame stays on screen): the solver's message if the solve failed, otherwise a
/// "solving" note.
pub fn show_waiting(ui: &AppWindow, app: &WebApp, generation: u64) {
    let text = match &app.solve {
        SolveState::Failed {
            generation: failed,
            message,
        } if *failed == generation => format!("Preview cannot be solved: {message}"),
        _ => "Solving the design for the preview...".to_string(),
    };
    let model = ui.global::<SolidModel>();
    model.set_status(text.into());
    model.set_stale(false);
}

/// The "12.3 ms" readout: the last update's plan + raster time on the main thread.
pub fn show_timing(ui: &AppWindow, took: Duration) {
    ui.global::<SolidModel>()
        .set_timing_text(format!("{:.1} ms", took.as_secs_f64() * 1000.0).into());
}
