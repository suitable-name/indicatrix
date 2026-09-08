use slint::{Rgba8Pixel, SharedPixelBuffer};

pub struct FramebufferTransfer {
    pixel_buffer: SharedPixelBuffer<Rgba8Pixel>,
}

impl FramebufferTransfer {
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            pixel_buffer: SharedPixelBuffer::<Rgba8Pixel>::new(width, height),
        }
    }

    pub fn copy_from_gpu_slice(&mut self, gpu_bytes: &[u8]) -> SharedPixelBuffer<Rgba8Pixel> {
        let slice = self.pixel_buffer.make_mut_slice();
        // Zero-copy reinterpret RGBA8 bytes into Rgba8Pixel slice
        let rgba_slice: &[Rgba8Pixel] = bytemuck::cast_slice(gpu_bytes);
        slice.copy_from_slice(rgba_slice);
        self.pixel_buffer.clone()
    }
}
