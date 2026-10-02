# indicatrix-web-core

The compute side of the indicatrix browser app (`apps/indicatrix-web`), with no DOM
and no Slint in it. Everything except the `host` module builds and is tested natively:

```
cargo test -p indicatrix-web-core
```

## Pieces

| Module | What it holds |
|---|---|
| `protocol` | `ToWorker` / `FromWorker`, postcard-encoded into a transferred `ArrayBuffer`; `PROTOCOL_VERSION` (8), checked on `Init`. Requests: trace a chunk, make a picture, set or clear an HDR map (render Workers and the analysis Worker), and the solve-role jobs (`SolveRequest`: Solve, Optimize, Retarget, Metrics, Tilt). New variants are appended at the end; messages of each direction, and the HDR fields of the metrics messages, are pinned byte for byte in the tests |
| `scene` | `SceneSpec` (planes, finishes, material, camera, lighting, bounces, size, HDR id) and `OwnedScene`, which builds exactly the desktop's scene for the same settings |
| `render` | `handle_trace_chunk` (one interleaved partition over a sample range), `Accumulator` (merges complete passes in order), `ChunkPlanner` (sizes passes to about 200 ms per Worker chunk), the live (64 to 1024) and export (16 to 4096) sample bounds |
| `solve` | `handle_solve`: a native-TOML design in, solved masts, planes, status and warnings out, like the desktop's background solve. `analysis` is Optimize and Retarget (cooperatively cancellable, with progress), `optical` is the current-view metrics (scored under the HDR map the Worker holds when the request names it, else the lighting preset) and the four-axis tilt sweep (always under the preset, as on the desktop), all the desktop's own functions |
| `display` | Pixels from an accumulation: the live tone map, the settled denoise, the PNG export (sRGB or Display P3, ICC embedded) and its file name; the 4096 px export edge |
| `settings` | The persisted render settings and `SessionPayload` (the JSON in `sessionStorage`), and `scene_spec`, which turns them into a `SceneSpec` with the desktop's conversions |
| `custom_material` | The Design settings dialog's custom-material fields turned into a `GemMaterial` and the snapshot a `.indicatrix` file stores for it |
| `design_meta` | The `.indicatrix` file's descriptive `[meta]` table as the page keeps it: `stamped_for_save` (new `modified_at`, an id and creation time when missing, sorted tags, everything else unchanged), `iso8601_utc_from_epoch_ms` and `uuid_v4_from_bytes`; the codec reads no clock, so the page supplies both |
| `guide` | The guided walkthrough's tab-session state, and `WEB_WORDING`, the few walkthrough lines reworded for the browser (a test pins that each still exists in the shared steps) |
| `hdr` | The browser HDR limits (64 MiB, 8192 x 4096) and the render-Worker count rule (never downsample; fewer render Workers instead), which counts the analysis Worker's copy of the map against the 1.5 GiB budget |
| `worker` | `WorkerHandler`, the whole Worker state machine; `apps/indicatrix-web-compute` only moves bytes in and out of it |
| `host` (wasm32) | `WorkerPool` = `RenderPool` (N render Workers + a picture Worker created on first use) + `SolveClient` (1 solve Worker) + an analysis `SolveClient` created on first use |
| `solve_error` | `SolveError` (superseded, cancelled, failed): the typed reason a solve-role job produced no answer |

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
- Every Worker runs the same script, `indicatrix-web-compute_loader.js`, which Trunk
  writes next to the app (`WORKER_LOADER_URL`). The page creates the pool only when a
  design is shown on the Render tab or a solve needs the solve Worker.

## Bit-identity

A render Worker traces with `renderer::cpu_frame::trace_pixels_interleaved`, the same
thread-free core the desktop's CPU renderer runs per thread. The accumulator merges a
pass only when every partition has delivered it, in pass order, so the running sum is
bit-identical to the desktop's `cpu_accumulate` called once per pass -- on the same
target. The code path is the same in the browser, but bitwise identity holds only on the
same target: wasm and native take their transcendental functions (`powf`, `sin`, ...)
from different libms. The tests run natively and check this against `cpu_accumulate`
itself; no test runs the tracer on wasm.
