use super::{WhittedGpuError, WhittedGpuRenderer};
use crate::{
    geometry::tool::{
        GPU_MAX_TOOL_PRIMITIVES, TOOL_RECORD_VECS, ToolPrimitive, pack_planes_and_tools,
    },
    optics::raytracer::FacetFinish,
    renderer::{
        buffers::{GpuCameraParams, whitted::GpuWhittedParams},
        frame_scene::FrameScene,
        gpu::compute,
        whitted::{WhittedPrep, env::WhittedGpuEnv},
    },
};
use wgpu::util::DeviceExt;

/// Owns all scene uploads, allowing fair interleaving with another request without
/// overwriting its geometry, tables, or camera. Reuse throughout a tap batch.
pub struct WhittedGpuScene {
    pub(super) device_id: u64,
    pub(super) camera: wgpu::Buffer,
    pub(super) planes: wgpu::Buffer,
    pub(super) environment: wgpu::Buffer,
    pub(super) dimensions: wgpu::Buffer,
    pub(super) frost: wgpu::Buffer,
    pub(super) params: GpuWhittedParams,
    pub pixels: u32,
    pub taps: u32,
}

/// Environment texels and their `[width, height, 0, 0]` dimension record.
type EnvironmentUpload = (Vec<[f32; 4]>, [u32; 4]);

/// Flatten the prepared environment into the storage-buffer texels the shader reads.
///
/// # Errors
///
/// Returns [`WhittedGpuError::Declined`] when an HDR map would exceed `storage_limit` bytes.
#[cfg_attr(
    not(test),
    expect(
        clippy::match_wildcard_for_single_variants,
        reason = "the only other variant, Constant, exists under cfg(test) alone"
    )
)]
fn environment_upload(
    env: &WhittedGpuEnv<'_>,
    storage_limit: u64,
) -> Result<EnvironmentUpload, WhittedGpuError> {
    Ok(match *env {
        WhittedGpuEnv::Hdr(map) => {
            let bytes = (map.width() as u64)
                .saturating_mul(map.height() as u64)
                .saturating_mul(16);
            if bytes > storage_limit {
                return Err(WhittedGpuError::Declined(
                    "HDR map exceeds Ray-Traced GPU storage limit".into(),
                ));
            }
            let data: Vec<[f32; 4]> = map
                .pixels()
                .iter()
                .map(|p| [p[0], p[1], p[2], 0.0])
                .collect();
            let width = u32::try_from(map.width())
                .expect("HDR width fits u32 (storage-limit checked above)");
            let height = u32::try_from(map.height())
                .expect("HDR height fits u32 (storage-limit checked above)");
            (data, [width, height, 0, 0])
        }
        WhittedGpuEnv::Aset { table, .. } => (
            table.iter().map(|v| [v.x, v.y, v.z, 0.0]).collect(),
            [1024, 1, 0, 0],
        ),
        _ => (vec![[0.0; 4]], [1, 1, 0, 0]),
    })
}

/// Per-plane frost RGB plus a frosted-finish flag in the fourth channel.
fn frost_records(scene: &FrameScene<'_>, prep: &WhittedPrep<'_>, count: usize) -> Vec<[f32; 4]> {
    (0..count)
        .map(|index| {
            let rgb = prep.frost.get(index).copied().unwrap_or(glam::Vec3::ZERO);
            [
                rgb.x,
                rgb.y,
                rgb.z,
                if matches!(scene.facet_finishes.get(index), Some(FacetFinish::Frosted)) {
                    1.0
                } else {
                    0.0
                },
            ]
        })
        .collect()
}

impl WhittedGpuRenderer {
    /// Upload one frame's camera, planes, tools, environment and frost tables.
    ///
    /// # Errors
    ///
    /// Returns an error when the renderer is unhealthy, or declines the scene
    /// because its dimensions, geometry or environment exceed the GPU limits.
    ///
    /// # Panics
    ///
    /// Does not panic in practice: the `u32` conversions it unwraps are guarded by
    /// the limit checks that precede them.
    pub fn prepare(
        &self,
        scene: &FrameScene<'_>,
        tools: &[ToolPrimitive],
        prep: &WhittedPrep<'_>,
    ) -> Result<WhittedGpuScene, WhittedGpuError> {
        self.healthy()?;
        let pixels = u64::from(scene.width) * u64::from(scene.height);
        if pixels == 0
            || pixels > u64::from(u32::MAX)
            || tools.len() > GPU_MAX_TOOL_PRIMITIVES
            || scene.planes.len() > u32::MAX as usize
        {
            return Err(WhittedGpuError::Declined(
                "Ray-Traced GPU geometry or dimensions exceed supported limits".into(),
            ));
        }
        let storage_limit = self
            .ctx
            .device
            .limits()
            .max_storage_buffer_binding_size
            .min(self.ctx.device.limits().max_buffer_size);
        // Check before packing: tools require an owned copy of every plane.
        let packed_count = (scene.planes.len() as u64)
            .saturating_add((tools.len() as u64).saturating_mul(TOOL_RECORD_VECS as u64))
            .max(1);
        let count = (scene.planes.len() + tools.len()).max(1);
        if packed_count.saturating_mul(16) > storage_limit
            || (count as u64).saturating_mul(16) > storage_limit
        {
            return Err(WhittedGpuError::Declined(
                "Ray-Traced geometry exceeds GPU storage budget".into(),
            ));
        }
        let (texels, mut dims) = environment_upload(&prep.env.gpu_data(), storage_limit)?;
        let mut packed = pack_planes_and_tools(scene.planes, tools);
        if packed.is_empty() {
            packed
                .to_mut()
                .push(crate::geometry::GpuFacetPlane::new(glam::Vec3::ZERO, 0.0));
        }
        // Keep raster indexing integer-exact even beyond f32's exact integer range.
        dims[2] = scene.width;
        dims[3] = scene.height;
        let frost = frost_records(scene, prep, count);
        let camera = GpuCameraParams {
            origin: scene.camera.origin.to_array(),
            fov_tan: scene.camera.fov_tan,
            forward: scene.camera.forward.to_array(),
            width: scene.width as f32,
            right: scene.camera.right.to_array(),
            height: scene.height as f32,
            up: scene.camera.up.to_array(),
            num_samples: 1,
        };
        let upload = |label, bytes, usage| {
            self.ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytes,
                    usage,
                })
        };
        let params = GpuWhittedParams::new(scene, tools.len(), prep);
        Ok(WhittedGpuScene {
            device_id: self.id,
            camera: upload(
                "Ray camera",
                bytemuck::bytes_of(&camera),
                wgpu::BufferUsages::UNIFORM,
            ),
            planes: upload(
                "Ray planes/tools",
                bytemuck::cast_slice(&packed),
                wgpu::BufferUsages::STORAGE,
            ),
            environment: upload(
                "Ray HDR/ASET",
                bytemuck::cast_slice(&texels),
                wgpu::BufferUsages::STORAGE,
            ),
            dimensions: upload(
                "Ray HDR dimensions",
                bytemuck::bytes_of(&dims),
                wgpu::BufferUsages::UNIFORM,
            ),
            frost: compute::upload(
                &self.ctx.device,
                "Ray frost",
                &frost,
                wgpu::BufferUsages::STORAGE,
            ),
            params,
            pixels: u32::try_from(pixels).expect("pixel count checked against u32::MAX above"),
            taps: params.counts[2],
        })
    }
}
