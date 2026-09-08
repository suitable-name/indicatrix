//! GPU-side HDR environment-map data (behind the `gpu` feature).
//!
//! Two independent pieces:
//!
//! - [`GpuEnvironmentMap`]: **unused scaffolding, pending a GPU port** -- see
//!   `renderer::pipeline`'s module doc comment for the broader context: the rasterized/
//!   hybrid preview path this texture upload would feed has never run in this workspace.
//!   Nothing in `indicatrix` constructs one yet; kept alongside the rest of this module
//!   for whichever renderer path needs GPU display of an environment.
//! - [`HdrEnvGpuData`] (finding G6, extended by finding G7): the storage-buffer upload
//!   `spectral_transport.wgsl`'s `transport_main` megakernel actually reads from at
//!   `env_mode == transport_env_mode::HDR_MAP`. Unlike `GpuEnvironmentMap`'s
//!   `Rgba32Float` TEXTURE (built for a sampled/filtered raster lookup), the megakernel
//!   reads texels itself via a plain `array<vec4<f32>>` STORAGE buffer and does its own
//!   bilinear filtering (`hdr_env_sample_bilinear` in `spectral_transport.wgsl`) to
//!   mirror [`super::env_map::EnvironmentMap::radiance_at`]'s exact `f32` arithmetic
//!   order -- `wgpu`'s hardware texture sampler has no such bit-exactness guarantee,
//!   which is why this is a second, independent upload path rather than a reuse of
//!   [`GpuEnvironmentMap`]'s texture.
//!
//! # `HdrEnvGpuData` bindings
//!
//! Bound at `spectral_transport.wgsl` group 0, bindings 10 (`hdr_texels`, `vec4<f32>` per
//! texel, row-major, alpha channel unused/zero), 11 (`hdr_env_dims`, a `width`/`height`
//! uniform), and -- finding G7's next-event-estimation GPU port -- 12/13/14 (the
//! [`super::env_map_distribution::Distribution2D`] importance-sampling data
//! `dist1d_find_bucket`/`dist1d_sample_continuous`/`dist1d_pdf`/`dist2d_sample`/
//! `dist2d_pdf` binary-search against). These bindings are ALWAYS part of
//! `transport_main`'s auto-inferred bind group layout -- `sample_environment_with_rig`
//! calls `hdr_env_radiance_at` unconditionally in the compiled kernel (the `env_mode`
//! branch is a RUNTIME check, not an `override`-resolved one, so naga's per-entry-point
//! reachability analysis cannot prune it) -- so every dispatch of `transport_main`, HDR
//! or not, must bind something valid here. [`HdrEnvGpuData::dummy`] is the
//! always-present fallback for every non-HDR scene (and every self-test that never
//! exercises HDR at all): a single black texel plus its (degenerate, but well-formed)
//! `Distribution2D`, bound the same way a real map would be, so `transport_main`'s bind
//! group never needs a HDR-specific pipeline variant.
//!
//! # Bindings 12/13/14's layout
//!
//! `Distribution2D` (see that type's own doc comment) is a marginal
//! [`super::env_map_distribution::Distribution1D`] over rows plus one conditional
//! `Distribution1D` per row. Rather than one buffer per `Distribution1D` field (which
//! would need a variable number of bindings, one per row), the marginal and every row's
//! conditional are flattened into two shared storage buffers:
//!
//! - Binding 12 (`dist_func`, `array<f32>`, length `width*height + height`): the first
//!   `width*height` entries are each row's own raw (un-normalized) bucket weights
//!   (`Distribution1D::func`), row-major; the trailing `height` entries are the
//!   marginal's own `func` array, which -- by [`super::env_map_distribution::Distribution2D::new`]'s
//!   own construction (`marginal_func.push(dist.func_int)` per row) -- is EXACTLY each
//!   row's own [`super::env_map_distribution::Distribution1D::func_int`], so this one
//!   trailing slice serves both "the marginal's own bucket weights" AND "each
//!   conditional row's own normalization scalar" without uploading the same numbers
//!   twice.
//! - Binding 13 (`dist_cdf`, `array<f32>`, length `height*(width+1) + (height+1)`): the
//!   first `height*(width+1)` entries are each row's own `Distribution1D::cdf` (length
//!   `width+1`), row-major; the trailing `height+1` entries are the marginal's own cdf.
//! - Binding 14 (`dist_dims`, uniform): `width`, `height`, and the marginal's own
//!   overall [`super::env_map_distribution::Distribution1D::func_int`] (distinct from
//!   any individual row's -- see binding 12 above), needed by `dist1d_pdf`'s marginal
//!   call exactly like [`GpuHdrEnvDims`] already carries `width`/`height` for bindings
//!   10/11.
use super::env_map::EnvironmentMap;
use crate::renderer::gpu::compute;

