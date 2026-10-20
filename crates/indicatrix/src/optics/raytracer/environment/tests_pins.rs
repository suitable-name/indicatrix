//! Bit pins of the "As Lit" environment lookup (backdrop 0), recorded from the code as it was
//! before the backdrop-tray campaign.
//!
//! The tray changes only the `tray > 0.0` branch of every lighting model; with no backdrop the
//! radiance of every direction must stay exactly the bits it has today. This file is the guard:
//! for every [`LightingPreset`], two light poses, the observer off and on, three head-shadow
//! sizes and 48 directions (16 clearly above the horizon, 16 inside the +-0.05 horizon band,
//! 16 clearly below), the radiance bits at 450, 550 and 650 nm are folded into one FNV-1a hash
//! per row and compared with the literals in [`PINS`].
//!
//! The literals are not guessed: [`PINS`] starts as all zeros ("unpinned") and the test is
//! `#[ignore]`d until the owner has run it once at Gate A:
//!
//! ```text
//! cargo test -p indicatrix --lib as_lit_is_bit_identical -- --ignored --nocapture
//! ```
//!
//! The run prints the whole table in paste-ready form; the lead copies it into [`PINS`] and
//! removes the `#[ignore]`. From then on a zero table is not accepted: the test compares every
//! row and names the failing (preset, pose, observer, head shadow) combination.

use super::*;
use crate::optics::studio_rig::StudioRig;

/// The three wavelengths each direction is evaluated at.
const LAMBDAS: [f32; 3] = [450.0, 550.0, 650.0];
/// A non-trivial exposure, so the exposure terms inside each lighting arm are exercised.
const EXPOSURE: f32 = 1.3;
/// `(light_yaw, light_pitch)` of the two light poses.
const POSES: [(f32, f32); 2] = [(0.84, 0.94), (-2.1, 0.3)];
/// Head-shadow sizes in degrees: off, the default, wide.
const SHADOWS_DEG: [f32; 3] = [0.0, 16.0, 30.0];
/// Unit horizontal directions `(x, z)` the elevation rings are spread over.
const AZIMUTHS: [(f32, f32); 4] = [(1.0, 0.0), (0.0, 1.0), (0.6, 0.8), (-0.8, -0.6)];
/// Direction heights `y`: above the horizon, inside the horizon band, below the horizon.
const HEIGHTS: [[f32; 4]; 3] = [
    [0.98, 0.7, 0.35, 0.1],
    [0.049, 0.02, -0.02, -0.049],
    [-0.1, -0.35, -0.7, -0.98],
];

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

