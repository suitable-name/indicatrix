//! GPU compute pipeline for `shaders/raytracer.wgsl` -- **unused scaffolding,
//! pending a real GPU port**.
//!
//! Never constructed anywhere in this workspace; calling [`IndicatrixRaytracerPipeline::new`]
//! today panics in `create_shader_module` because the shader is quarantined (see its
//! header comment) and has no valid entry point. Kept only as a hook for a future port,
//! which must translate the CPU renderer (`optics::raytracer`) from scratch and validate
//! it with a CPU/GPU equivalence harness -- not resurrect this shader.
use std::borrow::Cow;
use wgpu::PipelineCompilationOptions;

pub struct IndicatrixRaytracerPipeline {
    pub compute_pipeline: wgpu::ComputePipeline,
}

impl IndicatrixRaytracerPipeline {
    /// # Panics
    ///
    /// Always, currently: the shader is quarantined with no `main` entry point, so
    /// `create_compute_pipeline` fails. Do not call until a real GPU port lands.
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Indicatrix Raytracer Shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("shaders/raytracer.wgsl"))),
        });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Indicatrix Raytracer Compute Pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: PipelineCompilationOptions::default(),
            cache: None,
        });

        Self { compute_pipeline }
    }
}
