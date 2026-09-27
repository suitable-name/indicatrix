//! Sample-range and work-item scheduling for indicatrix render lanes.
//!
//! This crate is the scheduler a render coordinator runs: several backends ("lanes":
//! a joined remote worker, the coordinator's own CPU/GPU, anything implementing
//! [`WorkerLane`]) contribute disjoint absolute sample ranges to ONE image, and their
//! per-chunk radiance sums are merged into one buffer with an exact sample count.
//!
//! It holds no networking, no GUI and no GPU code. It depends only on the protocol
//! crate (for [`indicatrix_net::SceneState`]) and `glam`, so it can be linked into the
//! worker binary as well as the desktop app.
//!
//! # Why merging by plain addition is sound
//!
//! A path sample is identified by `(global_pixel_index, absolute_sample_index)` and
//! nothing else, on every backend. Any set of backends tracing **disjoint** sample
//! ranges of the **same scene at the same resolution** therefore merges by per-pixel
//! addition, divided by the total count. Disjointness is the whole correctness
//! guarantee, and [`SampleCursor`] provides it by construction.
//!
//! # Contents
//!
//! - [`SampleCursor`]: the atomic claim point handing out disjoint sample ranges.
//! - [`WorkerLane`], [`ChunkResult`], [`SampleRange`]: what a lane is and returns.
//! - [`RateModel`], [`ChunkPolicy`]: per-lane throughput calibration and chunk sizing.
//! - [`LanePool`]: N lanes against one cursor for one image epoch, with failure
//!   reclaim, backoff, retirement, cancellation and events.
//! - [`Merger`]: the deterministic, chunk-start-ordered merge of chunk sums.
//! - [`ItemQueue`]: whole-item distribution (batches) over N lanes.
//! - [`CancelToken`]: a cloneable cancellation flag.

mod cancel;
mod item_queue;
mod lane;
mod merge;
mod pool;
mod rate;
mod sample_cursor;

pub use cancel::CancelToken;
pub use item_queue::{FailOutcome, ItemQueue, ItemTicket, QueueCounts};
pub use lane::{ChunkResult, SampleRange, WorkerLane};
pub use merge::{MergeError, Merger};
pub use pool::{LanePool, MergerMismatch, PoolConfig, PoolEvent, PoolOutcome, PoolStatus};
pub use rate::{ChunkPolicy, DEFAULT_MAX_CHUNK_SAMPLES, RateModel, marginal_rate};
pub use sample_cursor::SampleCursor;
