//! The disk cache of forward records: the key, and a versioned binary format.
//!
//! The key is an FNV-1a hash over everything the records depend on (mesh, alignment, rig,
//! lighting, surface classes, zone geometry, inclusions, views and masks, camera and backlight
//! spectra, sample counts, bins, record budget, roughness through the surface map, seed).
//! Re-fits that only change the absorption never change it, so they never trace again.
//!
//! File layout (all little endian): the magic `ZFWD`, the format version (`u32`), the key
//! (`u64`), the payload, and an FNV-1a checksum (`u64`) of everything before it. A file with the
//! wrong magic, version, key or checksum, or with inconsistent lengths, is ignored (the caller
//! traces afresh and overwrites it).

use std::path::{Path, PathBuf};

use indicatrix::optics::zoning::ZonedAbsorption;

use super::{
    ForwardInput, StoneIndex,
    lighting::{LightModel, PanelGeom, RigLighting},
    records::{
        CacheStatus, EvalPoint, ForwardRecords, ForwardStats, LENGTHS, PathRecord, PixelLoss,
        SpectralSetup, ViewRecords,
    },
    surface::{SurfaceClass, SurfaceMap},
};
use crate::rough_plan::{
    locate::{InclusionShell, Projection, RigProfile, Rigid},
    photometry::WorkingGrid,
    shape::RoughMesh,
};

/// The version of the binary format and of the key. Bump it when either changes.
pub const FORMAT_VERSION: u32 = 1;

const MAGIC: [u8; 4] = *b"ZFWD";

/// FNV-1a, 64 bits.
#[derive(Debug, Clone, Copy)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv {
    #[must_use]
    pub const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub fn bytes(&mut self, data: &[u8]) {
        for &byte in data {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn usize(&mut self, v: usize) {
        self.u64(v as u64);
    }

    pub fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }

    pub fn f32(&mut self, v: f32) {
        self.bytes(&v.to_bits().to_le_bytes());
    }

    pub fn str(&mut self, s: &str) {
        self.usize(s.len());
        self.bytes(s.as_bytes());
    }

    #[must_use]
    pub const fn finish(&self) -> u64 {
        self.0
    }
}

fn hash_f64s(h: &mut Fnv, values: &[f64]) {
    for &v in values {
        h.f64(v);
    }
}

fn hash_mesh(h: &mut Fnv, mesh: &RoughMesh) {
    h.usize(mesh.vertices().len());
    for v in mesh.vertices() {
        hash_f64s(h, &v.to_array());
    }
    h.usize(mesh.triangles().len());
    for t in mesh.triangles() {
        for &i in t {
            h.u64(u64::from(i));
        }
    }
}

fn hash_rig(h: &mut Fnv, rig: &RigProfile) {
    h.usize(rig.views.len());
    for view in &rig.views {
        hash_f64s(h, &view.position);
        hash_f64s(h, &view.forward);
        hash_f64s(h, &view.up);
        match view.projection {
            Projection::Pinhole { focal_px } => {
                h.u64(1);
                h.f64(focal_px);
            }
            Projection::Orthographic { px_per_mm } => {
                h.u64(2);
                h.f64(px_per_mm);
            }
        }
        hash_f64s(h, &view.principal_point);
        h.u64(u64::from(view.image_size[0]));
        h.u64(u64::from(view.image_size[1]));
    }
    h.f64(rig.stone_n);
    h.f64(rig.surround_n);
}

fn hash_panel(h: &mut Fnv, panel: &PanelGeom) {
    hash_f64s(h, &panel.centre);
    hash_f64s(h, &panel.normal);
    hash_f64s(h, &panel.up);
    hash_f64s(h, &panel.size_mm);
}

fn hash_lighting(h: &mut Fnv, lighting: &RigLighting) {
    match &lighting.model {
        LightModel::Backlight { per_view, white } => {
            h.u64(1);
            h.usize(per_view.len());
            for panel in per_view {
                hash_panel(h, panel);
            }
            h.usize(white.len());
            for frame in white {
                match frame {
                    None => h.u64(0),
                    Some(frame) => {
                        h.u64(1);
                        let [fw, fh] = frame.full_size();
                        let (w, hh, scale) = frame.cells();
                        h.usize(fw);
                        h.usize(fh);
                        h.usize(w);
                        h.usize(hh);
                        h.f64(scale);
                        for &level in frame.levels() {
                            h.f32(level);
                        }
                    }
                }
            }
        }
        LightModel::UniformSurround => h.u64(2),
    }
    match &lighting.holder {
        None => h.u64(0),
        Some(mesh) => {
            h.u64(1);
            hash_mesh(h, mesh);
        }
    }
}

