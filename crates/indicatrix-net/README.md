# indicatrix-net

Wire protocol between a viewer and a remote server: **offloading `indicatrix` spectral
ray-sample computation**, and **reading a remote design library**. Both run over one
authenticated connection, and a server may offer either or both — `WELCOME` says
which.

**Types, codec, and framing only — no networking, no sockets.** Every function in
this crate operates on an in-memory buffer, a `[u8]` slice, or a generic
`Read`/`Write`. The whole crate is testable with nothing more than a
`std::io::Cursor`, and every test in it does exactly that. `apps/indicatrix-worker`
(the server side) and `apps/indicatrix-cut`'s `bridge::remote_render` /
`bridge::library_client` (the client side) are what wire these functions to a real
`TcpStream`/TLS connection.

| Module | What it carries |
|---|---|
| `messages` | The render protocol, `HELLO`/`WELCOME`, and the `ClientMessage` envelope |
| `library` | The read-only design-library protocol |
| `scene` | `SceneState` — a fully-resolved scene (`render` feature) |
| `radiance` | The raw radiance-buffer encoding |
| `framing` / `handshake` | Length-prefixed framing; the version and build-hash gate |
| `tls` | Mutual-TLS config, the fingerprint allowlist, restricted-permission key writes |
| `enroll` / `token` | One-time enrollment tokens and the CA-pinning claim client |
| `client` | Helpers for driving a connection from the viewer's side |

## Why the split works

`indicatrix::optics::raytracer::trace_spectral_ray` returns one sample's XYZ radiance,
and a viewer's accumulation buffer is just `Vec<Vec3>` of running per-pixel sums.
Samples are additive and order-independent, so a remote node's contribution is
nothing more than more terms in that same sum — there is no partial-frame
reassembly or per-tile stitching to get right.

Work is partitioned **by sample index** (every node traces a disjoint range of
sample numbers across the *whole* frame), not by screen-space tile: a gem only
occupies part of the frame and background pixels are nearly free to trace, so tile
partitioning would load-balance badly, while sample partitioning divides the work
evenly by construction. This only works because the per-sample RNG seed is a pure
function of `(pixel_index, sample_number)` — never of which batch a sample happens
to land in, or how many samples are in that batch. `crates/indicatrix-net/tests/partition_correctness.rs`
reproduces the exact seed formula from `apps/indicatrix-cut/src/bridge/render_thread.rs`
and `apps/indicatrix-worker/src/render_core.rs` and proves additivity end to end
against the real `trace_spectral_ray`: tracing samples `[0,64)` in one batch sums
to (within float-rounding tolerance) the same radiance as tracing `[0,32)` and
`[32,64)` separately and adding the results.

**Samples are summed, never averaged**, anywhere in this crate or its wire
format. Normalizing a sum into a displayable average (dividing by total sample
count) is a client-side, display-time operation — the wire payload is always a raw
radiance sum, which is what makes it valid to keep adding more nodes' contributions
into the same buffer without knowing in advance how many total samples there will
be.

## Protocol version and message set

`messages::PROTOCOL_VERSION: u16 = 9`. Three protocols share one authenticated
connection: **render** (offload sample tracing), **tilt curves** (offload one design's
full tilt-performance sweep), and **library** (read a design catalogue). A peer may
serve any subset, and — for a viewer — consume all three.

```text
-> HELLO    { protocol_version, build_hash }
<- WELCOME  { protocol_version, build_hash, render: Option<RenderCapability>, library: bool, tilt_curves: bool }
                                  RenderCapability { backend, max_pixels, min_cadence_ms }
```

`WELCOME` is where a peer says what it can actually do. **Check it before sending
anything**: `render` is `None` on a library-only server (one built without
`indicatrix-worker`'s `worker` feature), `tilt_curves` is `false` on that same server (it
tracks `render.is_some()` in this phase -- see `messages::Welcome::tilt_curves`'s own
doc comment), and `library` is `false` on a server that only renders. Discovering the
absence by having a request rejected is the wrong shape — `bridge::remote_render`'s
`NoRenderCapacity` and `bridge::library_client`'s `NoLibraryCapacity` are the
client-side counterparts.

