//! Export request validation.
//!
//! Split out of `bridge::export_thread` purely to keep that module from growing
//! further. The default output path lives in `filename_template`'s configurable
//! resolver, not here.

use crate::settings::{ExportTransfer, WorkerSettings};

/// Sane bounds for user-supplied export dimensions. `MAX_EXPORT_DIM` exists so
/// nobody can point the exporter at, say, 100000x100000 and lock up the machine.
pub const MIN_EXPORT_DIM: u32 = 16;
/// Largest allowed export width or height in pixels.
pub const MAX_EXPORT_DIM: u32 = 8192;
/// Smallest allowed export samples per pixel.
pub const MIN_EXPORT_SPP: u32 = 1;
/// Deliberately generous: at 4K, 32768 spp is ~272 billion spectral paths (roughly
/// 3+ hours on this project's integrated AMD Radeon, ~23M samples/sec). The cap only
/// exists to stop a typo (an extra zero) from locking up the machine, not to
/// second-guess a deliberate overnight run.
pub const MAX_EXPORT_SPP: u32 = 32768;

/// Bounds for the export's max-bounce cap -- matches the ladder
/// `export_dialog.slint`/`settings_dialog.slint` offer (4/8/12/24/64/128). The
/// ceiling is 128 because `bounce_cost.rs` measurements found zero further image
/// change up to a 1024-bounce reference. Validated defensively even though the
/// dialog only ever sends one of the six rungs.
pub const MIN_EXPORT_BOUNCES: u32 = 1;
/// Largest allowed export bounce count.
pub const MAX_EXPORT_BOUNCES: u32 = 128;

/// User-chosen parameters of one export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportParams {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Samples per pixel.
    pub samples_per_pixel: u32,
    /// The export's OWN max-bounce cap, independent of the live viewport -- must
    /// override `SceneSnapshot::capture`'s `guard.max_bounces` rather than being read
    /// from it (see `gui::render_export`).
    pub max_bounces: u32,
}

/// Which engine(s) an export should use -- the export dialog's "Compute" pill,
/// matching `export_dialog.slint`'s `compute_target` property (0/1/2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeTarget {
    LocalOnly,
    RemoteOnly,
    /// Both engines trace disjoint sample ranges concurrently -- see
    /// `export_thread::remote` and `run_export`. The default whenever a worker
    /// advertising render capacity is configured.
    Both,
}

impl ComputeTarget {
    /// Whether the export suspends the live viewport for its duration. `RemoteOnly` uses
    /// no local lane, no GPU adapter and no hybrid probe (see `frame_local_compute` in the
    /// tilt video and `run_export`), so the viewport keeps tracing while the remote renders;
    /// `LocalOnly` and `Both` share the CPU, GPU and adapter with it and pause it.
    #[must_use]
    pub const fn pauses_live_viewport(self) -> bool {
        !matches!(self, Self::RemoteOnly)
    }
}

/// Everything about the REMOTE side of one export or tilt video, bundled so the render
/// entry points take one argument for it: which engines ([`ComputeTarget`]), the
/// configured remote endpoint's connection (`None` when no remote is configured -- there
/// is at most one), and how its result travels ([`ExportTransfer`]: full data or final
/// picture only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSelection {
    /// The export dialog's "Compute" pill (the tilt video section has its own).
    pub compute_target: ComputeTarget,
    /// The remote endpoint's connection settings, if one is configured.
    pub worker: Option<WorkerSettings>,
    /// "Transfer: Full data / Final picture only".
    pub transfer: ExportTransfer,
    /// v16: whether this machine's own idle CPU/GPU may trace a share of a
    /// final-picture export's sample budget alongside the remote -- the settings
    /// dialog's "Final-picture exports: this machine renders a share too" switch
    /// (`AppSettings::contribute_to_final_picture`). Only ever matters when `transfer`
    /// is [`ExportTransfer::FinalPicture`] and `compute_target` is
    /// [`ComputeTarget::Both`] -- see
    /// `worker::final_picture::contribution_allowed`'s own guards.
    pub contribute_local: bool,
}

#[cfg(test)]
impl RemoteSelection {
    /// A local-only selection (no remote involved at all) -- test fixtures.
    #[must_use]
    pub const fn local_only() -> Self {
        Self {
            compute_target: ComputeTarget::LocalOnly,
            worker: None,
            transfer: ExportTransfer::FullData,
            contribute_local: false,
        }
    }
}

