//! HDR environment maps in the browser: the upload limits and the render-worker count
//! rule.
//!
//! Every render Worker decodes its own copy of the map, and so does the analysis Worker
//! (Workers share no memory without `SharedArrayBuffer`); the analysis Worker scores the
//! metrics HUD under the same map the viewport is lit with. A map is never downsampled;
//! instead fewer render Workers run, so that `copies x per-copy map memory` stays within
//! [`RENDER_MEMORY_BUDGET_BYTES`], and a map is refused when even one render Worker cannot
//! hold it. The analysis Worker's copy is the first to go: when the budget holds exactly
//! one copy, the render Worker keeps it and the metrics stay under the lighting preset.

use indicatrix::renderer::env_map::HdrLimits;

/// Largest accepted `.hdr` file.
pub const MAX_HDR_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Widest accepted map, in texels.
pub const MAX_HDR_WIDTH: u32 = 8192;

/// Tallest accepted map, in texels.
pub const MAX_HDR_HEIGHT: u32 = 4096;

/// The memory all the Workers together (render Workers and the analysis Worker) may spend
/// on their map copies: 1.5 GiB.
pub const RENDER_MEMORY_BUDGET_BYTES: u64 = 3 * 512 * 1024 * 1024;

/// Peak bytes per texel while a Worker decodes a map.
///
/// Also what its wasm memory keeps afterwards, since linear memory never shrinks. The
/// decoder's `Rgb32F` buffer (12) and the map's own texel copy (12) live at the same
/// time as the importance-sampling distribution being built (8: a function value and a
/// CDF entry per texel).
pub const DECODE_PEAK_BYTES_PER_TEXEL: u64 = 32;

/// The decode limits a Worker applies: the texel caps, 12 bytes per decoded texel.
#[must_use]
pub const fn web_hdr_limits() -> HdrLimits {
    HdrLimits {
        max_width: MAX_HDR_WIDTH,
        max_height: MAX_HDR_HEIGHT,
        max_decoded_bytes: MAX_HDR_WIDTH as u64 * MAX_HDR_HEIGHT as u64 * 12,
    }
}

/// The render-Worker count for a machine reporting `hardware_concurrency` logical
/// cores: one core is left for the page and the solve Worker, clamped to `1..=8`.
#[must_use]
pub fn render_worker_count(hardware_concurrency: f64) -> u32 {
    let cores = if hardware_concurrency.is_finite() && hardware_concurrency >= 1.0 {
        hardware_concurrency.min(64.0) as u32
    } else {
        1
    };
    cores.saturating_sub(1).clamp(1, 8)
}

/// Why a map was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HdrRefusal {
    /// The file is over [`MAX_HDR_FILE_BYTES`].
    FileTooLarge {
        /// File size in bytes.
        bytes: u64,
    },
    /// The header is not a Radiance `.hdr` header this reader understands.
    NotRadiance,
    /// The map is wider or taller than [`MAX_HDR_WIDTH`] x [`MAX_HDR_HEIGHT`].
    TooManyTexels {
        /// Width in texels.
        width: u32,
        /// Height in texels.
        height: u32,
    },
    /// Even one Worker's copy would exceed the memory budget.
    TooLargeForOneWorker {
        /// Bytes one Worker needs.
        per_worker_bytes: u64,
    },
}

impl std::fmt::Display for HdrRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const MIB: u64 = 1024 * 1024;
        match self {
            Self::FileTooLarge { bytes } => write!(
                f,
                "this HDR file is {} MiB; the browser app accepts up to {} MiB",
                bytes.div_ceil(MIB),
                MAX_HDR_FILE_BYTES / MIB
            ),
            Self::NotRadiance => write!(
                f,
                "this does not look like a Radiance .hdr file (no '#?RADIANCE' header \
                 with a resolution line)"
            ),
            Self::TooManyTexels { width, height } => write!(
                f,
                "this HDR map is {width}x{height} texels; the browser app accepts up to \
                 {MAX_HDR_WIDTH}x{MAX_HDR_HEIGHT}"
            ),
            Self::TooLargeForOneWorker { per_worker_bytes } => write!(
                f,
                "this HDR map needs {} MiB per render worker, more than the {} MiB the \
                 browser app allows",
                per_worker_bytes.div_ceil(MIB),
                RENDER_MEMORY_BUDGET_BYTES / MIB
            ),
        }
    }
}

impl std::error::Error for HdrRefusal {}

