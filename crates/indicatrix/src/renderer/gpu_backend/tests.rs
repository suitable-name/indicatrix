//! [`Turnstile`] is plain `std::sync` -- no GPU, no adapter, no `--features gpu` device
//! needed -- so its fairness property is unit-testable directly, unlike everything else
//! in this module tree that actually dispatches. Gated on `feature = "gpu"` only because
//! `Turnstile` itself is (see that type's own `#[cfg]`), not because these tests need
//! hardware.
//!
//! The tests that do need an adapter skip with a printed note when none is present, unless
//! the environment variable `INDICATRIX_REQUIRE_GPU` is `1` -- then a missing adapter fails
//! them, so a CI machine that is supposed to have a GPU cannot silently stop testing it.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use glam::Vec3;

use super::{
    GpuAccumulate, GpuBackend, GpuBatchItem, GpuSceneRef,
    recovery::{COOL_DOWN, MAX_ATTEMPTS_PER_WINDOW, RecoveryPolicy, WINDOW},
    turnstile::Turnstile,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
    },
};

/// Whether `INDICATRIX_REQUIRE_GPU=1` demands that hardware tests find an adapter.
fn gpu_required() -> bool {
    std::env::var("INDICATRIX_REQUIRE_GPU").is_ok_and(|value| value == "1")
}

/// A backend with a real renderer, or `None` (the caller returns, skipping its test) on
/// a machine with no adapter.
///
/// # Panics
///
/// If no adapter could be acquired while `INDICATRIX_REQUIRE_GPU=1`.
pub(super) fn acquire_or_skip(test: &str) -> Option<GpuBackend> {
    let backend = GpuBackend::acquire();
    if backend.renderer.is_some() {
        return Some(backend);
    }
    assert!(
        !gpu_required(),
        "{test}: INDICATRIX_REQUIRE_GPU=1 but no GPU adapter could be acquired"
    );
    println!("skipping {test}: no GPU adapter");
    None
}

/// The fairness property [`Turnstile`] exists for: three threads holding tickets
/// `t0 < t1 < t2`, asked to wait for their turn in REVERSE ticket order (`t2` and
/// `t1` call `wait_for_turn` before `t0` does), must still be SERVED in ascending
/// ticket order -- exactly what lets `GpuBackend::try_accumulate_cancellable`'s
/// "take a turn, rejoin the back of the queue" loop alternate fairly among several
/// concurrent requests instead of a plain `Mutex`'s unspecified wakeup order
/// letting one thread cut back in ahead of others already waiting.
///
/// Deterministic without any `sleep`: `take_ticket` (called by the TEST, not the
/// worker threads) fixes the serving order up front, and a waiter for ticket `k`
/// can only proceed once `k` prior turns have each been explicitly dropped -- so the
/// recorded order reflects ticket order regardless of thread-scheduling timing,
/// including however early or late each thread actually calls `wait_for_turn`.
#[test]
fn turnstile_serves_tickets_in_order_even_when_threads_queue_out_of_order() {
    let turnstile = Turnstile::new();
    let served_order: Mutex<Vec<u64>> = Mutex::new(Vec::new());

    // Tickets are claimed here, on the main thread, in the exact order this test
    // wants served -- take_ticket's own doc comment: ordering is decided at the
    // moment a ticket is taken, not when wait_for_turn later blocks.
    let t0 = turnstile.take_ticket();
    let t1 = turnstile.take_ticket();
    let t2 = turnstile.take_ticket();

    thread::scope(|scope| {
        // Spawned in REVERSE ticket order -- t2's thread calls wait_for_turn before
        // t1's does -- to prove arrival order at the call site has no bearing on
        // service order.
        scope.spawn(|| {
            let _turn = turnstile.wait_for_turn(t2);
            served_order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(2);
        });
        scope.spawn(|| {
            let _turn = turnstile.wait_for_turn(t1);
            served_order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(1);
        });
        // The main thread holds ticket 0's turn last and briefly, on purpose: every
        // other waiter is provably still blocked on the condvar right up until this
        // drops, so releasing it is what starts the ascending cascade.
        {
            let _turn0 = turnstile.wait_for_turn(t0);
            served_order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(0);
        }
    });

    assert_eq!(
        *served_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![0, 1, 2],
        "tickets must be served in the order they were taken, not the order threads \
         happened to start waiting"
    );
}