/// Validates raw (Slint-supplied, hence signed) export request fields, rejecting zero,
/// negative, and absurdly large values before anything is allocated or a thread is
/// spawned. Pure and side-effect-free so it's directly unit-testable.
pub fn validate_export_params(
    width: i32,
    height: i32,
    samples_per_pixel: i32,
    max_bounces: i32,
) -> Result<ExportParams, String> {
    if width <= 0 || height <= 0 || samples_per_pixel <= 0 || max_bounces <= 0 {
        return Err(
            "Width, height, sample count, and max bounces must all be positive.".to_string(),
        );
    }
    let width = width as u32;
    let height = height as u32;
    let samples_per_pixel = samples_per_pixel as u32;
    let max_bounces = max_bounces as u32;

    if width < MIN_EXPORT_DIM || height < MIN_EXPORT_DIM {
        return Err(format!(
            "Minimum export size is {MIN_EXPORT_DIM}x{MIN_EXPORT_DIM} px."
        ));
    }
    if width > MAX_EXPORT_DIM || height > MAX_EXPORT_DIM {
        return Err(format!(
            "Maximum export size is {MAX_EXPORT_DIM}x{MAX_EXPORT_DIM} px (requested {width}x{height})."
        ));
    }
    if samples_per_pixel < MIN_EXPORT_SPP {
        return Err("Sample count must be at least 1.".to_string());
    }
    if samples_per_pixel > MAX_EXPORT_SPP {
        return Err(format!(
            "Maximum sample count is {MAX_EXPORT_SPP} samples per pixel."
        ));
    }
    if !(MIN_EXPORT_BOUNCES..=MAX_EXPORT_BOUNCES).contains(&max_bounces) {
        return Err(format!(
            "Max bounces must be between {MIN_EXPORT_BOUNCES} and {MAX_EXPORT_BOUNCES}."
        ));
    }

    Ok(ExportParams {
        width,
        height,
        samples_per_pixel,
        max_bounces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_export_params_rejects_zero_and_negative() {
        assert!(validate_export_params(0, 1080, 64, 12).is_err());
        assert!(validate_export_params(1920, 0, 64, 12).is_err());
        assert!(validate_export_params(1920, 1080, 0, 12).is_err());
        assert!(validate_export_params(1920, 1080, 64, 0).is_err());
        assert!(validate_export_params(-1920, 1080, 64, 12).is_err());
        assert!(validate_export_params(1920, -1080, 64, 12).is_err());
        assert!(validate_export_params(1920, 1080, -1, 12).is_err());
        assert!(validate_export_params(1920, 1080, 64, -1).is_err());
    }

    #[test]
    fn validate_export_params_rejects_absurdly_large_requests() {
        let err = validate_export_params(100_000, 100_000, 64, 12).unwrap_err();
        assert!(err.contains("Maximum export size"));
    }

    #[test]
    fn validate_export_params_rejects_excessive_sample_counts() {
        let err = validate_export_params(1920, 1080, 1_000_000, 12).unwrap_err();
        assert!(err.contains("Maximum sample count"));
    }

    #[test]
    fn validate_export_params_rejects_tiny_dimensions_below_minimum() {
        assert!(validate_export_params(1, 1, 64, 12).is_err());
    }

    /// The ladder `export_dialog.slint` offers tops out at 128, so anything above that
    /// is rejected rather than silently clamped.
    #[test]
    fn validate_export_params_rejects_bounce_caps_outside_the_ladders_range() {
        let err = validate_export_params(1920, 1080, 64, 129).unwrap_err();
        assert!(err.contains("Max bounces"));
        assert!(validate_export_params(1920, 1080, 64, 0).is_err());
    }

    #[test]
    fn validate_export_params_accepts_sensible_presets() {
        assert_eq!(
            validate_export_params(1920, 1080, 256, 12).unwrap(),
            ExportParams {
                width: 1920,
                height: 1080,
                samples_per_pixel: 256,
                max_bounces: 12,
            }
        );
        assert_eq!(
            validate_export_params(3840, 2160, 64, 128).unwrap(),
            ExportParams {
                width: 3840,
                height: 2160,
                samples_per_pixel: 64,
                max_bounces: 128,
            }
        );
    }

    #[test]
    fn validate_export_params_accepts_boundary_values() {
        assert!(
            validate_export_params(
                MIN_EXPORT_DIM as i32,
                MIN_EXPORT_DIM as i32,
                MIN_EXPORT_SPP as i32,
                MIN_EXPORT_BOUNCES as i32,
            )
            .is_ok()
        );
        assert!(
            validate_export_params(
                MAX_EXPORT_DIM as i32,
                MAX_EXPORT_DIM as i32,
                MAX_EXPORT_SPP as i32,
                MAX_EXPORT_BOUNCES as i32,
            )
            .is_ok()
        );
    }

    // Default output path coverage lives in `filename_template::tests`, which
    // tests the configurable template resolver.
}
