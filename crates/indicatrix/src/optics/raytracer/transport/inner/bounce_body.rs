//! The bounce loop's body, split out of `inner.rs` as a pure extraction.
//!
//! [`trace_spectral_ray_core`] builds a [`PathState`] holding every local the loop used to
//! carry, then runs [`PathState::run_bounce`] per iteration. Each helper below is one
//! contiguous block of the former loop, with its floating-point expressions untouched and in
//! the original evaluation order (the WGSL twin is written against that order). The loop's
//! `continue`/`break` exits became the [`Flow`] return value.

#[cfg(debug_assertions)]
use super::debug_assert_inside_gem_matches_geometry;
use super::{
    TraceScene, TraceSwitches, apply_interior_segment, build_ray_context, cached_studio_rig,
    capture_primary_hit, facet_finish_for, record_termination, shading_normal_for_hit,
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::sample_vertex,
        materials::GemMaterial,
        polarization::StokesVector,
        raytracer::{
            NUM_CHANNELS,
            absorption::{apply_absorption, rotate_stokes_to_plane_of_incidence},
            camera::{FacetFinish, HitRecord, Ray},
            color::{
                apply_von_kries_white_balance, integrate_channels_to_xyz,
                integrate_channels_to_xyz_families,
            },
            environment::{
                EnvironmentSource, environment_nee_pdf, environment_white_balance, fill_backdrop,
            },
            intersect_stone::intersect_stone_soa,
            refraction::{
                ExitSplitCtx, RayMaterialContext, RayWavelengthCache,
                compute_bounce_refraction_geometry,
            },
            sampling::{
                FLUORESCENCE_DIR_U_STREAM, FLUORESCENCE_DIR_V_STREAM, FLUORESCENCE_DISTANCE_STREAM,
                FLUORESCENCE_EMITTER_STREAM, FLUORESCENCE_EXCITATION_STREAM, hash_u32,
            },
            scattering::{NeeContext, ScatterStepOutcome, balance_heuristic, try_scatter_step},
            transport::{
                PathTermination,
                bounce::{accumulate_miss_radiance, apply_russian_roulette, dispatch_bounce},
                wrapped_hero_wavelengths,
            },
        },
    },
};
use glam::Vec3;

/// Upper bound on the expected number of fluorescence vertices the free-path sampler
/// of one segment aims for (`mu_s * segment <= CAP`): the sampling rate is
/// `min(mu_f, CAP / segment)`, and the estimator weights absorb the difference
/// (`mu_f / mu_s` at a vertex, `exp(mu_s * ell)` for a path that goes on). The plain
/// exponential at `mu_s = mu_f` is unbiased too, but a strongly emitting line
/// (`mu_f * segment` in the tens) would give the surviving paths weights of
/// `exp(mu_f * segment)`, an unusable variance; capped at 2 the largest such weight is 7.4.
const FLUORESCENCE_SAMPLING_CAP: f32 = 2.0;

/// Which slot of `lambdas` (and every other per-channel array) holds the wavelength driving
/// the shared geometric path. Provably 0 for every invocation under this construction
/// (`lambdas[0] == lambda_hero` identically), but named so the code documents which channel
/// plays the hero role.
const HERO_IDX: usize = 0;

/// A `[0, 1]` draw from `rng_seed`'s stream `stream` at `bounce`, in the same form every
/// other per-bounce draw of this module tree uses.
fn unit_draw(rng_seed: u32, bounce: u32, stream: u32) -> f32 {
    (hash_u32(rng_seed ^ hash_u32(bounce ^ stream)) as f32) / 4_294_967_295.0
}

/// Sets every channel's Stokes vector to unpolarized light of `weight` times its
/// current intensity: the fluorescence emission is isotropic and unpolarized.
fn depolarize_and_scale(stokes: &mut [StokesVector; NUM_CHANNELS], weight: f32) {
    for s in &mut *stokes {
        *s = StokesVector::unpolarized(s.intensity() * weight);
    }
}

