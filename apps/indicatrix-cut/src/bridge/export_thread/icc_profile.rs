//! Minimal ICC v2.4 matrix/TRC RGB profile generator for wide-gamut export.
//!
//! # The trap this exists to avoid
//!
//! A PNG holding Display P3 or Rec.2020 pixel values with no colour-space tag is
//! interpreted by every viewer as sRGB -- silently wrong, not just unlabeled. `image`
//! 0.25's `codecs::png::PngEncoder` can write an `iCCP` chunk via
//! `ImageEncoder::set_icc_profile`, but does not generate profile bytes itself.
//!
//! Rather than embedding a third-party profile's raw bytes (unverifiable here, and
//! liable to drift from the pixels actually encoded), this builds a small ICC v2.4
//! matrix/TRC profile from the same `indicatrix::color::space::ColorSpace` data
//! `ColorSpace::encode` itself uses: `xyz_to_rgb_matrix` (inverted, then
//! Bradford-adapted from the space's D65 white to the ICC profile-connection-space's
//! mandatory D50) for the `rXYZ`/`gXYZ`/`bXYZ` colorant tags, and
//! `TransferFunction::decode` -- called directly, not reimplemented -- sampled into
//! the `rTRC`/`gTRC`/`bTRC` curve tag. So the embedded profile is provably consistent
//! with the pixels: both derive from the identical matrix and transfer-function
//! values.
//!
//! # Verification, given no ICC-aware viewer is available here
//!
//! [`tests`] pins the mechanical/structural facts checkable without an external
//! profile inspector: ICC magic bytes and header fields land at spec-mandated
//! offsets, every tag is 4-byte aligned and within the declared file size,
//! `rgb_to_xyz_d50` maps white `(1,1,1)` to the D50 profile-connection-space white by
//! construction, and the matrix inversion + Bradford adaptation reproduce the
//! published sRGB D65 and Bradford-adapted D50 XYZ matrices within tolerance.

use indicatrix::color::{ColorSpace, TransferFunction};

/// D50, the ICC profile-connection-space's mandatory reference white (CIE standard
/// illuminant D50, `Y` normalized to `1.0`). Every colorimetric tag in this module
/// (`wtpt`, and the Bradford-adapted `rXYZ`/`gXYZ`/`bXYZ`) is expressed relative to
/// this per ICC.1:2001-04 §6.3.4.3, regardless of the source space's own white point.
const D50_WHITE: [f32; 3] = [0.9642, 1.0, 0.8249];

/// The Bradford cone-response matrix (Lam 1985 / Lindbloom's constants), used for
/// chromatic adaptation from a source white to [`D50_WHITE`] -- see [`bradford_adapt`].
const BRADFORD: [[f32; 3]; 3] = [
    [0.895_1, 0.266_4, -0.161_4],
    [-0.750_2, 1.713_5, 0.036_7],
    [0.038_9, -0.068_5, 1.029_6],
];

/// Builds a minimal ICC v2.4 matrix/TRC RGB profile for `color_space`, suitable for a
/// PNG `iCCP` chunk. See the module doc comment for what this profile contains and why
/// its colorimetry is derived from, not independent of, `color_space`'s own matrix and
/// transfer function.
#[must_use]
pub fn build(color_space: ColorSpace) -> Vec<u8> {
    let rgb_to_xyz_d50 = rgb_to_xyz_d50_matrix(color_space);
    let tf = color_space.transfer_function();

    let mut builder = ProfileBuilder::new();
    let desc = builder.add_block(text_description_tag(space_description(color_space)));
    let cprt = builder.add_block(text_tag(
        "No copyright. Profile generated at export time from indicatrix::color::space's own matrices.",
    ));
    let wtpt = builder.add_block(xyz_tag(D50_WHITE));
    let r_xyz = builder.add_block(xyz_tag(column(rgb_to_xyz_d50, 0)));
    let g_xyz = builder.add_block(xyz_tag(column(rgb_to_xyz_d50, 1)));
    let b_xyz = builder.add_block(xyz_tag(column(rgb_to_xyz_d50, 2)));
    // rTRC/gTRC/bTRC are IDENTICAL -- one shared curve -- so all three tag-table
    // entries point at this single block, the standard way RGB profiles avoid storing
    // the same curve three times.
    let trc = builder.add_block(curve_tag(tf));

    builder.add_tag(*b"desc", desc);
    builder.add_tag(*b"cprt", cprt);
    builder.add_tag(*b"wtpt", wtpt);
    builder.add_tag(*b"rXYZ", r_xyz);
    builder.add_tag(*b"gXYZ", g_xyz);
    builder.add_tag(*b"bXYZ", b_xyz);
    builder.add_tag(*b"rTRC", trc);
    builder.add_tag(*b"gTRC", trc);
    builder.add_tag(*b"bTRC", trc);
    builder.build()
}