/// An admitted map: its size and how many Workers may hold it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HdrAdmission {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// Bytes one Worker needs for it.
    pub per_worker_bytes: u64,
    /// Render Workers to run while this map is loaded (never more than asked for).
    pub workers: u32,
    /// Whether the analysis Worker holds a copy too (it was asked to and the budget has
    /// room for it beside at least one render Worker). When it does not, the metrics are
    /// scored under the lighting preset.
    pub analysis_copy: bool,
}

/// What the page tells the user about an admitted map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HdrNotice {
    /// Whether the map costs something the user should know about: fewer render Workers
    /// than before, or no copy for the analysis Worker.
    pub is_warning: bool,
    /// The message.
    pub text: String,
}

impl HdrAdmission {
    /// The message for loading the map called `name` when `render_workers_before` render
    /// Workers were running.
    #[must_use]
    pub fn notice(&self, name: &str, render_workers_before: u32) -> HdrNotice {
        const MIB: u64 = 1024 * 1024;
        let copy_mib = self.per_worker_bytes.div_ceil(MIB);
        let fewer = self.workers < render_workers_before;
        let head = format!("Lighting with {name} ({} x {})", self.width, self.height);
        let workers = if fewer {
            format!(
                "; to fit the browser's memory it renders on {} of {render_workers_before} workers",
                self.workers
            )
        } else {
            format!(" on {} workers", self.workers)
        };
        let memory = if !self.analysis_copy {
            format!(
                "; no room is left for the analysis worker's {copy_mib} MiB copy, so the metrics \
                 are scored under the lighting preset"
            )
        } else if fewer {
            format!(
                " (each render worker and the analysis worker hold their own {copy_mib} MiB copy)"
            )
        } else {
            String::new()
        };
        HdrNotice {
            is_warning: fewer || !self.analysis_copy,
            text: format!("{head}{workers}{memory}."),
        }
    }
}

/// Reads a Radiance `.hdr` header's resolution line (e.g. `-Y 2048 +X 4096`) without
/// decoding anything. Returns `(width, height)`.
///
/// # Errors
///
/// [`HdrRefusal::NotRadiance`] when the magic line, the blank line ending the header
/// or a resolution line with one X and one Y axis is missing.
pub fn hdr_dimensions(bytes: &[u8]) -> Result<(u32, u32), HdrRefusal> {
    // The header is ASCII and short; only look at its first few KiB.
    let head = &bytes[..bytes.len().min(16 * 1024)];
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split('\n');
    let magic = lines.next().ok_or(HdrRefusal::NotRadiance)?.trim_end();
    if !magic.starts_with("#?") {
        return Err(HdrRefusal::NotRadiance);
    }
    // Header variables up to the first blank line, then the resolution line.
    lines
        .by_ref()
        .find(|line| line.trim_end().is_empty())
        .ok_or(HdrRefusal::NotRadiance)?;
    let resolution = lines.next().ok_or(HdrRefusal::NotRadiance)?;
    parse_resolution(resolution).ok_or(HdrRefusal::NotRadiance)
}

fn parse_resolution(line: &str) -> Option<(u32, u32)> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let [axis_a, len_a, axis_b, len_b] = tokens.as_slice() else {
        return None;
    };
    let len_a: u32 = len_a.parse().ok()?;
    let len_b: u32 = len_b.parse().ok()?;
    let axis = |token: &str| token.trim_start_matches(['+', '-']).to_ascii_uppercase();
    match (axis(axis_a).as_str(), axis(axis_b).as_str()) {
        ("Y", "X") => Some((len_b, len_a)),
        ("X", "Y") => Some((len_a, len_b)),
        _ => None,
    }
}

/// Bytes one Worker needs for a `width x height` map sent as a `file_bytes`-byte file:
/// the decode peak plus the received file itself.
#[must_use]
pub const fn per_worker_bytes(width: u32, height: u32, file_bytes: u64) -> u64 {
    width as u64 * height as u64 * DECODE_PEAK_BYTES_PER_TEXEL + file_bytes
}

/// Decides whether a map may be loaded and on how many render Workers (at most
/// `render_workers`, at least 1).
///
/// `analysis_copy` says the analysis Worker should hold a copy as well. The copies that
/// will exist are the render Workers' and that one, all within
/// [`RENDER_MEMORY_BUDGET_BYTES`]: a render Worker is given up for it, but never the last
/// one, so a budget with room for a single copy leaves it to the render Worker
/// ([`HdrAdmission::analysis_copy`] is then `false`).
///
/// # Errors
///
/// See [`HdrRefusal`].
pub fn admit_hdr(
    bytes: &[u8],
    render_workers: u32,
    analysis_copy: bool,
) -> Result<HdrAdmission, HdrRefusal> {
    admit_hdr_within(
        bytes,
        render_workers,
        analysis_copy,
        RENDER_MEMORY_BUDGET_BYTES,
    )
}

