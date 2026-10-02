//! The browser's clock and random source for the design file's `[meta]` table.
//!
//! The file codec reads neither, so the page supplies them: `Date.now()` for the
//! modification time and `crypto.getRandomValues` for a design id. The text shapes are
//! `indicatrix_web_core::design_meta`'s, where they are tested natively.

use super::state::DesignState;
use indicatrix_cut_core::native::DesignMetadata;
use indicatrix_web_core::design_meta::{iso8601_utc_from_epoch_ms, stamped_for_save};

/// Now as ISO-8601 UTC text (`2026-10-02T09:30:00Z`).
fn now_iso() -> String {
    iso8601_utc_from_epoch_ms(js_sys::Date::now())
}

/// 16 random bytes: `crypto.getRandomValues` where the page has it, `Math.random`
/// otherwise (an id needs to be unlikely to collide, not secret).
fn random_bytes() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    let filled = web_sys::window()
        .and_then(|window| window.crypto().ok())
        .is_some_and(|crypto| crypto.get_random_values_with_u8_array(&mut bytes).is_ok());
    if !filled {
        for byte in &mut bytes {
            *byte = (js_sys::Math::random() * 256.0) as u8;
        }
    }
    bytes
}

/// The metadata a save of `design` writes: what the design was opened with, stamped
/// with the current time, an id when it had none, and sorted tags.
#[must_use]
pub fn stamped_metadata(design: &DesignState) -> DesignMetadata {
    stamped_for_save(&design.metadata, &now_iso(), random_bytes)
}
