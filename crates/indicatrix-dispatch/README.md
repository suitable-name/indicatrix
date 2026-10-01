# indicatrix-dispatch

Sample-range and work-item scheduling for indicatrix render lanes.

`indicatrix-dispatch` is the scheduler a render coordinator runs: several backends
("lanes" — a joined remote worker, the coordinator's own CPU/GPU, anything
implementing `WorkerLane`) contribute disjoint absolute sample ranges to **one**
image, and their per-chunk radiance sums are merged into one buffer with an exact
sample count. It holds no networking, no GUI and no GPU code; it depends only on
`indicatrix-net` (for `SceneState`) and `glam`, so both the worker binary and the
desktop app can link it.

## Why merging by plain addition is sound

A path sample is identified by `(global_pixel_index, absolute_sample_index)` and
nothing else, on every backend. Any set of backends tracing **disjoint** sample
ranges of the **same scene at the same resolution** therefore merges by per-pixel
addition, divided by the total count. Disjointness is the whole correctness
guarantee, and `SampleCursor` provides it by construction.

## Main types

- **`SampleCursor`** — the atomic claim point handing out disjoint sample ranges.
- **`WorkerLane`**, **`ChunkResult`**, **`SampleRange`** — what a lane is, what it
  is asked to trace and what it returns.
- **`RateModel`**, **`ChunkPolicy`** — per-lane throughput calibration and chunk
  sizing (`ChunkPolicy::fixed` for a timing-independent partition).
- **`LanePool`** / **`PoolConfig`** — N lanes against one cursor for one image epoch,
  with failure reclaim, backoff, retirement, cancellation and `PoolEvent`s.
- **`Merger`** — the deterministic merge of chunk sums (see below).
- **`CancelToken`** — a cloneable cancellation flag.

## Determinism

Chunks finish in whatever order the lanes finish them. `Merger` does not add them on
arrival; it folds them **in ascending `first_sample` order**, parking early arrivals
until the chunk at the frontier comes in. For a given partition of the range into
chunks, the merged buffer is therefore bit-identical no matter which lane traced which
chunk or in which order they finished. The partition itself depends on measured rates
unless `ChunkPolicy::fixed` is used (and no lane fails); different backends are never
bit-identical to each other anyway (GPU `fma` fusion).

## Who uses it

- **`apps/indicatrix-worker`'s coordinator** (`serve`) — fans a viewer's render or
  final-image request out as a `LanePool` job over joined workers plus its own lane,
  and emits the `Merger`'s result.
- **`apps/indicatrix-cut`** — its live render loop's sample cursor
  (`bridge::sample_cursor`) is this crate's `SampleCursor`.

## Testing

```
cargo test -p indicatrix-dispatch
```

Every test lives inline next to its module (`sample_cursor`, `rate`, `merge`,
`pool`); the pool tests drive fake lanes, so no network or GPU is needed.
