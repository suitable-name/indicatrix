# indicatrix-cut — remote rendering: preview-then-handoff

How the viewer decides between local CPU rendering and handing a render off to a
configured `indicatrix-worker`, and how denoising and the TLS connection are handled
across that handoff. For everything else see [the README](../README.md); for the
`WorkerSettings` configuration itself see [settings.md](settings.md).

While the camera or light is moving, rendering is ordinary local CPU progressive
accumulation — nothing remote-specific happens. A repeating 100ms timer polls
the current camera/light pose; once it has been unchanged for a 600ms debounce
window *and* a remote worker is configured, the app hands off:

1. **The local preview buffer is discarded, never summed into the remote
   result.** A discard action always precedes sending the render request to the
   worker — there is no code path that carries a buffer from one source into
   the other. This mirrors `indicatrix-net`'s `FRAME`-vs-`PREVIEW` distinction:
   mixing a local partial accumulation into a remote one would be exactly the
   kind of un-composable mixing that protocol is built to prevent.
2. Local rendering is suspended entirely (a remote worker now owns the
   displayed image) and the remote worker starts streaming `FRAME`/`PREVIEW`
   deltas back, accumulated the normal `indicatrix-net` way.
3. If the user resumes dragging the camera mid-settle or mid-remote-render, the
   symmetric thing happens: a `CANCEL` is sent, the remote partial accumulation
   is discarded (never salvaged into the resumed local preview), and local
   previewing resumes from a clean buffer.

**Denoising is applied once, to the final merged accumulation buffer — never
per-frame and never per-source.** The À-Trous denoiser runs over the
accumulation's running *average*, never over the raw running sum (feeding
filtered output back into a progressive estimator would bias it), and it's a
single toggle covering the whole image: denoising is a nonlinear operation, so
running it separately on a local partial and a remote partial and then
combining the results would not equal running it once on the true combined
total. The same merge-then-denoise code path is reused for both local-only and
remote-sourced frames (remote `FRAME`/`PREVIEW` payloads carry only XYZ
radiance, never the depth/normal/facet-id guide buffers the denoiser also
needs, so those are regenerated locally with a cheap primary-ray-only prepass
before denoising a remote frame).

**The mutual-TLS connection is owned by one thread for its whole lifetime.**
TLS record state isn't safely readable and writable from two threads
concurrently the way a plaintext socket split would be, so
`bridge::remote_render` alternates, on one thread, between a short
timeout-bounded read attempt and a non-blocking check of an inbound command
channel — mirroring `indicatrix-worker`'s own emitter design — so a `CANCEL` can be
written promptly without a second thread ever touching the same connection.

### Timeouts and liveness

Every blocking step has a deadline, so a worker that stops responding — a
crash mid-render, a NAT entry expiring, a laptop sleeping — is reported as a
failure rather than leaving the viewport frozen on a partial image forever.
Connect, handshake and every write are each bounded; on top of that, a
**liveness deadline** watches for a request going too long without a single
event (`FRAME`/`PREVIEW`/`PROGRESS`/`DONE`/`ERROR`) at all.

That liveness deadline is two-tiered, not one flat value:

- A generous **first-event timeout** (30s) applies to the wait for a
  dispatch's very first event. A worker's own calibration/warm-up — especially
  at a high resolution combined with a coarse, sample-count cadence — can
  legitimately take longer than the steady-state deadline below. This was
  confirmed as a real false positive: at 4K with a worker cadence of 20
  samples/tick, the worker's first tick could legitimately run past the
  steady-state deadline while genuinely still computing, and the connection
  was reported as silent out from under it.
- A tighter, worker-heartbeat-derived **steady-state timeout** (8s — four
  times the worker's own guaranteed heartbeat interval of at least one
  `PROGRESS` every 2 seconds, regardless of cadence) applies to every wait
  after that first event.

A separate concern — a thread that owns the connection's liveness clock doing
unrelated CPU-heavy work (a local render tail, denoising, tone-mapping, PNG
encoding) between reads, so the clock effectively keeps running while nobody
is listening to the socket — was investigated and ruled out for this
codebase's actual thread structure: the connection-owning thread never does
anything but read, apply, and forward; every genuinely expensive step runs on
a different thread (or, on the UI thread, is deferred via
`Weak::upgrade_in_event_loop` rather than run inline). The liveness deadline
is therefore only ever checked immediately after a real, just-attempted, empty
read — see `bridge::remote::remote_render::connection`'s own "Timeouts and
liveness" and "Why a busy consumer can never make either deadline fire early"
module doc sections for the full reasoning, constants, and the
`liveness_deadline` decision that picks between the two tiers.

When the deadline does fire, the failure is never silent: the live viewport's
`remote_active` flag is cleared so local progressive tracing resumes, and a
toast reports the failure (including how long the connection had been
silent).
