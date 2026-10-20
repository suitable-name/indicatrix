//! Standalone four-storage-buffer renderer. Materials of every optical class use
//! the same scalar-index kernel; only UV and fluorescence are refused by prep.
mod buffers;
pub(crate) mod context;
mod dispatch;
mod pipeline;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

pub use buffers::WhittedGpuScene;
pub use pipeline::WhittedGpuRenderer;

pub const SHADER_SOURCE: &str = include_str!(concat!(env!("OUT_DIR"), "/whitted.generated.wgsl"));
pub const STORAGE_BUFFERS: u32 = 4;
pub const CHUNK_PIXELS: u32 = 4096;
pub use crate::renderer::buffers::whitted::WHITTED_KERNEL_HASH;

/// Acquire only the four-storage-buffer limits, without compiling either image
/// kernel. Browser admission can select this when the spectral device is declined.
///
/// # Errors
///
/// Returns an error when no adapter is available or it cannot provide the
/// four-storage-buffer limits.
#[cfg_attr(
    target_arch = "wasm32",
    allow(
        clippy::future_not_send,
        reason = "wgpu's web backend futures are inherently !Send (browser-side Rc \
                  handles); wasm32-unknown-unknown has no second thread regardless"
    )
)]
pub async fn acquire_whitted_context() -> Result<super::GpuContext, super::GpuAcquireError> {
    context::acquire().await
}

#[derive(Debug)]
pub enum WhittedGpuError {
    Declined(String),
    DeviceLost(String),
}
impl std::fmt::Display for WhittedGpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Declined(text) | Self::DeviceLost(text) => f.write_str(text),
        }
    }
}
impl std::error::Error for WhittedGpuError {}
