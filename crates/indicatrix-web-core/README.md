# indicatrix-web-core

The compute side of the indicatrix browser app (`apps/indicatrix-web`), with no DOM
and no Slint in it. Everything except the `host` module builds and is tested natively:

```
cargo test -p indicatrix-web-core
```

## Pieces

Ray-Traced engine selection travels beside the unchanged scene message. Its CPU workers
cache tint, environment and frost preparation per scene. Its small WebGPU kernel accepts
every material class, with independent admission keyed by the whitted shader hash; the
existing event-wavefront keys and records stay valid. The GPU worker compares a 16×16,
four-tap render with the CPU under G-D10 before admitting the small kernel. Oversized HDR maps and more than 128 tools (more than a scene can hold) decline the GPU route.

| Module | What it holds |
|---|---|
| `protocol` | `ToWorker` / `FromWorker`, postcard-encoded into a transferred `ArrayBuffer`; `PROTOCOL_VERSION` (24), checked on `Init`. Requests: trace a chunk, trace a full GPU pass, compile GPU kernels, make a picture, set or clear an HDR map, select an engine beside the unchanged scene payload (`SetEngine`, index 12), and the solve-role jobs. New variants are appended; existing scene bytes remain unchanged |
| `scene` | `SceneSpec` (planes, finishes, material, camera, lighting, bounces, size, HDR id) and `OwnedScene`, which builds exactly the desktop's scene for the same settings |
| `render` | `handle_trace_chunk` (one interleaved partition over a sample range), `Accumulator` (merges complete passes in order), `ChunkPlanner` (sizes passes to about 200 ms per Worker chunk), the live (64 to 1024) and export (16 to 4096) sample bounds |
| `solve` | `handle_solve`: a native-TOML design in, solved masts, planes, status and warnings out, like the desktop's background solve. `analysis` is Optimize and Retarget (cooperatively cancellable, with progress), `optical` is the current-view metrics (scored under the HDR map the Worker holds when the request names it, else the lighting preset) and the four-axis tilt sweep (always under the preset, as on the desktop), all the desktop's own functions; both score a concave stone with its tools (`SolveRequest::metrics` / `tilt` send `MetricsWithTools` / `TiltWithTools`, and the planar variants, bytes unchanged, when there are none) |
| `display` | Pixels from an accumulation: the live tone map, the settled denoise, the PNG export (sRGB or Display P3, ICC embedded) and its file name; the 4096 px export edge |
| `settings` | The persisted render settings and `SessionPayload` (the JSON in `sessionStorage`), and `scene_spec`, which turns them into a `SceneSpec` with the desktop's conversions |
| `custom_material` | The Design settings dialog's custom-material fields turned into a `GemMaterial` and the snapshot an `.isdd` file stores for it |
| `design_meta` | The `.isdd` file's descriptive `[meta]` table as the page keeps it: `stamped_for_save` (new `modified_at`, an id and creation time when missing, sorted tags, everything else unchanged), `iso8601_utc_from_epoch_ms` and `uuid_v4_from_bytes`; the codec reads no clock, so the page supplies both |
| `guide` | The guided walkthrough's tab-session state, and `WEB_WORDING`, the few walkthrough lines reworded for the browser (a test pins that each still exists in the shared steps) |
| `hdr` | The browser HDR limits (64 MiB, 8192 x 4096) and the render-Worker count rule (never downsample; fewer render Workers instead), which counts the analysis Worker's copy of the map against the 1.5 GiB budget |
| `worker` | `WorkerHandler`, the whole Worker state machine; `apps/indicatrix-web-compute` only moves bytes in and out of it |
| `host` (wasm32) | `WorkerPool` = `RenderPool` (N render Workers + a picture Worker created on first use) + `SolveClient` (1 solve Worker) + an analysis `SolveClient` created on first use; the render side also owns the optional GPU render Worker slot (`gpu_slot`: a pass ledger, scene re-send after the self-check, passes handed back to the planner when the GPU is turned off, lost or silent) |
| `gpu_admission` | The pure half of the GPU admission gate: scene eligibility (isotropic, not zoned, at most 128 concave tools, HDR fits), the per-adapter admission record, the adapter key (FNV-1a over the adapter fields, limits, browser major and kernel set hash) and the 16x16 self-check scene |
| `gpu_ledger` | `GpuLedger`: the passes posted to the GPU Worker and not yet answered, so a stale answer never clears the supervision of a newer pass |
| `solve_error` | `SolveError` (superseded, cancelled, failed): the typed reason a solve-role job produced no answer |
| `gpu_startup` | The pure half of the GPU start-up test: what the page does at load (`StartupPlan`), the crash marker, the progress of a run and the sentence that says what the app renders with |
| `input` | The size limits checked on a picked or dropped file before it is read |
| `open_route` | What the Open picker offers and the file names a design is saved under (which loader a file goes to is `indicatrix_editor::files::InputFileKind::classify`) |
| `rows` | Pure helpers of the page's tier table, nudges, schedule readout and history list |
| `new_design` | The template path of the New Design dialog without the window |
| `storage_keys` | The `indicatrix.<name>.v<n>` storage key builders |
| `tab_state` | The design name and unsaved flag, which belong to one browser tab rather than to the shared settings |