/// `color_space`'s primaries, expressed as an RGB->XYZ matrix relative to the ICC
/// profile-connection-space's D50 white -- the inverse of
/// `ColorSpace::xyz_to_rgb_matrix` (XYZ->RGB relative to the space's own D65/D60
/// white), Bradford-adapted from that white to D50.
fn rgb_to_xyz_d50_matrix(color_space: ColorSpace) -> [[f32; 3]; 3] {
    let rgb_to_xyz_src = invert3(color_space.xyz_to_rgb_matrix());
    let (wx, wy) = color_space.white_point_xy();
    let src_white = xy_to_xyz(wx, wy);
    let adapt = bradford_adapt(src_white, D50_WHITE);
    matmul3(adapt, rgb_to_xyz_src)
}

/// CIE `xyY` chromaticity to `XYZ`, `Y` normalized to `1.0` -- the standard
/// `X = x/y, Y = 1, Z = (1-x-y)/y` conversion.
fn xy_to_xyz(x: f32, y: f32) -> [f32; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// The 3x3 chromatic-adaptation matrix taking `XYZ` values relative to `src_white` to
/// the equivalent values relative to `dst_white`, via the Bradford cone-response
/// transform (von Kries adaptation in Bradford cone space): `M_A^-1 . D . M_A`, where
/// `D` is the diagonal matrix of per-cone `dst/src` response ratios.
fn bradford_adapt(src_white: [f32; 3], dst_white: [f32; 3]) -> [[f32; 3]; 3] {
    let bradford_inv = invert3(BRADFORD);
    let src_cone = matvec3(BRADFORD, src_white);
    let dst_cone = matvec3(BRADFORD, dst_white);
    let scale = [
        [dst_cone[0] / src_cone[0], 0.0, 0.0],
        [0.0, dst_cone[1] / src_cone[1], 0.0],
        [0.0, 0.0, dst_cone[2] / src_cone[2]],
    ];
    matmul3(bradford_inv, matmul3(scale, BRADFORD))
}

/// Column `col` of row-major `m`, as a plain 3-vector -- column `j` is "what RGB
/// primary `j` alone maps to", exactly what each of `rXYZ`/`gXYZ`/`bXYZ` wants.
const fn column(m: [[f32; 3]; 3], col: usize) -> [f32; 3] {
    [m[0][col], m[1][col], m[2][col]]
}

/// General 3x3 matrix inverse via the adjugate/cofactor formula. Never guards against
/// a singular `m` -- the well-conditioned colour matrices this module works with
/// always have a non-zero determinant; a singular input would indicate a broken
/// `ColorSpace::xyz_to_rgb_matrix`, which the cross-check tests below would catch.
#[expect(
    clippy::suboptimal_flops,
    reason = "this is the textbook 2x2-cofactor cross-product expansion of a 3x3 \
              adjugate, called a handful of times per export (never per-pixel) -- \
              rewriting every term into nested mul_add chains would trade a formula \
              anyone can check against a linear-algebra reference for an unrecognizable \
              one, for a speedup that doesn't matter here"
)]
#[expect(
    clippy::many_single_char_names,
    reason = "a,b,c / d,e,f / g,h,i (row-major m[0]/m[1]/m[2]) are the standard names \
              for a 3x3 matrix's entries in the adjugate-inverse formula this \
              implements -- renaming them would make the formula harder, not easier, \
              to check against a linear-algebra reference"
)]
fn invert3(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let [a, b, c] = m[0];
    let [d, e, f] = m[1];
    let [g, h, i] = m[2];

    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let inv_det = 1.0 / det;

    [
        [
            (e * i - f * h) * inv_det,
            (c * h - b * i) * inv_det,
            (b * f - c * e) * inv_det,
        ],
        [
            (f * g - d * i) * inv_det,
            (a * i - c * g) * inv_det,
            (c * d - a * f) * inv_det,
        ],
        [
            (d * h - e * g) * inv_det,
            (b * g - a * h) * inv_det,
            (a * e - b * d) * inv_det,
        ],
    ]
}