fn hash_surfaces(h: &mut Fnv, surfaces: &SurfaceMap) {
    let class = |h: &mut Fnv, class: SurfaceClass| match class {
        SurfaceClass::Polished => h.u64(0),
        SurfaceClass::Frosted { roughness } => {
            h.u64(1);
            h.f32(roughness);
        }
    };
    class(h, surfaces.default_class());
    h.usize(surfaces.overrides().len());
    for &(triangle, c) in surfaces.overrides() {
        h.u64(u64::from(triangle));
        class(h, c);
    }
}

fn hash_index(h: &mut Fnv, index: &StoneIndex) {
    match index {
        StoneIndex::Rig => h.u64(0),
        StoneIndex::Constant(n) => {
            h.u64(1);
            h.f64(*n);
        }
        StoneIndex::Dispersion(model) => {
            h.u64(2);
            h.str(&format!("{model:?}"));
        }
    }
}

fn hash_zones(h: &mut Fnv, zones: Option<&ZonedAbsorption>) {
    match zones {
        None => h.u64(0),
        Some(z) => {
            h.u64(1);
            h.str(&format!("{z:?}"));
        }
    }
}

fn hash_inclusion(h: &mut Fnv, shell: &InclusionShell) {
    h.usize(shell.points.len());
    for p in &shell.points {
        hash_f64s(h, &p.to_array());
    }
    h.usize(shell.triangles.len());
    for t in &shell.triangles {
        for &i in t {
            h.u64(u64::from(i));
        }
    }
    h.f64(shell.margin_mm);
}

fn hash_alignment(h: &mut Fnv, alignment: &Rigid) {
    hash_f64s(h, &alignment.rotation);
    hash_f64s(h, &alignment.translation);
}

/// The cache key of a trace request.
#[must_use]
pub fn cache_key(input: &ForwardInput<'_>) -> u64 {
    let mut h = Fnv::new();
    h.u64(u64::from(FORMAT_VERSION));
    hash_mesh(&mut h, input.mesh);
    hash_alignment(&mut h, &input.alignment);
    hash_rig(&mut h, &input.rig.rig);
    hash_lighting(&mut h, &input.rig.lighting);
    hash_surfaces(&mut h, input.surfaces);
    hash_index(&mut h, input.index);
    hash_zones(&mut h, input.zones);
    h.usize(input.inclusions.len());
    for shell in input.inclusions {
        hash_inclusion(&mut h, shell);
    }
    let opts = input.options;
    h.usize(input.views.len());
    for v in input.views {
        h.usize(v.view);
        h.usize(v.grid.width);
        h.usize(v.grid.height);
        hash_f64s(&mut h, &v.grid.origin);
        h.f64(v.grid.scale);
        match v.mask {
            None => h.u64(0),
            Some(mask) => {
                h.u64(1);
                h.usize(mask.width());
                h.usize(mask.height());
                for &bits in mask.bits() {
                    h.bytes(&[bits & opts.skip_flags]);
                }
            }
        }
        match v.rel_error_targets {
            None => h.u64(0),
            Some(targets) => {
                h.u64(1);
                for &t in targets {
                    h.f32(t);
                }
            }
        }
    }
    for channel in input.camera.sensitivity() {
        hash_f64s(&mut h, channel);
    }
    hash_f64s(&mut h, input.backlight.spd());
    h.usize(opts.samples);
    h.usize(opts.bins);
    hash_f64s(&mut h, &opts.lambda_range_nm);
    h.usize(opts.spectral_sub);
    h.usize(opts.max_records);
    h.usize(opts.max_depth);
    h.usize(opts.max_sample_factor);
    h.f64(opts.target_rel_error);
    h.f64(opts.abs_error);
    h.f64(opts.reference_alpha_per_mm);
    h.f64(opts.flagged_limit);
    h.f64(opts.inclusion_limit);
    h.u64(u64::from(opts.skip_flags));
    h.u64(opts.seed);
    h.finish()
}