**Render** (gated behind this crate's `render` feature — see below):

```text
-> RENDER   { request_id, scene: SceneState, first_sample, samples, stream: StreamConfig }
<- FRAME    { request_id, first_sample, samples, payload_len, xyz_bytes }        -- DELTA, full-res
<- PREVIEW  { request_id, width, height, samples_done, payload_len, xyz_bytes }  -- CUMULATIVE, reduced-res
<- PROGRESS { request_id, samples_done }
-> CANCEL   { request_id }
<- DONE     { request_id, cancelled, stats: Stats }
<- ERROR    { code, message }
```

**Tilt curves** (gated behind this crate's `render` feature, same as render — see
below): a single request, a single reply, never a `StreamEvent` stream. One design's
full brilliance/extinction/windowing sweep across 4 axes x 181 tilt points (8,688 bytes,
measured ~1.36s to compute) is small and fast enough that `RENDER`'s progressive
delta/preview/cadence machinery buys nothing — see `messages::tilt`'s module doc
comment.

```text
-> TILT_CURVES  { request_id, scene: SceneState }         -- only scene.planes/material/light_yaw/light_pitch matter
<- <TiltCurvesResponse>
     Curves    { request_id, axes: [AxisTiltCurves; 4] }  -- brilliance_pct/extinction_pct/windowing_pct: Vec<f32>, 181 points each
     Cancelled { request_id }
     Error     { code, message }                          -- same ErrorMsg shape and codes RENDER's own validation/panic paths use
```

**Library** (always available, `library::LibraryRequest`/`LibraryResponse`):

```text
-> Search       { filters }              <- SearchResults(Vec<DesignSummary>)
-> SearchPage   { filters, after_id }    <- SearchResultsPage { summaries, next_after_id }
-> FilterOptions                          <- FilterOptions { shapes, gears, ranges }
-> FetchDesign  { entry_id }             <- Design(DesignRecord)   -- metadata, not bytes
-> FetchAttachment { attachment_id }     <- Attachment { bytes }
                                          <- NotFound | Error
```

`SearchPage` is the one to use for anything that must see the whole catalogue:
`Search` caps at 1000 rows, so a mirror built on it silently stops there. Paging is
a **keyset cursor** (`after_id`), not an offset — `diagram_entries.id` is
`AUTOINCREMENT`, so `ORDER BY id ASC` is a total order and a walk cannot skip or
duplicate rows when something is inserted mid-sync.

Both protocols run over one connection and one handshake. A client doing many
requests should hold the stream open rather than reconnecting per request — a full
mutual-TLS handshake per `FetchDesign` turns a 3,000-design mirror into 3,000
handshakes.

### The `render` feature

`indicatrix-net` depends on `indicatrix` **only** behind its `render` feature, because
`SceneState`, `RenderRequest`, and `TiltCurvesRequest`/`TiltCurvesResponse` all need
`indicatrix`'s resolved scene and material types (or, for the response, `indicatrix`'s tilt-curve
sweep shape). Off by default, so a library-only `indicatrix-worker` build never compiles the
renderer in at all. A viewer enables it — it is a render client.

One consequence worth knowing: `ClientMessage::RenderRequest` and
`ClientMessage::TiltCurvesRequest` exist only under that feature, and it is a *different
crate's* flag from `indicatrix-worker`'s `worker`. Cargo unifies features across a workspace
build, so a library-only worker can be compiled with either variant in scope.
`indicatrix-worker`'s dispatch handles that as a runtime case rather than a compile-time
impossibility — a peer can always send one regardless of what `WELCOME` advertised (see
`apps/indicatrix-worker/src/serve/connection.rs`'s `handle_non_render_message` for exactly
how, and why it can only answer both with the same `StreamEvent::Error` shape in that
one narrow fallback path).

### When the version must be bumped