/// One FNV-1a hash per (preset, pose, observer, head shadow) row, in the order the test
/// enumerates them: `LightingPreset::ALL`, then pose, then observer off/on, then head shadow.
/// All zeros until Gate A.
const PINS: [u64; 180] = [
    0x4bbf_4f01_a2ac_4d0d, // LightTent pose 0 observer 0 head shadow 0
    0x4bbf_4f01_a2ac_4d0d, // LightTent pose 0 observer 0 head shadow 16
    0x4bbf_4f01_a2ac_4d0d, // LightTent pose 0 observer 0 head shadow 30
    0x4bbf_4f01_a2ac_4d0d, // LightTent pose 0 observer 1 head shadow 0
    0x5a2e_8322_8229_f076, // LightTent pose 0 observer 1 head shadow 16
    0x178f_459a_1b35_55db, // LightTent pose 0 observer 1 head shadow 30
    0x2e5c_fe83_0967_4f4f, // LightTent pose 1 observer 0 head shadow 0
    0x2e5c_fe83_0967_4f4f, // LightTent pose 1 observer 0 head shadow 16
    0x2e5c_fe83_0967_4f4f, // LightTent pose 1 observer 0 head shadow 30
    0x2e5c_fe83_0967_4f4f, // LightTent pose 1 observer 1 head shadow 0
    0x171b_e73e_e874_f14f, // LightTent pose 1 observer 1 head shadow 16
    0x5704_63e8_694d_6b45, // LightTent pose 1 observer 1 head shadow 30
    0x6b8e_f66d_51fa_cf95, // IsoHemisphere pose 0 observer 0 head shadow 0
    0x6b8e_f66d_51fa_cf95, // IsoHemisphere pose 0 observer 0 head shadow 16
    0x6b8e_f66d_51fa_cf95, // IsoHemisphere pose 0 observer 0 head shadow 30
    0x6b8e_f66d_51fa_cf95, // IsoHemisphere pose 0 observer 1 head shadow 0
    0x69e0_7608_30ea_2421, // IsoHemisphere pose 0 observer 1 head shadow 16
    0xc197_7a07_dd98_96f6, // IsoHemisphere pose 0 observer 1 head shadow 30
    0x6b8e_f66d_51fa_cf95, // IsoHemisphere pose 1 observer 0 head shadow 0
    0x6b8e_f66d_51fa_cf95, // IsoHemisphere pose 1 observer 0 head shadow 16
    0x6b8e_f66d_51fa_cf95, // IsoHemisphere pose 1 observer 0 head shadow 30
    0x6b8e_f66d_51fa_cf95, // IsoHemisphere pose 1 observer 1 head shadow 0
    0x69e0_7608_30ea_2421, // IsoHemisphere pose 1 observer 1 head shadow 16
    0xc197_7a07_dd98_96f6, // IsoHemisphere pose 1 observer 1 head shadow 30
    0xeb65_7e58_f073_d005, // WhiteTray pose 0 observer 0 head shadow 0
    0xeb65_7e58_f073_d005, // WhiteTray pose 0 observer 0 head shadow 16
    0xeb65_7e58_f073_d005, // WhiteTray pose 0 observer 0 head shadow 30
    0xeb65_7e58_f073_d005, // WhiteTray pose 0 observer 1 head shadow 0
    0x5e3c_499e_f461_c4a5, // WhiteTray pose 0 observer 1 head shadow 16
    0x4d18_d615_2db2_a2dd, // WhiteTray pose 0 observer 1 head shadow 30
    0xeb65_7e58_f073_d005, // WhiteTray pose 1 observer 0 head shadow 0
    0xeb65_7e58_f073_d005, // WhiteTray pose 1 observer 0 head shadow 16
    0xeb65_7e58_f073_d005, // WhiteTray pose 1 observer 0 head shadow 30
    0xeb65_7e58_f073_d005, // WhiteTray pose 1 observer 1 head shadow 0
    0x5e3c_499e_f461_c4a5, // WhiteTray pose 1 observer 1 head shadow 16
    0x4d18_d615_2db2_a2dd, // WhiteTray pose 1 observer 1 head shadow 30
    0x8e57_6d4d_771c_0fab, // ShopLights pose 0 observer 0 head shadow 0
    0x8e57_6d4d_771c_0fab, // ShopLights pose 0 observer 0 head shadow 16
    0x8e57_6d4d_771c_0fab, // ShopLights pose 0 observer 0 head shadow 30
    0x8e57_6d4d_771c_0fab, // ShopLights pose 0 observer 1 head shadow 0
    0x2864_d1fc_cd9b_5bdb, // ShopLights pose 0 observer 1 head shadow 16
    0xe87f_4634_70ce_8734, // ShopLights pose 0 observer 1 head shadow 30
    0x5d22_5e2f_e9ab_2cb1, // ShopLights pose 1 observer 0 head shadow 0
    0x5d22_5e2f_e9ab_2cb1, // ShopLights pose 1 observer 0 head shadow 16
    0x5d22_5e2f_e9ab_2cb1, // ShopLights pose 1 observer 0 head shadow 30
    0x5d22_5e2f_e9ab_2cb1, // ShopLights pose 1 observer 1 head shadow 0
    0x6899_eeb5_6e26_7e75, // ShopLights pose 1 observer 1 head shadow 16
    0x62d0_0081_2b57_a084, // ShopLights pose 1 observer 1 head shadow 30
    0x1bb0_fdd4_d5f4_705e, // WindowDaylight pose 0 observer 0 head shadow 0
    0x1bb0_fdd4_d5f4_705e, // WindowDaylight pose 0 observer 0 head shadow 16
    0x1bb0_fdd4_d5f4_705e, // WindowDaylight pose 0 observer 0 head shadow 30
    0x1bb0_fdd4_d5f4_705e, // WindowDaylight pose 0 observer 1 head shadow 0
    0x0381_a399_f0a4_7ce6, // WindowDaylight pose 0 observer 1 head shadow 16
    0x4149_b587_db6e_fcd8, // WindowDaylight pose 0 observer 1 head shadow 30
    0x271b_b8a2_2703_5500, // WindowDaylight pose 1 observer 0 head shadow 0
    0x271b_b8a2_2703_5500, // WindowDaylight pose 1 observer 0 head shadow 16
    0x271b_b8a2_2703_5500, // WindowDaylight pose 1 observer 0 head shadow 30
    0x271b_b8a2_2703_5500, // WindowDaylight pose 1 observer 1 head shadow 0
    0x5fcf_9d1a_d970_8294, // WindowDaylight pose 1 observer 1 head shadow 16
    0x943a_9406_0104_2b23, // WindowDaylight pose 1 observer 1 head shadow 30
    0xc7fa_3633_20da_8f59, // DaylightDome pose 0 observer 0 head shadow 0
    0xc7fa_3633_20da_8f59, // DaylightDome pose 0 observer 0 head shadow 16
    0xc7fa_3633_20da_8f59, // DaylightDome pose 0 observer 0 head shadow 30
    0xc7fa_3633_20da_8f59, // DaylightDome pose 0 observer 1 head shadow 0
    0x0b73_795e_43a9_e00d, // DaylightDome pose 0 observer 1 head shadow 16
    0x68fa_5e77_e73a_08ab, // DaylightDome pose 0 observer 1 head shadow 30
    0x2f56_d761_0622_c355, // DaylightDome pose 1 observer 0 head shadow 0
    0x2f56_d761_0622_c355, // DaylightDome pose 1 observer 0 head shadow 16
    0x2f56_d761_0622_c355, // DaylightDome pose 1 observer 0 head shadow 30
    0x2f56_d761_0622_c355, // DaylightDome pose 1 observer 1 head shadow 0
    0x5802_b636_7455_7b65, // DaylightDome pose 1 observer 1 head shadow 16
    0x189c_225c_32da_d14f, // DaylightDome pose 1 observer 1 head shadow 30
    0xc7fa_3633_20da_8f59, // DaylightSun pose 0 observer 0 head shadow 0
    0xc7fa_3633_20da_8f59, // DaylightSun pose 0 observer 0 head shadow 16
    0xc7fa_3633_20da_8f59, // DaylightSun pose 0 observer 0 head shadow 30
    0xc7fa_3633_20da_8f59, // DaylightSun pose 0 observer 1 head shadow 0
    0x0b73_795e_43a9_e00d, // DaylightSun pose 0 observer 1 head shadow 16
    0x68fa_5e77_e73a_08ab, // DaylightSun pose 0 observer 1 head shadow 30
    0x2f56_d761_0622_c355, // DaylightSun pose 1 observer 0 head shadow 0
    0x2f56_d761_0622_c355, // DaylightSun pose 1 observer 0 head shadow 16
    0x2f56_d761_0622_c355, // DaylightSun pose 1 observer 0 head shadow 30
    0x2f56_d761_0622_c355, // DaylightSun pose 1 observer 1 head shadow 0
    0x5802_b636_7455_7b65, // DaylightSun pose 1 observer 1 head shadow 16
    0x189c_225c_32da_d14f, // DaylightSun pose 1 observer 1 head shadow 30
    0x705f_43be_7ca7_cca0, // Daylight pose 0 observer 0 head shadow 0
    0x705f_43be_7ca7_cca0, // Daylight pose 0 observer 0 head shadow 16
    0x705f_43be_7ca7_cca0, // Daylight pose 0 observer 0 head shadow 30
    0x705f_43be_7ca7_cca0, // Daylight pose 0 observer 1 head shadow 0
    0x705f_43be_7ca7_cca0, // Daylight pose 0 observer 1 head shadow 16
    0x705f_43be_7ca7_cca0, // Daylight pose 0 observer 1 head shadow 30
    0x4b8a_9577_fe5b_f416, // Daylight pose 1 observer 0 head shadow 0
    0x4b8a_9577_fe5b_f416, // Daylight pose 1 observer 0 head shadow 16
    0x4b8a_9577_fe5b_f416, // Daylight pose 1 observer 0 head shadow 30
    0x4b8a_9577_fe5b_f416, // Daylight pose 1 observer 1 head shadow 0
    0x4b8a_9577_fe5b_f416, // Daylight pose 1 observer 1 head shadow 16
    0x4b8a_9577_fe5b_f416, // Daylight pose 1 observer 1 head shadow 30
    0xb21c_9087_1ab5_6b58, // Incandescent pose 0 observer 0 head shadow 0
    0xb21c_9087_1ab5_6b58, // Incandescent pose 0 observer 0 head shadow 16
    0xb21c_9087_1ab5_6b58, // Incandescent pose 0 observer 0 head shadow 30
    0xb21c_9087_1ab5_6b58, // Incandescent pose 0 observer 1 head shadow 0
    0xb21c_9087_1ab5_6b58, // Incandescent pose 0 observer 1 head shadow 16
    0xb21c_9087_1ab5_6b58, // Incandescent pose 0 observer 1 head shadow 30
    0x8d6e_6991_5278_0665, // Incandescent pose 1 observer 0 head shadow 0
    0x8d6e_6991_5278_0665, // Incandescent pose 1 observer 0 head shadow 16
    0x8d6e_6991_5278_0665, // Incandescent pose 1 observer 0 head shadow 30
    0x8d6e_6991_5278_0665, // Incandescent pose 1 observer 1 head shadow 0
    0x8d6e_6991_5278_0665, // Incandescent pose 1 observer 1 head shadow 16
    0x8d6e_6991_5278_0665, // Incandescent pose 1 observer 1 head shadow 30
    0xfdfd_0994_9973_a410, // IlluminantA pose 0 observer 0 head shadow 0
    0xfdfd_0994_9973_a410, // IlluminantA pose 0 observer 0 head shadow 16
    0xfdfd_0994_9973_a410, // IlluminantA pose 0 observer 0 head shadow 30
    0xfdfd_0994_9973_a410, // IlluminantA pose 0 observer 1 head shadow 0
    0xfdfd_0994_9973_a410, // IlluminantA pose 0 observer 1 head shadow 16
    0xfdfd_0994_9973_a410, // IlluminantA pose 0 observer 1 head shadow 30
    0x513c_1bc6_2228_085c, // IlluminantA pose 1 observer 0 head shadow 0
    0x513c_1bc6_2228_085c, // IlluminantA pose 1 observer 0 head shadow 16
    0x513c_1bc6_2228_085c, // IlluminantA pose 1 observer 0 head shadow 30
    0x513c_1bc6_2228_085c, // IlluminantA pose 1 observer 1 head shadow 0
    0x513c_1bc6_2228_085c, // IlluminantA pose 1 observer 1 head shadow 16
    0x513c_1bc6_2228_085c, // IlluminantA pose 1 observer 1 head shadow 30
    0x267e_ba12_797a_c638, // RingLights pose 0 observer 0 head shadow 0
    0x267e_ba12_797a_c638, // RingLights pose 0 observer 0 head shadow 16
    0x267e_ba12_797a_c638, // RingLights pose 0 observer 0 head shadow 30
    0x267e_ba12_797a_c638, // RingLights pose 0 observer 1 head shadow 0
    0x267e_ba12_797a_c638, // RingLights pose 0 observer 1 head shadow 16
    0x267e_ba12_797a_c638, // RingLights pose 0 observer 1 head shadow 30
    0x393c_7768_63a6_afb6, // RingLights pose 1 observer 0 head shadow 0
    0x393c_7768_63a6_afb6, // RingLights pose 1 observer 0 head shadow 16
    0x393c_7768_63a6_afb6, // RingLights pose 1 observer 0 head shadow 30
    0x393c_7768_63a6_afb6, // RingLights pose 1 observer 1 head shadow 0
    0x393c_7768_63a6_afb6, // RingLights pose 1 observer 1 head shadow 16
    0x393c_7768_63a6_afb6, // RingLights pose 1 observer 1 head shadow 30
    0x4910_4652_9b65_ab65, // DarkSpotlight pose 0 observer 0 head shadow 0
    0x4910_4652_9b65_ab65, // DarkSpotlight pose 0 observer 0 head shadow 16
    0x4910_4652_9b65_ab65, // DarkSpotlight pose 0 observer 0 head shadow 30
    0x4910_4652_9b65_ab65, // DarkSpotlight pose 0 observer 1 head shadow 0
    0x4910_4652_9b65_ab65, // DarkSpotlight pose 0 observer 1 head shadow 16
    0x4910_4652_9b65_ab65, // DarkSpotlight pose 0 observer 1 head shadow 30
    0x84e3_28f2_dadc_ce2d, // DarkSpotlight pose 1 observer 0 head shadow 0
    0x84e3_28f2_dadc_ce2d, // DarkSpotlight pose 1 observer 0 head shadow 16
    0x84e3_28f2_dadc_ce2d, // DarkSpotlight pose 1 observer 0 head shadow 30
    0x84e3_28f2_dadc_ce2d, // DarkSpotlight pose 1 observer 1 head shadow 0
    0x84e3_28f2_dadc_ce2d, // DarkSpotlight pose 1 observer 1 head shadow 16
    0x84e3_28f2_dadc_ce2d, // DarkSpotlight pose 1 observer 1 head shadow 30
    0xe3ff_5bbc_0bb6_b305, // Aset pose 0 observer 0 head shadow 0
    0xe3ff_5bbc_0bb6_b305, // Aset pose 0 observer 0 head shadow 16
    0xe3ff_5bbc_0bb6_b305, // Aset pose 0 observer 0 head shadow 30
    0xe3ff_5bbc_0bb6_b305, // Aset pose 0 observer 1 head shadow 0
    0xe3ff_5bbc_0bb6_b305, // Aset pose 0 observer 1 head shadow 16
    0xe3ff_5bbc_0bb6_b305, // Aset pose 0 observer 1 head shadow 30
    0xe3ff_5bbc_0bb6_b305, // Aset pose 1 observer 0 head shadow 0
    0xe3ff_5bbc_0bb6_b305, // Aset pose 1 observer 0 head shadow 16
    0xe3ff_5bbc_0bb6_b305, // Aset pose 1 observer 0 head shadow 30
    0xe3ff_5bbc_0bb6_b305, // Aset pose 1 observer 1 head shadow 0
    0xe3ff_5bbc_0bb6_b305, // Aset pose 1 observer 1 head shadow 16
    0xe3ff_5bbc_0bb6_b305, // Aset pose 1 observer 1 head shadow 30
    0xb9d0_059d_6108_3025, // UvLamp365 pose 0 observer 0 head shadow 0
    0xb9d0_059d_6108_3025, // UvLamp365 pose 0 observer 0 head shadow 16
    0xb9d0_059d_6108_3025, // UvLamp365 pose 0 observer 0 head shadow 30
    0xb9d0_059d_6108_3025, // UvLamp365 pose 0 observer 1 head shadow 0
    0xb9d0_059d_6108_3025, // UvLamp365 pose 0 observer 1 head shadow 16
    0xb9d0_059d_6108_3025, // UvLamp365 pose 0 observer 1 head shadow 30
    0xb9d0_059d_6108_3025, // UvLamp365 pose 1 observer 0 head shadow 0
    0xb9d0_059d_6108_3025, // UvLamp365 pose 1 observer 0 head shadow 16
    0xb9d0_059d_6108_3025, // UvLamp365 pose 1 observer 0 head shadow 30
    0xb9d0_059d_6108_3025, // UvLamp365 pose 1 observer 1 head shadow 0
    0xb9d0_059d_6108_3025, // UvLamp365 pose 1 observer 1 head shadow 16
    0xb9d0_059d_6108_3025, // UvLamp365 pose 1 observer 1 head shadow 30
    0xeba4_66cb_6a2c_ddbe, // UvLamp395 pose 0 observer 0 head shadow 0
    0xeba4_66cb_6a2c_ddbe, // UvLamp395 pose 0 observer 0 head shadow 16
    0xeba4_66cb_6a2c_ddbe, // UvLamp395 pose 0 observer 0 head shadow 30
    0xeba4_66cb_6a2c_ddbe, // UvLamp395 pose 0 observer 1 head shadow 0
    0xeba4_66cb_6a2c_ddbe, // UvLamp395 pose 0 observer 1 head shadow 16
    0xeba4_66cb_6a2c_ddbe, // UvLamp395 pose 0 observer 1 head shadow 30
    0x8375_7d95_8ded_d556, // UvLamp395 pose 1 observer 0 head shadow 0
    0x8375_7d95_8ded_d556, // UvLamp395 pose 1 observer 0 head shadow 16
    0x8375_7d95_8ded_d556, // UvLamp395 pose 1 observer 0 head shadow 30
    0x8375_7d95_8ded_d556, // UvLamp395 pose 1 observer 1 head shadow 0
    0x8375_7d95_8ded_d556, // UvLamp395 pose 1 observer 1 head shadow 16
    0x8375_7d95_8ded_d556, // UvLamp395 pose 1 observer 1 head shadow 30
];