/// Scales every channel's Stokes vector (polarization state kept).
fn scale_stokes(stokes: &mut [StokesVector; NUM_CHANNELS], weight: f32) {
    for s in &mut *stokes {
        *s = s.scale(weight);
    }
}

/// Applies the surface-glare scale to every channel's Stokes vector. Called only for a
/// camera path whose first event is the polished specular reflection off the stone's
/// surface (it never entered), and only when the scale is below one.
fn apply_surface_glare(stokes: &mut [StokesVector; NUM_CHANNELS], surface_glare: f32) {
    for s in &mut *stokes {
        *s = s.scale(surface_glare);
    }
}

/// Debug-only checks that a fluorescent scene is one the vertex sampler supports.
///
/// Emitters are `PerMm`-only: the pseudo-extinction is in 1/mm and the vertex sampling scales
/// the segment by `absorption_path_scale`. A `ModelUnit` material's scale is
/// `(W / 7) / MODEL_UNIT_FACE_UP_PATH`, which would make its emitters fluoresce about 2.5x
/// (1 / K) too weakly. Today only `PerMm` physics recipes carry emitters. A zoned medium is not
/// homogeneous either: the vertex sampler (a single exponential clock at one rate) does not
/// apply to it.
fn debug_assert_fluorescence_supported(material: &GemMaterial, fluorescent: bool) {
    debug_assert!(
        !fluorescent
            || material.absorption_unit == crate::optics::materials::AbsorptionUnit::PerMm
            || material.absorption_path_scale.to_bits() == 1.0_f32.to_bits(),
        "fluorescent emitters need a PerMm material (or an unscaled bare one)"
    );
    debug_assert!(
        !fluorescent || material.zoning.is_none(),
        "fluorescent emitters are not supported on a zoned material"
    );
}

/// What the bounce loop does after one of the body's blocks.
enum Flow {
    /// Fall through to the next block of the same iteration.
    Facet,
    /// The former `continue`: go straight to the next bounce.
    Next,
    /// The former `break`: the path is finished.
    Stop,
}

/// Where the fluorescence free-path sampler stopped one interior segment.
///
/// `segment_t` is where the scatter sampler must stop: the vertex if one fires first, else
/// the boundary. Without fluorescence `segment_t == hit_t` and nothing was drawn.
#[derive(Clone, Copy)]
struct FluorescenceSegment {
    segment_t: f32,
    /// The vertex distance in scaled (absorption) units, when one fired before the boundary.
    vertex_t: Option<f32>,
    sampling_rate: f32,
}