**This is pre-release software: every peer runs its own private build, so there is no
deployed fleet to stay wire-compatible with.** There is no compatibility shim between
versions — `handshake::verify_compatible` simply refuses to pair peers speaking
different ones; see `messages::PROTOCOL_VERSION`'s own doc comment for the full version
history (v1 initial, v2 widened `RangeFilterWire`, v3 added `TILT_CURVES`, v4 widened
`library::DesignSummary`/`LibraryResponse::SearchResults`/`SearchResultsPage` so
remote-browsed catalogue rows carry `ignored` and a missing-tilt-curves exclusion
count, v5 widened `library::DesignRecord` with `preview_material`, v6 widened
`library::DesignRecord` again with `hw_ratio`/`tw_ratio`/`uw_ratio`/`pw_ratio`/
`cw_ratio`/`symmetry_order`/`mirror_symmetry`/`designer` so a remote design's detail
chips stop blanking out, v7 widened `GemMaterial` with `absorption_path_scale` and
documents `StreamEvent::Progress` as doubling as this stream's liveness heartbeat).

What matters is knowing when to bump it, and that follows entirely from postcard
being **not self-describing**:

- **Enums are encoded by declaration-order index, not by name.** Appending a variant
  leaves every existing message byte-identical, but a peer that predates the new
  variant sees an unrecognized index and fails with an opaque per-message decode
  error. It cannot skip what it does not recognize.
- **Structs are encoded as their fields in declaration order**, with no field names
  and no length prefix. Appending a field is worse than an unknown variant: the other
  peer silently *misaligns* every field after it instead of failing. This applies
  transitively — a field appended to anything embedded in `RenderRequest`, such as
  `SceneState` or the `GemMaterial` inside it, changes the layout of every message
  carrying it.

So append a variant to a wire enum, or a field to a wire struct or anything it
embeds, and bump. The bump is what converts an opaque decode failure (or worse,
silently misread data) into a diagnosable refusal at the handshake.

`#[serde(default)]` is not a substitute. It matters for self-describing formats —
`indicatrix-worker`'s local `scene.json` files, which never cross this protocol — and does
nothing at all for postcard's fixed-layout encoding.

### Tagged envelopes, and why they exist

A fixed reply shape — one `RENDER` producing exactly one `FRAME` or `ERROR` — would
need no tag at all. This protocol does: one `RENDER` produces a variable-length,
*interleaved* sequence of `FRAME`/`PREVIEW`/`PROGRESS` messages before a terminal
`DONE`, and a trace panic can happen mid-stream, after some of those have already
gone out, not only as the very first reply. Without a tag, a reader has no way to
tell "the next message is a `FRAME`" from "the next message is actually an `ERROR`,
because tracing just panicked after streaming three frames already" other than
guessing.
`messages::StreamEvent` is that tag for the reply direction.

The request direction has the same problem once a client is allowed to pipeline its
next `RenderRequest` without first waiting for `DONE` on the one currently
streaming: whatever bytes arrive next could be either a `CANCEL` for the in-flight
request or the next `RenderRequest`. `messages::ClientMessage` is the mirror-image
tag for that direction (`RenderRequest` is boxed inside it purely to keep the
enum's stack footprint small, since most `ClientMessage`s are `Cancel`).

## `FRAME` (delta) vs `PREVIEW` (cumulative) — read this before touching either

This distinction is load-bearing for correctness, not a naming choice:

- **`FRAME` carries a delta** — the summed contribution of exactly the sample
  sub-range named by `FrameHeader::first_sample`/`FrameHeader::samples`, at full
  resolution. A client's `Accumulator` **sums** every `FRAME` it receives (whose
  `request_id` matches its current epoch) straight into its running total. This is
  what makes deltas coalesce losslessly under backpressure: two adjacent,
  not-yet-sent deltas sum to one delta over their union, and the sum is identical
  whether it went out as one `FRAME` or several.
- **`PREVIEW` carries a cumulative, reduced-resolution snapshot** — the *full*
  running total so far (not a delta), downsampled to `PreviewHeader::width` x
  `PreviewHeader::height`. Each `PREVIEW` **replaces** the previous one; a client
  never sums two `PREVIEW`s together, and never sums a `PREVIEW` into the
  full-resolution accumulator — a reduced-resolution buffer is not additive with a
  full-resolution one under any arithmetic. This is also what makes `PREVIEW`
  freely droppable under backpressure: losing one just means the next one (which
  already reflects everything the dropped one did, plus more) arrives instead.

`client::accumulate::Accumulator::apply` is the one place this rule is
implemented: a `StreamEvent::Frame` is summed elementwise into `buffer`; a
`StreamEvent::Preview` outright replaces `last_preview`. A test
(`preview_snapshots_replace_rather_than_sum_and_never_touch_the_frame_buffer`)
sends preview values 10.0 then 20.0 and asserts the stored preview is `20.0`, not
`30.0`, and that a separately-summed `FRAME` buffer is untouched by either.

## `request_id` and cancellation epochs

Every message from `RENDER` onward — `FrameHeader`, `PreviewHeader`, `Progress`,
`Cancel`, `Done` — echoes the `request_id` the client chose for that `RENDER`. A
`CANCEL` can be in flight past a worker that's already mid-batch, so `FRAME`/`PREVIEW`
payloads for the just-cancelled request may still be in transit when the client
moves on to its next request. The rule that makes "never merge a stale partial into
the next render" mechanical, rather than a race:

> Honor/sum/display a payload iff its `request_id` matches the accumulator's
> **current epoch**; drop everything else.

`Accumulator::begin_request(request_id)` starts a new epoch: it zeroes the buffer,
resets the sample count, and clears any pending preview. Callers **must** call it
before or immediately after sending the next `RenderRequest` — before any reply for
it can possibly arrive — so that bytes for the old epoch still in flight see the
new epoch already in place and get dropped by `apply`. `StreamEvent::Error` is the
one message with no `request_id`; it is never epoch-gated.

`send_cancel` itself does not touch the accumulator — it just writes the `CANCEL`
message. The accumulator naturally stops accepting further payload for the
cancelled request the next time `begin_request` is called for a different id.

## The `BUILD_ID` handshake gate

`handshake::verify_compatible(local: &Hello, remote: &Hello) -> Result<(), Incompatible>`
refuses to pair a viewer and a worker whose `indicatrix::BUILD_ID` (a content hash of
`indicatrix`'s own source — see `indicatrix`'s README) or `PROTOCOL_VERSION` disagree. There
is deliberately no "close enough" tier: any mismatch anywhere means refuse, full
stop, including two peers that both report an `UNKNOWN_BUILD_HASH` (i.e. two
unknown builds are never treated as compatible even with each other).

Why this can't be relaxed: the whole remote-offload design rests on
`sample_sum += trace_spectral_ray(..)` being valid regardless of which node
computed which term, and that is only true if every node runs the *same* physics.
Two builds that differ by, say, a spectral-MIS weighting fix produce numbers that
both look like plausible radiance — there is no runtime signal (no NaN, no panic,
no obviously-wrong magnitude) that distinguishes "two different physics
implementations summed together" from "a converged render." `indicatrix`'s physics has
already changed many times in quick succession during development, which is
exactly the situation a hand-maintained version number can't catch.

The client re-runs `verify_compatible` itself against the worker's `WELCOME` reply
even though the worker already refused a bad `HELLO` on its own side — two
independent checks, neither trusting the other, for the same reason: there is no
runtime signal that would let either side detect the mismatch on its own if the
other side's check silently failed.

## TLS — a different question from the `BUILD_ID` handshake

`tls::server_config`/`tls::client_config` build a TLS **1.3-only** `rustls`
config (`tls12` is deliberately excluded from the workspace's `rustls` feature
list — this workspace never has a reason to negotiate down). The server config
requires a client certificate (`WebPkiClientVerifier`) — mutual TLS is what
replaces a password in this design.

CA-chain validity only proves "this client certificate was signed by a CA I
trust" — it says nothing about *which* signed client should be trusted.
`tls::Allowlist` closes that gap: a plain-text file of SHA-256 client-certificate
fingerprints (`tls::fingerprint`), one 64-hex-char line per line, `#` comments
allowed. `apps/indicatrix-worker`'s `serve` command checks a connecting client's
fingerprint against it after the TLS handshake completes, and re-reads the file on
every connection (no restart needed to revoke access — delete the line). A
malformed non-comment, non-blank line is a hard load error (fail-closed on
purpose) rather than being silently skipped.

TLS and the handshake module answer two different questions and neither
substitutes for the other: TLS+allowlist answers "may this peer talk to me at
all"; `handshake::verify_compatible` answers "are we running the same ray-tracing
physics." A peer can pass one and fail the other.

## Public API tour

### `scene::SceneState` — a fully-resolved scene, never a name or an id

```rust
pub struct SceneState {
    pub width: u32,
    pub height: u32,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub light_yaw: f32,
    pub light_pitch: f32,
    pub exposure: f32,
    pub max_bounces: u32,
    pub lighting_preset: LightingPreset,
    pub material: GemMaterial,
    pub planes: Vec<GpuFacetPlane>,
}
```

A viewer stores a material as a name plus a list of custom materials loaded from
its own local SQLite database, and stores a diagram as an id looked up against
`indicatrix-vault`. A remote worker has neither. Sending the *name* or the *id*
instead of the resolved value would compile, serialize, and deserialize just fine
— and then silently render the wrong stone (or the wrong facet geometry) the
moment someone selects a custom material or a diagram the worker's own copy of the
data doesn't happen to agree with, with no error at all. So `SceneState` always
carries the fully-resolved `GemMaterial` and facet-plane geometry, never a name or
an id. It also deliberately excludes local UI/session state (`dirty`, `paused`,
`target_samples`, ...) — none of it changes what a worker computes, since the
viewer already resolves its own accumulation target down to the concrete sample
count it asks for.

`SceneState` round-trips exactly through `postcard` (verified in
`tests/scene_roundtrip.rs` against a material with absorption bands and biaxial
data, every `LightingPreset`, a diamond with empty absorption, and an empty plane
set) — this is exactly the kind of fidelity a name/id-based scheme would have
silently lost.

### Sending a render request

```rust
use indicatrix_net::{client, messages::{RenderRequest, StreamConfig, TransferMode}};
use std::io::{Read, Write};

fn send_one_request<S: Read + Write>(
    stream: &mut S,
    accumulator: &mut client::Accumulator,
    scene: indicatrix_net::SceneState,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Handshake -- refuses to proceed on a protocol or BUILD_ID mismatch.
    let welcome = client::handshake(stream)?;
    println!("worker backend: {:?}, build {:x?}", welcome.backend, welcome.build_hash);

    // 2. Send a render request and start a new accumulation epoch for it.
    let request = RenderRequest {
        request_id: 1,
        scene,
        first_sample: 0,
        samples: 256,
        stream: StreamConfig {
            transfer_mode: TransferMode::LiveProgressive,
            cadence_ms: 250,
            preview: None,
        },
    };
    client::send_render_request(stream, &request)?;
    accumulator.begin_request(request.request_id); // before/immediately after sending

    // 3. Drain the reply stream, folding FRAME deltas and PREVIEW snapshots in.
    client::run_client_session(stream, accumulator, |update| {
        println!("{update:?}");
    })?;
    Ok(())
}
```

`client::test_connection` performs step 1 only (handshake, report worker identity/
backend/build compatibility, then the caller disconnects) — this is what a "Test
connection" button in a settings UI should call; no `RenderRequest` is ever sent.

`client::send_cancel(writer, request_id)` writes a `CANCEL` for a request already
in flight; it does not touch the accumulator (the next `begin_request` call is what
actually starts ignoring the cancelled epoch's stragglers). Everything here is
generic over `Read`/`Write` — wrapping an actual `TcpStream` (optionally inside
`rustls::StreamOwned`, built via `tls::client_config`) happens at the call site;
see `apps/indicatrix-worker/src/serve.rs` for the server side of the same split, and
`apps/indicatrix-cut/src/bridge/remote_render.rs` for the client side.

### Reading the radiance buffer

```rust
use indicatrix_net::radiance;
use glam::Vec3;

let buffer: Vec<Vec3> = vec![Vec3::new(1.0, 2.0, 3.0); 64 * 64];
let bytes = radiance::encode(&buffer);          // zero-copy POD view via bytemuck
let decoded = radiance::decode(&bytes, 64 * 64)?;
assert_eq!(buffer, decoded);
# Ok::<(), indicatrix_net::radiance::RadianceError>(())
```

The radiance payload is raw POD `Vec3` bytes (`bytemuck::cast_slice`), never a
serialization framework — this is the hot-path payload (up to ~100 MiB for a 4K
frame), and `postcard` framing overhead has no reason to touch it.

## Key invariants (do not "fix" these)

- **Samples are summed, never averaged**, anywhere on the wire.
- **`FRAME` is additive; `PREVIEW` is not.** Never sum two previews, and never sum
  a preview into the full-resolution buffer.
- **A payload's `request_id` must match the accumulator's current epoch** or it is
  dropped, unconditionally — this is the only defense against a stale, in-flight
  reply from a just-cancelled request being merged into the next one.
- **A `BUILD_ID`/protocol mismatch is always refused**, never downgraded or
  warned-and-continued.
- Every message from `RENDER` onward is tagged (`StreamEvent` / `ClientMessage`) —
  do not add a new untagged reply type; that reintroduces exactly the ambiguity the
  tagging exists to remove (see "Tagged envelopes, and why they exist" above).

## Testing

```
cargo test -p indicatrix-net
```

No networking or sockets are exercised — every test runs against a `std::io::Cursor`
or an in-memory `Vec<u8>`, consistent with the crate's "types, codec, and framing
only" scope. This includes:

- `tests/scene_roundtrip.rs` — `SceneState` round-trips exactly through `postcard`.
- `tests/partition_correctness.rs` — reproduces the viewer's/worker's real per-sample
  seed formula and proves sample-range partitioning is additive against the real
  `trace_spectral_ray` (batch `[0,64)` equals batch `[0,32)` + batch `[32,64)`, an
  uneven three-way split, and single-sample batches summed one at a time), using a
  `1e-4` relative tolerance rather than exact equality since float addition is not
  associative — the test is there to catch a *real* discrepancy (a seed depending on
  batch-relative state, a dropped sample), not float-rounding noise.
- Extensive inline `#[cfg(test)]` unit tests in every `src/*.rs` module covering
  round-trips, malformed-payload rejection, epoch-gating, and partial/dribbling
  reads.
