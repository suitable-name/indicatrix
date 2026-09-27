// ---------------------------------------------------------------------------------
// optics::raytracer::Camera::generate_ray -- bit-exact per camera_check.
// ---------------------------------------------------------------------------------

struct RayGen {
    origin: vec3<f32>,
    dir: vec3<f32>,
}

fn generate_camera_ray(pixel: u32, jx: f32, jy: f32) -> RayGen {
    let x = f32(pixel % u32(camera.width));
    let y = f32(pixel / u32(camera.width));
    let aspect = camera.width / camera.height;
    let u = ((x + jx) / camera.width - 0.5) * 2.0 * aspect * camera.fov_tan;
    let v = (0.5 - (y + jy) / camera.height) * 2.0 * camera.fov_tan;
    let dir = normalize(camera.forward + camera.right * u + camera.up * v);
    var result: RayGen;
    result.origin = camera.origin;
    result.dir = dir;
    return result;
}

// optics::raytracer::apply_internal_mode_coupling -- stochastic o<->e re-coupling at an
// INTERNAL reflection inside an anisotropic crystal. This is a RELABELING, not a SPLIT:
// unlike the entry split (BIREFRINGENT_SPLIT_STREAM, where one incident ray genuinely
// becomes two physically distinct rays each carrying its own ~0.5 energy share already
// matched by its own 0.5 selection probability, needing no `1/p` scaling), this draw
// only re-rolls which eigenmode label governs the index used for the path's next
// bounce -- no new ray, no energy split, nothing for `stokes`/`path_pdf` to do. This
// draw's own selection probability is polarization-weighted
// (`entry_eigenmode_selection`) rather than a blanket 50/50, which is safe precisely
// because `stokes`/`path_pdf` are never touched here.

// optics::raytracer::refraction::entry_eigenmode_selection -- polarization-weighted
// probability that the uniaxial ORDINARY eigenmode is the physically correct label for
// the hero-driven path, plus that eigenmode's own doubled polarization azimuth
// (`cos(2*psi_o)`, `sin(2*psi_o)`) in the SAME Stokes reference frame
// `current_plane_normal` establishes (Malus's law energy split; a unit-vector
// double-angle identity in place of an atan2/half-angle round-trip). WGSL has no
// `Option`, so `valid == 0u` stands in for the Rust `None` case (negligible `i`,
// degenerate plane of incidence, or negligible linear polarization) -- callers must
// check it before trusting `p_o`/`cos_2psi_o`/`sin_2psi_o`.
struct EntryEigenmodeSelection {
    valid: u32,
    p_o: f32,
    cos_2psi_o: f32,
    sin_2psi_o: f32,
}

fn entry_eigenmode_selection(
    c_axis: vec3<f32>,
    current_plane_normal: vec3<f32>,
    k_hat: vec3<f32>,
    hero_i: f32,
    hero_q: f32,
    hero_u: f32,
) -> EntryEigenmodeSelection {
    var result: EntryEigenmodeSelection;
    result.valid = 0u;
    result.p_o = 0.5;
    result.cos_2psi_o = 0.0;
    result.sin_2psi_o = 0.0;
    if (hero_i <= 1e-7 || dot(current_plane_normal, current_plane_normal) <= 1e-6) {
        return result;
    }
    if (fma(hero_q, hero_q, hero_u * hero_u) <= 1e-12) {
        return result;
    }
    let o_hat = ordinary_eigen_polarization(k_hat, c_axis);
    // `current_plane_normal` is already unit and exactly perpendicular to `k_hat` --
    // no re-orthogonalization needed.
    let s_hat = current_plane_normal;
    let p_hat = cross(k_hat, s_hat);
    let s_comp = dot(o_hat, s_hat);
    let p_comp = dot(o_hat, p_hat);
    let cos_2psi_o = fma(s_comp, s_comp, -(p_comp * p_comp));
    let sin_2psi_o = 2.0 * s_comp * p_comp;
    result.valid = 1u;
    result.p_o = clamp(0.5 + 0.5 * fma(hero_q, cos_2psi_o, hero_u * sin_2psi_o) / hero_i, 0.0, 1.0);
    result.cos_2psi_o = cos_2psi_o;
    result.sin_2psi_o = sin_2psi_o;
    return result;
}

// Mirrors `apply_internal_mode_coupling`'s mode-selection draw. `has_exact_p_o`/
// `exact_p_o` stand in for its `exact_p_o: Option<f32>` (WGSL has no `Option`): when
// set, reuses the closed-form `internal_solve` Poynting-weighted `R_o/(R_o+R_e)` split
// already computed for reflected-energy accounting, instead of the
// polarization-projection heuristic (`entry_eigenmode_selection`). `is_biaxial` keeps
// the blanket 50/50 unconditionally, matching the CPU's biaxial exclusion.
fn internal_mode_coupling_draw(
    c_axis: vec3<f32>,
    is_biaxial: bool,
    current_plane_normal: vec3<f32>,
    new_k: vec3<f32>,
    hero_i: f32,
    hero_q: f32,
    hero_u: f32,
    has_exact_p_o: bool,
    exact_p_o: f32,
    seed0: u32,
    bounce: u32,
) -> bool {
    var p_o = 0.5;
    if (has_exact_p_o) {
        p_o = exact_p_o;
    } else if (!is_biaxial) {
        let sel = entry_eigenmode_selection(c_axis, current_plane_normal, new_k, hero_i, hero_q, hero_u);
        if (sel.valid != 0u) {
            p_o = sel.p_o;
        }
    }
    let split_rand = f32(hash_u32(seed0 ^ hash_u32(bounce ^ MODE_COUPLING_STREAM))) / 4294967295.0;
    return split_rand >= p_o;
}

// One-bounce status codes -- see this file's header comment.
const BOUNCE_STATUS_CONTINUE: u32 = 0u;
const BOUNCE_STATUS_TERMINATE: u32 = 1u;

