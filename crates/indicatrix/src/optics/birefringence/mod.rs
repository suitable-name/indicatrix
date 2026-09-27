//! Birefringence: uniaxial and biaxial crystal optics, and pleochroic (polarization-
//! dependent) absorption.
//!
//! Split from a single `birefringence.rs` by responsibility: [`helpers`] holds the
//! low-level vector helpers shared by both the uniaxial and biaxial machinery,
//! [`uniaxial`] holds [`BirefringenceParams`], [`absorption_tensor`] holds
//! [`AbsorptionTensor3`] and the pleochroic-absorption functions built on it, and
//! [`biaxial`] holds [`BiaxialIndicatrix`] and the general Poynting-direction formula.
//! Every path reachable as `birefringence::X` before the split is still reachable at
//! exactly that path via the re-exports below.

mod absorption_tensor;
mod biaxial;
mod helpers;
#[cfg(test)]
mod tests;
mod uniaxial;

pub use absorption_tensor::{
    AbsorptionTensor3, assigned_mode_alpha, assigned_mode_e_field_uniaxial,
    effective_pleochroic_alpha, pleochroic_channel_alpha,
};
pub use biaxial::{BiaxialIndicatrix, poynting_direction};
pub use uniaxial::BirefringenceParams;
