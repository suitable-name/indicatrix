//! Muxing the numbered PNG frame sequence into a video: an ffmpeg H.264 MP4 when an
//! `ffmpeg` executable is found on `PATH`, an animated GIF via the `image` crate
//! otherwise, or -- if even that fails -- leaving the frame sequence in place with a
//! `README.txt` explaining how to mux it by hand.
//!
//! No ICC colour profile is embedded in either output: unlike the still-image export's
//! single PNG (`bridge::export_thread::tonemap_png::save_png`), these frames feed
//! either `ffmpeg` (which re-encodes to `yuv420p` and carries no per-frame ICC concept)
//! or a GIF (an 8-bit palette format with no colour-management story of its own), so an
//! embedded profile on the intermediate PNGs would be silently dropped either way.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// What [`mux_mp4`]/[`mux_gif`]/[`write_readme`] actually produced, for the caller's
/// own status message/toast.
#[derive(Debug)]
pub enum EncodeOutcome {
    Mp4(PathBuf),
    Gif(PathBuf),
    FramesOnly { readme: PathBuf },
}

/// Whether an `ffmpeg` executable answers on `PATH` -- probed by actually trying to run
/// it (`-version`) rather than walking `PATH` by hand, so this behaves identically on
/// Windows (`ffmpeg.exe`) and Unix without platform-specific path-joining.
#[must_use]
pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The exact ffmpeg command [`mux_mp4`] runs, also written into `README.txt` when no
/// video could be produced automatically -- kept as one function so the two can never
/// drift apart.
#[must_use]
pub fn ffmpeg_command_line(digits: usize, fps: u32, out_name: &str) -> String {
    format!(
        "ffmpeg -y -framerate {fps} -i \"frame_%0{digits}d.png\" -c:v libx264 -pix_fmt yuv420p -crf 18 \"{out_name}.mp4\""
    )
}

/// Muxes the PNG sequence in `frame_dir` (named `frame_%0{digits}d.png`, see
/// `params::frame_file_name`) into an H.264 MP4 at `fps`, written to `out_path`.
/// Writes to a `.partial` sibling first and renames on success only, so a run that
/// fails partway through (or is killed) never leaves a broken/incomplete file at the
/// final `.mp4` name.
///
/// # Errors
///
/// Returns an error string if ffmpeg cannot be launched, exits non-zero, or the final
/// rename fails.
pub fn mux_mp4(frame_dir: &Path, digits: usize, fps: u32, out_path: &Path) -> Result<(), String> {
    let partial = out_path.with_extension("mp4.partial");
    let pattern = format!("frame_%0{digits}d.png");
    let status = Command::new("ffmpeg")
        .current_dir(frame_dir)
        .arg("-y")
        .arg("-framerate")
        .arg(fps.to_string())
        .arg("-i")
        .arg(&pattern)
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18"])
        .arg(&partial)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("Failed to launch ffmpeg: {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("ffmpeg exited with status {status}"));
    }
    std::fs::rename(&partial, out_path)
        .map_err(|e| format!("Failed to finalize {}: {e}", out_path.display()))
}

