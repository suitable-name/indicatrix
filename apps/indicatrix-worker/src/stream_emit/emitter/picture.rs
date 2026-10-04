//! The 8-bit picture events (protocol v14, "final picture only"): `DISPLAY_FRAME` for a
//! `TransferMode::DisplayOnly` request and `FINAL_IMAGE` for a `FinalImageRequest`.
//!
//! A `FINAL_IMAGE` tone-maps the float sum with the GUI export's own code in `indicatrix`
//! ([`tonemap_accumulation`], the export's one final 8-bit conversion), so the PNG is
//! exactly what the viewer would have written from the same sum.
//!
//! A `DISPLAY_FRAME` is the viewer's live-view picture of the merged sum: averaged,
//! À-Trous DENOISED with guides from the coordinator's own primary-ray prepass, and
//! tone-mapped (`indicatrix::renderer::frame_denoise`, the GUI's own function) -- made
//! off the emitter thread by `super::display` and written here by
//! [`write_display_rgba`]. [`write_display_frame`] (the plain tone-mapped running
//! average, the GUI's `denoise_enabled == false` picture) is only the fallback for a
//! denoise thread that could not run.

use glam::Vec3;
use indicatrix::{color::ColorSpace, renderer::tonemap::tonemap_accumulation};
use indicatrix_net::messages::{
    DisplayEncoding, DisplayFrameHeader, FinalImageHeader, NetError, StreamEvent,
    adaptive::PeerLink,
};
use std::io::Write;

/// Writes one `DISPLAY_FRAME`: `sum` (`samples_done` samples) tone-mapped for sRGB (the
/// live view's color space) WITHOUT denoising -- the fallback when the display denoiser
/// is unavailable. Nothing is written for an empty sum.
pub(super) fn write_display_frame<S: Write>(
    stream: &mut S,
    request_id: u32,
    (width, height): (u32, u32),
    samples_done: u32,
    sum: &[Vec3],
    link: &PeerLink,
) -> Result<bool, NetError> {
    if samples_done == 0 || sum.len() != width as usize * height as usize {
        return Ok(false);
    }
    let rgba = tonemap_accumulation(width, height, samples_done, sum, ColorSpace::Srgb);
    write_display_rgba(
        stream,
        request_id,
        (width, height),
        samples_done,
        &rgba,
        link,
    )
}

/// Writes one `DISPLAY_FRAME` carrying `rgba` (a finished `width x height` picture of
/// `samples_done` samples), encoded by `link` for the connection's measured speed (PNG or
/// raw RGBA8). `Ok(false)` (logged, nothing written) if the encoder refuses it.
pub(super) fn write_display_rgba<S: Write>(
    stream: &mut S,
    request_id: u32,
    (width, height): (u32, u32),
    samples_done: u32,
    rgba: &[u8],
    link: &PeerLink,
) -> Result<bool, NetError> {
    link.send_display((width, height), rgba, false, |encoding, payload| {
        let header = DisplayFrameHeader {
            request_id,
            samples_done,
            width,
            height,
            encoding,
            payload_len: payload.len() as u32,
        };
        indicatrix_net::messages::write_stream_event(
            stream,
            &StreamEvent::DisplayFrame(header),
            Some(payload),
        )
    })
}

/// Writes the one `FINAL_IMAGE` of a `FinalImageRequest`: `sum` (`samples` samples)
/// through [`tonemap_accumulation`] for `color_space`, as a PNG whatever the link (the
/// export's product); its write is timed like any other frame's.
///
/// # Errors
///
/// [`NetError`] for a transport failure; an encoder refusal is returned as
/// `Ok(Err(message))` for the caller to report as a stream error instead of `DONE`.
pub(super) fn write_final_image<S: Write>(
    stream: &mut S,
    request_id: u32,
    (width, height): (u32, u32),
    samples: u32,
    sum: &[Vec3],
    color_space: ColorSpace,
    link: &PeerLink,
) -> Result<Result<(), String>, NetError> {
    let refused = || {
        Err(format!(
            "could not encode the finished {width}x{height} picture"
        ))
    };
    if samples == 0 || sum.len() != width as usize * height as usize {
        return Ok(refused());
    }
    let rgba = tonemap_accumulation(width, height, samples, sum, color_space);
    let written = link.send_display(
        (width, height),
        &rgba,
        true,
        |encoding: DisplayEncoding, payload| {
            let header = FinalImageHeader {
                request_id,
                width,
                height,
                samples_done: samples,
                encoding,
                payload_len: payload.len() as u32,
            };
            indicatrix_net::messages::write_stream_event(
                stream,
                &StreamEvent::FinalImage(header),
                Some(payload),
            )
        },
    )?;
    Ok(if written { Ok(()) } else { refused() })
}
