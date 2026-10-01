//! [`CancelUrl`]: the revocable `blob:` URL a Worker polls to learn that the page gave up
//! on the job it is computing.
//!
//! A Worker cannot read a message while it computes, but it can make a synchronous
//! request, and a request for a revoked `blob:` URL fails. The page hands the URL over
//! with `ToWorker::WatchCancel` and revokes it (by dropping the [`CancelUrl`]) to cancel.

/// A live `blob:` URL, revoked when dropped.
pub(super) struct CancelUrl(String);

impl CancelUrl {
    /// A fresh URL, or `None` when the browser will not make one (the job then cannot be
    /// cancelled gracefully).
    pub(super) fn create() -> Option<Self> {
        let blob = web_sys::Blob::new().ok()?;
        web_sys::Url::create_object_url_with_blob(&blob)
            .ok()
            .map(Self)
    }

    /// The URL to hand to the Worker.
    pub(super) fn url(&self) -> &str {
        &self.0
    }
}

impl Drop for CancelUrl {
    fn drop(&mut self) {
        let _ = web_sys::Url::revoke_object_url(&self.0);
    }
}
