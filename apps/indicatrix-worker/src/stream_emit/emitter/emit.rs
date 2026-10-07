//! The emission logic itself: building and writing `FRAME`/`PREVIEW`/`PROGRESS`
//! [`StreamEvent`]s from an [`EmitterAccum`]'s state. [`emit_tick`] is one cadence-tick
//! emission, [`emit_final`] is the last emission once tracing finishes, and the
//! remaining functions are the heartbeat/backstop machinery [`super::run_stream_loop`]
//! and [`super::wait_for_tracer_to_stop`] drive between real ticks.

use super::{
    super::{
        Output, StreamOutcome, StreamSpec, downsample::downsample_preview, tracer::SharedState,
    },
    accum::EmitterAccum,
    display::{DisplayDenoiser, DisplayUpdate},
};
use glam::Vec3;
use indicatrix_net::{
    messages::{
        Done, ErrorMsg, FrameHeader, NetError, PreviewConfig, PreviewHeader, Progress,
        RenderRequest, Stats, StreamEvent, TransferMode, error_codes,
    },
    radiance::EncodedPayload,
};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Computes [`Stats::effective_cadence_ms`]: the average wall-clock interval between
/// emissions over `elapsed`, or `0` if fewer than two emissions happened (nothing to
/// average -- see that field's doc comment).
pub(in crate::stream_emit) fn effective_cadence_ms(elapsed: Duration, emission_count: u32) -> u32 {
    if emission_count < 2 {
        0
    } else {
        (elapsed.as_millis() / u128::from(emission_count - 1)) as u32
    }
}

/// One cadence-tick opportunity in `run_stream`'s main loop: the full [`emit_tick`]
/// once `last_emit.elapsed() >= cadence`, otherwise the bare
/// [`maybe_emit_heartbeat_backstop`] heartbeat. Pulled out of `run_stream` to keep that
/// function under clippy's line-count limit.
pub(super) fn emit_tick_or_heartbeat<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    emitter_accum: &mut EmitterAccum,
    cadence: Duration,
    last_emit: &mut Instant,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    if last_emit.elapsed() >= cadence {
        emit_tick(stream, request, state, emitter_accum, emission_count)?;
        *last_emit = Instant::now();
        return Ok(());
    }
    maybe_emit_heartbeat_backstop(
        stream,
        request,
        state,
        super::super::HEARTBEAT_INTERVAL,
        last_emit,
        emission_count,
    )
}

/// The `HEARTBEAT_INTERVAL` backstop inside `run_stream`'s own cadence-paced loop:
/// writes a bare `Progress` heartbeat and resets `*last_emit` whenever
/// `last_emit.elapsed() >= heartbeat_interval`. Called only when a cadence-due tick
/// isn't already covering this instant, so it never fires for a `cadence` at or under
/// [`super::super::HEARTBEAT_INTERVAL`] and exists purely to cap the gap for a wider one.
fn maybe_emit_heartbeat_backstop<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    heartbeat_interval: Duration,
    last_emit: &mut Instant,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    if last_emit.elapsed() < heartbeat_interval {
        return Ok(());
    }
    emit_progress_heartbeat(stream, request, state, emission_count)?;
    *last_emit = Instant::now();
    Ok(())
}

/// Writes a bare `StreamEvent::Progress` -- no `FRAME`/`PREVIEW` -- for
/// [`super::wait_for_tracer_to_stop`]'s heartbeat; see that function's and this module's
/// own doc comments for why.
pub(super) fn emit_progress_heartbeat<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    let samples_done = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .samples_done;
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Progress(Progress {
            request_id: request.request_id,
            samples_done,
        }),
        None,
    )?;
    *emission_count += 1;
    Ok(())
}