/// Every local the bounce loop carried, gathered so each block of it can be its own method.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the former locals of the bounce loop; the flags are independent per-path switches"
)]
struct PathState<'a> {
    scene: TraceScene<'a>,
    rng_seed: u32,
    /// The surface-glare scale clamped to `[0, 1]` at the boundary; a NaN fails the
    /// `< 1.0` test in the facet block and so behaves as off.
    surface_glare: f32,
    enable_internal_mode_coupling: bool,
    /// Exit-event splitting after the fluorescence override (its companions would all be
    /// the same wavelength, so a fluorescent scene turns it off).
    exit_splitting: bool,
    fluorescent: bool,
    /// Corrects the importance-sampled camera wavelength; 1 and unused without fluorescence.
    camera_weight: f32,
    /// The wavelengths the camera integrates the result at; `lambdas` itself moves to the
    /// excitation wavelength at a fluorescence vertex.
    camera_lambdas: [f32; NUM_CHANNELS],
    fluorescence_rate: f32,
    /// Whether this path may still take its (single) fluorescence vertex.
    fluorescence_armed: bool,
    lambdas: [f32; NUM_CHANNELS],
    stokes: [StokesVector; NUM_CHANNELS],
    radiance: [f32; NUM_CHANNELS],
    /// A Henyey-Greenstein scattering-point NEE deposit is integrated to XYZ immediately,
    /// using THAT moment's own `path_pdf`/`compat` -- see `try_scatter_step`'s "NEE spectral
    /// weighting" doc comment for why this must not ride the shared `radiance` array (which
    /// is later weighted by the FINAL, post-loop `path_pdf`/`compat` instead). Summed
    /// unconditionally into the final result, regardless of how the path terminates.
    nee_xyz: Vec3,
    /// Per-channel running density of "technique k (channel k as hero) would have generated
    /// this exact realized path". Starts at 1.0 for every channel.
    path_pdf: [f32; NUM_CHANNELS],
    current_ray: Ray,
    /// The wave normal `k`, tracked alongside `current_ray.dir` (the Poynting/energy
    /// direction `S`); starts equal to the initial ray direction (k == S in isotropic air).
    current_k: Vec3,
    inside_gem: bool,
    /// `None` until the first plane of incidence is recorded, so no spurious frame rotation
    /// applies at the very first surface.
    prev_plane_normal: Option<Vec3>,
    /// Which eigenmode the ray inside the crystal was stochastically assigned at its most
    /// recent air->crystal entry; carried across internal bounces until it exits. For a
    /// UNIAXIAL material this is the ordinary/extraordinary split; for a BIAXIAL one it is
    /// a plain two-valued selector (`false` = mode A, the lower-index root).
    is_extraordinary: bool,
    mat_ctx: RayMaterialContext<'a>,
    wavelength_cache: RayWavelengthCache,
    /// Per-trace context for exit-event spectral splitting, with the staging accumulator
    /// `split_radiance` it borrows.
    exit_split_ctx: ExitSplitCtx<'a>,
    /// Set where `PathTermination::Escaped` is recorded -- the only point `radiance` is ever
    /// populated from a real environment sample, so the only case `split_radiance` commits.
    path_escaped: bool,
    nee_ctx: NeeContext<'a>,
    /// The one piece of state that carries from a scatter event to its VERY NEXT loop
    /// iteration -- see `ScatterStepOutcome::ScatteredAndSurvived`'s doc comment.
    pending_light_mis: Option<(f32, Vec3)>,
}

/// The shared bounce loop's body: see `inner::trace_spectral_ray_inner` for the contract.
pub(super) fn trace_spectral_ray_core(
    scene: &TraceScene<'_>,
    initial_ray: Ray,
    rng_seed: u32,
    hero_rand: f32,
    switches: TraceSwitches,
    outputs: (
        Option<&mut Option<HitRecord>>,
        Option<&mut (u32, PathTermination)>,
    ),
) -> Vec3 {
    let (mut primary_hit_out, mut termination_out) = outputs;
    // Exit-event splitting's own staging accumulator -- see `ExitSplitCtx::split_radiance`'s
    // doc comment for why a split channel's contribution lands here first, folded into
    // `radiance` only if the shared/hero path terminates via `PathTermination::Escaped`.
    let mut split_radiance = [0.0f32; NUM_CHANNELS];
    let mut path = PathState::new(
        *scene,
        initial_ray,
        rng_seed,
        hero_rand,
        switches,
        &mut split_radiance,
    );
    for bounce in 0..path.scene.max_bounces {
        let flow = path.run_bounce(bounce, &mut primary_hit_out, &mut termination_out);
        if matches!(flow, Flow::Stop) {
            break;
        }
    }
    path.finish()
}