/// `spectral_transport.wgsl`'s `HdrEnvDims` uniform struct, binding 11.
///
/// See this module's doc comment. Four `u32`s, 16 bytes, no manual padding needed: WGSL's
/// uniform address-space layout rules only require each member's own alignment (4 for
/// `u32`), and 16 is already a multiple of the struct's own (4-byte) alignment. Private --
/// only [`HdrEnvGpuData::upload`]/[`HdrEnvGpuData::dummy`] (this module) ever construct
/// one.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuHdrEnvDims {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

const _: () = assert!(size_of::<GpuHdrEnvDims>() == 16);

/// `spectral_transport.wgsl`'s `GpuDistDims` uniform struct, binding 14 -- see this
/// module's doc comment ("Bindings 12/13/14's layout") for what each field feeds.
/// `_pad0` keeps the struct's size a multiple of its own (4-byte scalar) alignment,
/// mirroring [`GpuHdrEnvDims`]'s identical padding rationale.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuDistDims {
    width: u32,
    height: u32,
    marginal_func_int: f32,
    _pad0: f32,
}

const _: () = assert!(size_of::<GpuDistDims>() == 16);

/// The five GPU buffers backing `spectral_transport.wgsl`'s bindings 10-14 -- see this
/// module's doc comment. Rebuilt (never mutated in place) whenever the bound
/// [`EnvironmentMap`] changes identity; see `renderer::gpu::frame::FrameSceneBuffers`'s
/// own doc comment for the caching policy around this type.
pub(crate) struct HdrEnvGpuData {
    pub(crate) texels: wgpu::Buffer,
    pub(crate) dims: wgpu::Buffer,
    /// Binding 12 (`dist_func`) -- see this module's doc comment.
    pub(crate) dist_func: wgpu::Buffer,
    /// Binding 13 (`dist_cdf`) -- see this module's doc comment.
    pub(crate) dist_cdf: wgpu::Buffer,
    /// Binding 14 (`dist_dims`) -- see this module's doc comment.
    pub(crate) dist_dims: wgpu::Buffer,
}

/// Flattens `dist`'s marginal + per-row-conditional [`Distribution1D`]s into the
/// `(dist_func, dist_cdf, dist_dims)` triple bindings 12/13/14 read -- see this module's
/// doc comment ("Bindings 12/13/14's layout") for the exact layout this builds.
/// Shared by [`HdrEnvGpuData::upload`] and [`HdrEnvGpuData::dummy`] (the latter via a
/// degenerate `1x1` [`EnvironmentMap::uniform`]) so there is exactly one place that
/// encodes this layout, rather than a hand-derived dummy risking drifting from it.
fn flatten_distribution(
    device: &wgpu::Device,
    dist: &super::env_map::Distribution2D,
    label_prefix: &str,
) -> (wgpu::Buffer, wgpu::Buffer, wgpu::Buffer) {
    let width = dist.width();
    let height = dist.height();
    let marginal = dist.marginal();
    let conditional = dist.conditional();

    let mut func = Vec::with_capacity(width * height + height);
    let mut cdf = Vec::with_capacity(height * (width + 1) + (height + 1));
    for row in conditional {
        func.extend_from_slice(row.func());
        cdf.extend_from_slice(row.cdf());
    }
    // Trailing `height` entries: the marginal's own func array, which is exactly each
    // row's own `func_int` by `Distribution2D::new`'s construction -- see this module's
    // doc comment for why one array serves both roles.
    func.extend_from_slice(marginal.func());
    cdf.extend_from_slice(marginal.cdf());

    let func_buf = compute::upload(
        device,
        &format!("{label_prefix} dist func"),
        &func,
        wgpu::BufferUsages::STORAGE,
    );
    let cdf_buf = compute::upload(
        device,
        &format!("{label_prefix} dist cdf"),
        &cdf,
        wgpu::BufferUsages::STORAGE,
    );
    let dims = GpuDistDims {
        width: width as u32,
        height: height as u32,
        marginal_func_int: marginal.func_int(),
        _pad0: 0.0,
    };
    let dims_buf = compute::upload(
        device,
        &format!("{label_prefix} dist dims"),
        std::slice::from_ref(&dims),
        wgpu::BufferUsages::UNIFORM,
    );
    (func_buf, cdf_buf, dims_buf)
}