/// A turn that is dropped without ever completing (mirrors a `?`-propagated GPU
/// error, or a cancellation, cutting `try_accumulate_cancellable`'s loop short
/// mid-turn) must still release the turnstile for the next waiter -- the RAII
/// `Drop` on `super::turnstile::TurnstileTurn` is what this test actually exercises,
/// not any explicit "release" call site.
#[test]
fn turnstile_releases_on_an_early_return_out_of_the_turn_scope() {
    fn takes_a_turn_then_bails_early(turnstile: &Turnstile, ticket: u64) -> Option<()> {
        let _turn = turnstile.wait_for_turn(ticket);
        None // early return -- `_turn` drops here, releasing the turnstile.
    }

    let turnstile = Turnstile::new();
    let t0 = turnstile.take_ticket();
    let t1 = turnstile.take_ticket();

    assert_eq!(takes_a_turn_then_bails_early(&turnstile, t0), None);

    // If the early return above had leaked the turn (never dropped it), this would
    // hang forever waiting for a `now_serving` bump that would never come --
    // `thread::scope` with no spawned threads just runs straight through, so a hang
    // here fails the test the same way any other infinite loop would.
    let _turn1 = turnstile.wait_for_turn(t1);
}

/// A poisoned `renderer` mutex (a previous turn's `wgpu` call panicked
/// while holding the guard, despite this module tree's own `on_uncaptured_error`/
/// `abandon_in_flight` defenses -- e.g. a bug in `wgpu` itself) must make
/// [`GpuBackend::try_accumulate_cancellable`] return [`GpuAccumulate::Declined`] and
/// set [`GpuBackend::lost`](super::backend::GpuBackend), not silently recover the
/// possibly-mid-panic renderer state via `PoisonError::into_inner` -- see that
/// function's own comment on the `mutex.lock()` match arm this test exercises.
///
/// Needs a real adapter (unlike every other test in this module): a poisoned
/// `std::sync::Mutex<GpuFrameRenderer>` can only exist around a REAL
/// `GpuFrameRenderer`, [`GpuBackend::disabled`] never builds one at all.
///
/// Reaches `GpuBackend::renderer`/`GpuBackend::lost` (private fields) directly, which
/// is why this test lives here rather than in `renderer::gpu::frame`'s own hardware
/// test module.
#[test]
fn poisoned_renderer_mutex_declines_and_sets_lost() {
    let Some(backend) = acquire_or_skip("poisoned_renderer_mutex_declines_and_sets_lost") else {
        return;
    };
    let mutex = backend
        .renderer
        .as_ref()
        .expect("acquire_or_skip only returns a backend with a renderer");

    // Lock the renderer mutex on a scoped thread that panics while still holding the
    // guard -- the standard way to poison a `std::sync::Mutex` from a test. `.join()`
    // (not `.unwrap()`) catches the panic itself, the same effect `catch_unwind`
    // would have, without needing `GpuFrameRenderer: UnwindSafe`.
    let join_result = thread::scope(|scope| {
        scope
            .spawn(|| {
                let _guard = mutex
                    .lock()
                    .expect("first lock of a fresh mutex cannot fail");
                panic!("deliberate poison for poisoned_renderer_mutex_declines_and_sets_lost");
            })
            .join()
    });
    assert!(
        join_result.is_err(),
        "the scoped thread was expected to panic (that's what poisons the mutex)"
    );
    assert!(mutex.is_poisoned(), "the mutex should now be poisoned");

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let scene = GpuSceneRef {
        camera: &camera,
        width: 4,
        height: 4,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 2,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let mut accum = vec![Vec3::ZERO; 16];
    let never_cancel = AtomicBool::new(false);
    let outcome = backend.try_accumulate_cancellable(&scene, 0, 1, &mut accum, &never_cancel);
    assert_eq!(
        outcome,
        GpuAccumulate::Declined,
        "a poisoned renderer mutex must decline, not dispatch into possibly-corrupt state"
    );
    assert!(
        backend.lost.load(std::sync::atomic::Ordering::Relaxed),
        "a poisoned renderer mutex must set `lost`, like DeviceLost does"
    );

    // A SECOND call must also decline, via the `self.lost` fast path this time
    // (never touching the still-poisoned mutex again).
    let outcome2 = backend.try_accumulate_cancellable(&scene, 0, 1, &mut accum, &never_cancel);
    assert_eq!(outcome2, GpuAccumulate::Declined);
}

/// Guards against the live viewport freezing after its first frame: it shows an
/// initial image but renders nothing further, ignoring camera drags from then on.
///
/// `renderer::gpu::frame`'s own hardware tests exercise `GpuFrameRenderer::accumulate`
/// directly, which drives one UNLIMITED-chunk-budget turn per call -- not what the
/// live viewport actually does. `apps::indicatrix-cut::bridge::render_thread::
/// gpu_backend::ViewportGpu::try_accumulate` calls `GpuBackend::try_accumulate`
/// (this module tree), which drives `try_accumulate_cancellable`'s loop: EVERY single
/// viewport frame is broken into many `CHUNKS_PER_TURN`-chunk turns, each one
/// re-taking a `Turnstile` ticket and re-running `prepare_turn`'s full scene
/// re-upload -- far more turn-boundary churn per frame than the unlimited-chunk
/// path exercises. This test drives that EXACT production entry point across
/// several simulated viewport frames, each with a different camera pose (as
/// `on_camera_orbit` produces on every drag `moved` event), to see whether that
/// extra churn is what trips the poisoning this bug report describes.
#[test]
fn viewport_frames_with_camera_changes_never_poison_the_backend() {
    let Some(backend) =
        acquire_or_skip("viewport_frames_with_camera_changes_never_poison_the_backend")
    else {
        return;
    };
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let environment = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);
    let never_cancel = AtomicBool::new(false);

    // A realistic viewport resolution at a realistic per-frame `spp` -- large
    // enough (480*360*2 = 345_600 (pixel, sample) tuples) that CHUNKS_PER_TURN's
    // small per-turn budget forces MANY turns per single `try_accumulate` call,
    // exactly like the real render loop's own frames.
    let (width, height) = (480u32, 360u32);
    let spp = 2u32;
    let mut accum = vec![Vec3::ZERO; (width * height) as usize];

    for frame in 0..12u32 {
        let yaw = (frame as f32).mul_add(0.29, 0.1);
        let pitch = (frame as f32).mul_add(0.13, 0.2).clamp(-1.4, 1.4);
        let camera = Camera::new(yaw, pitch, 5.0, 18.0);
        let scene = GpuSceneRef {
            camera: &camera,
            width,
            height,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 4,
            environment,
        };
        let outcome =
            backend.try_accumulate_cancellable(&scene, frame * spp, spp, &mut accum, &never_cancel);
        assert_eq!(
            outcome,
            GpuAccumulate::Done,
            "frame {frame} (yaw={yaw}, pitch={pitch}) did not complete -- got {outcome:?} \
             instead of Done; `backend.lost` = {}",
            backend.lost.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
    assert!(
        accum.iter().any(|v| v.length_squared() > 0.0),
        "a lit studio-rig scene traced over 12 frames must leave SOME nonzero radiance"
    );
}

/// A scene description both the failure-injection tests share: a lit round brilliant at
/// `width` x `height`, with a cheap bounce budget.
const fn spinel_scene<'a>(
    camera: &'a Camera,
    planes: &'a [crate::geometry::GpuFacetPlane],
    material: &'a GemMaterial,
    width: u32,
    height: u32,
) -> GpuSceneRef<'a> {
    GpuSceneRef {
        camera,
        width,
        height,
        planes,
        facet_finishes: &[],
        material,
        max_bounces: 2,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    }
}