impl<'a> PathState<'a> {
    /// The per-trace setup: camera spectrum (the hero comb, or the single fluorescence
    /// wavelength), the material/wavelength caches, and the exit-split and NEE contexts.
    ///
    /// Hero is drawn over the full visible range [380, 780) with wraparound, so a hero draw
    /// `h` and `h + channel_width` generate the same 8-member comb, cyclically rotated --
    /// each member is equally likely to be drawn as hero, the premise spectral MIS requires.
    /// A scene with fluorescence traces SINGLE-wavelength paths instead: all channels carry
    /// the one camera wavelength `lambda_em` (see `Fluorescence::camera_wavelength`), so the
    /// in-medium vertex can switch the whole path to the excitation wavelength and the image
    /// still converges unbiased.
    fn new(
        scene: TraceScene<'a>,
        initial_ray: Ray,
        rng_seed: u32,
        hero_rand: f32,
        switches: TraceSwitches,
        split_radiance: &'a mut [f32; NUM_CHANNELS],
    ) -> Self {
        let TraceSwitches {
            internal_mode_coupling: enable_internal_mode_coupling,
            exit_splitting: enable_exit_splitting,
            nee: enable_nee,
        } = switches;
        let material = scene.material;
        let fluorescence = scene.fluorescence;
        let environment = scene.environment;
        let fluorescent = !fluorescence.is_empty();
        let enable_exit_splitting = enable_exit_splitting && !fluorescent;
        let (camera_wavelength, camera_weight) = if fluorescent {
            fluorescence.camera_wavelength(hero_rand)
        } else {
            (0.0, 1.0)
        };
        let lambdas: [f32; NUM_CHANNELS] = if fluorescent {
            [camera_wavelength; NUM_CHANNELS]
        } else {
            wrapped_hero_wavelengths(hero_rand)
        };
        let camera_lambdas = lambdas;
        debug_assert_fluorescence_supported(material, fluorescent);
        let fluorescence_rate = if fluorescent {
            fluorescence.pseudo_extinction(camera_lambdas[0])
        } else {
            0.0
        };
        let (mat_ctx, wavelength_cache) =
            build_ray_context(material, lambdas, HERO_IDX, enable_internal_mode_coupling);
        // Built once per trace and shared: the exit-split probes read it through
        // `exit_split_ctx.studio_rig`, and the escape lookup in `accumulate_miss_radiance`
        // borrows that same instance. It depends only on `(light_yaw, light_pitch)`, so the
        // single build is value-identical to rebuilding it at each use.
        let studio_rig = match environment {
            EnvironmentSource::Studio {
                light_yaw,
                light_pitch,
                ..
            } => Some(cached_studio_rig(light_yaw, light_pitch)),
            EnvironmentSource::HdrMap(_) => None,
        };
        let exit_split_ctx = ExitSplitCtx {
            plane_soa: scene.plane_soa,
            tools: scene.tools,
            environment,
            studio_rig,
            // Unit direction from the stone back towards the eye: the lit lighting models
            // darken exit directions inside the head-shadow cone around it. Per pixel
            // rather than the camera axis, so the cone is centred where this pixel's eye is.
            observer: -initial_ray.dir,
            split_radiance,
            enabled: enable_exit_splitting,
            compat: [u8::MAX; NUM_CHANNELS],
            split_mis_weight: 1.0,
        };
        let nee_ctx = NeeContext {
            environment,
            plane_soa: scene.plane_soa,
            tools: scene.tools,
            enabled: enable_nee,
        };
        Self {
            scene,
            rng_seed,
            surface_glare: scene.surface_glare.clamp(0.0, 1.0),
            enable_internal_mode_coupling,
            exit_splitting: enable_exit_splitting,
            fluorescent,
            camera_weight,
            camera_lambdas,
            fluorescence_rate,
            fluorescence_armed: fluorescence_rate > 0.0,
            lambdas,
            stokes: [StokesVector::unpolarized(1.0); NUM_CHANNELS],
            radiance: [0.0f32; NUM_CHANNELS],
            nee_xyz: Vec3::ZERO,
            path_pdf: [1.0f32; NUM_CHANNELS],
            current_ray: initial_ray,
            current_k: initial_ray.dir,
            inside_gem: false,
            prev_plane_normal: None,
            is_extraordinary: false,
            mat_ctx,
            wavelength_cache,
            exit_split_ctx,
            path_escaped: false,
            nee_ctx,
            pending_light_mis: None,
        }
    }

