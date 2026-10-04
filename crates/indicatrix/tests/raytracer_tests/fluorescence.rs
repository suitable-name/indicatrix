//! Fluorescence and UV-lamp tests (`docs/fluorescence-plan.md` section 6): the empty
//! `Fluorescence` is the old renderer bit for bit, the transport conserves energy, a
//! ruby glows red under the 365 nm lamp (and not when quenched), the 395 nm lamp lights a
//! non-fluorescent stone only faintly, and a fluorescent trace is deterministic.
//!
//! The emitters are hand-built (a ruby-like Cr3+ emitter: excitation bands at 410 nm and
//! 556 nm of 3500 and 2800 cm^-1 FWHM, the 694 nm R line of 2 nm FWHM, `Phi = 0.9`); the
//! chromophore-recipe route to them is tested with the recipe code.

use glam::Vec3;
use indicatrix::{
    geometry::{StoneGeometry, cuts::StandardGemCuts},
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        chromophore::{ChromophoreCatalogue, colorRecipe, resolve, resolve_fluorescence},
        fluorescence::{EmissionBand, Fluorescence, FluorescentEmitter},
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, LightingPreset, Ray, cie_1931_cmf, intersect_polyhedron,
            trace_spectral_ray, trace_spectral_ray_geom,
        },
    },
    renderer::{
        env_map::{EnvironmentMap, rgb_to_spectral_radiance},
        gpu_backend::scene_routes_to_gpu,
    },
};

use crate::fixtures::furnace_mean_xyz;

/// Excitation bands of the ruby-like emitter (and of the ruby-like stone, so that what
/// absorbs is what glows), `peak` per mm.
fn ruby_bands(peak: f32) -> Vec<AbsorptionBand> {
    vec![
        AbsorptionBand::energy(410.0, 3500.0, peak),
        AbsorptionBand::energy(556.0, 2800.0, peak),
    ]
}

/// A corundum-like stone (n = 1.77, isotropic) with the ruby-like absorption bands.
fn ruby_stone(peak: f32) -> GemMaterial {
    GemMaterial::new_custom("Ruby-like", 1.77, 0.012, 0.0, [0.0, 0.0, 0.0])
        .with_chromophore_absorption(AbsorptionTensor::isotropic(ruby_bands(peak)))
}

/// The hand-built ruby-like emitter with quantum yield `phi`.
fn ruby_emitter(phi: f32, peak: f32) -> FluorescentEmitter {
    FluorescentEmitter {
        excitation: ruby_bands(peak),
        emission: vec![EmissionBand::new(694.0, 2.0, 1.0)],
        quantum_yield: phi,
    }
}

/// CIELAB hue angle (degrees) of an XYZ color, with the color scaled to a relative
/// luminance of 0.4 first (the image's absolute scale is arbitrary) against the D65 white.
#[expect(
    clippy::many_single_char_names,
    reason = "CIELAB's own X, Y, Z, f, a, b"
)]
fn lab_hue_deg(xyz: Vec3) -> f32 {
    let s = 0.4 / xyz.y;
    let (x, y, z) = (xyz.x * s / 0.950_47, 0.4, xyz.z * s / 1.088_83);
    let f = |t: f32| {
        if t > 216.0 / 24_389.0 {
            t.cbrt()
        } else {
            (24_389.0f32 / 27.0).mul_add(t, 16.0) / 116.0
        }
    };
    let a = 500.0 * (f(x) - f(y));
    let b = 200.0 * (f(y) - f(z));
    b.atan2(a).to_degrees().rem_euclid(360.0)
}

/// Mean XYZ of the 12 x 12 furnace-camera image of the round brilliant, `spp` samples a
/// pixel.
fn render_mean(
    material: &GemMaterial,
    fluorescence: &Fluorescence,
    lighting: LightingPreset,
    spp: u32,
    salt: u32,
) -> Vec3 {
    let planes = StandardGemCuts::standard_round_brilliant();
    let environment = lighting.studio(1.0, 0.4, 0.35);
    furnace_mean_xyz(spp, salt, |ray, seed, hero| {
        trace_spectral_ray_geom(
            ray,
            StoneGeometry::planes_only(&planes),
            material,
            fluorescence,
            12,
            environment,
            seed,
            hero,
            None,
        )
    })
    .0
}

