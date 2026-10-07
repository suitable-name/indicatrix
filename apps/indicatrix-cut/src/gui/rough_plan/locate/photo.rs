//! Loading a photo for a canvas.
//!
//! A photo is read from where it is (its path is remembered, the file is never copied or
//! stored in the database), decoded on a thread of its own, and kept for display at a reduced
//! size ([`display_size`]); the marks the user places on it are in the pixels of the full-size
//! photo, which is why the full size travels with it.

use crate::locate_io::overlay::display_size;
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};
use std::path::{Path, PathBuf};
use tracing::warn;

/// A photo decoded off the UI thread: its pixels, reduced for display, and its own size.
pub(super) struct Decoded {
    /// The pixels at display size.
    pub buffer: SharedPixelBuffer<Rgba8Pixel>,
    /// The photo's own width and height in pixels.
    pub full_size: [u32; 2],
}

/// A photo on the UI thread, ready for an `Image` element.
pub(super) struct Photo {
    /// Where the file is.
    pub path: PathBuf,
    /// The picture, at display size.
    pub image: Image,
    /// The photo's own width and height in pixels.
    pub size: [u32; 2],
}

impl Photo {
    /// The photo of a decoded file.
    pub(super) fn new(path: PathBuf, decoded: Decoded) -> Self {
        Self {
            path,
            image: Image::from_rgba8(decoded.buffer),
            size: decoded.full_size,
        }
    }

    /// The file's name, for the slot's line.
    pub(super) fn file_name(&self) -> String {
        file_name(&self.path)
    }
}

/// The last component of `path`, or the whole path when it has none.
pub(super) fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Reads and decodes the image at `path` (PNG, JPEG, GIF), reduced for display.
fn decode(path: &Path) -> Result<Decoded, String> {
    let name = file_name(path);
    let reader = image::ImageReader::open(path)
        .map_err(|error| format!("Could not open {name}: {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("Could not read {name}: {error}"))?;
    let decoded = reader.decode().map_err(|error| {
        format!("{name} is not an image the program reads (PNG or JPEG): {error}")
    })?;
    let full_size = [decoded.width(), decoded.height()];
    let [width, height] = display_size(full_size);
    let rgba = if [width, height] == full_size {
        decoded.to_rgba8()
    } else {
        decoded
            .resize_exact(width, height, image::imageops::FilterType::Triangle)
            .to_rgba8()
    };
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer
        .make_mut_slice()
        .copy_from_slice(bytemuck::cast_slice(rgba.as_raw()));
    Ok(Decoded { buffer, full_size })
}

/// Decodes the photo at `path` on a thread of its own, then runs `done` on the UI thread with
/// the result (unless the window is gone by then). A thread that cannot be started is a
/// message in the result.
pub(super) fn spawn_decode<W: ComponentHandle + 'static>(
    weak: slint::Weak<W>,
    path: PathBuf,
    done: impl FnOnce(PathBuf, Result<Decoded, String>) + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name("locate-photo".to_string())
        .spawn(move || {
            let result = decode(&path);
            let _ = weak.upgrade_in_event_loop(move |_window| done(path, result));
        });
    if let Err(error) = spawned {
        warn!("Locate: could not start the photo thread: {error}");
    }
}