    /// One iteration of the bounce loop: intersect, miss/escape handling, the interior
    /// segment (fluorescence + scattering), then the facet event.
    #[inline]
    fn run_bounce(
        &mut self,
        bounce: u32,
        primary_hit_out: &mut Option<&mut Option<HitRecord>>,
        termination_out: &mut Option<&mut (u32, PathTermination)>,
    ) -> Flow {
        let scene = self.scene;
        // Consumed unconditionally every iteration: meaningful only if THIS iteration's ray
        // turns out to have escaped directly; otherwise silently dropped, which is correct
        // -- a phase-sampled continuation that instead hits more geometry has no competing
        // NEE sample to weigh itself against.
        let phase_pdf_for_mis_this_check = self.pending_light_mis.take();
        // With no tools this is `intersect_polyhedron_soa` exactly. On a concave stone a
        // hit after an exit is a legitimate re-entry across the cavity, not an error.
        let hit = intersect_stone_soa(
            self.current_ray,
            scene.plane_soa,
            scene.planes.len(),
            scene.tools,
        );
        // Denoiser wiring: the primary ray's first-hit depth/normal/facet index feed the
        // A-Trous denoiser's guide buffers. Captured only at bounce 0, which is
        // unambiguously the hero channel's own first hit.
        capture_primary_hit(primary_hit_out, bounce, hit);
        let Some(hit_rec) = hit else {
            self.handle_miss(bounce, phase_pdf_for_mis_this_check, termination_out);
            return Flow::Stop;
        };
        #[cfg(debug_assertions)]
        debug_assert_inside_gem_matches_geometry(
            StoneGeometry {
                planes: scene.planes,
                tools: scene.tools,
            },
            self.current_ray,
            hit_rec.t,
            self.inside_gem,
        );
        if self.inside_gem {
            let segment = self.sample_fluorescence_segment(bounce, hit_rec.t);
            match self.scatter_step(bounce, hit_rec.t, segment, termination_out) {
                Flow::Facet => {}
                flow => return flow,
            }
        }
        self.facet_event(
            bounce,
            hit_rec,
            phase_pdf_for_mis_this_check,
            termination_out,
        )
    }

