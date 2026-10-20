//! The miss/escape block of the bounce loop, moved out of `bounce_body.rs` unchanged to keep
//! that file under its size limit. A child module of `bounce_body`, so it sees
//! [`PathState`]'s private fields exactly as the original method did.

use super::{PathState, record_termination};
use crate::optics::raytracer::{
    environment::environment_nee_pdf,
    scattering::balance_heuristic,
    transport::{PathTermination, bounce::accumulate_miss_radiance},
};
use glam::Vec3;

impl PathState<'_> {
    /// The ray exited or missed the gemstone: the environment source is sampled and the path
    /// ends as `Escaped`. A camera ray that misses the stone (bounce 0) takes exactly the same
    /// lookup as an exit at any later bounce -- the lit backdrop tray below the horizon is
    /// part of the environment function, so a window and the surround show the same world.
    /// At bounce 0 the Stokes intensity is 1 and the MIS weight 1, so the value is the
    /// environment in that direction bit for bit.
    ///
    /// If an earlier bounce left a pending NEE-eligible carry still live (a scatter event
    /// whose continuation just now escaped directly, possibly after first transmitting out
    /// through a polished exit facet -- see `dispatch_bounce`'s doc comment for why the carry
    /// must survive that intervening refraction), this escape is the SAME light-sampling
    /// technique's competing (BSDF/phase-sampled) continuation -- weight it by the balance
    /// heuristic so the two techniques' contributions sum to the true value rather than
    /// double-counting. Evaluated at the carried INTERIOR direction, the same measure
    /// `nee_contribution_hg_scatter` sampled in -- NOT `current_ray.dir`, which by the time a
    /// transmit-out carry reaches here is the refracted EXTERIOR direction instead.
    /// `map_or(1.0, ..)` reproduces the full-weight behaviour exactly whenever no carry is
    /// live (including every trace with NEE disabled, where this is always `None`).
    #[inline]
    pub(super) fn handle_miss(
        &mut self,
        bounce: u32,
        phase_pdf_for_mis_this_check: Option<(f32, Vec3)>,
        termination_out: &mut Option<&mut (u32, PathTermination)>,
    ) {
        let environment = self.scene.environment;
        let mis_weight = phase_pdf_for_mis_this_check.map_or(1.0, |(phase_pdf, interior_dir)| {
            let light_pdf = environment_nee_pdf(environment, interior_dir);
            balance_heuristic(phase_pdf, light_pdf)
        });
        accumulate_miss_radiance(
            &self.exit_split_ctx,
            self.current_ray.dir,
            &self.lambdas,
            &self.stokes,
            mis_weight,
            &mut self.radiance,
        );
        self.path_escaped = true;
        record_termination(termination_out, bounce, PathTermination::Escaped);
    }
}