fn matmul3(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for (row, out_row) in out.iter_mut().enumerate() {
        for (col, out_cell) in out_row.iter_mut().enumerate() {
            *out_cell = (0..3).map(|k| a[row][k] * b[k][col]).sum();
        }
    }
    out
}

fn matvec3(a: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        a[0][0].mul_add(v[0], a[0][1].mul_add(v[1], a[0][2] * v[2])),
        a[1][0].mul_add(v[0], a[1][1].mul_add(v[1], a[1][2] * v[2])),
        a[2][0].mul_add(v[0], a[2][1].mul_add(v[1], a[2][2] * v[2])),
    ]
}

/// The `desc` tag's human-readable name for `color_space` -- purely descriptive
/// (`build` is never called for `Srgb`/`AcesCg` in practice, but this stays exhaustive).
const fn space_description(color_space: ColorSpace) -> &'static str {
    match color_space {
        ColorSpace::Srgb => "sRGB",
        ColorSpace::DisplayP3 => "Display P3",
        ColorSpace::Rec2020 => "Rec. 2020",
        ColorSpace::AcesCg => "ACEScg",
    }
}

/// Encodes `v` as an ICC `s15Fixed16Number`: a signed 16.16 fixed-point value,
/// big-endian.
fn encode_s15_fixed16(v: f32) -> [u8; 4] {
    let fixed = (v * 65536.0).round() as i32;
    fixed.to_be_bytes()
}

/// Builds an ICC `textDescriptionType` (`desc`) tag body (ICC.1:2001-04 §6.5.17) --
/// the legacy structure ICC v2 profiles use (v4's simpler `mluc` type is not part of
/// the v2.4 header this module writes). The Unicode and Macintosh alternate-encoding
/// fields are always empty/zeroed: every consumer of this profile falls back to the
/// mandatory ASCII field.
fn text_description_tag(desc: &str) -> Vec<u8> {
    let ascii = desc.as_bytes();
    // +1 for the ASCII NUL terminator the count must include.
    let ascii_count = ascii.len() as u32 + 1;

    let mut out = Vec::new();
    out.extend_from_slice(b"desc");
    out.extend_from_slice(&[0u8; 4]); // reserved
    out.extend_from_slice(&ascii_count.to_be_bytes());
    out.extend_from_slice(ascii);
    out.push(0); // ASCII NUL terminator
    out.extend_from_slice(&0u32.to_be_bytes()); // Unicode language code (none)
    out.extend_from_slice(&0u32.to_be_bytes()); // Unicode description count (none)
    out.extend_from_slice(&0u16.to_be_bytes()); // ScriptCode code (none)
    out.push(0); // Macintosh description count (none)
    out.extend_from_slice(&[0u8; 67]); // Macintosh description, fixed 67-byte field
    out
}