/// The file the records with this key live in.
#[must_use]
pub fn cache_path(dir: &Path, key: u64) -> PathBuf {
    dir.join(format!("forward-{key:016x}.zfwd"))
}

// ---------------------------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------------------------

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_f64(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&v.to_bits().to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_bits().to_le_bytes());
}

/// Serialises `records` for `key`.
#[must_use]
pub fn encode(records: &ForwardRecords, key: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(records.memory_bytes() + 1024);
    out.extend_from_slice(&MAGIC);
    put_u32(&mut out, FORMAT_VERSION);
    put_u64(&mut out, key);
    let spectral = &records.spectral;
    put_f64(&mut out, spectral.lambda_min_nm);
    put_f64(&mut out, spectral.lambda_max_nm);
    put_u32(&mut out, spectral.bins as u32);
    put_u32(&mut out, spectral.traced_bins as u32);
    put_u32(&mut out, spectral.eval_points.len() as u32);
    for point in &spectral.eval_points {
        put_u32(&mut out, point.bin as u32);
        put_f64(&mut out, point.lambda_nm);
        for w in point.weight {
            put_f64(&mut out, w);
        }
    }
    put_u32(&mut out, records.n_zones as u32);
    put_u64(&mut out, records.stats.samples);
    put_f32(&mut out, records.stats.max_cluster_range_mm);
    put_u32(&mut out, records.views.len() as u32);
    for view in &records.views {
        put_u32(&mut out, view.view as u32);
        put_u64(&mut out, view.grid.width as u64);
        put_u64(&mut out, view.grid.height as u64);
        put_f64(&mut out, view.grid.origin[0]);
        put_f64(&mut out, view.grid.origin[1]);
        put_f64(&mut out, view.grid.scale);
        out.extend_from_slice(&view.status);
        for &s in &view.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        for &v in &view.mc_mean {
            put_f32(&mut out, v);
        }
        for &v in &view.mc_variance {
            put_f32(&mut out, v);
        }
        for loss in &view.loss {
            put_f32(&mut out, loss.depth);
            put_f32(&mut out, loss.invalid);
            put_f32(&mut out, loss.inclusion);
            put_f32(&mut out, loss.flagged);
        }
        for &o in &view.offsets {
            put_u32(&mut out, o);
        }
        put_u64(&mut out, view.records.len() as u64);
        for record in &view.records {
            for &l in &record.lengths {
                put_f32(&mut out, l);
            }
            put_f32(&mut out, record.weight);
        }
    }
    let mut sum = Fnv::new();
    sum.bytes(&out);
    put_u64(&mut out, sum.finish());
    out
}

// ---------------------------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------------------------

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8s(&mut self, n: usize) -> Option<Vec<u8>> {
        self.take(n).map(<[u8]>::to_vec)
    }

    fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Option<u64> {
        let b = self.take(8)?;
        Some(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn f64(&mut self) -> Option<f64> {
        self.u64().map(f64::from_bits)
    }

    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }

    fn usize(&mut self) -> Option<usize> {
        usize::try_from(self.u64()?).ok()
    }

    fn u16s(&mut self, n: usize) -> Option<Vec<u16>> {
        let bytes = self.take(n.checked_mul(2)?)?;
        Some(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect(),
        )
    }

    fn u32s(&mut self, n: usize) -> Option<Vec<u32>> {
        let bytes = self.take(n.checked_mul(4)?)?;
        Some(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )
    }

    fn f32s(&mut self, n: usize) -> Option<Vec<f32>> {
        Some(self.u32s(n)?.into_iter().map(f32::from_bits).collect())
    }

    /// One view's records (the body of the per-view loop of [`decode`]); `None` when truncated
    /// or when the per-pixel offsets are not a monotone cover of the records.
    fn view_records(&mut self, traced_bins: usize) -> Option<ViewRecords> {
        let view = self.u32()? as usize;
        let width = self.usize()?;
        let height = self.usize()?;
        let origin = [self.f64()?, self.f64()?];
        let scale = self.f64()?;
        let n = width.checked_mul(height)?;
        let status = self.u8s(n)?;
        let samples_per_pixel = self.u16s(n)?;
        let mc_mean = self.f32s(n)?;
        let mc_variance = self.f32s(n)?;
        let loss_raw = self.f32s(n.checked_mul(4)?)?;
        let loss = loss_raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| PixelLoss {
                depth: c[0],
                invalid: c[1],
                inclusion: c[2],
                flagged: c[3],
            })
            .collect();
        let offsets = self.u32s(n.checked_mul(traced_bins)?.checked_add(1)?)?;
        let record_count = self.usize()?;
        let raw = self.f32s(record_count.checked_mul(LENGTHS + 1)?)?;
        let records: Vec<PathRecord> = raw
            .as_chunks::<{ LENGTHS + 1 }>()
            .0
            .iter()
            .map(|c| {
                let mut lengths = [0.0_f32; LENGTHS];
                lengths.copy_from_slice(&c[..LENGTHS]);
                PathRecord {
                    lengths,
                    weight: c[LENGTHS],
                }
            })
            .collect();
        let monotone = offsets.first() == Some(&0)
            && offsets.windows(2).all(|w| w[0] <= w[1])
            && offsets.last().map(|&o| o as usize) == Some(records.len());
        if !monotone {
            return None;
        }
        Some(ViewRecords {
            view,
            grid: WorkingGrid {
                width,
                height,
                origin,
                scale,
            },
            status,
            samples: samples_per_pixel,
            mc_mean,
            mc_variance,
            loss,
            offsets,
            records,
        })
    }
}