    /// The ray exited or missed the gemstone: the backdrop card or the environment source is
    /// sampled and the path ends as `Escaped`.
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
    fn handle_miss(
        &mut self,
        bounce: u32,
        phase_pdf_for_mis_this_check: Option<(f32, Vec3)>,
        termination_out: &mut Option<&mut (u32, PathTermination)>,
    ) {
        let environment = self.scene.environment;
        // The camera ray sees the backdrop card, if the scene has one -- only the stone's
        // own light reaches the environment lookup below.
        if !(bounce == 0 && fill_backdrop(environment, &self.lambdas, &mut self.radiance)) {
            let mis_weight =
                phase_pdf_for_mis_this_check.map_or(1.0, |(phase_pdf, interior_dir)| {
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
        }
        self.path_escaped = true;
        record_termination(termination_out, bounce, PathTermination::Escaped);
    }

    /// Fluorescence: samples a vertex along the interior segment, competing with the
    /// scattering sample and the boundary (see `FLUORESCENCE_SAMPLING_CAP`). Without
    /// fluorescence (or once the single vertex was taken) nothing here draws.
    #[inline]
    fn sample_fluorescence_segment(&self, bounce: u32, hit_t: f32) -> FluorescenceSegment {
        let mut segment_t = hit_t;
        let mut vertex_t = None;
        let mut sampling_rate = 0.0f32;
        let path_scale = self.scene.material.absorption_path_scale;
        if self.fluorescence_armed && hit_t > 0.0 {
            let segment_scaled = hit_t * path_scale;
            sampling_rate = self
                .fluorescence_rate
                .min(FLUORESCENCE_SAMPLING_CAP / segment_scaled);
            let u = unit_draw(self.rng_seed, bounce, FLUORESCENCE_DISTANCE_STREAM);
            let t_scaled = -((1.0 - u).max(1e-7).ln()) / sampling_rate;
            if t_scaled < segment_scaled {
                segment_t = t_scaled / path_scale;
                vertex_t = Some(t_scaled);
            }
        }
        FluorescenceSegment {
            segment_t,
            vertex_t,
            sampling_rate,
        }
    }

    /// Attempts a Henyey-Greenstein scattering event along the interior segment before the
    /// facet is processed -- see `try_scatter_step`'s doc comment -- and takes the
    /// fluorescence vertex when it fired before the boundary.
    #[inline]
    fn scatter_step(
        &mut self,
        bounce: u32,
        hit_t: f32,
        segment: FluorescenceSegment,
        termination_out: &mut Option<&mut (u32, PathTermination)>,
    ) -> Flow {
        let FluorescenceSegment {
            segment_t,
            vertex_t,
            sampling_rate,
        } = segment;
        let path_scale = self.scene.material.absorption_path_scale;
        let origin_before = self.current_ray.origin;
        let outcome = try_scatter_step(
            &self.mat_ctx,
            &self.wavelength_cache,
            self.scene.material,
            self.is_extraordinary,
            &mut self.current_ray,
            &mut self.current_k,
            segment_t,
            self.rng_seed,
            bounce,
            &mut self.stokes,
            &mut self.path_pdf,
            self.exit_split_ctx.split_radiance,
            self.nee_ctx,
            &self.lambdas,
            &mut self.nee_xyz,
            self.exit_splitting,
            self.exit_split_ctx.compat,
            self.scene.facet_finishes,
        );
        match outcome {
            ScatterStepOutcome::NotApplicable | ScatterStepOutcome::ReachedBoundary => {
                if let Some(t_scaled) = vertex_t {
                    return self.fluorescence_vertex(
                        bounce,
                        segment_t,
                        t_scaled,
                        sampling_rate,
                        termination_out,
                    );
                }
                // No vertex before the boundary: the path goes on, reweighted by the inverse
                // of that probability (1 / exp(-mu_s * ell)).
                if sampling_rate > 0.0 {
                    let g = (sampling_rate * hit_t * path_scale).exp();
                    scale_stokes(&mut self.stokes, g);
                }
                Flow::Facet
            }
            ScatterStepOutcome::ScatteredAndSurvived(phase_pdf_for_mis) => {
                if sampling_rate > 0.0 {
                    // Scattered at distance `s` with the vertex clock not having fired:
                    // weight `exp(mu_s * s)`.
                    let s = (self.current_ray.origin - origin_before).length() * path_scale;
                    scale_stokes(&mut self.stokes, (sampling_rate * s).exp());
                }
                // Scattered Stokes vectors are already depolarized, so the previous plane of
                // incidence is no longer meaningful -- reset it so the next facet hit applies
                // no spurious frame rotation.
                self.prev_plane_normal = None;
                // Consumed by the VERY NEXT iteration's escape check, and only there.
                self.pending_light_mis = phase_pdf_for_mis;
                Flow::Next
            }
            ScatterStepOutcome::ScatteredAndTerminated => {
                record_termination(termination_out, bounce, PathTermination::ScatterAbsorbed);
                Flow::Stop
            }
        }
    }

    /// The fluorescence vertex fired before the boundary: the medium's extinction up to it is
    /// already applied for a scattering material (`try_scatter_step`), else the Beer-Lambert
    /// absorption of the stretch to the vertex is applied here. The path then continues from
    /// the vertex in an isotropic direction at the excitation wavelength, with no further
    /// vertex.
    #[inline]
    fn fluorescence_vertex(
        &mut self,
        bounce: u32,
        segment_t: f32,
        t_scaled: f32,
        sampling_rate: f32,
        termination_out: &mut Option<&mut (u32, PathTermination)>,
    ) -> Flow {
        let material = self.scene.material;
        if material.scattering_sigma_s <= 0.0 {
            apply_absorption(
                &self.mat_ctx,
                &self.wavelength_cache,
                self.current_k,
                self.is_extraordinary,
                segment_t,
                &mut self.stokes,
            );
        }
        let rng_seed = self.rng_seed;
        let Some(vertex) = sample_vertex(
            self.scene.fluorescence,
            self.camera_lambdas[0],
            unit_draw(rng_seed, bounce, FLUORESCENCE_EMITTER_STREAM),
            unit_draw(rng_seed, bounce, FLUORESCENCE_EXCITATION_STREAM),
        ) else {
            record_termination(termination_out, bounce, PathTermination::RussianRoulette);
            return Flow::Stop;
        };
        // Estimator weight: the rate ratio of the capped sampler, the reweighting of its
        // truncated exponential (`exp(mu_s * t)`), and the photon-energy ratio of the Stokes
        // shift.
        let weight = (self.fluorescence_rate / sampling_rate)
            * (sampling_rate * t_scaled).exp()
            * vertex.energy_ratio;
        depolarize_and_scale(&mut self.stokes, weight);
        let z = (-2.0f32).mul_add(unit_draw(rng_seed, bounce, FLUORESCENCE_DIR_U_STREAM), 1.0);
        let phi = std::f32::consts::TAU * unit_draw(rng_seed, bounce, FLUORESCENCE_DIR_V_STREAM);
        #[allow(
            clippy::suboptimal_flops,
            reason = "transport results are pinned bit for bit (goldens, twins); a fused `1 - z*z` rounds once and moves the last bit"
        )]
        let r = (1.0 - z * z).max(0.0).sqrt();
        let new_dir = Vec3::new(r * phi.cos(), r * phi.sin(), z);
        self.current_ray.origin += segment_t * self.current_ray.dir;
        self.current_ray.dir = new_dir;
        self.current_k = new_dir;
        self.prev_plane_normal = None;
        self.lambdas = [vertex.lambda_ex; NUM_CHANNELS];
        (self.mat_ctx, self.wavelength_cache) = build_ray_context(
            material,
            self.lambdas,
            HERO_IDX,
            self.enable_internal_mode_coupling,
        );
        self.fluorescence_armed = false;
        Flow::Next
    }

