//! The render pool's silence watchdog: a repeating browser timer the pool uses to find a
//! render Worker that took a chunk and went quiet (a hang, or a crash that raised no
//! `error` event).

use wasm_bindgen::{JsCast, closure::Closure};

/// How often the watchdog looks, in milliseconds.
const CHECK_INTERVAL_MS: i32 = 5_000;

/// Silence from a Worker holding a chunk that counts as a hang, however small the
/// chunk: 30 s.
pub(super) const SILENCE_LIMIT_MS: f64 = 30_000.0;

/// The longest a pixel-sample may plausibly take, in milliseconds.
///
/// A big chunk (one sample of a 4096 px export frame over a few Workers) legitimately runs
/// for minutes on a slow machine, so the silence limit grows with the chunk.
const MS_PER_PIXEL_SAMPLE: f64 = 0.25;

/// How long a Worker may stay silent on a chunk of `pixels` pixels at `spp` samples per
/// pixel before it counts as hung: [`SILENCE_LIMIT_MS`], or more for a chunk big enough
/// to need it.
pub(super) fn silence_limit_ms(pixels: u32, spp: u32) -> f64 {
    (f64::from(pixels) * f64::from(spp) * MS_PER_PIXEL_SAMPLE).max(SILENCE_LIMIT_MS)
}

/// A repeating browser timer, cleared when dropped.
pub(super) struct Watchdog {
    id: i32,
    _tick: Closure<dyn FnMut()>,
}

impl Watchdog {
    /// Calls `tick` every few seconds until the returned value is dropped; `None` when
    /// the page has no `window` to set a timer on.
    pub(super) fn start(tick: impl FnMut() + 'static) -> Option<Self> {
        let closure = Closure::<dyn FnMut()>::new(tick);
        let id = web_sys::window()?
            .set_interval_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                CHECK_INTERVAL_MS,
            )
            .ok()?;
        Some(Self { id, _tick: closure })
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            window.clear_interval_with_handle(self.id);
        }
    }
}