/// Builds an ICC `textType` (`cprt`) tag body (ICC.1:2001-04 §6.5.18): a bare
/// NUL-terminated ASCII string.
fn text_tag(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"text");
    out.extend_from_slice(&[0u8; 4]); // reserved
    out.extend_from_slice(text.as_bytes());
    out.push(0);
    out
}

/// Builds an ICC `XYZType` (`XYZ `) tag body (ICC.1:2001-04 §6.5.26) holding a single
/// `XYZNumber` -- what `wtpt`/`rXYZ`/`gXYZ`/`bXYZ` each need.
fn xyz_tag(xyz: [f32; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"XYZ ");
    out.extend_from_slice(&[0u8; 4]); // reserved
    for component in xyz {
        out.extend_from_slice(&encode_s15_fixed16(component));
    }
    out
}

/// Number of samples in each `curv` tag's lookup table -- generous enough (a step of
/// roughly `1/65535` in device-value terms per two output codes) that quantizing the
/// already-8-bit-quantized pixel data through this curve introduces no additional
/// visible banding beyond what encoding to 8 bits already costs.
const CURVE_SAMPLES: usize = 1024;

/// Builds an ICC `curveType` (`curv`) tag body (ICC.1:2001-04 §6.5.3) as a
/// `CURVE_SAMPLES`-point sampled lookup table, evaluated by calling
/// `TransferFunction::decode` directly (never reimplementing its constants) so this
/// curve is provably the same one `ColorSpace::encode` applies to every pixel --
/// see the module doc comment.
fn curve_tag(tf: TransferFunction) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"curv");
    out.extend_from_slice(&[0u8; 4]); // reserved
    out.extend_from_slice(&(CURVE_SAMPLES as u32).to_be_bytes());
    for i in 0..CURVE_SAMPLES {
        let x = i as f32 / (CURVE_SAMPLES - 1) as f32;
        let y = tf.decode(x).clamp(0.0, 1.0);
        out.extend_from_slice(&((y * 65535.0).round() as u16).to_be_bytes());
    }
    out
}

/// Accumulates tag data blocks and the (signature -> block) table that references
/// them, then lays both out into one ICC-conformant byte buffer in [`Self::build`].
/// Kept separate from the module-level `build()` so the alignment/size bookkeeping is
/// in one place, exercised directly by `tests::profile_builder_*` below.
struct ProfileBuilder {
    blocks: Vec<Vec<u8>>,
    tags: Vec<([u8; 4], usize)>,
}

