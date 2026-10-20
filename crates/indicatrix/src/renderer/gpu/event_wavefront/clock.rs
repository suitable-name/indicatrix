//! Time source and per-chunk deadline for the event wavefront renderer.
//!
//! `std::time::Instant::now()` panics on `wasm32-unknown-unknown`, so the wasm target
//! uses `web_time::Instant` (backed by `performance.now()`), which has the same API. The
//! chunk-time EMA and the ~30 ms chunk target therefore work in the browser exactly as
//! they do natively.
//!
//! The deadline half ([`race_deadline`]) exists for wasm32 only in effect: native
//! readback is already bounded by `compute::GPU_WAIT_TIMEOUT`'s poll loop, so there the
//! race degrades to a plain `await`.

use std::sync::atomic::{AtomicU64, Ordering};

use super::accumulate::CHUNK_TIMING_EMA_ALPHA;

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;
#[cfg(target_arch = "wasm32")]
pub use web_time::Instant;

/// Floor of the per-chunk deadline in milliseconds (plan 6.2: `max(2000 ms, 20 x EMA)`).
pub const MIN_CHUNK_DEADLINE_MS: f64 = 2000.0;

/// Multiple of the EMA-predicted chunk time that is still accepted as "slow but alive".
pub const CHUNK_DEADLINE_EMA_FACTOR: f64 = 20.0;

/// Deadline for a chunk dispatched before any EMA exists. The first chunk may include
/// driver-side pipeline compilation that the staged `GpuCompile` probe did not cover.
pub const FIRST_CHUNK_DEADLINE_MS: f64 = 10_000.0;

/// Deadline for one chunk in milliseconds: `max(2000, 20 x predicted chunk time)`, or
/// [`FIRST_CHUNK_DEADLINE_MS`] when no timing measurement exists yet.
#[must_use]
pub fn chunk_deadline_ms(ema_ns_per_tuple: Option<f64>, chunk_rays: usize) -> f64 {
    ema_ns_per_tuple
        .filter(|v| v.is_finite() && *v > 0.0)
        .map_or(FIRST_CHUNK_DEADLINE_MS, |ns| {
            (ns * chunk_rays as f64 * 1.0e-6 * CHUNK_DEADLINE_EMA_FACTOR).max(MIN_CHUNK_DEADLINE_MS)
        })
}

/// Folds a new per-tuple measurement into the exponential moving average.
#[must_use]
pub fn blend_ema(previous: Option<f64>, ns_per_tuple: f64) -> f64 {
    previous.map_or(ns_per_tuple, |ema| {
        // Wall-clock chunk sizing only: no result and no GPU twin depends on these bits.
        CHUNK_TIMING_EMA_ALPHA.mul_add(ns_per_tuple, (1.0 - CHUNK_TIMING_EMA_ALPHA) * ema)
    })
}

/// Readback deadline (milliseconds, `f64` bits) the next `readback::map_read` honours.
/// `accumulate` sets it before every chunk, because `chunk::execute_chunk` calls
/// `map_read` without a deadline parameter.
static READBACK_DEADLINE_MS_BITS: AtomicU64 = AtomicU64::new(FIRST_CHUNK_DEADLINE_MS.to_bits());

/// Sets the deadline the next readback honours.
pub fn set_readback_deadline_ms(ms: f64) {
    READBACK_DEADLINE_MS_BITS.store(ms.to_bits(), Ordering::Relaxed);
}

/// The deadline the next readback honours, in milliseconds.
#[must_use]
pub fn readback_deadline_ms() -> f64 {
    f64::from_bits(READBACK_DEADLINE_MS_BITS.load(Ordering::Relaxed))
}

/// Outcome of [`race_deadline`].
pub enum Raced<T> {
    /// The future finished first.
    Done(T),
    /// The deadline fired first; the future was dropped.
    TimedOut,
}