/// A device lost on a LATER turn must not leave the earlier turns' samples in the
/// caller's buffer: the worker's CPU fallback re-traces the whole range into the same
/// buffer, so leftovers would be double counted.
///
/// 1024 x 768 at 4 spp is 3.1 million (pixel, sample) tuples; the first, uncalibrated
/// chunks are capped at one million tuples each, so a two-chunk turn cannot finish the
/// frame and the request needs at least a second turn, the one the failure is injected
/// into.
#[test]
fn a_failure_on_a_later_turn_leaves_accum_untouched() {
    let Some(backend) = acquire_or_skip("a_failure_on_a_later_turn_leaves_accum_untouched") else {
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let (width, height) = (1024u32, 768u32);
    let scene = spinel_scene(&camera, &planes, &material, width, height);
    let mut accum = vec![Vec3::ZERO; (width * height) as usize];
    let never_cancel = AtomicBool::new(false);

    backend.fail_on_turn.store(2, Ordering::Relaxed);
    let outcome = backend.try_accumulate_cancellable(&scene, 0, 4, &mut accum, &never_cancel);

    assert_eq!(
        backend.fail_on_turn.load(Ordering::Relaxed),
        0,
        "the request finished before its second turn, so nothing was injected -- enlarge \
         the frame (outcome {outcome:?})"
    );
    assert_eq!(outcome, GpuAccumulate::Declined);
    assert!(
        accum.iter().all(|pixel| *pixel == Vec3::ZERO),
        "a decline on the second turn left the first turn's samples in accum"
    );
    assert!(
        backend.is_lost(),
        "an injected DeviceLost marks the backend lost"
    );
}

/// After a loss the backend declines through the cool-down, then re-acquires a device on
/// the next request and renders again. The loss is aged by rewriting the policy's
/// timestamp instead of sleeping for 30 s.
#[test]
fn a_lost_backend_recovers_after_the_cool_down_and_renders_again() {
    let Some(backend) =
        acquire_or_skip("a_lost_backend_recovers_after_the_cool_down_and_renders_again")
    else {
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let scene = spinel_scene(&camera, &planes, &material, 64, 64);
    let mut accum = vec![Vec3::ZERO; 64 * 64];
    let never_cancel = AtomicBool::new(false);

    backend.fail_on_turn.store(1, Ordering::Relaxed);
    assert_eq!(
        backend.try_accumulate_cancellable(&scene, 0, 1, &mut accum, &never_cancel),
        GpuAccumulate::Declined
    );
    assert!(backend.is_lost());
    assert!(
        backend.adapter_label().is_none(),
        "a lost backend must not claim an adapter"
    );

    assert_eq!(
        backend.try_accumulate_cancellable(&scene, 0, 1, &mut accum, &never_cancel),
        GpuAccumulate::Declined,
        "inside the cool-down the backend must keep declining"
    );
    assert!(backend.is_lost());

    let Some(aged) = Instant::now().checked_sub(COOL_DOWN * 2) else {
        println!("skipping the recovery half: the monotonic clock is younger than the cool-down");
        return;
    };
    backend
        .recovery
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record_loss(aged);

    assert_eq!(
        backend.try_accumulate_cancellable(&scene, 0, 1, &mut accum, &never_cancel),
        GpuAccumulate::Done,
        "after the cool-down the next request must re-acquire the device and render"
    );
    assert!(!backend.is_lost());
    assert!(backend.adapter_label().is_some());
    assert!(
        accum.iter().any(|pixel| pixel.length_squared() > 0.0),
        "the recovered render must have produced radiance"
    );
}

/// A backend that never had a renderer has nothing to lose or recover, and declines
/// without touching `accum`.
#[test]
fn a_disabled_backend_declines_and_is_never_lost() {
    let backend = GpuBackend::disabled();
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let scene = spinel_scene(&camera, &planes, &material, 8, 8);
    let mut accum = vec![Vec3::ZERO; 64];
    let never_cancel = AtomicBool::new(false);

    assert_eq!(
        backend.try_accumulate_cancellable(&scene, 0, 1, &mut accum, &never_cancel),
        GpuAccumulate::Declined
    );
    assert!(accum.iter().all(|pixel| *pixel == Vec3::ZERO));
    assert!(!backend.is_lost());
    assert!(
        backend.try_recover(),
        "nothing is lost, so nothing to recover"
    );
    assert!(backend.adapter_label().is_none());
}

/// Nothing may be attempted before the cool-down has run out, and nothing at all without
/// a recorded loss.
#[test]
fn recovery_waits_out_the_cool_down() {
    let start = Instant::now();
    let mut policy = RecoveryPolicy::new();
    assert!(!policy.may_attempt(start), "no loss on record");

    policy.record_loss(start);
    assert!(!policy.may_attempt(start + COOL_DOWN.saturating_sub(Duration::from_secs(1))));
    assert!(policy.may_attempt(start + COOL_DOWN));

    policy.record_recovered();
    assert!(
        !policy.may_attempt(start + COOL_DOWN * 10),
        "a recovered device has no pending attempt"
    );
}

/// A failed attempt is followed by another full cool-down, measured from the attempt.
#[test]
fn a_failed_attempt_restarts_the_cool_down() {
    let start = Instant::now();
    let mut policy = RecoveryPolicy::new();
    policy.record_loss(start);
    let attempt = start + COOL_DOWN;
    assert!(policy.may_attempt(attempt));
    policy.record_attempt(attempt);

    assert!(!policy.may_attempt(attempt + COOL_DOWN.saturating_sub(Duration::from_secs(1))));
    assert!(policy.may_attempt(attempt + COOL_DOWN));
}

/// At most [`MAX_ATTEMPTS_PER_WINDOW`] attempts start in any one [`WINDOW`]; the oldest
/// ages out exactly one window after it began.
#[test]
fn at_most_six_attempts_start_per_hour() {
    assert_eq!(MAX_ATTEMPTS_PER_WINDOW, 6);
    assert_eq!(WINDOW, Duration::from_hours(1));
    let start = Instant::now();
    let mut policy = RecoveryPolicy::new();
    policy.record_loss(start);

    let mut now = start;
    for _ in 0..MAX_ATTEMPTS_PER_WINDOW {
        now += COOL_DOWN;
        assert!(policy.may_attempt(now));
        policy.record_attempt(now);
    }
    now += COOL_DOWN;
    assert!(
        !policy.may_attempt(now),
        "a seventh attempt inside the hour must be refused even after the cool-down"
    );

    let first_attempt = start + COOL_DOWN;
    assert!(!policy.may_attempt(first_attempt + WINDOW.saturating_sub(Duration::from_secs(1))));
    assert!(policy.may_attempt(first_attempt + WINDOW));
}

// ---------------------------------------------------------------------------------------
// Batches: `try_accumulate_batch_cancellable` must equal each picture traced alone.
// ---------------------------------------------------------------------------------------

/// One picture of a test batch, owning what its `GpuSceneRef` borrows.
struct BatchSpec {
    camera: Camera,
    material: GemMaterial,
    width: u32,
    height: u32,
    first_sample: u32,
    samples: u32,
}

impl BatchSpec {
    fn new(
        material: GemMaterial,
        yaw: f32,
        (width, height): (u32, u32),
        first_sample: u32,
        samples: u32,
    ) -> Self {
        Self {
            camera: Camera::new(yaw, 0.28, 5.0, 18.0),
            material,
            width,
            height,
            first_sample,
            samples,
        }
    }

    const fn scene<'a>(&'a self, planes: &'a [crate::geometry::GpuFacetPlane]) -> GpuSceneRef<'a> {
        spinel_scene(
            &self.camera,
            planes,
            &self.material,
            self.width,
            self.height,
        )
    }

    const fn pixels(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// Three pictures that differ in material (and so pipeline class where the library's
/// birefringent stones classify differently), size, camera and sample range.
fn three_different_pictures() -> Vec<BatchSpec> {
    vec![
        BatchSpec::new(
            GemMaterial::by_name("Spinel").expect("built-in"),
            0.35,
            (64, 48),
            0,
            3,
        ),
        BatchSpec::new(GemMaterial::diamond(), 0.9, (40, 40), 5, 2),
        BatchSpec::new(
            GemMaterial::by_name("Ruby").expect("built-in"),
            1.7,
            (56, 32),
            0,
            4,
        ),
    ]
}

/// The bit patterns of `pixels`, so equality is exact (`-0.0`, NaN payloads and all).
pub(super) fn bits(pixels: &[Vec3]) -> Vec<[u32; 3]> {
    pixels
        .iter()
        .map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])
        .collect()
}

/// Each picture traced alone through the single-request path: the reference a batch must
/// reproduce bit for bit.
fn traced_alone(
    backend: &GpuBackend,
    specs: &[BatchSpec],
    planes: &[crate::geometry::GpuFacetPlane],
) -> Vec<Vec<Vec3>> {
    let never_cancel = AtomicBool::new(false);
    specs
        .iter()
        .map(|spec| {
            let mut accum = vec![Vec3::ZERO; spec.pixels()];
            let outcome = backend.try_accumulate_cancellable(
                &spec.scene(planes),
                spec.first_sample,
                spec.samples,
                &mut accum,
                &never_cancel,
            );
            assert_eq!(
                outcome,
                GpuAccumulate::Done,
                "reference render must succeed"
            );
            accum
        })
        .collect()
}

/// Runs `specs` as one batch into fresh zeroed buffers; returns the buffers and the
/// outcome `on_done` reported for each picture (`None` = never reported).
fn traced_as_batch(
    backend: &GpuBackend,
    specs: &[BatchSpec],
    planes: &[crate::geometry::GpuFacetPlane],
    cancel: &AtomicBool,
    mut on_done: impl FnMut(usize, GpuAccumulate),
) -> (Vec<Vec<Vec3>>, Vec<Option<GpuAccumulate>>) {
    let mut outs: Vec<Vec<Vec3>> = specs
        .iter()
        .map(|spec| vec![Vec3::ZERO; spec.pixels()])
        .collect();
    let mut outcomes = vec![None; specs.len()];
    {
        let mut items: Vec<GpuBatchItem<'_>> = specs
            .iter()
            .zip(outs.iter_mut())
            .map(|(spec, out)| GpuBatchItem {
                scene: spec.scene(planes),
                first_sample: spec.first_sample,
                samples: spec.samples,
                out: out.as_mut_slice(),
            })
            .collect();
        backend.try_accumulate_batch_cancellable(&mut items, cancel, &mut |index, outcome| {
            assert!(
                outcomes[index].is_none(),
                "picture {index} reported more than once"
            );
            outcomes[index] = Some(outcome);
            on_done(index, outcome);
        });
    }
    (outs, outcomes)
}

/// A batch of three different pictures equals each picture traced alone, bitwise, and
/// every picture is reported once, in input order.
#[test]
fn a_batch_of_different_scenes_equals_each_scene_alone() {
    let Some(backend) = acquire_or_skip("a_batch_of_different_scenes_equals_each_scene_alone")
    else {
        return;
    };
    let planes = StandardGemCuts::standard_round_brilliant();
    let specs = three_different_pictures();
    let expected = traced_alone(&backend, &specs, &planes);

    let never_cancel = AtomicBool::new(false);
    let mut order = Vec::new();
    let (outs, outcomes) = traced_as_batch(&backend, &specs, &planes, &never_cancel, |index, _| {
        order.push(index);
    });

    assert_eq!(order, vec![0, 1, 2], "pictures are reported in input order");
    for (index, outcome) in outcomes.iter().enumerate() {
        assert_eq!(*outcome, Some(GpuAccumulate::Done), "picture {index}");
        assert_eq!(
            bits(&outs[index]),
            bits(&expected[index]),
            "picture {index} differs from its single-request render"
        );
    }
    assert!(
        outs.iter()
            .any(|out| out.iter().any(|p| p.length_squared() > 0.0)),
        "the batch must have produced radiance"
    );
}

/// A batch whose pictures span several turns, run while another thread keeps issuing
/// single requests on the same backend: both sides stay bit-identical to running alone.
#[test]
fn a_batch_interleaved_with_a_concurrent_single_request_stays_bit_identical() {
    let Some(backend) =
        acquire_or_skip("a_batch_interleaved_with_a_concurrent_single_request_stays_bit_identical")
    else {
        return;
    };
    let planes = StandardGemCuts::standard_round_brilliant();
    // Large enough that every picture needs more than CHUNKS_PER_TURN chunks (the first,
    // uncalibrated chunks are capped at one million tuples), so the batch really yields.
    let specs = vec![
        BatchSpec::new(
            GemMaterial::by_name("Spinel").expect("built-in"),
            0.2,
            (512, 384),
            0,
            4,
        ),
        BatchSpec::new(
            GemMaterial::by_name("Ruby").expect("built-in"),
            1.1,
            (400, 300),
            3,
            4,
        ),
        BatchSpec::new(GemMaterial::diamond(), 2.0, (320, 320), 0, 4),
    ];
    let single = BatchSpec::new(
        GemMaterial::by_name("Spinel").expect("built-in"),
        0.7,
        (384, 256),
        2,
        4,
    );
    let expected_batch = traced_alone(&backend, &specs, &planes);
    let expected_single = traced_alone(&backend, std::slice::from_ref(&single), &planes);

    let never_cancel = AtomicBool::new(false);
    thread::scope(|scope| {
        let single_thread = scope.spawn(|| {
            (0..3)
                .map(|_| {
                    let mut accum = vec![Vec3::ZERO; single.pixels()];
                    let outcome = backend.try_accumulate_cancellable(
                        &single.scene(&planes),
                        single.first_sample,
                        single.samples,
                        &mut accum,
                        &never_cancel,
                    );
                    (outcome, accum)
                })
                .collect::<Vec<_>>()
        });
        let (outs, outcomes) = traced_as_batch(&backend, &specs, &planes, &never_cancel, |_, _| {});
        for (index, outcome) in outcomes.iter().enumerate() {
            assert_eq!(*outcome, Some(GpuAccumulate::Done), "batch picture {index}");
            assert_eq!(
                bits(&outs[index]),
                bits(&expected_batch[index]),
                "batch picture {index} changed under contention"
            );
        }
        for (outcome, accum) in single_thread.join().expect("single-request thread") {
            assert_eq!(outcome, GpuAccumulate::Done);
            assert_eq!(
                bits(&accum),
                bits(&expected_single[0]),
                "the concurrent single request changed under contention"
            );
        }
    });
}

/// Cancelling from inside the first `on_done` reports the first picture `Done`, the last
/// one `Cancelled` (its `out` untouched), and leaves the renderer reusable.
#[test]
fn cancelling_mid_batch_reports_cancelled_and_leaves_the_renderer_reusable() {
    let Some(backend) =
        acquire_or_skip("cancelling_mid_batch_reports_cancelled_and_leaves_the_renderer_reusable")
    else {
        return;
    };
    let planes = StandardGemCuts::standard_round_brilliant();
    let specs = three_different_pictures();
    let expected = traced_alone(&backend, &specs, &planes);

    let cancel = AtomicBool::new(false);
    let (outs, outcomes) = traced_as_batch(&backend, &specs, &planes, &cancel, |index, outcome| {
        if index == 0 && outcome == GpuAccumulate::Done {
            cancel.store(true, Ordering::Relaxed);
        }
    });

    assert_eq!(outcomes[0], Some(GpuAccumulate::Done));
    assert_eq!(bits(&outs[0]), bits(&expected[0]));
    // Picture 1's only chunk was already queued when the flag fired, so it may still
    // finish; picture 2 had not been dispatched and must be cancelled.
    assert!(matches!(
        outcomes[1],
        Some(GpuAccumulate::Done | GpuAccumulate::Cancelled)
    ));
    assert_eq!(outcomes[2], Some(GpuAccumulate::Cancelled));
    for (index, outcome) in outcomes.iter().enumerate() {
        match outcome {
            Some(GpuAccumulate::Done) => assert_eq!(bits(&outs[index]), bits(&expected[index])),
            Some(GpuAccumulate::Cancelled) => assert!(
                outs[index].iter().all(|p| *p == Vec3::ZERO),
                "cancelled picture {index} must leave its output untouched"
            ),
            other => panic!("picture {index}: unexpected outcome {other:?}"),
        }
    }
    assert!(!backend.is_lost(), "a cancel is not a device loss");

    // Reusable: the same batch, uncancelled, matches the reference again.
    let never_cancel = AtomicBool::new(false);
    let (outs, outcomes) = traced_as_batch(&backend, &specs, &planes, &never_cancel, |_, _| {});
    for index in 0..specs.len() {
        assert_eq!(outcomes[index], Some(GpuAccumulate::Done), "rerun {index}");
        assert_eq!(bits(&outs[index]), bits(&expected[index]), "rerun {index}");
    }
}

/// A device lost during a batch declines every picture not yet reported (their outputs
/// untouched), marks the backend lost, and after the cool-down the next batch recovers
/// and is bit-identical to the single-request reference.
#[test]
fn a_device_loss_during_a_batch_declines_the_rest_and_the_backend_recovers() {
    let Some(backend) =
        acquire_or_skip("a_device_loss_during_a_batch_declines_the_rest_and_the_backend_recovers")
    else {
        return;
    };
    let planes = StandardGemCuts::standard_round_brilliant();
    let specs = three_different_pictures();
    let expected = traced_alone(&backend, &specs, &planes);
    let never_cancel = AtomicBool::new(false);

    // The seam counts batch STEPS (one chunk each): the first step fails before any
    // picture can have been reported.
    backend.fail_on_turn.store(1, Ordering::Relaxed);
    let (outs, outcomes) = traced_as_batch(&backend, &specs, &planes, &never_cancel, |_, _| {});
    assert!(
        outcomes
            .iter()
            .all(|outcome| *outcome == Some(GpuAccumulate::Declined)),
        "{outcomes:?}"
    );
    assert!(
        outs.iter().all(|out| out.iter().all(|p| *p == Vec3::ZERO)),
        "a declined picture must leave its output untouched"
    );
    assert!(backend.is_lost());

    let Some(aged) = Instant::now().checked_sub(COOL_DOWN * 2) else {
        println!("skipping the recovery half: the monotonic clock is younger than the cool-down");
        return;
    };
    backend
        .recovery
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record_loss(aged);

    let (outs, outcomes) = traced_as_batch(&backend, &specs, &planes, &never_cancel, |_, _| {});
    assert!(!backend.is_lost());
    for index in 0..specs.len() {
        assert_eq!(
            outcomes[index],
            Some(GpuAccumulate::Done),
            "recovered {index}"
        );
        assert_eq!(
            bits(&outs[index]),
            bits(&expected[index]),
            "recovered {index}"
        );
    }
}

/// A disabled backend reports every picture `Declined` and touches no output.
#[test]
fn a_disabled_backend_declines_every_batch_item() {
    let backend = GpuBackend::disabled();
    let planes = StandardGemCuts::standard_round_brilliant();
    let specs = three_different_pictures();
    let never_cancel = AtomicBool::new(false);
    let (outs, outcomes) = traced_as_batch(&backend, &specs, &planes, &never_cancel, |_, _| {});
    assert!(
        outcomes
            .iter()
            .all(|outcome| *outcome == Some(GpuAccumulate::Declined))
    );
    assert!(outs.iter().all(|out| out.iter().all(|p| *p == Vec3::ZERO)));
}

/// The yield test a batch uses: only tickets handed out AFTER the holder's count.
#[test]
fn has_waiters_behind_sees_only_later_tickets() {
    let turnstile = Turnstile::new();
    let first = turnstile.take_ticket();
    assert!(!turnstile.has_waiters_behind(first));
    let second = turnstile.take_ticket();
    assert!(turnstile.has_waiters_behind(first));
    assert!(!turnstile.has_waiters_behind(second));
}