impl ProfileBuilder {
    const fn new() -> Self {
        Self {
            blocks: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// Registers a tag data block, returning its index for [`Self::add_tag`]. Calling
    /// this once and reusing the returned index across multiple `add_tag` calls (as
    /// `build` above does for `rTRC`/`gTRC`/`bTRC`) makes those tags share one offset
    /// and one copy of the data, rather than writing identical bytes three times.
    fn add_block(&mut self, data: Vec<u8>) -> usize {
        self.blocks.push(data);
        self.blocks.len() - 1
    }

    fn add_tag(&mut self, sig: [u8; 4], block: usize) {
        self.tags.push((sig, block));
    }

    /// Lays out the 128-byte header, the tag table, and every block (each block padded
    /// with zero bytes up to the next 4-byte boundary, per ICC.1:2001-04 §6.4's
    /// "tagged element data...shall start on a 4-byte boundary" requirement) into one
    /// buffer, then patches the header's total-size field once the real length is
    /// known.
    fn build(self) -> Vec<u8> {
        const HEADER_LEN: usize = 128;
        let tag_table_len = 4 + self.tags.len() * 12;
        // Both terms are already multiples of 4 (128, and 4 + 12*n), so this starting
        // offset needs no rounding of its own -- only the running total after each
        // block does.
        let mut offset = HEADER_LEN + tag_table_len;
        let mut offsets = Vec::with_capacity(self.blocks.len());
        for block in &self.blocks {
            offsets.push(offset as u32);
            offset += block.len();
            offset = offset.div_ceil(4) * 4;
        }
        let total_size = offset as u32;

        let mut out = Vec::with_capacity(offset);
        out.extend_from_slice(&build_header(total_size));
        out.extend_from_slice(&(self.tags.len() as u32).to_be_bytes());
        for (sig, block_idx) in &self.tags {
            out.extend_from_slice(sig);
            out.extend_from_slice(&offsets[*block_idx].to_be_bytes());
            out.extend_from_slice(&(self.blocks[*block_idx].len() as u32).to_be_bytes());
        }
        for block in &self.blocks {
            out.extend_from_slice(block);
            while out.len() % 4 != 0 {
                out.push(0);
            }
        }
        debug_assert_eq!(
            out.len(),
            total_size as usize,
            "layout/patched-size mismatch"
        );
        out
    }
}

/// Builds the fixed 128-byte ICC profile header (ICC.1:2001-04 §6.1.1), stamping
/// `total_size` into its first field. Every field this module has no real value for
/// (CMM type, primary platform, flags, manufacturer/model, attributes, creator,
/// profile ID) is left zero -- valid per spec ("if there is no ... shall be set to
/// zero") for every one of those, not a shortcut specific to this profile.
fn build_header(total_size: u32) -> [u8; 128] {
    let mut h = [0u8; 128];
    h[0..4].copy_from_slice(&total_size.to_be_bytes());
    // CMM type (4..8): zero, see fn doc comment.
    h[8..12].copy_from_slice(&[0x02, 0x40, 0x00, 0x00]); // profile version 2.4.0.0
    h[12..16].copy_from_slice(b"mntr"); // device class: display monitor
    h[16..20].copy_from_slice(b"RGB "); // data colour space
    h[20..24].copy_from_slice(b"XYZ "); // profile connection space
    // Fixed, arbitrary creation date/time (24..36) -- this profile is generated fresh
    // at every export, so there is no real "authored on" date to record; any
    // well-formed date is spec-valid.
    for (offset, value) in [(24, 2000u16), (26, 1), (28, 1), (30, 0), (32, 0), (34, 0)] {
        h[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }
    h[36..40].copy_from_slice(b"acsp"); // required profile file signature
    // Primary platform (40..44), flags (44..48), manufacturer (48..52), model
    // (52..56), attributes (56..64): zero, see fn doc comment.
    // Rendering intent (64..68): 0 = perceptual.
    let d50_x = encode_s15_fixed16(D50_WHITE[0]);
    let d50_y = encode_s15_fixed16(D50_WHITE[1]);
    let d50_z = encode_s15_fixed16(D50_WHITE[2]);
    h[68..72].copy_from_slice(&d50_x); // PCS illuminant: CIE D50, X
    h[72..76].copy_from_slice(&d50_y); // PCS illuminant: CIE D50, Y
    h[76..80].copy_from_slice(&d50_z); // PCS illuminant: CIE D50, Z
    // Profile creator (80..84), profile ID (84..100), reserved (100..128): zero.
    h
}

#[cfg(test)]
mod tests {
    use super::{
        D50_WHITE, ProfileBuilder, bradford_adapt, build, column, curve_tag, invert3, matmul3,
        matvec3, rgb_to_xyz_d50_matrix, xy_to_xyz,
    };
    use indicatrix::color::{ColorSpace, TransferFunction};

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    fn matrices_approx_eq(a: [[f32; 3]; 3], b: [[f32; 3]; 3], eps: f32) -> bool {
        (0..3).all(|r| (0..3).all(|c| approx_eq(a[r][c], b[r][c], eps)))
    }

    #[test]
    fn invert3_is_the_inverse_of_srgbs_own_matrix() {
        let m = ColorSpace::Srgb.xyz_to_rgb_matrix();
        let identity = matmul3(m, invert3(m));
        let expected = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert!(
            matrices_approx_eq(identity, expected, 1e-4),
            "m * invert3(m) should be the identity, got {identity:?}"
        );
    }

    /// Cross-checks `invert3` against the published sRGB (D65) RGB->XYZ matrix
    /// (standard IEC 61966-2-1 constants) -- an external reference check, not just
    /// internal self-consistency.
    #[test]
    fn invert3_of_srgb_matches_the_published_srgb_to_xyz_d65_matrix() {
        let computed = invert3(ColorSpace::Srgb.xyz_to_rgb_matrix());
        let published = [
            [0.412_456_4, 0.357_576_1, 0.180_437_5],
            [0.212_672_9, 0.715_152_2, 0.072_175],
            [0.019_333_9, 0.119_192, 0.950_304_1],
        ];
        assert!(
            matrices_approx_eq(computed, published, 5e-4),
            "computed {computed:?} vs published {published:?}"
        );
    }

    #[test]
    fn xy_to_xyz_of_d65_matches_the_standard_d65_tristimulus_values() {
        // CIE Standard Illuminant D65 chromaticity, matching
        // `ColorSpace::Srgb.white_point_xy()`.
        let xyz = xy_to_xyz(0.3127, 0.3290);
        assert!(approx_eq(xyz[0], 0.95047, 1e-3));
        assert!(approx_eq(xyz[1], 1.0, 1e-6));
        assert!(approx_eq(xyz[2], 1.08883, 1e-3));
    }

    /// A Bradford-adapted RGB->XYZ(D50) matrix must map RGB white `(1,1,1)` to the D50
    /// PCS white by construction, for any correctly-adapted RGB space.
    #[test]
    fn rgb_to_xyz_d50_matrix_maps_white_to_d50() {
        for space in [ColorSpace::Srgb, ColorSpace::DisplayP3, ColorSpace::Rec2020] {
            let m = rgb_to_xyz_d50_matrix(space);
            let white = matvec3(m, [1.0, 1.0, 1.0]);
            assert!(
                approx_eq(white[0], D50_WHITE[0], 1e-3)
                    && approx_eq(white[1], D50_WHITE[1], 1e-3)
                    && approx_eq(white[2], D50_WHITE[2], 1e-3),
                "{space:?}: RGB(1,1,1) mapped to {white:?}, expected D50 {D50_WHITE:?}"
            );
        }
    }

    /// Cross-checks the Bradford adaptation against the published "sRGB, D50"
    /// RGB->XYZ matrix (the D50 counterpart of the D65 matrix pinned above).
    #[test]
    fn bradford_adapted_srgb_matches_the_published_srgb_d50_matrix() {
        let computed = rgb_to_xyz_d50_matrix(ColorSpace::Srgb);
        let published = [
            [0.436_074_7, 0.385_064_9, 0.143_080_4],
            [0.222_504_5, 0.716_878_6, 0.060_616_9],
            [0.013_932_2, 0.097_104_5, 0.714_173_3],
        ];
        assert!(
            matrices_approx_eq(computed, published, 3e-3),
            "computed {computed:?} vs published {published:?}"
        );
    }

    #[test]
    fn bradford_adapt_is_the_identity_when_source_and_target_white_match() {
        let m = bradford_adapt(D50_WHITE, D50_WHITE);
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert!(matrices_approx_eq(m, identity, 1e-5));
    }

    #[test]
    fn column_reads_out_the_right_axis() {
        let m = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]];
        assert_eq!(column(m, 0), [1.0, 4.0, 7.0]);
        assert_eq!(column(m, 1), [2.0, 5.0, 8.0]);
        assert_eq!(column(m, 2), [3.0, 6.0, 9.0]);
    }

    /// `curve_tag` must sample `TransferFunction::decode` directly, not a
    /// reimplementation -- pins the two endpoints (`decode(0) == 0`, `decode(1) == 1`)
    /// so a divergent reimplementation would be caught here.
    #[test]
    fn curve_tag_endpoints_match_transfer_function_decode() {
        for tf in [
            TransferFunction::Srgb,
            TransferFunction::Rec2020,
            TransferFunction::Linear,
        ] {
            let bytes = curve_tag(tf);
            // Header is 12 bytes (sig + reserved + count); samples follow as u16 BE.
            let first = u16::from_be_bytes([bytes[12], bytes[13]]);
            let last_offset = bytes.len() - 2;
            let last = u16::from_be_bytes([bytes[last_offset], bytes[last_offset + 1]]);
            assert_eq!(first, 0, "{tf:?}: decode(0.0) must sample to 0");
            assert_eq!(last, 65535, "{tf:?}: decode(1.0) must sample to 65535");
        }
    }

    #[test]
    fn profile_builder_shares_one_offset_for_a_reused_block() {
        let mut b = ProfileBuilder::new();
        let block = b.add_block(vec![1, 2, 3, 4]);
        b.add_tag(*b"aaaa", block);
        b.add_tag(*b"bbbb", block);
        b.add_tag(*b"cccc", block);
        let bytes = b.build();

        // Tag table starts right after the 128-byte header.
        let entry = |i: usize| {
            let start = 128 + 4 + i * 12;
            let offset = u32::from_be_bytes(bytes[start + 4..start + 8].try_into().unwrap());
            let size = u32::from_be_bytes(bytes[start + 8..start + 12].try_into().unwrap());
            (offset, size)
        };
        assert_eq!(entry(0), entry(1));
        assert_eq!(entry(1), entry(2));
        // Only one copy of the 4-byte block was written, not three.
        assert_eq!(bytes.len(), 128 + 4 + 3 * 12 + 4);
    }

    /// Structural validation of a real built profile: required header fields land at
    /// spec-mandated offsets, declared size matches buffer length, and every tag's
    /// offset is 4-byte aligned and fits within the buffer.
    #[test]
    fn build_produces_a_structurally_valid_profile() {
        for space in [ColorSpace::DisplayP3, ColorSpace::Rec2020] {
            let bytes = build(space);

            assert!(
                bytes.len() >= 128,
                "{space:?}: shorter than the fixed header"
            );
            assert_eq!(&bytes[12..16], b"mntr", "{space:?}: device class");
            assert_eq!(&bytes[16..20], b"RGB ", "{space:?}: data colour space");
            assert_eq!(&bytes[20..24], b"XYZ ", "{space:?}: PCS");
            assert_eq!(&bytes[36..40], b"acsp", "{space:?}: profile file signature");

            let declared_size = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
            assert_eq!(
                declared_size as usize,
                bytes.len(),
                "{space:?}: header size field"
            );

            let tag_count = u32::from_be_bytes(bytes[128..132].try_into().unwrap());
            assert_eq!(
                tag_count, 9,
                "{space:?}: desc/cprt/wtpt/rXYZ/gXYZ/bXYZ/rTRC/gTRC/bTRC"
            );

            for i in 0..tag_count as usize {
                let start = 132 + i * 12;
                let offset = u32::from_be_bytes(bytes[start + 4..start + 8].try_into().unwrap());
                let size = u32::from_be_bytes(bytes[start + 8..start + 12].try_into().unwrap());
                assert_eq!(
                    offset % 4,
                    0,
                    "{space:?}: tag {i} offset must be 4-byte aligned"
                );
                assert!(
                    (offset + size) as usize <= bytes.len(),
                    "{space:?}: tag {i} extends past the end of the buffer"
                );
            }
        }
    }

    #[test]
    fn display_p3_and_rec2020_profiles_have_different_colorant_matrices() {
        let p3 = build(ColorSpace::DisplayP3);
        let rec2020 = build(ColorSpace::Rec2020);
        assert_ne!(
            p3, rec2020,
            "the two wide-gamut spaces must not embed identical bytes"
        );
    }
}