    /// The facet event of one bounce: edge-rounded shading normal, Stokes frame rotation,
    /// interior absorption, refraction geometry, the TIR/reflect/refract (or frosted)
    /// dispatch, surface glare and Russian roulette.
    ///
    /// The Stokes plane-of-incidence frame is defined by the wave normal `k`, not `S`;
    /// assigned-mode absorption's E-field direction uses `k` too, while `hit_rec.t` stays
    /// geometric/`S`-based, and so do the index lookups, angles and every downstream
    /// Snell/Fresnel evaluation. `dispatch_bounce`'s return is `Some` for an NEE-eligible
    /// frosted-facet outcome, OR for a polished transmit-out event that had a live incoming
    /// carry (`phase_pdf_for_mis_this_check`) to pass through -- `None` for every other
    /// dispatch.
    #[inline]
    fn facet_event(
        &mut self,
        bounce: u32,
        hit_rec: HitRecord,
        phase_pdf_for_mis_this_check: Option<(f32, Vec3)>,
        termination_out: &mut Option<&mut (u32, PathTermination)>,
    ) -> Flow {
        let scene = self.scene;
        let hit_point = self.current_ray.origin + hit_rec.t * self.current_ray.dir;
        let mut normal = shading_normal_for_hit(
            StoneGeometry {
                planes: scene.planes,
                tools: scene.tools,
            },
            hit_point,
            hit_rec.facet_idx,
            hit_rec.normal,
            scene.material.edge_rounding_radius,
        );
        let current_plane_normal = rotate_stokes_to_plane_of_incidence(
            self.current_k,
            normal,
            self.prev_plane_normal,
            &mut self.stokes,
        );
        self.prev_plane_normal = Some(current_plane_normal);
        apply_interior_segment(
            (&self.mat_ctx, &self.wavelength_cache),
            self.current_k,
            self.is_extraordinary,
            (self.current_ray, hit_rec.t),
            self.inside_gem,
            &mut normal,
            &mut self.stokes,
        );
        let geo = compute_bounce_refraction_geometry(
            &self.mat_ctx,
            &self.wavelength_cache,
            normal,
            self.current_k,
            self.inside_gem,
            self.is_extraordinary,
        );
        // Girdle finish: `Polished` (default) takes the pre-existing dispatch; `Frosted`
        // takes `apply_frosted_bounce` instead.
        let finish = facet_finish_for(scene.facet_finishes, hit_rec.facet_idx);
        self.pending_light_mis = dispatch_bounce(
            &self.mat_ctx,
            &self.wavelength_cache,
            &geo,
            hit_point,
            normal,
            current_plane_normal,
            finish,
            self.rng_seed,
            bounce,
            &mut self.stokes,
            &mut self.path_pdf,
            &mut self.exit_split_ctx,
            &mut self.current_ray,
            &mut self.current_k,
            &mut self.inside_gem,
            &mut self.is_extraordinary,
            self.nee_ctx,
            &self.lambdas,
            &mut self.radiance,
            phase_pdf_for_mis_this_check,
        );
        // Surface glare: the camera path's first event at a polished facet left the ray
        // outside the stone, so it was the specular (Fresnel) reflection and the light never
        // entered. Scaling the throughput here scales everything this path later deposits.
        // Skipped entirely at `1.0` (and for a frosted facet, whose diffuse scatter is not a
        // mirror image), so that case is bit-identical.
        if bounce == 0
            && !self.inside_gem
            && self.surface_glare < 1.0
            && finish != FacetFinish::Frosted
        {
            apply_surface_glare(&mut self.stokes, self.surface_glare);
        }
        // Reborrowed through `exit_split_ctx.split_radiance` since `exit_split_ctx` already
        // holds that local borrowed mutably for the whole loop.
        if bounce > 4
            && !apply_russian_roulette(
                bounce,
                self.rng_seed,
                &mut self.stokes,
                self.exit_split_ctx.split_radiance,
            )
        {
            record_termination(termination_out, bounce, PathTermination::RussianRoulette);
            return Flow::Stop;
        }
        Flow::Next
    }

