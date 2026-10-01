//! The live view's HDR map: uploading it to the Workers or dropping it as the settings
//! ask, and telling the user what it cost.
//!
//! The map goes to the render Workers and to the analysis Worker (the metrics HUD is
//! scored under the map the viewport is lit with), through one call,
//! `indicatrix_web_core::host::WorkerPool::set_hdr`, which counts every Worker's decoded
//! copy against the memory budget.

use super::LIVE;
use crate::app::{
    Ctx,
    push::{MessageKind, show_message},
};
use indicatrix_web_core::host::WorkerPool;

/// The HDR upload the pool holds, by identity (the buffer's address and length: an
/// upload is replaced, never edited in place).
type HdrIdentity = (usize, usize);

/// What the live view has done with the uploaded map.
#[derive(Default)]
pub(super) struct HdrState {
    /// The upload the Workers hold.
    sent: Option<HdrIdentity>,
    /// The upload the Workers refused (it is not offered again).
    refused: Option<HdrIdentity>,
    /// The id the newest upload was given (`SceneSpec::hdr_id`, `MetricsParams::hdr_id`).
    id: u64,
}

/// Whether the Workers hold the uploaded map and the live scene is lit by it.
pub(super) fn in_use() -> bool {
    LIVE.with(|live| live.borrow().hdr.sent.is_some())
}

/// The HDR map id the live scene uses, uploading or dropping the map as the state
/// asks. Upload results are reported as messages.
pub(super) fn sync_hdr(ctx: &Ctx, pool: &WorkerPool) -> Option<u64> {
    let wanted: Option<HdrIdentity> = {
        let app = ctx.state.borrow();
        app.hdr
            .as_ref()
            .filter(|_| app.settings.use_hdr)
            .map(|h| (h.bytes.as_ptr() as usize, h.bytes.len()))
    };
    let (sent, refused, id) = LIVE.with(|live| {
        let live = live.borrow();
        (live.hdr.sent, live.hdr.refused, live.hdr.id)
    });
    let Some(identity) = wanted else {
        if sent.is_some() {
            LIVE.with(|live| live.borrow_mut().hdr.sent = None);
            if let Err(error) = pool.clear_hdr() {
                show_message(ctx, MessageKind::Error, &format!("Render workers: {error}"));
            }
        }
        return None;
    };
    if sent == Some(identity) {
        return Some(id);
    }
    if refused == Some(identity) {
        return None;
    }
    upload_hdr(ctx, pool, identity, id + 1)
}

/// Sends the uploaded map to the Workers under `id`; the new id when they took it.
fn upload_hdr(ctx: &Ctx, pool: &WorkerPool, identity: HdrIdentity, id: u64) -> Option<u64> {
    let upload = {
        let app = ctx.state.borrow();
        app.hdr.as_ref().map(|h| (h.name.clone(), h.bytes.clone()))
    };
    let (name, bytes) = upload?;
    let before = pool.render().worker_count();
    LIVE.with(|live| live.borrow_mut().hdr.id = id);
    match pool.set_hdr(id, bytes) {
        Ok(admission) => {
            LIVE.with(|live| {
                let mut live = live.borrow_mut();
                live.hdr.sent = Some(identity);
                live.hdr.refused = None;
            });
            let notice = admission.notice(&name, before);
            let kind = if notice.is_warning {
                MessageKind::Warning
            } else {
                MessageKind::Success
            };
            show_message(ctx, kind, &notice.text);
            Some(id)
        }
        Err(refusal) => {
            LIVE.with(|live| live.borrow_mut().hdr.refused = Some(identity));
            show_message(
                ctx,
                MessageKind::Error,
                &format!("{name} cannot light the render: {refusal}"),
            );
            None
        }
    }
}
