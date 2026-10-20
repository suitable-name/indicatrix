/// "Tilt average": starts the tilt run of `surface`'s live session on a worker thread.
/// Does nothing unless both sides were measured and no run is going or finished.
pub(super) fn start_tilt(surface: Surface) {
    let started = with_live_on(surface, |live| {
        let id = live.id;
        let ready = live.ready.as_mut()?;
        let request = ready.metrics.begin_tilt(&ready.session)?;
        Some((id, request, ready.metrics.strip()))
    });
    let Some((handle, Some((id, request, strip)))) = started else {
        return;
    };
    let run = request.run;
    handle.with_model(|model| push_strip(&model, &strip));
    if let Err(reason) = metrics::spawn_tilt(id, request, post_tilt) {
        warn!("{reason}");
        let failed = with_live(|live| {
            let ready = live.ready.as_mut().filter(|_| live.id == id)?;
            ready
                .metrics
                .tilt_failed_to_start(run, reason)
                .then(|| ready.metrics.strip())
        });
        if let Some((handle, Some(strip))) = failed {
            handle.with_model(|model| push_strip(&model, &strip));
        }
    }
}

/// "Cancel" on the running tilt average of `surface`'s live session: the thread stops and
/// its late reports are ignored.
pub(super) fn cancel_tilt(surface: Surface) {
    let cancelled = with_live_on(surface, |live| {
        let ready = live.ready.as_mut()?;
        ready.metrics.cancel_tilt().then(|| ready.metrics.strip())
    });
    if let Some((handle, Some(strip))) = cancelled {
        handle.with_model(|model| push_strip(&model, &strip));
    }
}

/// Hands a tilt thread's report to the UI thread -- the `post` every tilt thread is
/// spawned with. Fails silently only once the event loop has already quit.
fn post_tilt(report: TiltReport) {
    let _ = slint::invoke_from_event_loop(move || deliver_tilt(report));
}

/// What one accepted tilt report changes on screen.
enum TiltUpdate {
    /// The progress bar moved.
    Progress(f32),
    /// The run ended: the whole strip changes.
    Strip(Box<StripText>),
}

/// Shows a tilt report iff it belongs to the live session's running tilt run -- a
/// cancelled run's or a replaced session's late reports change nothing.
fn deliver_tilt(report: TiltReport) {
    let TiltReport {
        session_id,
        run,
        event,
    } = report;
    let update = with_live(|live| {
        if live.id != session_id {
            return None;
        }
        let ready = live.ready.as_mut()?;
        match event {
            TiltEvent::Progress { done } => ready
                .metrics
                .tilt_progress(run, done)
                .then(|| TiltUpdate::Progress(ready.metrics.progress_fraction())),
            TiltEvent::Finished(outcome) => ready
                .metrics
                .tilt_finished(run, &outcome)
                .then(|| TiltUpdate::Strip(Box::new(ready.metrics.strip()))),
        }
    });
    let Some((handle, Some(update))) = update else {
        return;
    };
    handle.with_model(|model| match update {
        TiltUpdate::Progress(fraction) => model.set_tilt_progress(fraction),
        TiltUpdate::Strip(strip) => push_strip(&model, &strip),
    });
}