/// Native: readback is bounded by its own poll loop, so this is a plain `await`.
#[cfg(not(target_arch = "wasm32"))]
pub async fn race_deadline<F: std::future::Future>(fut: F, _deadline_ms: f64) -> Raced<F::Output> {
    Raced::Done(fut.await)
}

/// wasm32: races `fut` against a `setTimeout` timer of `deadline_ms`.
///
/// If no `setTimeout` exists in the global scope the future is simply awaited.
#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::future_not_send,
    reason = "wgpu web backend futures are !Send; wasm32 is single-threaded"
)]
pub async fn race_deadline<F: std::future::Future>(fut: F, deadline_ms: f64) -> Raced<F::Output> {
    use std::{future::Future, pin::Pin, task::Poll};

    let (tx, mut rx) = futures_channel::oneshot::channel::<()>();
    let Some(timer_id) = schedule_timeout(deadline_ms, tx) else {
        return Raced::Done(fut.await);
    };
    let mut fut = std::pin::pin!(fut);
    let out = std::future::poll_fn(|cx| {
        if let Poll::Ready(value) = fut.as_mut().poll(cx) {
            return Poll::Ready(Raced::Done(value));
        }
        if Pin::new(&mut rx).poll(cx).is_ready() {
            return Poll::Ready(Raced::TimedOut);
        }
        Poll::Pending
    })
    .await;
    clear_timeout(&timer_id);
    out
}

#[cfg(target_arch = "wasm32")]
fn schedule_timeout(
    deadline_ms: f64,
    tx: futures_channel::oneshot::Sender<()>,
) -> Option<wasm_bindgen::JsValue> {
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};

    let global = js_sys::global();
    let set_timeout = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    let callback = Closure::once_into_js(move || {
        let _ = tx.send(());
    });
    set_timeout
        .call2(&global, &callback, &JsValue::from_f64(deadline_ms.max(1.0)))
        .ok()
}

#[cfg(target_arch = "wasm32")]
fn clear_timeout(timer_id: &wasm_bindgen::JsValue) {
    use wasm_bindgen::{JsCast, JsValue};

    let global = js_sys::global();
    if let Some(clear) = js_sys::Reflect::get(&global, &JsValue::from_str("clearTimeout"))
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
    {
        let _ = clear.call1(&global, timer_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_floors_at_two_seconds_and_scales_with_ema() {
        assert!((chunk_deadline_ms(None, 1000) - FIRST_CHUNK_DEADLINE_MS).abs() < 1e-9);
        // 100 ns/tuple x 1000 rays = 0.1 ms predicted; 20x = 2 ms -> floor.
        assert!((chunk_deadline_ms(Some(100.0), 1000) - MIN_CHUNK_DEADLINE_MS).abs() < 1e-9);
        // 1000 ns x 1e6 rays = 1000 ms predicted; 20x = 20000 ms.
        let big = chunk_deadline_ms(Some(1000.0), 1_000_000);
        assert!((big - 20_000.0).abs() < 1e-6, "got {big}");
        assert!((chunk_deadline_ms(Some(f64::NAN), 10) - FIRST_CHUNK_DEADLINE_MS).abs() < 1e-9);
        assert!((chunk_deadline_ms(Some(-1.0), 10) - FIRST_CHUNK_DEADLINE_MS).abs() < 1e-9);
    }

    #[test]
    fn ema_blend_seeds_then_smooths() {
        assert!((blend_ema(None, 50.0) - 50.0).abs() < 1e-12);
        let next = blend_ema(Some(100.0), 200.0);
        let expected =
            CHUNK_TIMING_EMA_ALPHA.mul_add(200.0, (1.0 - CHUNK_TIMING_EMA_ALPHA) * 100.0);
        assert!((next - expected).abs() < 1e-12);
    }

    #[test]
    fn readback_deadline_round_trips() {
        let before = readback_deadline_ms();
        set_readback_deadline_ms(1234.5);
        assert!((readback_deadline_ms() - 1234.5).abs() < 1e-12);
        set_readback_deadline_ms(before);
    }
}
