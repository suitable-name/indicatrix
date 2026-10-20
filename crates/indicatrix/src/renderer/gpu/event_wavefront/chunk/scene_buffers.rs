//! Persistent GPU-side scene buffers of the event wavefront renderer.
//!
//! NOTE: this still duplicates `frame::scene_buffers::FrameSceneBuffers` (plan section 5 asked
//! for reuse); folding the two needs `EventWavefrontRenderer` (accumulate.rs) to hand over a
//! `SceneBufferInputs`, which is outside the chunk module.

use wgpu::BufferUsages;

use crate::{
    geometry::GpuFacetPlane,
    optics::raytracer::EnvironmentSource,
    renderer::{
        buffers::{GpuCameraParams, GpuGemMaterial, GpuTransportParams},
        env_map_gpu::HdrEnvGpuData,
        gpu::compute,
    },
};

use crate::renderer::gpu::event_wavefront::EwError;

/// Persistent GPU-side scene buffers, updated in place across frames/chunks.
///
/// `planes` below is the *packed* planes binding: pass `pack_planes_and_tools(planes, tools)`
/// wherever a `planes: &[GpuFacetPlane]` argument is taken, and a facet-finish list of the same
/// (packed) length.
pub struct SceneBuffers {
    pub camera: wgpu::Buffer,
    pub params: wgpu::Buffer,
    pub material: wgpu::Buffer,
    /// The `planes` binding: the stone's planes, then the packed concave-tool records
    /// (`geometry::tool::pack_planes_and_tools`; the tool count is
    /// `GpuTransportParams::num_tools`). Reusing the binding keeps every kernel's storage-buffer
    /// count unchanged.
    pub planes: wgpu::Buffer,
    pub planes_capacity: usize,
    /// Entries (planes plus 5 per tool) the current scene occupies in `planes`, which is also
    /// the entry count of `facet_finishes`. The binding size follows it, not the capacity: a
    /// buffer grown for an earlier, larger scene must not make `arrayLength(&planes)` lie.
    pub planes_len: usize,
    pub facet_finishes: wgpu::Buffer,
    pub facet_finishes_capacity: usize,
    pub hdr_env: HdrEnvGpuData,
    pub hdr_env_identity: Option<usize>,
}

impl SceneBuffers {
    /// Creates the persistent scene buffers.
    pub fn new(
        device: &wgpu::Device,
        camera_params: &GpuCameraParams,
        params: &GpuTransportParams,
        material: &GpuGemMaterial,
        planes: &[GpuFacetPlane],
        facet_finishes: &[u32],
        environment: EnvironmentSource<'_>,
    ) -> Result<Self, EwError> {
        let (hdr_env, hdr_env_identity) = match environment {
            EnvironmentSource::HdrMap(map) => {
                if !HdrEnvGpuData::fits_storage_binding(device, map) {
                    return Err(EwError::UnsupportedEnvironment);
                }
                let id = std::ptr::from_ref(map) as usize;
                (HdrEnvGpuData::upload(device, map), Some(id))
            }
            EnvironmentSource::Studio { .. } => (HdrEnvGpuData::dummy(device), None),
        };

        Ok(Self {
            camera: compute::upload(
                device,
                "ew camera (persistent)",
                std::slice::from_ref(camera_params),
                BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            ),
            params: compute::upload(
                device,
                "ew params (persistent)",
                std::slice::from_ref(params),
                BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            ),
            material: compute::upload(
                device,
                "ew material (persistent)",
                std::slice::from_ref(material),
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
            planes: compute::upload(
                device,
                "ew planes (persistent)",
                planes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
            planes_capacity: planes.len(),
            planes_len: planes.len(),
            facet_finishes: compute::upload(
                device,
                "ew facet finishes (persistent)",
                facet_finishes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
            facet_finishes_capacity: facet_finishes.len(),
            hdr_env,
            hdr_env_identity,
        })
    }

    /// Updates scene buffers in place, reallocating only if capacity must grow.
    ///
    /// # Errors
    ///
    /// [`EwError::UnsupportedEnvironment`] if an HDR environment map does not fit a storage
    /// binding of the device.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors `SceneBuffers::new` plus device and queue; the planned fold into `frame::SceneBufferInputs` (see the module note) replaces this argument list"
    )]
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera_params: &GpuCameraParams,
        params: &GpuTransportParams,
        material: &GpuGemMaterial,
        planes: &[GpuFacetPlane],
        facet_finishes: &[u32],
        environment: EnvironmentSource<'_>,
    ) -> Result<(), EwError> {
        queue.write_buffer(&self.camera, 0, bytemuck::bytes_of(camera_params));
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(params));
        queue.write_buffer(&self.material, 0, bytemuck::bytes_of(material));

        self.planes_len = planes.len();
        if planes.len() > self.planes_capacity {
            self.planes = compute::upload(
                device,
                "ew planes (persistent, grown)",
                planes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            );
            self.planes_capacity = planes.len();
        } else {
            queue.write_buffer(&self.planes, 0, bytemuck::cast_slice(planes));
        }

        if facet_finishes.len() > self.facet_finishes_capacity {
            self.facet_finishes = compute::upload(
                device,
                "ew facet finishes (persistent, grown)",
                facet_finishes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            );
            self.facet_finishes_capacity = facet_finishes.len();
        } else {
            queue.write_buffer(
                &self.facet_finishes,
                0,
                bytemuck::cast_slice(facet_finishes),
            );
        }

        let new_identity = match environment {
            EnvironmentSource::HdrMap(map) => Some(std::ptr::from_ref(map) as usize),
            EnvironmentSource::Studio { .. } => None,
        };
        if new_identity != self.hdr_env_identity {
            let (hdr_env, hdr_env_identity) = match environment {
                EnvironmentSource::HdrMap(map) => {
                    if !HdrEnvGpuData::fits_storage_binding(device, map) {
                        return Err(EwError::UnsupportedEnvironment);
                    }
                    let id = std::ptr::from_ref(map) as usize;
                    (HdrEnvGpuData::upload(device, map), Some(id))
                }
                EnvironmentSource::Studio { .. } => (HdrEnvGpuData::dummy(device), None),
            };
            self.hdr_env = hdr_env;
            self.hdr_env_identity = hdr_env_identity;
        }

        Ok(())
    }
}