/// [`admit_hdr`] against an explicit memory `budget_bytes`.
///
/// With the real budget every map the texel caps allow fits one Worker, so the "too large
/// for one Worker" refusal can only be reached with a smaller one.
fn admit_hdr_within(
    bytes: &[u8],
    render_workers: u32,
    analysis_copy: bool,
    budget_bytes: u64,
) -> Result<HdrAdmission, HdrRefusal> {
    let file_bytes = bytes.len() as u64;
    if file_bytes > MAX_HDR_FILE_BYTES {
        return Err(HdrRefusal::FileTooLarge { bytes: file_bytes });
    }
    let (width, height) = hdr_dimensions(bytes)?;
    if width == 0 || height == 0 || width > MAX_HDR_WIDTH || height > MAX_HDR_HEIGHT {
        return Err(HdrRefusal::TooManyTexels { width, height });
    }
    let per_worker = per_worker_bytes(width, height, file_bytes);
    let fit = budget_bytes / per_worker;
    if fit == 0 {
        return Err(HdrRefusal::TooLargeForOneWorker {
            per_worker_bytes: per_worker,
        });
    }
    // The analysis copy only exists beside at least one render Worker's.
    let analysis_copy = analysis_copy && fit >= 2;
    let render_fit = fit - u64::from(analysis_copy);
    Ok(HdrAdmission {
        width,
        height,
        per_worker_bytes: per_worker,
        workers: render_workers
            .max(1)
            .min(u32::try_from(render_fit).unwrap_or(u32::MAX)),
        analysis_copy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(resolution: &str) -> Vec<u8> {
        format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\nEXPOSURE=1.0\n\n{resolution}\n\x02\x02")
            .into_bytes()
    }

    #[test]
    fn the_worker_count_leaves_a_core_and_stays_in_one_to_eight() {
        assert_eq!(render_worker_count(1.0), 1);
        assert_eq!(render_worker_count(2.0), 1);
        assert_eq!(render_worker_count(4.0), 3);
        assert_eq!(render_worker_count(9.0), 8);
        assert_eq!(render_worker_count(32.0), 8);
        assert_eq!(render_worker_count(f64::NAN), 1);
        assert_eq!(render_worker_count(0.0), 1);
    }

    #[test]
    fn resolution_lines_parse_in_either_axis_order() {
        assert_eq!(hdr_dimensions(&header("-Y 2048 +X 4096")), Ok((4096, 2048)));
        assert_eq!(hdr_dimensions(&header("+X 300 -Y 100")), Ok((300, 100)));
        assert_eq!(hdr_dimensions(b"not an hdr"), Err(HdrRefusal::NotRadiance));
        assert_eq!(
            hdr_dimensions(&header("-Y 20 +Z 40")),
            Err(HdrRefusal::NotRadiance)
        );
        assert_eq!(
            hdr_dimensions(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n"),
            Err(HdrRefusal::NotRadiance)
        );
    }

    #[test]
    fn big_maps_run_on_fewer_workers_and_oversize_ones_are_refused() {
        // 2048 x 1024: 64 MiB per worker plus the file -- 8 workers fit easily.
        let small = admit_hdr(&header("-Y 1024 +X 2048"), 8, false).expect("admitted");
        assert_eq!(small.workers, 8);
        // 8192 x 4096: 1 GiB per worker -- one worker.
        let big = admit_hdr(&header("-Y 4096 +X 8192"), 8, false).expect("admitted");
        assert_eq!(big.workers, 1);
        // 4096 x 2048: 256 MiB -> five fit in 1.5 GiB.
        let mid = admit_hdr(&header("-Y 2048 +X 4096"), 8, false).expect("admitted");
        assert_eq!(mid.workers, 5);
        assert_eq!(
            admit_hdr(&header("-Y 4097 +X 8192"), 8, false),
            Err(HdrRefusal::TooManyTexels {
                width: 8192,
                height: 4097
            })
        );
        let limits = web_hdr_limits();
        assert!(limits.admits(MAX_HDR_WIDTH, MAX_HDR_HEIGHT));
        assert!(!limits.admits(MAX_HDR_WIDTH + 1, 1));
    }

    #[test]
    fn the_largest_map_the_caps_allow_still_fits_one_worker() {
        // 8192 x 4096 texels in a file padded to the 64 MiB cap: the worst case.
        let mut bytes = header("-Y 4096 +X 8192");
        bytes.resize(MAX_HDR_FILE_BYTES as usize, 0);
        let admitted = admit_hdr(&bytes, 8, true).expect("admitted");
        assert_eq!(
            admitted.per_worker_bytes,
            8192 * 4096 * DECODE_PEAK_BYTES_PER_TEXEL + MAX_HDR_FILE_BYTES
        );
        assert!(admitted.per_worker_bytes <= RENDER_MEMORY_BUDGET_BYTES);
        assert_eq!(admitted.workers, 1);
        assert!(
            !admitted.analysis_copy,
            "a second copy of the largest map does not fit"
        );
    }

    #[test]
    fn a_map_no_worker_can_hold_is_refused_under_a_smaller_budget() {
        // 2048 x 1024: 64 MiB per worker plus the file.
        let bytes = header("-Y 1024 +X 2048");
        let need = per_worker_bytes(2048, 1024, bytes.len() as u64);
        assert_eq!(
            admit_hdr_within(&bytes, 8, true, need - 1),
            Err(HdrRefusal::TooLargeForOneWorker {
                per_worker_bytes: need
            })
        );
        let exact = admit_hdr_within(&bytes, 8, false, need).expect("one worker fits exactly");
        assert_eq!(exact.workers, 1);
        let refusal = admit_hdr_within(&bytes, 8, true, 0).expect_err("nothing fits");
        assert!(refusal.to_string().contains("per render worker"));
    }

    /// The analysis Worker's copy counts against the budget: it costs the render pool one
    /// Worker, except that the last render Worker is never given up for it.
    #[test]
    fn the_analysis_workers_copy_is_counted_and_never_displaces_the_last_render_worker() {
        // 4096 x 2048: 256 MiB per copy -> five copies fit; one is the analysis Worker's.
        let mid = admit_hdr(&header("-Y 2048 +X 4096"), 8, true).expect("admitted");
        assert_eq!((mid.workers, mid.analysis_copy), (4, true));
        // Fewer render Workers asked for than fit: nothing is given up.
        let few = admit_hdr(&header("-Y 2048 +X 4096"), 2, true).expect("admitted");
        assert_eq!((few.workers, few.analysis_copy), (2, true));
        // Two copies fit exactly: one render Worker and the analysis Worker.
        let bytes = header("-Y 1024 +X 2048");
        let need = per_worker_bytes(2048, 1024, bytes.len() as u64);
        let two = admit_hdr_within(&bytes, 8, true, 2 * need).expect("admitted");
        assert_eq!((two.workers, two.analysis_copy), (1, true));
        // One copy fits: the render Worker keeps it.
        let one = admit_hdr_within(&bytes, 8, true, 2 * need - 1).expect("admitted");
        assert_eq!((one.workers, one.analysis_copy), (1, false));
        // Not asking for an analysis copy leaves every copy to the render Workers.
        let render_only = admit_hdr_within(&bytes, 8, false, 3 * need).expect("admitted");
        assert_eq!((render_only.workers, render_only.analysis_copy), (3, false));
    }

    #[test]
    fn the_notice_names_what_the_map_costs() {
        let all = HdrAdmission {
            width: 2048,
            height: 1024,
            per_worker_bytes: 64 * 1024 * 1024,
            workers: 8,
            analysis_copy: true,
        };
        let plain = all.notice("sky.hdr", 8);
        assert!(!plain.is_warning);
        assert_eq!(
            plain.text,
            "Lighting with sky.hdr (2048 x 1024) on 8 workers."
        );

        let fewer = HdrAdmission { workers: 4, ..all }.notice("sky.hdr", 8);
        assert!(fewer.is_warning);
        assert_eq!(
            fewer.text,
            "Lighting with sky.hdr (2048 x 1024); to fit the browser's memory it renders on \
             4 of 8 workers (each render worker and the analysis worker hold their own 64 MiB \
             copy)."
        );

        let no_copy = HdrAdmission {
            analysis_copy: false,
            ..all
        }
        .notice("sky.hdr", 8);
        assert!(no_copy.is_warning);
        assert!(
            no_copy
                .text
                .ends_with("so the metrics are scored under the lighting preset."),
            "{}",
            no_copy.text
        );
    }
}