    /// Folds the staged exit-split contributions in, integrates the channels to XYZ and
    /// applies the Von Kries white balance.
    ///
    /// The split contributions are committed into `radiance` only if the shared/hero path
    /// itself reached its own environment lookup -- see `ExitSplitCtx::split_radiance`'s doc
    /// comment for why this all-or-nothing gate is required. `nee_xyz` is already fully
    /// integrated to XYZ, so it is added in directly rather than run back through the
    /// integrator. The white balance is skipped entirely for `HdrMap`: the round trip is not
    /// quite the identity in f32 (the two published Bradford matrices are not exact inverses,
    /// `max|B*A - I| ~= 5.2e-7`), and the WGSL twin gates the same transform on
    /// `params.env_mode == 1u` (`Studio`), so skipping it keeps hybrid CPU/GPU HDR frames
    /// exact rather than merely close. The final `.max(Vec3::ZERO)` is a physical floor on
    /// `xyz` itself (the Bradford LMS matrices have negative off-diagonal entries), kept
    /// unconditionally.
    #[inline]
    fn finish(mut self) -> Vec3 {
        // Each channel's MIS family, final after the last interior dispersive event.
        let compat = self.exit_split_ctx.compat;
        if self.path_escaped {
            for k in 0..NUM_CHANNELS {
                self.radiance[k] += self.exit_split_ctx.split_radiance[k];
            }
        }
        let environment = self.scene.environment;
        let hero_idx = HERO_IDX;
        // With splitting enabled every channel is weighted over its own family instead.
        let xyz = if self.exit_splitting {
            integrate_channels_to_xyz_families(
                &self.radiance,
                &self.camera_lambdas,
                &self.path_pdf,
                hero_idx,
                compat,
            )
        } else {
            integrate_channels_to_xyz(
                &self.radiance,
                &self.camera_lambdas,
                &self.path_pdf,
                hero_idx,
            )
        };
        let xyz = xyz + self.nee_xyz;
        let xyz = if self.fluorescent {
            xyz * self.camera_weight
        } else {
            xyz
        };
        match environment {
            EnvironmentSource::Studio { .. } => {
                apply_von_kries_white_balance(xyz, environment_white_balance(environment))
            }
            EnvironmentSource::HdrMap(_) => xyz,
        }
        .max(Vec3::ZERO)
    }
}
