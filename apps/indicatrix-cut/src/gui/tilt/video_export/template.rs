//! Resolves the video export's own folder-name template (`{design}`, `{material}`,
//! `{axis}`, `{fps}`, ... -- see [`substitute_video_only`] for the video-specific
//! variables) into a sanitised folder name.
//!
//! Deliberately its own small resolver rather than a reuse of
//! `bridge::export_thread::filename_template::resolve_export_path`: that function
//! always appends `.png` and resolves a FILE path inside a directory it asserts already
//! exists, where this needs a FOLDER name (the parent of the numbered PNG frames plus
//! the muxed video) -- and its sanitiser/collision-avoider are private to that
//! module. It reuses that module's fully public
//! [`filename_template::render`]/[`TemplateContext`] for every variable the two share
//! (`{design}`, `{material}`, `{width}`, ...), pre-substituting the video-only
//! variables in a first pass.

use crate::bridge::export_thread::filename_template::{self, TemplateContext};
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The default folder-name template for a tilt video export -- mirrors
/// `TiltVideoExportModel.filename_template`'s own Slint-side default; the two are kept
/// in sync by hand since a Slint property default cannot reference a Rust constant.
pub const DEFAULT_VIDEO_TEMPLATE: &str =
    "tilt_video_{design}_{material}_axis{axis}_{width}x{height}_{timestamp}";

/// Video-only template variables `filename_template::render` doesn't know about --
/// substituted in a first pass before handing the result to `render` for every
/// variable the still-image export already supports.
#[derive(Debug, Clone)]
pub struct VideoTemplateExtras {
    /// The swept axis's own label, e.g. `"0"`/`"45"`/`"90"`/`"135"` (degrees) --
    /// `{axis}`.
    pub axis_label: String,
    pub fps: u32,
    pub step_deg: f64,
    pub start_deg: f64,
    pub end_deg: f64,
    pub total_frames: usize,
}

fn substitute_video_only(template: &str, extras: &VideoTemplateExtras) -> String {
    template
        .replace("{axis}", &extras.axis_label)
        .replace("{fps}", &extras.fps.to_string())
        .replace("{step}", &format!("{:.2}", extras.step_deg))
        .replace("{start}", &format!("{:.1}", extras.start_deg))
        .replace("{end}", &format!("{:.1}", extras.end_deg))
        .replace("{frames}", &extras.total_frames.to_string())
}

const fn is_forbidden_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20
}

/// Sanitises a rendered folder name against the same forbidden-character/trailing-dot
/// rules `filename_template::sanitize_filename` applies to a file stem -- that function
/// is private to its own module, so this is a small, deliberate duplicate for a FOLDER
/// name rather than a `.png` file stem (no Windows-reserved-device-name check: unlike a
/// bare `CON.png`, `CON` is not itself a device name once other template text is almost
/// always adjacent, and a folder named exactly `CON` is vanishingly unlikely given this
/// template's own defaults -- accepted as a minor, documented gap rather than
/// duplicating that whole table too).
fn sanitize_folder_name(raw: &str) -> String {
    let mut cleaned: String = raw
        .chars()
        .map(|c| if is_forbidden_char(c) { '_' } else { c })
        .collect();
    while matches!(cleaned.chars().last(), Some('.' | ' ')) {
        cleaned.pop();
    }
    if cleaned.is_empty() {
        cleaned = "tilt_video".to_string();
    }
    cleaned
}

/// Renders and sanitises `template` against `ctx`/`extras`, without resolving
/// collisions against the filesystem. Used both for the real export (via
/// [`unique_folder_path`]) and the dialog's live "Resolves to" preview, which has no
/// filesystem to check collisions against -- matching `ExportModel.preview_filename_template`'s
/// own "preview against a fixed example, not a real collision check" convention.
#[must_use]
pub fn resolve_folder_name(
    template: &str,
    ctx: &TemplateContext,
    extras: &VideoTemplateExtras,
) -> String {
    let pre_substituted = substitute_video_only(template, extras);
    let rendered = filename_template::render(&pre_substituted, ctx, SystemTime::now());
    sanitize_folder_name(&rendered)
}

/// Finds a folder name inside `export_dir` that doesn't already exist, appending
/// ` (2)`, ` (3)`, ... -- the folder counterpart of `filename_template`'s own
/// `unique_path`.
#[must_use]
pub fn unique_folder_path(export_dir: &Path, name: &str) -> PathBuf {
    let mut candidate = export_dir.join(name);
    let mut suffix = 2u32;
    while candidate.exists() {
        candidate = export_dir.join(format!("{name} ({suffix})"));
        suffix += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ctx() -> TemplateContext {
        TemplateContext {
            design: "Round Brilliant".to_string(),
            designer: String::new(),
            shape: "Round".to_string(),
            material: "Diamond".to_string(),
            ri: "2.42".to_string(),
            width: 512,
            height: 512,
            spp: 64,
            bounces: 12,
            colorspace: "sRGB".to_string(),
            preset: String::new(),
            lighting: "Gem Studio Ring Lights".to_string(),
            yaw_deg: 0.0,
            pitch_deg: 90.0,
            distance: 2.4,
            exposure: 1.0,
        }
    }

    fn sample_extras() -> VideoTemplateExtras {
        VideoTemplateExtras {
            axis_label: "45".to_string(),
            fps: 30,
            step_deg: 1.0,
            start_deg: -90.0,
            end_deg: 90.0,
            total_frames: 181,
        }
    }

    #[test]
    fn resolve_folder_name_substitutes_both_shared_and_video_only_variables() {
        let name = resolve_folder_name(
            "{design}_{material}_axis{axis}_{width}x{height}_{fps}fps",
            &sample_ctx(),
            &sample_extras(),
        );
        assert_eq!(name, "Round Brilliant_Diamond_axis45_512x512_30fps");
    }

    #[test]
    fn resolve_folder_name_strips_forbidden_characters() {
        let mut ctx = sample_ctx();
        ctx.design = "A/B:C".to_string();
        let name = resolve_folder_name("{design}", &ctx, &sample_extras());
        assert_eq!(name, "A_B_C");
    }

    #[test]
    fn resolve_folder_name_never_returns_empty() {
        let mut ctx = sample_ctx();
        ctx.design.clear();
        let name = resolve_folder_name("", &ctx, &sample_extras());
        assert_eq!(name, "tilt_video");
    }

    #[test]
    fn resolve_folder_name_leaves_an_unrecognised_placeholder_literal() {
        let name = resolve_folder_name("{nonsense}", &sample_ctx(), &sample_extras());
        assert_eq!(name, "{nonsense}");
    }

    #[test]
    fn unique_folder_path_appends_a_counter_on_collision() {
        let dir =
            std::env::temp_dir().join(format!("tilt_video_template_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir(dir.join("clip")).unwrap();

        let resolved = unique_folder_path(&dir, "clip");
        assert_eq!(resolved, dir.join("clip (2)"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