/// Plan item 2: an empty `Fluorescence` (the shared one, a default one, or a `new` of no
/// emitters) traces bit-identically to the plain entry point, for the comb and a lamp
/// alike; and `scene_routes_to_gpu` is unchanged for every existing preset.
#[test]
fn empty_fluorescence_is_bit_identical_and_routes_as_before() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::ruby();
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let default = Fluorescence::default();
    let none = [
        Fluorescence::none(),
        &default,
        &Fluorescence::new(Vec::new()),
    ];
    for lighting in [
        LightingPreset::Daylight,
        LightingPreset::LightTent,
        LightingPreset::UvLamp365,
    ] {
        let environment = lighting.studio(1.0, 0.4, 0.35);
        let mut compared = 0;
        for (i, (ix, iy)) in [(5.0, 5.0), (6.0, 7.0), (3.0, 9.0), (8.0, 4.0)]
            .into_iter()
            .enumerate()
        {
            let ray = camera.generate_ray(ix, iy, 12.0, 12.0, 0.5, 0.5);
            for s in 0..32u32 {
                let seed = indicatrix::optics::raytracer::hash_u32(s ^ ((i as u32) << 8));
                let hero = (indicatrix::optics::raytracer::hash_u32(seed) as f32) / 4_294_967_295.0;
                let plain =
                    trace_spectral_ray(ray, &planes, &material, 12, environment, seed, hero, None);
                for fl in none {
                    let with = trace_spectral_ray_geom(
                        ray,
                        StoneGeometry::planes_only(&planes),
                        &material,
                        fl,
                        12,
                        environment,
                        seed,
                        hero,
                        None,
                    );
                    assert_eq!(
                        [plain.x.to_bits(), plain.y.to_bits(), plain.z.to_bits()],
                        [with.x.to_bits(), with.y.to_bits(), with.z.to_bits()],
                        "{lighting:?} seed {seed}"
                    );
                    compared += 1;
                }
            }
        }
        assert_eq!(compared, 4 * 32 * 3);
    }

    // Routing: unchanged for every existing preset, and false for the UV lamps.
    let stone = StoneGeometry::planes_only(&planes);
    for preset in LightingPreset::ALL {
        assert_eq!(
            scene_routes_to_gpu(&material, stone, Fluorescence::none(), preset),
            material.gpu_supported() && !preset.is_uv_lamp(),
            "{preset:?}"
        );
    }
}