/// One periodic (cadence-elapsed) emission: `FRAME` (if [`TransferMode::LiveProgressive`]
/// and there's a pending delta), `PREVIEW` (if configured, at least one sample has
/// landed, and it isn't redundant with a `FRAME` this same tick), then `PROGRESS` --
/// always, even under [`TransferMode::FinalOnly`] and before the first sample.
///
/// # The only work done under `state`'s lock is the swap
///
/// `emitter.swap_and_fold(state)` (see [`EmitterAccum`]) is the only place this function
/// touches `state`: it locks just long enough to exchange `pending_delta`'s buffer and
/// read `samples_done`, releasing the lock before folding, encoding, downsampling, or
/// writing anything. This avoids cloning the full-resolution `running_total` while
/// `state` is locked -- a `memcpy` of the whole frame (about 25 MB at 1080p) that would
/// otherwise stand directly between `run_tracer` and the lock it needs after every
/// sub-batch.
///
/// The swap happens unconditionally regardless of [`TransferMode`], since
/// `running_total` must stay current for `PREVIEW` under every mode; whether the
/// swapped-out delta is also written as a `FRAME` is decided separately below.
pub(in crate::stream_emit) fn emit_tick<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    emitter: &mut EmitterAccum,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    let (range, samples_done) = emitter.swap_and_fold(state);

    // "Final picture only" live view: a denoised DISPLAY_FRAME of the whole request so
    // far (never a FRAME or PREVIEW), see `emit_display_tick`.
    if request.stream.transfer_mode == TransferMode::DisplayOnly {
        if emit_display_tick(stream, request, emitter, range.is_some(), samples_done)? {
            *emission_count += 1;
        }
        return write_progress(stream, request.request_id, samples_done, emission_count);
    }

    // Whether this tick actually wrote a `FRAME` -- consulted below before deciding
    // whether a full-scale `PREVIEW` would be redundant with it. Only
    // `LiveProgressive` ever sends the swapped-out delta as a `FRAME` here;
    // `FinalOnly` still needed the swap to keep `running_total` current, but
    // `emit_final` sends its one and only `FRAME` once tracing completes.
    let frame_sent_this_tick =
        range.is_some() && matches!(request.stream.transfer_mode, TransferMode::LiveProgressive);

    if let Some((first_sample, samples)) = range
        && frame_sent_this_tick
    {
        // Encoded for the connection's measured link; for `Raw` this is a reinterpreted
        // view over `emitter`'s own delta buffer, not a fresh per-tick `Vec<u8>`
        // allocation+copy (see `radiance::as_bytes`).
        emitter.send_delta(|encoded| {
            let header =
                FrameHeader::for_encoded(request.request_id, first_sample, samples, encoded);
            indicatrix_net::messages::write_stream_event(
                stream,
                &StreamEvent::Frame(header),
                Some(encoded.bytes),
            )
        })?;
        *emission_count += 1;
    }

    // Skip the PREVIEW when nothing has been folded in yet: without this check the
    // first tick would write a full-size PREVIEW of pure zeros (~99.5 MB at 4K) for a
    // client about to overwrite it. PROGRESS still goes out unconditionally below.
    //
    // Also skip it when `cfg` is exactly the frame's own full resolution and this tick
    // already sent a `FRAME` delta: a client's `Accumulator` rebuilds the identical
    // image from `FRAME` deltas already received, so a full-scale `PREVIEW` riding
    // alongside a `FRAME` would just double that tick's bandwidth. Only applies under
    // `LiveProgressive`; a downscaled `PREVIEW` is never skipped.
    //
    // And skip it when `samples_done` hasn't advanced by [`EmitterAccum::preview_due`]'s
    // 1%-of-budget threshold since the last one this emitter actually wrote: a
    // coordinator job's running total (and so `samples_done`) only changes when a chunk
    // merges, so calling this every cadence tick regardless would otherwise resend a
    // byte-identical `PREVIEW` between merges.
    if let Some(cfg) = request.stream.preview
        && samples_done > 0
    {
        let full_scale_and_redundant = frame_sent_this_tick
            && cfg.width == request.scene.width
            && cfg.height == request.scene.height;
        if !full_scale_and_redundant && emitter.preview_due(samples_done, request.samples) {
            write_preview(stream, request, cfg, emitter, samples_done)?;
            emitter.record_preview_sent(samples_done);
        }
    }

    write_progress(stream, request.request_id, samples_done, emission_count)
}

/// Writes one `PROGRESS` and counts it as an emission.
fn write_progress<S: Write>(
    stream: &mut S,
    request_id: u32,
    samples_done: u32,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Progress(Progress {
            request_id,
            samples_done,
        }),
        None,
    )?;
    *emission_count += 1;
    Ok(())
}

