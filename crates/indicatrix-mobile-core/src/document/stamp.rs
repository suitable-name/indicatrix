//! The clock and the random source for a design file's `[meta]` table.
//!
//! The file codec reads neither, so the app supplies both: the system clock for the
//! modification time and the standard library's randomly seeded hasher for a design id
//! (an id needs to be unlikely to collide, not secret). The text shapes are
//! `indicatrix_web_core::design_meta`'s, where they are tested.

use std::{
    hash::{BuildHasher, Hasher, RandomState},
    time::{SystemTime, UNIX_EPOCH},
};

use indicatrix_cut_core::native::DesignMetadata;
use indicatrix_web_core::design_meta::{iso8601_utc_from_epoch_ms, stamped_for_save};

/// Now as ISO-8601 UTC text (`2026-10-02T09:30:00Z`).
fn now_iso() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0);
    iso8601_utc_from_epoch_ms(millis)
}

/// 16 bytes from two independently seeded hashers fed with the clock and the process id.
fn random_bytes() -> [u8; 16] {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut bytes = [0_u8; 16];
    for chunk in bytes.chunks_mut(8) {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(nanos);
        hasher.write_u32(std::process::id());
        let word = hasher.finish().to_le_bytes();
        chunk.copy_from_slice(&word[..chunk.len()]);
    }
    bytes
}

/// The metadata a save writes: what the design was opened with, stamped with the
/// current time, an id when it had none, and sorted tags.
#[must_use]
pub(super) fn stamped(opened: &DesignMetadata) -> DesignMetadata {
    stamped_for_save(opened, &now_iso(), random_bytes)
}