impl HdrEnvGpuData {
    /// Uploads `map`'s texels (row-major RGB, see [`EnvironmentMap::pixels`]) as a
    /// `vec4<f32>`-padded storage buffer (alpha channel `0.0`, unread by
    /// `hdr_env_sample_bilinear`) plus its `(width, height)` uniform, and -- finding
    /// G7 -- `map`'s own importance-sampling [`Distribution2D`](super::env_map::Distribution2D)
    /// flattened via [`flatten_distribution`].
    ///
    /// A fresh upload every call -- no in-place update -- since a caller only calls this
    /// when the map's identity has actually changed (a whole new panorama, not a
    /// per-frame refresh of the same one).
    pub(crate) fn upload(device: &wgpu::Device, map: &EnvironmentMap) -> Self {
        let texels: Vec<[f32; 4]> = map
            .pixels()
            .iter()
            .map(|&[r, g, b]| [r, g, b, 0.0])
            .collect();
        let texels_buf = compute::upload(
            device,
            "hdr env texels",
            &texels,
            wgpu::BufferUsages::STORAGE,
        );
        let dims = GpuHdrEnvDims {
            width: map.width() as u32,
            height: map.height() as u32,
            _pad0: 0,
            _pad1: 0,
        };
        let dims_buf = compute::upload(
            device,
            "hdr env dims",
            std::slice::from_ref(&dims),
            wgpu::BufferUsages::UNIFORM,
        );
        let (dist_func, dist_cdf, dist_dims) =
            flatten_distribution(device, map.distribution(), "hdr env");
        Self {
            texels: texels_buf,
            dims: dims_buf,
            dist_func,
            dist_cdf,
            dist_dims,
        }
    }

    /// A single black (`0,0,0,0`) texel at `1x1` -- the always-valid placeholder bound
    /// whenever the scene's environment is NOT an HDR map (see this module's doc comment
    /// for why bindings 10/11 must still hold something). `hdr_env_radiance_at` is never
    /// actually called against this data at runtime (the `env_mode` branch that would call
    /// it is only taken for `EnvironmentSource::HdrMap`), so its content is arbitrary; zero
    /// keeps it visibly inert if that invariant is ever violated by a future edit. Bindings
    /// 12/13/14 get the degenerate-but-well-formed `Distribution2D` a `1x1`
    /// [`EnvironmentMap::uniform`] builds (via [`flatten_distribution`]), for the same
    /// "never actually read, but must still be a valid bind group entry" reason.
    pub(crate) fn dummy(device: &wgpu::Device) -> Self {
        let texels: [[f32; 4]; 1] = [[0.0, 0.0, 0.0, 0.0]];
        let texels_buf = compute::upload(
            device,
            "hdr env texels (dummy, non-HDR scene)",
            &texels,
            wgpu::BufferUsages::STORAGE,
        );
        let dims = GpuHdrEnvDims {
            width: 1,
            height: 1,
            _pad0: 0,
            _pad1: 0,
        };
        let dims_buf = compute::upload(
            device,
            "hdr env dims (dummy, non-HDR scene)",
            std::slice::from_ref(&dims),
            wgpu::BufferUsages::UNIFORM,
        );
        let dummy_map = EnvironmentMap::uniform(1, 1, [0.0, 0.0, 0.0]);
        let (dist_func, dist_cdf, dist_dims) = flatten_distribution(
            device,
            dummy_map.distribution(),
            "hdr env (dummy, non-HDR scene)",
        );
        Self {
            texels: texels_buf,
            dims: dims_buf,
            dist_func,
            dist_cdf,
            dist_dims,
        }
    }
}

pub struct GpuEnvironmentMap {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

impl GpuEnvironmentMap {
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        width: u32,
        height: u32,
    ) -> Self {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Environment Map"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 16),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        Self {
            texture,
            view,
            sampler,
        }
    }
}
