//! Pipeline cache and compilation management for the event wavefront kernels.

use std::collections::HashMap;

use super::kernels::EwKernel;

/// Standalone reduce kernel source: sums `out_xyz` per-sample values into per-pixel XYZ.
const REDUCE_SHADER_SRC: &str = include_str!("../../shaders/reduce_xyz.wgsl");

/// Lazy pipeline cache mapping `(EwKernel, material_class)` pairs to compiled `wgpu::ComputePipeline`s,
/// plus the eagerly built `reduce_pipeline`.
pub struct EwPipelines {
    pipelines: HashMap<(EwKernel, u32), wgpu::ComputePipeline>,
    reduce_pipeline: wgpu::ComputePipeline,
}

impl EwPipelines {
    /// Creates a new pipeline cache and compiles `reduce_xyz_main`.
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        let reduce_pipeline = crate::renderer::gpu::compute::create_compute_pipeline_cached(
            device,
            None,
            "reduce_xyz_main",
            REDUCE_SHADER_SRC,
            "reduce_xyz_main",
            &[],
        );
        Self {
            pipelines: HashMap::new(),
            reduce_pipeline,
        }
    }

    /// Ensures that the pipeline for `(kernel, class)` is compiled and returns a reference to it.
    pub fn ensure(
        &mut self,
        device: &wgpu::Device,
        kernel: EwKernel,
        class: u32,
    ) -> &wgpu::ComputePipeline {
        self.pipelines.entry((kernel, class)).or_insert_with(|| {
            crate::renderer::gpu::compute::create_compute_pipeline_cached(
                device,
                None,
                kernel.entry_point(),
                kernel.source(),
                kernel.entry_point(),
                &[("MATERIAL_CLASS", f64::from(class))],
            )
        })
    }

    /// Retrieves an already-compiled compute pipeline.
    ///
    /// # Panics
    /// Panics if the pipeline was not compiled by [`Self::ensure`] first.
    #[must_use]
    pub fn get(&self, kernel: EwKernel, class: u32) -> &wgpu::ComputePipeline {
        self.pipelines
            .get(&(kernel, class))
            .unwrap_or_else(|| panic!("pipeline {kernel:?} (class {class}) was not compiled"))
    }

    /// Accessor for the shared `reduce_xyz_main` compute pipeline.
    #[must_use]
    pub const fn reduce_pipeline(&self) -> &wgpu::ComputePipeline {
        &self.reduce_pipeline
    }

    /// Returns an iterator over all compiled `(EwKernel, class)` pairs.
    pub fn compiled(&self) -> impl Iterator<Item = (EwKernel, u32)> + '_ {
        self.pipelines.keys().copied()
    }
}
