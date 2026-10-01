# indicatrix-cut — remote rendering: preview-then-handoff

How the viewer decides between local CPU rendering and handing a render off to its
configured remote, and how denoising and the TLS connection are handled across that
handoff. For everything else see [the README](../README.md); for the
`RemoteEndpoint` configuration itself see [settings.md](settings.md).

## One remote endpoint

The viewer talks to exactly **one** remote, `AppSettings::remote`: a coordinator
(`indicatrix-worker serve`, which spreads work over the workers that `join` it and
renders itself only with `--render` — with `--render` and no joined workers it
behaves exactly like the single worker of earlier versions).
Every feature -- live view, still export, tilt video, batch preview, batch tilt,
library browse and mirror -- uses that endpoint; there is no worker list and no
viewer-side load balancing. A settings file from before this change migrates on load:
the first entry of its old `remote_workers` list becomes the endpoint (it was the only
one any feature used) and the dropped entries are logged. "Served by" names a
coordinator as `coordinator (N workers)`; a mid-connection `CAPABILITY_CHANGED` (a
worker joined or left) updates both that label and the persistent connection's cached
capability. A coordinator that loses every lane ends a stream with the
`ALL_WORKERS_LOST` error ("All remote workers were lost"), handled exactly like any
other remote failure below.

## Transfer: full data or final picture

- **Full data** (default): float radiance, merged with local samples -- everything
  below.
- **Final picture only**, still export and tilt video (export dialog / video section
  "Transfer", default from the endpoint): one `FinalImageRequest` per image or frame.
  The remote renders the whole sample budget, tone-maps it with the SAME
  `indicatrix::renderer::tonemap::tonemap_accumulation` the viewer uses, and returns a
  lossless PNG; the viewer decodes it to RGBA8 and writes it through its own PNG writer,
  so ICC embedding is identical to a local export of that RGBA. Local lanes do not
  normally take part; progress comes from the remote's `PROGRESS` heartbeats; cancel
  sends `CANCEL`. **v16 exception:** when `AppSettings::contribute_to_final_picture` is
  on (the default) and the export's `ComputeTarget` is `Both`, `worker::final_picture::contribution_allowed`
  lets `final_picture` fork instead: `FinalImageRequest.viewer_samples` reserves a tail
  of the sample budget for the viewer, whose local lanes trace that tail on a scoped
  thread while the remote traces the rest, uploading the sum as `-> CONTRIBUTION`
  for the coordinator to fold in before tone-mapping. If the contribution doesn't
  arrive within the coordinator's wait (or is invalid), the coordinator silently
  renders that tail itself and reports how many samples it had to take back in
  `Stats.reclaimed_samples` — a slow or interrupted local machine never stalls or
  corrupts the export. Every current coordinator implements Final picture only (with
  or without joined workers). `UNSUPPORTED_REQUEST` (a remote that does not) falls back
  to full data with one note and is remembered for that remote until it is re-saved;
  any other failure falls back to full data under Local + Remote and fails the image
  under Remote only.
- **Final picture**, live view ("Live Transfer" in the settings dialog): the settled
  epoch sends one `RenderRequest` with `TransferMode::DisplayOnly` over the whole
  budget; the remote streams finished, denoised 8-bit `DISPLAY_FRAME`s, which the viewer
  decodes and shows as they are (no local denoise or tonemap). The coordinator makes
  them with the viewer's own pipeline — `indicatrix::renderer::frame_denoise` over the
  merged sum's average, guide buffers from its own `indicatrix::renderer::guide_pass`
  prepass — so they match what the viewer would have shown for the same samples. 8-bit frames cannot be
  merged with local samples, so the epoch is remote-only: local tracing pauses after
  the handoff exactly like Remote only (`RenderContext::live_display_only` makes a
  `Both` epoch act as `RemoteOnly`). Drag, cancel and epoch rules are unchanged.
  `UNSUPPORTED_REQUEST` releases the epoch, shows one note, and the view re-dispatches
  with full data; the refusal is remembered until the connection is replaced.

## Full-data handoff

While the camera or light is moving, rendering is ordinary local CPU progressive
accumulation — nothing remote-specific happens. A repeating 100ms timer polls
the current camera/light pose; once it has been unchanged for a 600ms debounce
window *and* a remote is configured, the app hands off:

1. **The drag-time local preview is discarded, never summed into the settled
   image.** A new image *epoch* starts: one shared sample cursor over
   `[0, Target Samples)` plus the remote side's merged sums. Nothing of one epoch
   is ever carried into the next.
