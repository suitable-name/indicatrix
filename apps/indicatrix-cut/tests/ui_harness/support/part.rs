//! The part of a click-everything pass the running piece is limited to.
//!
//! A pass over a big state (the library with data, the Rough Planner's result) is one scenario,
//! and a parallel run cannot split it. A piece registered with `Piece::part(index, of)` runs the
//! same pass in `of` pieces, each pressing only the controls whose key hashes into its part. The
//! hash is FNV-1a over the control key, so the split is the same in every worker process and
//! every control (all occurrences of a key) lands in exactly one part.
//!
//! The runner stores the part before the piece runs; the hosts of the pass (`ctx.rs`,
//! `secondary.rs`) ask [`admits`] from `SmokeHost::admits`.

use std::cell::Cell;
use uiprobe::view::Control;

thread_local! {
    static PART: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

/// Sets (or clears) the part of the piece that is about to run.
pub fn set(part: Option<(usize, usize)>) {
    PART.with(|cell| cell.set(part));
}

/// FNV-1a, 64 bit.
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Whether the running piece presses `control`: always without a part, else when the control's
/// key falls into the piece's part.
#[must_use]
pub fn admits(control: &Control) -> bool {
    let Some((index, of)) = PART.with(Cell::get) else {
        return true;
    };
    let of = u64::try_from(of).unwrap_or(1).max(1);
    fnv1a(&control.key()) % of == u64::try_from(index).unwrap_or(0)
}