/// Encodes an animated GIF from the already-written PNG frame sequence, decoding one
/// frame at a time (never holding every frame in memory at once, even at 8K) -- the
/// second-tier fallback when `ffmpeg` isn't on `PATH`.
///
/// # Errors
///
/// Returns an error string if the output file can't be created, a frame can't be
/// decoded, or the encoder itself fails.
pub fn mux_gif(frame_paths: &[PathBuf], fps: u32, out_path: &Path) -> Result<(), String> {
    use image::{Delay, Frame, codecs::gif::GifEncoder};
    let file = std::fs::File::create(out_path).map_err(|e| e.to_string())?;
    // Speed 10 = fastest/lowest-effort quantization -- a GIF is already this
    // pipeline's lowest-fidelity fallback, so encode speed matters more than palette
    // optimality here.
    let mut encoder = GifEncoder::new_with_speed(file, 10);
    let delay_ms = u64::from(1000 / fps.max(1));
    for path in frame_paths {
        let decoded = image::open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .to_rgba8();
        let frame = Frame::from_parts(
            decoded,
            0,
            0,
            Delay::from_saturating_duration(std::time::Duration::from_millis(delay_ms)),
        );
        encoder.encode_frame(frame).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Writes the "mux it yourself" README into `frame_dir` -- the last-resort fallback
/// when neither ffmpeg nor the GIF encoder could produce a video.
///
/// # Errors
///
/// Returns an error string if the file can't be written.
pub fn write_readme(
    frame_dir: &Path,
    digits: usize,
    fps: u32,
    out_name: &str,
) -> Result<PathBuf, String> {
    let path = frame_dir.join("README.txt");
    let body = format!(
        "This folder holds the exported tilt-performance frame sequence.\n\n\
         No video was produced automatically (ffmpeg was not found on PATH, and the \
         GIF fallback did not succeed either). To build an MP4 yourself, install \
         ffmpeg (https://ffmpeg.org/) and run this from inside this folder:\n\n{}\n",
        ffmpeg_command_line(digits, fps, out_name)
    );
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Runs the full ffmpeg -> GIF -> README fallback chain for a finished frame sequence.
/// Never called when the export was cancelled (see `mod.rs`'s own guard) -- a
/// cancelled run leaves only whatever frames were written, no video and no README.
#[must_use]
pub fn encode(
    frame_dir: &Path,
    frame_paths: &[PathBuf],
    digits: usize,
    fps: u32,
    out_name: &str,
) -> EncodeOutcome {
    let mp4_path = frame_dir.join(format!("{out_name}.mp4"));
    if ffmpeg_available() {
        match mux_mp4(frame_dir, digits, fps, &mp4_path) {
            Ok(()) => return EncodeOutcome::Mp4(mp4_path),
            Err(e) => tracing::warn!("Tilt video: ffmpeg mux failed, falling back to GIF: {e}"),
        }
    }

    let gif_path = frame_dir.join(format!("{out_name}.gif"));
    match mux_gif(frame_paths, fps, &gif_path) {
        Ok(()) => return EncodeOutcome::Gif(gif_path),
        Err(e) => {
            tracing::warn!("Tilt video: GIF fallback failed, leaving the frame sequence: {e}");
        }
    }

    let readme = write_readme(frame_dir, digits, fps, out_name)
        .unwrap_or_else(|_| frame_dir.join("README.txt"));
    EncodeOutcome::FramesOnly { readme }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_command_line_names_every_required_flag() {
        let cmd = ffmpeg_command_line(5, 30, "clip");
        assert!(cmd.contains("-framerate 30"));
        assert!(cmd.contains("frame_%05d.png"));
        assert!(cmd.contains("-pix_fmt yuv420p"));
        assert!(cmd.contains("-crf 18"));
        assert!(cmd.contains("clip.mp4"));
    }

    /// `mux_mp4` must never leave a `.partial` OR a final `.mp4` behind when ffmpeg
    /// itself can't even be launched (simulated here by pointing at a directory that
    /// doesn't exist, which fails the launch on every platform) -- the "cancel/failure
    /// leaves no partial MP4" guarantee at the encode layer itself.
    #[test]
    fn mux_mp4_leaves_no_output_when_ffmpeg_cannot_run() {
        let dir = std::env::temp_dir().join(format!(
            "tilt_video_encode_test_{}_{}",
            std::process::id(),
            line!()
        ));
        let out = dir.join("clip.mp4");
        let result = mux_mp4(&dir, 3, 30, &out);
        assert!(result.is_err());
        assert!(!out.exists());
        assert!(!out.with_extension("mp4.partial").exists());
    }

    #[test]
    fn write_readme_embeds_the_exact_ffmpeg_command() {
        let dir =
            std::env::temp_dir().join(format!("tilt_video_readme_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_readme(&dir, 3, 24, "clip").unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains(&ffmpeg_command_line(3, 24, "clip")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