2. **Local + Remote (the default): both keep contributing.** The remote lane
   claims chunks from the epoch's cursor, each sized to about 1.5 s of the
   worker's measured rate, and sends one `RenderRequest{first_sample, samples}`
   per chunk on the persistent connection, requesting the next one when the
   previous chunk's `DONE` arrives. The local tracer claims its per-frame range
   from the same cursor, so no absolute sample index is ever traced twice. The
   displayed image is `(local sum + remote sum + in-flight chunk) / (their
   counts)`, denoised once. The epoch is complete once the combined count
   reaches **Target Samples** -- the single global target; there is no separate
   remote sample budget. **Remote only** pauses local tracing and lets the
   remote lane trace the whole epoch.
3. **Failure keeps what arrived.** A chunk that fails (worker error, dropped
   connection, liveness timeout) keeps its valid prefix; the untraced remainder
   goes back to the local tracer (or, in Remote only, is retried remotely).
   After two failures in a row the lane stops for that epoch with one status
   note -- Local + Remote finishes the image locally; Remote only falls back to
   local rendering as before. The next settle tries the worker again.
4. If the user resumes dragging the camera mid-epoch, the symmetric thing
   happens: a `CANCEL` is sent for the in-flight chunk, the epoch (remote sums
   included) is released, and local previewing resumes from a clean buffer. A
   hidden viewport only pauses the lane between chunks; it resumes when the
   viewport is visible again.
   Any other release -- a scene or settings change while settled, a
   Live Compute change -- re-dispatches a fresh epoch on its own once the
   scene has been stable for the settle debounce, without waiting for a drag
   (never for a scene whose remote lane already gave up). Each epoch is
   stamped with the scene generation it was dispatched for; the render loop
   releases an epoch whose scene no longer matches the one it traces, so a
   change racing the dispatch can never mix two scenes into one image.
5. **HDR environments render remotely only on an HDR-capable remote.** A
   loaded map travels by content hash (`SceneEnvironment::Hdr`); a remote whose
   `RenderCapability::hdr` is set asks for the bytes once (`NEED_ASSET`, answered
   from `bridge::remote::hdr_asset`) and decodes them with the viewer's own
   builder, so its samples are lit identically. A map with no source file, or
   one over the protocol's 256 MiB asset limit, is never sent, and a remote
   without HDR support never gets an HDR scene: the live view then renders
   locally (one status note), exactly like exports do -- see
   `bridge::remote::guard`, the one rule every remote path asks.

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
needs, so those are regenerated locally with a cheap primary-ray-only prepass —
`indicatrix::renderer::guide_pass`, cached by `bridge::frame_cache::guide_pass` —
before denoising a remote frame). The denoise + tone-map step itself is
`indicatrix::renderer::frame_denoise`, shared with the coordinator's "final
picture" display frames.

**One non-finite rule on every backend.** A traced sample with any NaN or ±Inf
component is dropped but still counted (`indicatrix::optics::raytracer::add_finite_sample`,
mirrored in the GPU reduction), on the CPU, the GPU, the export and every remote
alike. Per-pixel sums from different backends therefore merge by plain addition
with the total sample count as the divisor, whichever backend hit a bad sample.

**Payload compression.** Radiance payloads between viewer and remote are
compressed losslessly when both sides support it (byte shuffle + zstd by default,
negotiated in the handshake; raw on loopback or when it would not shrink), so a
compressed delta sums bit-identically to the raw one.

**The mutual-TLS connection is owned by one thread for its whole lifetime.**
TLS record state isn't safely readable and writable from two threads
concurrently the way a plaintext socket split would be, so
`bridge::remote::remote_render` alternates, on one thread, between a short
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
  legitimately take longer than the steady-state deadline below. Without this,
  a scenario like 4K with a worker cadence of 20 samples/tick could incorrectly
  report the connection as silent while the worker is genuinely still computing.
- A tighter, worker-heartbeat-derived **steady-state timeout** (8s — four
  times the worker's own guaranteed heartbeat interval of at least one
  `PROGRESS` every 2 seconds, regardless of cadence) applies to every wait
  after that first event.

A separate potential concern would be a thread that owns the connection's
liveness clock doing unrelated CPU-heavy work (a local render tail, denoising,
tone-mapping, PNG encoding) between reads. The connection-owning thread avoids
this: it never does anything but read, apply, and forward; every genuinely
expensive step runs on a different thread (or, on the UI thread, is deferred
via `Weak::upgrade_in_event_loop` rather than run inline). The liveness deadline
is therefore only ever checked immediately after a real, just-attempted, empty
read — see `bridge::remote::remote_render`'s module doc and the "Timeouts and
liveness" and "Why a busy consumer can never make either deadline fire early"
sections of `bridge::remote::remote_render::connection`'s `POLL_INTERVAL` doc
comment for the full reasoning, constants, and the `liveness_deadline` decision
that picks between the two tiers.

When the deadline does fire, the failure is never silent: the chunk is
handled like any other failed chunk (point 3 above) -- its prefix kept, its
remainder handed back -- and if it was the second failure in a row a note or
toast reports it (including how long the connection had been silent).
