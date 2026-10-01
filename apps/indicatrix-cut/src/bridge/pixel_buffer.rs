use slint::{Rgba8Pixel, SharedPixelBuffer};

/// How many frame buffers take turns: the one on screen, the one a pending redraw still
/// holds, and the one being written. Slint copies a shared buffer the moment it is
/// written, so a writer that only ever rewrites the image the UI is showing pays a full
/// clone every frame; rotating through a few buffers keeps the target unshared.
const FRAME_BUFFERS: usize = 3;

/// Hands finished RGBA8 frames to Slint, rewriting a rotating set of buffers in place
/// instead of cloning one the UI still holds.
///
/// Each frame goes into the next buffer of the rotation and the returned
/// [`SharedPixelBuffer`] shares that buffer's storage (a reference-count bump, not a
/// copy). By the time the rotation comes back to a buffer the UI has normally replaced it,
/// so the write finds it unshared. If the UI still holds it, Slint copies on write, which
/// is slower but leaves the held frame untouched: a returned frame is never altered.
pub struct FramebufferTransfer {
    width: u32,
    height: u32,
    /// Allocated on first use, so a one-shot transfer allocates exactly one buffer.
    buffers: [Option<SharedPixelBuffer<Rgba8Pixel>>; FRAME_BUFFERS],
    next: usize,
}

impl FramebufferTransfer {
    /// A transfer for `width` x `height` frames. Allocates nothing until the first frame.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            buffers: [const { None }; FRAME_BUFFERS],
            next: 0,
        }
    }

    /// Lets `fill` write the next frame's RGBA8 bytes (`width * height * 4` of them)
    /// straight into the buffer that is handed to Slint, so a caller that can tone-map
    /// into a slice needs no intermediate byte buffer and no second copy.
    ///
    /// The buffer still holds the frame from the last time it was used until `fill`
    /// overwrites it.
    pub fn fill_with(&mut self, fill: impl FnOnce(&mut [u8])) -> SharedPixelBuffer<Rgba8Pixel> {
        let index = self.next;
        self.next = (index + 1) % FRAME_BUFFERS;
        let (width, height) = (self.width, self.height);
        let buffer =
            self.buffers[index].get_or_insert_with(|| SharedPixelBuffer::new(width, height));
        fill(bytemuck::cast_slice_mut(buffer.make_mut_slice()));
        buffer.clone()
    }

    /// Copies `gpu_bytes` (RGBA8, `width * height * 4` of them) into the next frame
    /// buffer and returns it.
    pub fn copy_from_gpu_slice(&mut self, gpu_bytes: &[u8]) -> SharedPixelBuffer<Rgba8Pixel> {
        self.fill_with(|bytes| bytes.copy_from_slice(gpu_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_of(frame: &SharedPixelBuffer<Rgba8Pixel>) -> &[u8] {
        bytemuck::cast_slice(frame.as_slice())
    }

    /// A frame the caller still holds is never altered by later frames, even once the
    /// rotation has come back around to its buffer.
    #[test]
    fn a_held_frame_is_never_overwritten() {
        let mut transfer = FramebufferTransfer::new(2, 1);
        let held: Vec<_> = (1..=7_u8)
            .map(|shade| transfer.copy_from_gpu_slice(&[shade; 8]))
            .collect();
        for (shade, frame) in (1..=7_u8).zip(&held) {
            assert_eq!(bytes_of(frame), [shade; 8], "frame {shade}");
        }
    }

    /// Once the caller has dropped a frame, the rotation reuses its storage instead of
    /// allocating a new buffer.
    #[test]
    fn released_frames_are_reused_without_allocating() {
        let mut transfer = FramebufferTransfer::new(4, 4);
        let first = transfer.copy_from_gpu_slice(&[1; 64]).as_slice().as_ptr();
        for shade in 2..=FRAME_BUFFERS as u8 {
            drop(transfer.copy_from_gpu_slice(&[shade; 64]));
        }
        let again = transfer.copy_from_gpu_slice(&[9; 64]);
        assert_eq!(again.as_slice().as_ptr(), first);
        assert_eq!(bytes_of(&again), [9; 64]);
    }
}