/// Plan item 3: energy. A colorless n = 1 stone (no refraction or Fresnel losses: the
/// camera ray goes straight through, so the measurement is clean) absorbs through a
/// 450 nm band, sits in a uniform furnace, and carries a `Phi = 1` emitter whose emission
/// spectrum equals its excitation spectrum. At each camera wavelength the traced radiance
/// exceeds the non-fluorescent radiance by the emission the transport estimates; in the
/// optically thin limit that excess is
/// `chord * Phi f(l_em) Int_{300}^{l_em} alpha(l) (l / l_em) L(l) dl`, the absorbed
/// energy times the `lambda_ex / lambda_em` Stokes factor. The measured spectral sum must
/// match that to 12 % (sampling noise about 4 %, thin-limit bias about 3 %).
///
/// A transport that doubled the vertex weight would measure about twice the expectation,
/// and fails: with `energy_ratio` temporarily doubled in `fluorescence::sample_vertex` this
/// test read +103 % (the measured sum 0.174 against 0.0857) and the estimator-level
/// furnace test in `optics::fluorescence` read +100 %; undoubled they read +1 %.
#[test]
fn furnace_emission_energy_matches_absorbed_energy_times_the_stokes_factor() {
    const ALPHA_PEAK: f32 = 0.02;
    const SIGMA_NM: f32 = 25.0;
    const SAMPLES_PER_WAVELENGTH: u32 = 4000;
    let band = AbsorptionBand::new(450.0, SIGMA_NM, ALPHA_PEAK);
    let stone = GemMaterial::new_custom("n = 1 absorber", 1.0, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_chromophore_absorption(AbsorptionTensor::isotropic(vec![band]));
    let emitter = |phi: f32| FluorescentEmitter {
        excitation: vec![band],
        emission: vec![EmissionBand::new(450.0, SIGMA_NM * 2.354_82, 1.0)],
        quantum_yield: phi,
    };
    let glowing = Fluorescence::new(vec![emitter(1.0)]);
    let dark = Fluorescence::new(vec![emitter(0.0)]);
    let env_map = EnvironmentMap::uniform(1, 1, [1.0, 1.0, 1.0]);
    let environment = EnvironmentSource::HdrMap(&env_map);
    let planes = StandardGemCuts::standard_round_brilliant();
    let geom = StoneGeometry::planes_only(&planes);

    // One camera ray through the table, and its chord through the stone.
    let ray = Camera::new(0.35, 0.28, 5.0, 18.0).generate_ray(6.0, 6.0, 12.0, 12.0, 0.5, 0.5);
    let entry = intersect_polyhedron(ray, &planes).expect("the central ray hits the stone");
    let inside = Ray {
        origin: ray.origin + (entry.t + 1e-4) * ray.dir,
        dir: ray.dir,
    };
    let exit = intersect_polyhedron(inside, &planes).expect("and leaves it");
    let chord = exit.t + 1e-4;
    assert!(chord > 0.5, "chord {chord}");

    // Spectral radiance at `lambda`, one estimate per seed, from the traced XYZ: with every
    // channel at one wavelength, Y = Y-bar(l) * L(l) * 400 / 106.856 (times the sample
    // weight). The same seeds drive the glowing and the dark render, so a stochastic
    // event shared by both (the small Fresnel reflection at the entry facet, which sends
    // a path straight back out) cancels in their difference.
    let radiance_samples = |fluorescence: &Fluorescence, lambda: f32, samples: u32| -> Vec<f64> {
        let hero = if fluorescence.emitters()[0].quantum_yield > 0.0 {
            // First half of the mixture: the uniform branch, `lambda = 380 + 800 hero`.
            (lambda - 380.0) / 800.0
        } else {
            (lambda - 380.0) / 400.0
        };
        let (check, weight) = fluorescence.camera_wavelength(hero);
        assert!((check - lambda).abs() < 1e-3, "{check} vs {lambda}");
        let y_bar = f64::from(cie_1931_cmf(lambda).y);
        (0..samples)
            .map(|s| {
                let seed = indicatrix::optics::raytracer::hash_u32(s ^ 0x464C_5543);
                let xyz = trace_spectral_ray_geom(
                    ray,
                    geom,
                    &stone,
                    fluorescence,
                    8,
                    environment,
                    seed,
                    hero,
                    None,
                );
                f64::from(xyz.y) / (f64::from(weight) * y_bar * 400.0 / 106.856)
            })
            .collect()
    };

    let alpha = |l: f32| band.evaluate(l);
    let sigma_em = SIGMA_NM;
    let f_em = |l: f32| {
        (-0.5 * ((l - 450.0) / sigma_em).powi(2)).exp()
            / (sigma_em * (2.0 * std::f32::consts::PI).sqrt())
    };

    let mut measured = 0.0f64;
    let mut expected = 0.0f64;
    for lambda in (400..=520).step_by(10).map(|l| l as f32) {
        let with_glow = radiance_samples(&glowing, lambda, SAMPLES_PER_WAVELENGTH);
        let baseline = radiance_samples(&dark, lambda, SAMPLES_PER_WAVELENGTH);
        measured += with_glow
            .iter()
            .zip(&baseline)
            .map(|(g, d)| g - d)
            .sum::<f64>()
            / f64::from(SAMPLES_PER_WAVELENGTH);

        // Independent of every table in the crate: the analytic bands, 0.1 nm steps.
        let steps = ((lambda - 300.0) * 10.0) as usize;
        let inner: f64 = (0..steps)
            .map(|i| {
                let l = (i as f32 + 0.5).mul_add(0.1, 300.0);
                f64::from(alpha(l))
                    * f64::from(l / lambda)
                    * f64::from(rgb_to_spectral_radiance([1.0, 1.0, 1.0], l))
                    * 0.1
            })
            .sum();
        expected = (f64::from(chord) * f64::from(f_em(lambda))).mul_add(inner, expected);
    }
    let rel = (measured - expected) / expected;
    eprintln!("furnace emission: measured {measured:.5e}, expected {expected:.5e}, rel {rel:+.4}");
    assert!(
        rel.abs() < 0.12,
        "measured emitted radiance {measured:.5e} vs the analytic {expected:.5e} ({rel:+.3})"
    );
}

/// Plan item 4: ruby under the 365 nm lamp. The ruby-like stone with its emitter renders
/// with hue `h_ab` in [0, 40] degrees (the red R line), and more than ten times the
/// luminance of the same stone with `Phi = 0`; with the emitter quenched as the plan's Fe
/// law gives (`Phi_eff = Phi_0 / (1 + (c / c_half)^n)`, 1.0 wt% `FeO` against
/// `c_half = 0.2`, `n = 2`: 3.5 % of the yield) the luminance falls below 10 % of the
/// unquenched render.
#[test]
fn ruby_glows_red_under_the_365nm_lamp_and_quenching_dims_it() {
    const SPP: u32 = 640;
    let stone = ruby_stone(2.0);
    let lamp = LightingPreset::UvLamp365;
    let glowing = Fluorescence::new(vec![ruby_emitter(0.9, 2.0)]);
    let zero_yield = Fluorescence::new(vec![ruby_emitter(0.0, 2.0)]);
    let fe_over_half = 1.0f32 / 0.2;
    let quenched_phi = 0.9 / fe_over_half.mul_add(fe_over_half, 1.0);
    let quenched = Fluorescence::new(vec![ruby_emitter(quenched_phi, 2.0)]);

    let with = render_mean(&stone, &glowing, lamp, SPP, 0x5255_4259);
    let without = render_mean(&stone, &zero_yield, lamp, SPP, 0x5255_4259);
    let dimmed = render_mean(&stone, &quenched, lamp, SPP, 0x5255_4259);
    let hue = lab_hue_deg(with);
    eprintln!(
        "ruby under UV 365: with Phi=0.9 XYZ {with:?} (Y {:.4e}), hue {hue:.1} deg; \
         Phi=0 Y {:.4e}; quenched (Phi {quenched_phi:.4}) Y {:.4e}; ratios {:.1}x and {:.4}",
        with.y,
        without.y,
        dimmed.y,
        with.y / without.y.max(1e-30),
        dimmed.y / with.y
    );
    assert!(with.y > 0.0 && with.is_finite(), "{with:?}");
    assert!((0.0..=40.0).contains(&hue), "hue {hue}");
    assert!(
        with.y > 10.0 * without.y,
        "luminance {} must exceed ten times the Phi = 0 render {}",
        with.y,
        without.y
    );
    assert!(
        dimmed.y < 0.1 * with.y,
        "the quenched render {} must be under 10 % of the unquenched {}",
        dimmed.y,
        with.y
    );
}

/// `host_material` with the absorption of `recipe` (resolved from the catalogue, per mm) at
/// the 3.5 mm per model unit scale of a 7 mm stone, and the fluorescence the same recipe
/// resolves to: what a physics-mode custom material renders with.
fn recipe_stone(host_material: GemMaterial, recipe: &colorRecipe) -> (GemMaterial, Fluorescence) {
    let catalogue = ChromophoreCatalogue::global();
    let (tensor, _) = resolve(recipe, catalogue).expect("the recipe resolves");
    (
        host_material
            .with_chromophore_absorption(tensor)
            .with_absorption_path_scale(3.5),
        resolve_fluorescence(catalogue, recipe),
    )
}

/// `fluorescence` with every emitter's quantum yield set to zero (the same stone, not glowing).
fn without_yield(fluorescence: &Fluorescence) -> Fluorescence {
    Fluorescence::new(
        fluorescence
            .emitters()
            .iter()
            .cloned()
            .map(|e| FluorescentEmitter {
                quantum_yield: 0.0,
                ..e
            })
            .collect(),
    )
}

/// Plan item 4 through the recipe route: corundum + 0.5 wt% `Cr2O3`, resolved by
/// `resolve_fluorescence`, glows red under the 365 nm lamp (hue `h_ab` in [0, 40] degrees,
/// more than ten times the luminance of the `Phi = 0` render), and 1.0 wt% `FeO` added to
/// the recipe takes the glow below 10 % of the unquenched render.
#[test]
fn recipe_ruby_glows_red_under_the_365nm_lamp_and_iron_quenches_it() {
    const SPP: u32 = 320;
    let catalogue = ChromophoreCatalogue::global();
    let host = catalogue.host("corundum").expect("corundum");
    let fe_per_wt_pct_feo = host.n_site_for_unit("wt_pct_oxide:FeO")
        / host.n_site_for_unit(&host.element_unit("Fe").expect("Fe unit"));
    let mut ruby = colorRecipe::new("corundum", catalogue.data_version);
    ruby.set_amount("Cr", 0.5);
    let mut iron = ruby.clone();
    iron.set_amount("Fe", fe_per_wt_pct_feo);

    let lamp = LightingPreset::UvLamp365;
    let (stone, glowing) = recipe_stone(GemMaterial::ruby(), &ruby);
    let (iron_stone, quenched) = recipe_stone(GemMaterial::ruby(), &iron);
    assert_eq!(glowing.emitters().len(), 1);
    let with = render_mean(&stone, &glowing, lamp, SPP, 0x5255_5243);
    let without = render_mean(&stone, &without_yield(&glowing), lamp, SPP, 0x5255_5243);
    let dimmed = render_mean(&iron_stone, &quenched, lamp, SPP, 0x5255_5243);
    let hue = lab_hue_deg(with);
    eprintln!(
        "recipe ruby under UV 365: hue {hue:.1}, Y {:.4e}, Phi=0 Y {:.4e}, with FeO 1 wt% Y {:.4e}",
        with.y, without.y, dimmed.y
    );
    assert!(with.y > 0.0 && with.is_finite(), "{with:?}");
    assert!((0.0..=40.0).contains(&hue), "hue {hue}");
    assert!(with.y > 10.0 * without.y, "{} vs {}", with.y, without.y);
    assert!(dimmed.y < 0.1 * with.y, "{} vs {}", dimmed.y, with.y);
}

/// Plan item 6: a diamond with the N3 centre (nitrogen aggregates, the recipe's `N`) under
/// the 365 nm lamp glows blue: hue `h_ab` in [230, 290] degrees.
#[test]
#[ignore = "slow in debug: 12 x 12 pixels x 1024 samples of a weakly absorbing, weakly glowing stone"]
fn recipe_diamond_n3_glows_blue_under_the_365nm_lamp() {
    const SPP: u32 = 1024;
    let catalogue = ChromophoreCatalogue::global();
    let mut recipe = colorRecipe::new("diamond", catalogue.data_version);
    recipe.set_amount("N", 1500.0);
    let (stone, glowing) = recipe_stone(GemMaterial::diamond(), &recipe);
    assert_eq!(glowing.emitters().len(), 1, "the N3 centre");
    let with = render_mean(
        &stone,
        &glowing,
        LightingPreset::UvLamp365,
        SPP,
        0x4433_4e33,
    );
    let hue = lab_hue_deg(with);
    eprintln!("recipe diamond N3 under UV 365: XYZ {with:?}, hue {hue:.1}");
    assert!(with.y > 0.0 && with.is_finite(), "{with:?}");
    assert!((230.0..=290.0).contains(&hue), "hue {hue}");
}

/// Plan item 5: the 395 nm LED lights a non-fluorescent stone faintly (its tail reaches
/// 380-420 nm): the mean luminance is positive, below 5 % of Daylight's, and violet.
#[test]
fn uv395_lamp_lights_a_non_fluorescent_stone_only_faintly() {
    let diamond = GemMaterial::diamond();
    let none = Fluorescence::none();
    let uv = render_mean(&diamond, none, LightingPreset::UvLamp395, 96, 0x5556_3935);
    let day = render_mean(&diamond, none, LightingPreset::Daylight, 96, 0x5556_3935);
    eprintln!(
        "UV 395 Y {:.4e}, Daylight Y {:.4e}, ratio {:.2e}",
        uv.y,
        day.y,
        uv.y / day.y
    );
    assert!(uv.is_finite() && uv.y > 0.0, "{uv:?}");
    assert!(uv.y < 0.05 * day.y, "{} vs {}", uv.y, day.y);
    assert!(uv.z > uv.y && uv.z > uv.x, "violet, not white: {uv:?}");
}

/// Plan item 7: the same seed gives the same bits, fluorescent or not; another seed does
/// not.
#[test]
fn fluorescent_traces_are_deterministic() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let stone = ruby_stone(2.0);
    let fluorescence = Fluorescence::new(vec![ruby_emitter(0.9, 2.0)]);
    let again = fluorescence.clone();
    let environment = LightingPreset::UvLamp365.studio(1.0, 0.4, 0.35);
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let ray = camera.generate_ray(6.0, 6.0, 12.0, 12.0, 0.5, 0.5);
    let trace = |fl: &Fluorescence, seed: u32, hero: f32| {
        trace_spectral_ray_geom(
            ray,
            StoneGeometry::planes_only(&planes),
            &stone,
            fl,
            12,
            environment,
            seed,
            hero,
            None,
        )
    };
    let mut any_signal = false;
    let mut any_difference = false;
    for s in 0..4000u32 {
        let seed = indicatrix::optics::raytracer::hash_u32(s);
        // Heroes in the mixture's second half, which lands on the emission line.
        let hero = 0.5 + 0.5 * (s as f32 + 0.5) / 4000.0;
        let a = trace(&fluorescence, seed, hero);
        let b = trace(&again, seed, hero);
        assert_eq!(
            [a.x.to_bits(), a.y.to_bits(), a.z.to_bits()],
            [b.x.to_bits(), b.y.to_bits(), b.z.to_bits()]
        );
        any_signal |= a.y > 0.0;
        any_difference |= trace(&fluorescence, seed ^ 0x9E37, hero) != a;
    }
    assert!(any_signal, "some of the 4000 paths must carry light");
    assert!(any_difference, "another seed must give another result");
}