## Workers

- Render Workers: `clamp(navigator.hardwareConcurrency - 1, 1, 8)` (one when the browser
  reports no value). Worker `i` always traces partition `i` (pixels `i, i + N, ...`). A
  chunk is traced in row groups, and the page can stop it between them: it revokes the
  chunk's `blob:` URL when it replaces the scene, so an orbit does not wait for a long
  chunk of the old one. A Worker that crashes or stays silent for 30 s (longer for a huge
  chunk) while holding a chunk is terminated and replaced, up to three times in a row.
- One picture Worker (spawned on the first request): the settled live view's denoise and
  an export's tone map and PNG encode run there, so no tracing Worker stops tracing for
  them.
- One solve Worker. A new request supersedes the previous one; a job with no answer
  within 60 s (configurable) terminates and respawns the Worker. Optimize and Retarget
  cancel cooperatively (the Worker polls a revocable `blob:` URL between decisions and
  hands back its best result so far, after ~10 s of final scoring, and ~10 s more when
  the cancel came while the starting point was still being scored); a Worker that has
  not stopped after 30 s, or whose "Stop now" was pressed, is terminated and keeps no
  partial result. A browser that refuses the synchronous request the poll makes (a
  content-security policy without `blob:` in `connect-src`) is told apart from a revoked
  URL by requesting a URL the Worker makes itself; the search then simply runs on and is
  ended by the host's deadline instead of being stopped at its first check.
- One analysis Worker, created when the metrics HUD or the tilt sweep first runs, so those
  never wait behind or preempt a design solve. It holds a decoded copy of the HDR map the
  viewport is lit with (`WorkerPool::set_hdr` sends it to the render Workers and to this
  one, `clear_hdr` drops it), so the metrics HUD is scored under the map like the
  desktop's; the client remembers the map and gives it to a Worker created or replaced
  later. When the memory budget holds a single copy it is the render Worker's, and the
  metrics stay under the lighting preset (the result says which it was scored under).
  The tilt sweep is scored under the preset, as the desktop's dialog is.
- Compute Workers (render, solve, analysis) run `indicatrix-web-compute_loader.js`
  (`WORKER_LOADER_URL`), while the GPU render Worker runs
  `indicatrix-web-gpu_loader.js` (`GPU_WORKER_LOADER_URL`), both written by Trunk
  next to the app. The page creates the pool when a design is shown on the
  Render tab or a solve needs the solve Worker, and at page load when GPU rendering
  is on and the browser has WebGPU (the start-up test, `gpu_startup`).

## Bit-identity

A render Worker traces with `renderer::cpu_frame::trace_pixels_interleaved`, the same
thread-free core the desktop's CPU renderer runs per thread. The accumulator merges a
pass only when every partition has delivered it, in pass order, so the running sum is
bit-identical to the desktop's `cpu_accumulate` called once per pass -- on the same
target. The code path is the same in the browser, but bitwise identity holds only on the
same target: wasm and native take their transcendental functions (`powf`, `sin`, ...)
from different libms. The tests run natively and check this against `cpu_accumulate`
itself; no test runs the tracer on wasm.
