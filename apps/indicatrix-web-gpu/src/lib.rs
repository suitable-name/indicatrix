//! The WebGPU render Worker entry point of the indicatrix browser app.
//!
//! Trunk builds this crate as a worker (`data-type="worker"` in
//! `apps/indicatrix-web/index.html`): wasm-bindgen's `no-modules` output plus a loader
//! shim, `indicatrix-web-gpu_loader.js`, which the page starts with
//! `new Worker(...)` (see `indicatrix_web_core::GPU_WORKER_LOADER_URL`).
//!
//! On start the Worker installs its `onmessage` handler, then posts `Loaded`. Each
//! message is a postcard-encoded `ToWorker` in a transferred `ArrayBuffer`; it is
//! processed asynchronously in order, and replies are posted back as `FromWorker`.
//!
//! Like `apps/indicatrix-web-compute`, everything is `#[cfg(target_arch = "wasm32")]`,
//! so a native build of the workspace compiles this crate to nothing.

#[cfg(target_arch = "wasm32")]
mod entry {
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
        pin::pin,
        rc::Rc,
        sync::Arc,
    };

    use indicatrix::{
        geometry::tool::ToolPrimitive,
        renderer::{
            env_map::{EnvironmentMap, environment_from_hdr_bytes},
            gpu::event_wavefront::EventWavefrontRenderer,
            gpu::{
                GpuContext,
                whitted::{WhittedGpuRenderer, WhittedGpuScene},
            },
        },
    };
    use indicatrix_web_core::{
        protocol::{
            FromWorker, PROTOCOL_VERSION, ToWorker, WorkerRole, decode_to_worker,
            encode_from_worker,
        },
        scene::{GeometrySceneSpec, OwnedScene, SceneSpec},
    };
    use js_sys::{Array, ArrayBuffer, Uint8Array};
    use wasm_bindgen::{JsCast, closure::Closure, prelude::wasm_bindgen};
    use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

    mod adapter_info;
    mod compile;
    mod init;
    mod ray;
    mod trace;

    fn scope() -> DedicatedWorkerGlobalScope {
        js_sys::global().unchecked_into()
    }

    fn now_ms() -> f64 {
        scope().performance().map_or(0.0, |p| p.now())
    }

    fn post(message: &FromWorker) {
        let bytes = match encode_from_worker(message) {
            Ok(bytes) => bytes,
            Err(error) => {
                web_sys::console::error_1(&error.into());
                return;
            }
        };
        let Ok(length) = u32::try_from(bytes.len()) else {
            web_sys::console::error_1(&"indicatrix gpu worker: reply over 4 GiB".into());
            return;
        };
        let array = Uint8Array::new_with_length(length);
        array.copy_from(&bytes);
        let buffer = array.buffer();
        if let Err(error) = scope().post_message_with_transfer(&buffer, &Array::of1(&buffer)) {
            web_sys::console::error_2(&"indicatrix gpu worker: postMessage failed".into(), &error);
        }
    }

    enum RaceResult<T> {
        Completed(T),
        TimedOut,
    }

    #[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
    async fn timeout_race<F: std::future::Future>(
        future: F,
        timeout_ms: f64,
    ) -> RaceResult<F::Output> {
        use futures_util::future::{Either, select};

        let (timeout_tx, timeout_rx) = futures_channel::oneshot::channel();
        let closure = Closure::once(move || {
            let _ = timeout_tx.send(());
        });
        #[expect(
            clippy::cast_possible_truncation,
            reason = "timeouts are a few seconds; an f64 `as i32` cast saturates"
        )]
        let timeout = timeout_ms.max(1.0) as i32;
        let timer_id = scope()
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                closure.as_ref().unchecked_ref(),
                timeout,
            )
            .ok();

        let fut = pin!(future);
        let timer = pin!(timeout_rx);

        match select(fut, timer).await {
            Either::Left((val, _)) => {
                if let Some(id) = timer_id {
                    scope().clear_timeout_with_handle(id);
                }
                RaceResult::Completed(val)
            }
            Either::Right(_) => RaceResult::TimedOut,
        }
    }

    struct CachedScene {
        spec: SceneSpec,
        ray_gpu: Option<WhittedGpuScene>,
        scene_id: u64,
        /// Why this scene cannot run on the event renderer (the page's own classifier,
        /// `gpu_admission::gpu_ineligibility_reason`); the pass is refused when set.
        ineligible: Option<&'static str>,
        scene: OwnedScene,
        /// Whether the scene holds an `Arc` of the HDR map (dropped with the map, as in
        /// the CPU worker).
        uses_hdr: bool,
        width: u32,
        height: u32,
    }

    #[derive(Default)]
    struct GpuWorkerState {
        context: Option<GpuContext>,
        whitted: Option<WhittedGpuRenderer>,
        role: Option<WorkerRole>,
        renderer: Option<EventWavefrontRenderer>,
        cached_scene: Option<CachedScene>,
        cached_hdr: Option<(u64, Arc<EnvironmentMap>)>,
    }

    thread_local! {
        static STATE: Rc<RefCell<GpuWorkerState>> = Rc::new(RefCell::new(GpuWorkerState::default()));
        static QUEUE: Rc<RefCell<VecDeque<ToWorker>>> = Rc::new(RefCell::new(VecDeque::new()));
        static IS_DRAINING: Cell<bool> = const { Cell::new(false) };
        /// Set by the device-lost callback. Deliberately NOT part of `STATE`: the
        /// callback can fire while a handler is suspended with the state in use, and a
        /// `RefCell` access there would panic.
        static DEVICE_LOST: Cell<bool> = const { Cell::new(false) };
        /// Which `Init` the current device belongs to; a stale device's callback is ignored.
        static DEVICE_GENERATION: Cell<u32> = const { Cell::new(0) };
        /// Set after a failed or timed-out compile: no further compile starts.
        static COMPILE_BLOCKED: Cell<bool> = const { Cell::new(false) };
    }

    fn device_lost() -> bool {
        DEVICE_LOST.with(Cell::get)
    }

    fn block_compile() {
        COMPILE_BLOCKED.with(|c| c.set(true));
    }

    /// Resets `IS_DRAINING` on every exit path of the drain task.
    struct DrainGuard;

    impl Drop for DrainGuard {
        fn drop(&mut self) {
            IS_DRAINING.with(|flag| flag.set(false));
        }
    }

    /// Drops the cached scene when it holds the HDR map that is being replaced or cleared.
    fn drop_scene_using_hdr(state: &mut GpuWorkerState) {
        if state.cached_scene.as_ref().is_some_and(|c| c.uses_hdr) {
            state.cached_scene = None;
        }
    }

    /// Caches the scene `spec` minus `tools` (empty for a planar stone) as the Worker's one
    /// scene, remembering whether the GPU may trace it.
    fn set_scene(
        state_rc: &Rc<RefCell<GpuWorkerState>>,
        scene_id: u64,
        spec: SceneSpec,
        tools: Vec<ToolPrimitive>,
    ) {
        let mut state = state_rc.borrow_mut();
        state.cached_scene = None;

        let hdr = match (spec.hdr_id, &state.cached_hdr) {
            (Some(wanted), Some((held, map))) if wanted == *held => Some(Arc::clone(map)),
            _ => None,
        };

        // The page's classifier (isotropic, not zoned, no UV lamp, at most
        // `GPU_MAX_TOOL_PRIMITIVES` concave tools). The HDR size check needs the adapter
        // limits and stays with the page.
        let ineligible = indicatrix_web_core::gpu_admission::gpu_ineligibility_reason_with_tools(
            &spec,
            tools.len(),
            0,
            None,
            false,
        );
        let (uses_hdr, width, height) = (spec.hdr_id.is_some(), spec.width, spec.height);

        match (GeometrySceneSpec {
            scene: spec.clone(),
            tools,
        })
        .build(hdr)
        {
            Ok(scene) => {
                state.cached_scene = Some(CachedScene {
                    spec,
                    ray_gpu: None,
                    scene_id,
                    ineligible,
                    scene,
                    uses_hdr,
                    width,
                    height,
                });
            }
            Err(message) => {
                post(&FromWorker::SceneError { scene_id, message });
            }
        }
    }

    /// Decodes the HDR map `bytes` as map `id` and caches it. The old map, and a scene still
    /// holding it, are freed before decoding (exactly as the CPU worker does).
    fn load_hdr(state_rc: &Rc<RefCell<GpuWorkerState>>, id: u64, bytes: &[u8]) {
        let mut state = state_rc.borrow_mut();
        state.cached_hdr = None;
        drop_scene_using_hdr(&mut state);
        match environment_from_hdr_bytes(bytes, indicatrix_web_core::hdr::web_hdr_limits()) {
            Ok(map) => {
                let width = u32::try_from(map.width()).unwrap_or(u32::MAX);
                let height = u32::try_from(map.height()).unwrap_or(u32::MAX);
                state.cached_hdr = Some((id, Arc::new(map)));
                post(&FromWorker::HdrLoaded { id, width, height });
            }
            Err(error) => {
                post(&FromWorker::HdrError {
                    id,
                    message: error.to_string(),
                });
            }
        }
    }

    #[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
    async fn handle_message(state_rc: &Rc<RefCell<GpuWorkerState>>, message: ToWorker) {
        // After a device loss everything but a new `Init` is ignored.
        if device_lost() && !matches!(message, ToWorker::Init { .. }) {
            if matches!(
                message,
                ToWorker::GpuCompile { .. } | ToWorker::TraceFullPass { .. }
            ) {
                post(&FromWorker::GpuLost {
                    reason: "device is lost".to_string(),
                });
            }
            return;
        }
        match message {
            ToWorker::Init {
                protocol_version,
                role,
                worker_index: _,
            } => init::handle_init(state_rc, protocol_version, role).await,

            ToWorker::GpuCompile {
                class,
                kernels,
                soft_ms,
                hard_ms,
            } => compile::handle_compile(state_rc, class, &kernels, soft_ms, hard_ms).await,

            ToWorker::SetScene { scene_id, spec } => {
                set_scene(state_rc, scene_id, spec, Vec::new());
            }

            ToWorker::SetGeometryScene {
                scene_id,
                spec,
                tools,
            } => {
                set_scene(state_rc, scene_id, spec, tools);
            }

            ToWorker::SetEngine { scene_id, engine } => {
                ray::set_engine(state_rc, scene_id, engine);
            }

            ToWorker::HdrMap { id, bytes } => load_hdr(state_rc, id, &bytes),

            ToWorker::ClearHdr => {
                let mut state = state_rc.borrow_mut();
                state.cached_hdr = None;
                drop_scene_using_hdr(&mut state);
            }

            ToWorker::TraceFullPass {
                scene_id,
                pass_id,
                sample_offset,
                spp,
            } => {
                let pass = trace::Pass {
                    scene_id,
                    id: pass_id,
                    sample_offset,
                    spp,
                };
                let is_ray = state_rc
                    .borrow()
                    .cached_scene
                    .as_ref()
                    .is_some_and(|cached| cached.scene.engine().is_ray());
                if is_ray {
                    ray::trace_pass(state_rc, pass).await;
                } else {
                    trace::handle_full_pass(state_rc, pass).await;
                }
            }

            _ => {
                post(&FromWorker::Error {
                    message: "unsupported message sent to GPU worker".to_string(),
                });
            }
        }
    }

    fn drain_queue() {
        if IS_DRAINING.with(|flag| flag.replace(true)) {
            return;
        }
        wasm_bindgen_futures::spawn_local(async {
            // Resets IS_DRAINING on every exit path, including an unwinding panic.
            let _guard = DrainGuard;
            loop {
                let next_msg = QUEUE.with(|q| q.borrow_mut().pop_front());
                let Some(msg) = next_msg else {
                    break;
                };
                let state_rc = STATE.with(Rc::clone);
                handle_message(&state_rc, msg).await;
            }
        });
    }

    fn on_message(event: &MessageEvent) {
        let Ok(buffer) = event.data().dyn_into::<ArrayBuffer>() else {
            post(&FromWorker::Error {
                message: "a worker message was not an ArrayBuffer".to_string(),
            });
            return;
        };
        let bytes = Uint8Array::new(&buffer).to_vec();

        match decode_to_worker(&bytes) {
            Ok(msg) => {
                QUEUE.with(|q| q.borrow_mut().push_back(msg));
                drain_queue();
            }
            Err(err) => {
                post(&FromWorker::Error { message: err });
            }
        }
    }

    /// Runs when the WebGPU worker module is instantiated: installs `onmessage` and posts `Loaded`.
    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
        let closure = Closure::<dyn FnMut(MessageEvent)>::new(|event: MessageEvent| {
            on_message(&event);
        });
        scope().set_onmessage(Some(closure.as_ref().unchecked_ref()));
        closure.forget();

        post(&FromWorker::Loaded {
            protocol_version: PROTOCOL_VERSION,
        });
    }
}