/// The hash as a Rust literal with `_` separators (`0x1bb0_fdd4_d5f4_705e`), so a re-pin pastes
/// clean.
fn pin_literal(hash: u64) -> String {
    format!(
        "0x{:04x}_{:04x}_{:04x}_{:04x}",
        (hash >> 48) & 0xffff,
        (hash >> 32) & 0xffff,
        (hash >> 16) & 0xffff,
        hash & 0xffff
    )
}

fn fold(hash: &mut u64, bits: u32) {
    for byte in bits.to_le_bytes() {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

/// The 48 directions: 16 above, 16 in the horizon band, 16 below.
fn directions() -> Vec<Vec3> {
    let mut out = Vec::with_capacity(48);
    for group in HEIGHTS {
        for y in group {
            let radius = y.mul_add(-y, 1.0).sqrt();
            for (hx, hz) in AZIMUTHS {
                out.push(Vec3::new(hx * radius, y, hz * radius));
            }
        }
    }
    out
}

/// The hash of one row, through the tracer's per-escape lookup. Every channel is also checked
/// to equal the single-wavelength public entry point bit for bit (their documented contract).
fn row_hash(
    preset: LightingPreset,
    pose: (f32, f32),
    observer: Vec3,
    shadow_deg: f32,
    dirs: &[Vec3],
) -> u64 {
    let rig = StudioRig::new(pose.0, pose.1);
    let environment = preset
        .studio(EXPOSURE, pose.0, pose.1)
        .with_head_shadow(shadow_deg)
        .with_backdrop(0.0);
    let cosines = head_shadow_cosines(environment.head_shadow_deg());
    let mut hash = FNV_OFFSET;
    for &dir in dirs {
        let channels =
            sample_environment_channels(environment, dir, &LAMBDAS, Some(&rig), observer);
        for (&lambda, &value) in LAMBDAS.iter().zip(&channels) {
            let single = sample_studio_environment_with_rig_shadow(
                dir, lambda, preset, EXPOSURE, &rig, observer, cosines,
            );
            assert_eq!(
                value.to_bits(),
                single.to_bits(),
                "{preset:?} dir {dir:?} at {lambda} nm: channels vs single-wavelength entry"
            );
            fold(&mut hash, value.to_bits());
        }
    }
    hash
}

/// With no backdrop (As Lit) the environment is bit-identical to the pre-tray code for every
/// preset, direction, light pose, observer and head shadow. `B0` records, `B1` keeps.
#[test]
fn as_lit_is_bit_identical() {
    let dirs = directions();
    assert_eq!(dirs.len(), 48);
    let observers = [Vec3::ZERO, Vec3::new(0.1, 0.95, 0.3).normalize()];

    let mut rows = Vec::with_capacity(PINS.len());
    println!("const PINS: [u64; {}] = [", PINS.len());
    for preset in LightingPreset::ALL {
        for (pose_index, &pose) in POSES.iter().enumerate() {
            for (observer_index, &observer) in observers.iter().enumerate() {
                for &shadow_deg in &SHADOWS_DEG {
                    let hash = row_hash(preset, pose, observer, shadow_deg, &dirs);
                    println!(
                        "    {}, // {preset:?} pose {pose_index} observer {observer_index} head shadow {shadow_deg}",
                        pin_literal(hash)
                    );
                    rows.push((
                        format!(
                            "{preset:?} pose {pose_index} observer {observer_index} head shadow {shadow_deg}"
                        ),
                        hash,
                    ));
                }
            }
        }
    }
    println!("];");
    assert_eq!(rows.len(), PINS.len());

    if PINS.iter().all(|&pin| pin == 0) {
        println!("PINS is still all zeros: copy the table above into PINS, then remove #[ignore].");
        return;
    }
    for ((label, hash), &pin) in rows.iter().zip(&PINS) {
        assert_eq!(*hash, pin, "As Lit radiance bits moved for {label}");
    }
}