/// The final emission once the producer has finished (without cancellation, a panic or
/// a failure): the last payload, then `DONE { cancelled: false }`.
///
/// - [`Output::FinalImage`]: the one `FINAL_IMAGE` (see [`super::picture`]);
/// - [`TransferMode::FinalOnly`]: one `FRAME` of the whole request;
/// - [`TransferMode::LiveProgressive`]: whatever's left un-coalesced as a `FRAME`;
/// - [`TransferMode::DisplayOnly`]: the final, denoised `DISPLAY_FRAME`.
///
/// Under `FinalOnly`/`LiveProgressive` with a downscaled preview configured, a last
/// `PREVIEW` of the running total goes out first whenever the last one sent is stale.
///
/// The whole-request sum is the producer's own `final_total` when it left one (a
/// coordinator's deterministic merge), else the emitter's running total. Swaps once more
/// before reading `emitter`'s state: the producer's last chunk(s) may have landed after
/// the last cadence tick's swap but before `finished` was noticed.
///
/// Returns [`StreamOutcome::Failed`] (nothing written, no `DONE`) if the final picture
/// could not be encoded.
pub(super) fn emit_final<S: Write>(
    stream: &mut S,
    spec: &StreamSpec<'_>,
    state: &Arc<Mutex<SharedState>>,
    emitter: &mut EmitterAccum,
    streaming_start: Instant,
    emission_count: &mut u32,
) -> Result<StreamOutcome, NetError> {
    let request = spec.request;
    let (range, samples_done) = emitter.swap_and_fold(state);
    let (final_total, reclaimed_samples) = {
        let mut guard = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (guard.final_total.take(), guard.reclaimed_samples)
    };
    let dims = (request.scene.width, request.scene.height);

    if let Output::FinalImage(color_space) = spec.output {
        let total = final_total
            .as_deref()
            .unwrap_or_else(|| emitter.running_total());
        let written = super::picture::write_final_image(
            stream,
            request.request_id,
            dims,
            request.samples,
            total,
            color_space,
            emitter.link(),
        )?;
        if let Err(message) = written {
            return Ok(StreamOutcome::Failed(ErrorMsg {
                code: error_codes::TRACE_PANIC,
                message,
                // Left unstamped here; `request.request_id` is available above if a
                // future change wants to stamp it (the forwarding in
                // `stream_emit/emitter/mod.rs` already does for `StreamOutcome::Failed`).
                request_id: None,
            }));
        }
        *emission_count += 1;
        write_done(
            stream,
            request,
            streaming_start,
            *emission_count,
            reclaimed_samples,
        )?;
        return Ok(StreamOutcome::Completed);
    }

    // A downscaled `PREVIEW` of the finished state, so a client watching previews never
    // ends on a stale one: the last chunk(s) often land after the last cadence tick (a
    // short request can finish before any tick sees a sample at all). A full-scale
    // preview is skipped, as on a tick: the final `FRAME` already carries that picture.
    if request.stream.transfer_mode != TransferMode::DisplayOnly
        && let Some(cfg) = request.stream.preview
        && samples_done > 0
        && !(cfg.width == request.scene.width && cfg.height == request.scene.height)
        && emitter.preview_due(samples_done, request.samples)
    {
        write_preview(stream, request, cfg, emitter, samples_done)?;
        emitter.record_preview_sent(samples_done);
    }

    emit_final_payload(
        stream,
        request,
        state,
        emitter,
        range,
        final_total.as_deref(),
        emission_count,
    )?;

    write_done(
        stream,
        request,
        streaming_start,
        *emission_count,
        reclaimed_samples,
    )?;
    Ok(StreamOutcome::Completed)
}

/// A `DisplayOnly` cadence tick's picture (see `super::display`): a denoised picture
/// that finished since the last tick, if any, while the running total is handed to the
/// denoise thread only when no denoise is in flight -- so frames never outpace their
/// own denoising, and this emitter thread never denoises during a tick. Falls back to the
/// plain tone-mapped running average only if the denoise thread is unavailable.
/// Whether a `DISPLAY_FRAME` was written.
fn emit_display_tick<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    emitter: &mut EmitterAccum,
    fresh: bool,
    samples_done: u32,
) -> Result<bool, NetError> {
    let dims = (request.scene.width, request.scene.height);
    match emitter.display_tick(&request.scene, fresh, samples_done) {
        DisplayUpdate::Picture(picture) => super::picture::write_display_rgba(
            stream,
            request.request_id,
            dims,
            picture.samples,
            &picture.rgba,
            emitter.link(),
        ),
        DisplayUpdate::Plain => super::picture::write_display_frame(
            stream,
            request.request_id,
            dims,
            samples_done,
            emitter.running_total(),
            emitter.link(),
        ),
        DisplayUpdate::Nothing => Ok(false),
    }
}