/// Parses a cache file; `None` for anything that is not a valid file of this version and `key`.
#[must_use]
pub fn decode(bytes: &[u8], key: u64) -> Option<ForwardRecords> {
    let body_len = bytes.len().checked_sub(8)?;
    let (body, tail) = bytes.split_at(body_len);
    let stored = u64::from_le_bytes(tail.try_into().ok()?);
    let mut sum = Fnv::new();
    sum.bytes(body);
    if sum.finish() != stored {
        return None;
    }
    let mut r = Reader { data: body, pos: 0 };
    if r.take(4)? != MAGIC || r.u32()? != FORMAT_VERSION || r.u64()? != key {
        return None;
    }
    let lambda_min_nm = r.f64()?;
    let lambda_max_nm = r.f64()?;
    let bins = r.u32()? as usize;
    let traced_bins = r.u32()? as usize;
    let point_count = r.u32()? as usize;
    if bins == 0 || traced_bins == 0 || traced_bins > bins || point_count > 1 << 16 {
        return None;
    }
    let mut eval_points = Vec::with_capacity(point_count);
    for _ in 0..point_count {
        let bin = r.u32()? as usize;
        let lambda_nm = r.f64()?;
        let weight = [r.f64()?, r.f64()?, r.f64()?];
        if bin >= traced_bins {
            return None;
        }
        eval_points.push(EvalPoint {
            bin,
            lambda_nm,
            weight,
        });
    }
    let n_zones = r.u32()? as usize;
    if n_zones == 0 || n_zones > LENGTHS {
        return None;
    }
    let samples = r.u64()?;
    let max_cluster_range_mm = r.f32()?;
    let view_count = r.u32()? as usize;
    let mut views = Vec::new();
    for _ in 0..view_count {
        views.push(r.view_records(traced_bins)?);
    }
    if r.pos != body.len() {
        return None;
    }
    Some(ForwardRecords {
        spectral: SpectralSetup {
            lambda_min_nm,
            lambda_max_nm,
            bins,
            traced_bins,
            eval_points,
        },
        n_zones,
        views,
        stats: ForwardStats {
            cache: CacheStatus::Hit,
            samples,
            max_cluster_range_mm,
        },
    })
}

/// Reads the records for `key` from `dir`; `None` when there is no usable file.
#[must_use]
pub fn load(dir: &Path, key: u64) -> Option<ForwardRecords> {
    let bytes = std::fs::read(cache_path(dir, key)).ok()?;
    decode(&bytes, key)
}

/// Writes the records for `key` to `dir` (created if needed), through a temporary file and a
/// rename so a crash never leaves a half file. Returns whether it worked.
#[must_use]
pub fn store(dir: &Path, key: u64, records: &ForwardRecords) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let path = cache_path(dir, key);
    let temp = path.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&temp, encode(records, key)).is_err() {
        return false;
    }
    if std::fs::rename(&temp, &path).is_err() {
        let _ = std::fs::remove_file(&temp);
        return false;
    }
    true
}
