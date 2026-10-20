//! Deterministic Fresnel-weighted image tracer, independent of analytical metrics.
//!
//! Rays form a linear internal chain. Only concave geometry needs a bounded stack
//! for outside leaves that can enter another part of the stone. Output is summed
//! CIE XYZ, so existing accumulation and display code can consume it unchanged.

mod aa;
pub(crate) mod chain;
pub(crate) mod env;
mod frame;
pub(crate) mod fresnel;
mod frost;
mod prep;
mod tint;

pub use aa::aa_offset;
pub use env::WhittedEnv;
pub use frame::{whitted_pixels_interleaved_geom, whitted_primary_hit, whitted_tap_xyz};
pub use prep::{AlphaModel, Chain, WhittedPrep};

/// Increment when an intentional image change should invalidate cached drafts.
///
/// 2: the backdrop is a lit tray seen by every escape (primary misses included), not a
/// camera-only card.
pub const WHITTED_VERSION: u32 = 2;
pub const CUTOFF: f32 = 1.0e-3;
pub const WORKLIST_CAPACITY: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhittedDecline {
    UvLamp,
    Fluorescence,
}

impl WhittedDecline {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::UvLamp | Self::Fluorescence => {
                "Ray-Traced cannot show UV fluorescence. Use Path-Traced."
            }
        }
    }
}

impl std::fmt::Display for WhittedDecline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for WhittedDecline {}

#[cfg(test)]
mod tests_chain;
#[cfg(test)]
mod tests_frame;
#[cfg(test)]
mod tests_prep;