/// The final `DISPLAY_FRAME`: the whole request's sum (`final_total`, the job's
/// deterministic merge, else the running total) denoised by this request's display
/// denoiser -- waited for, with a `PROGRESS` heartbeat every
/// [`super::super::HEARTBEAT_INTERVAL`] so a slow 4K denoise never looks like a dead
/// server. Falls back to the plain tone-map if the denoise thread is unavailable.
fn emit_final_payload<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    emitter: &mut EmitterAccum,
    range: Option<(u32, u32)>,
    final_total: Option<&[Vec3]>,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    match request.stream.transfer_mode {
        TransferMode::FinalOnly => {
            let write = |encoded: &EncodedPayload<'_>| {
                let header = FrameHeader::for_encoded(
                    request.request_id,
                    request.first_sample,
                    request.samples,
                    encoded,
                );
                indicatrix_net::messages::write_stream_event(
                    stream,
                    &StreamEvent::Frame(header),
                    Some(encoded.bytes),
                )
            };
            match final_total {
                Some(total) => emitter.send_other(total, write)?,
                None => emitter.send_running_total(write)?,
            }
            *emission_count += 1;
        }
        TransferMode::LiveProgressive => {
            if let Some((first_sample, samples)) = range {
                emitter.send_delta(|encoded| {
                    let header = FrameHeader::for_encoded(
                        request.request_id,
                        first_sample,
                        samples,
                        encoded,
                    );
                    indicatrix_net::messages::write_stream_event(
                        stream,
                        &StreamEvent::Frame(header),
                        Some(encoded.bytes),
                    )
                })?;
                *emission_count += 1;
            }
        }
        TransferMode::DisplayOnly => {
            emit_final_display(stream, request, state, emitter, final_total, emission_count)?;
        }
    }
    Ok(())
}

fn emit_final_display<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    state: &Arc<Mutex<SharedState>>,
    emitter: &mut EmitterAccum,
    final_total: Option<&[Vec3]>,
    emission_count: &mut u32,
) -> Result<(), NetError> {
    let mut denoiser = emitter
        .take_display()
        .unwrap_or_else(|| DisplayDenoiser::spawn(&request.scene));
    let dims = (request.scene.width, request.scene.height);
    let total = final_total.unwrap_or_else(|| emitter.running_total());
    let denoised = denoiser.finish(
        request.samples,
        total,
        super::super::HEARTBEAT_INTERVAL,
        || emit_progress_heartbeat(stream, request, state, emission_count),
    )?;
    let written = match denoised {
        Some(rgba) => super::picture::write_display_rgba(
            stream,
            request.request_id,
            dims,
            request.samples,
            &rgba,
            emitter.link(),
        )?,
        None => super::picture::write_display_frame(
            stream,
            request.request_id,
            dims,
            request.samples,
            total,
            emitter.link(),
        )?,
    };
    *emission_count += u32::from(written);
    Ok(())
}

/// Writes the completed request's `DONE { cancelled: false }`.
fn write_done<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    streaming_start: Instant,
    emission_count: u32,
    reclaimed_samples: u32,
) -> Result<(), NetError> {
    let stats = Stats {
        samples_done: request.samples,
        requested_cadence_ms: request.stream.cadence_ms,
        effective_cadence_ms: effective_cadence_ms(streaming_start.elapsed(), emission_count),
        reclaimed_samples,
    };
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Done(Done {
            request_id: request.request_id,
            cancelled: false,
            stats,
        }),
        None,
    )
}

/// Downsamples `emitter`'s running total to `cfg` and writes it as a `PREVIEW`, encoded
/// with the connection's negotiated encoding like every `FRAME`.
fn write_preview<S: Write>(
    stream: &mut S,
    request: &RenderRequest,
    cfg: PreviewConfig,
    emitter: &EmitterAccum,
    samples_done: u32,
) -> Result<(), NetError> {
    let preview_buffer = downsample_preview(
        emitter.running_total(),
        request.scene.width,
        request.scene.height,
        cfg.width,
        cfg.height,
    );
    // `preview_buffer` above is already a fresh allocation (downsampling can't avoid
    // one); a `Raw` encoder hands its bytes to the writer without a second copy.
    emitter.send_other(&preview_buffer, |encoded| {
        let header = PreviewHeader::for_encoded(
            request.request_id,
            cfg.width,
            cfg.height,
            samples_done,
            encoded,
        );
        indicatrix_net::messages::write_stream_event(
            stream,
            &StreamEvent::Preview(header),
            Some(encoded.bytes),
        )
    })
}
