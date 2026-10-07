//! Collision-free names for job output, job file names, and the frame folder marker.

use crate::job::JOB_FILE_SUFFIX;
use std::path::{Path, PathBuf};

/// The file inside a video frame folder that names the job that owns the frames.
pub const FRAME_MARKER_FILE: &str = ".indicatrix-job";

const MARKER_HEADER: &str = "indicatrix render job";
const SLUG_MAX: usize = 48;

/// The text of the frame folder marker for a job token.
#[must_use]
pub fn marker_text(token: &str) -> String {
    format!("{MARKER_HEADER}\n{token}\n")
}

/// The job token in marker text, or `None` when the text is not a marker.
#[must_use]
pub fn marker_token(text: &str) -> Option<&str> {
    let mut lines = text.lines();
    if lines.next()?.trim_end() != MARKER_HEADER {
        return None;
    }
    let token = lines.next()?.trim();
    (!token.is_empty()).then_some(token)
}

/// Whether `path` is one of `taken`, or already exists. Names compare with ASCII case
/// ignored, which is safe on Windows and only over-careful elsewhere.
fn is_taken(path: &Path, taken: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> bool {
    let text = path.to_string_lossy();
    taken
        .iter()
        .any(|other| other.to_string_lossy().eq_ignore_ascii_case(&text))
        || exists(path)
}

/// `candidate`, or the first of `name (2).ext`, `name (3).ext`, ... that is neither in
/// `taken` nor reported by `exists`. The suffix goes before the extension.
#[must_use]
pub fn reserve_unique_file(
    candidate: &Path,
    taken: &[PathBuf],
    exists: &dyn Fn(&Path) -> bool,
) -> PathBuf {
    if !is_taken(candidate, taken, exists) {
        return candidate.to_path_buf();
    }
    let Some(stem) = candidate
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
    else {
        return candidate.to_path_buf();
    };
    let extension = candidate
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..=u32::MAX)
        .map(|n| candidate.with_file_name(format!("{stem} ({n}){extension}")))
        .find(|path| !is_taken(path, taken, exists))
        .unwrap_or_else(|| candidate.to_path_buf())
}

/// `candidate`, or the first of `name (2)`, `name (3)`, ... that is neither in `taken`
/// nor reported by `exists`. The suffix goes after the whole folder name.
#[must_use]
pub fn reserve_unique_folder(
    candidate: &Path,
    taken: &[PathBuf],
    exists: &dyn Fn(&Path) -> bool,
) -> PathBuf {
    if !is_taken(candidate, taken, exists) {
        return candidate.to_path_buf();
    }
    let Some(name) = candidate
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
    else {
        return candidate.to_path_buf();
    };
    (2..=u32::MAX)
        .map(|n| candidate.with_file_name(format!("{name} ({n})")))
        .find(|path| !is_taken(path, taken, exists))
        .unwrap_or_else(|| candidate.to_path_buf())
}

/// A file-name-safe version of a label: ASCII lowercase letters, digits and single
/// hyphens, at most 48 characters, never empty (`job` when nothing is left).
#[must_use]
pub fn slug(label: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    out.truncate(SLUG_MAX);
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "job".to_string()
    } else {
        out
    }
}

/// The job file name for the `index_one_based`-th of `total` jobs, like
/// `01-barion-oval.job.json`. The index has at least two digits, and as many as `total`
/// needs.
#[must_use]
pub fn job_file_name(index_one_based: usize, total: usize, label: &str) -> String {
    let width = total.to_string().len().max(2);
    format!("{index_one_based:0width$}-{}{JOB_FILE_SUFFIX}", slug(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never(_: &Path) -> bool {
        false
    }

    #[test]
    fn a_free_name_is_kept() {
        let candidate = Path::new("out").join("gem.png");
        assert_eq!(reserve_unique_file(&candidate, &[], &never), candidate);
        assert_eq!(
            reserve_unique_folder(Path::new("out/frames"), &[], &never),
            PathBuf::from("out/frames")
        );
    }

    #[test]
    fn taken_names_compare_ignoring_ascii_case() {
        let candidate = Path::new("out").join("Gem.PNG");
        let taken = [Path::new("out").join("gem.png")];
        assert_eq!(
            reserve_unique_file(&candidate, &taken, &never),
            Path::new("out").join("Gem (2).PNG")
        );
        let folder = Path::new("out").join("Frames");
        let taken = [Path::new("out").join("FRAMES")];
        assert_eq!(
            reserve_unique_folder(&folder, &taken, &never),
            Path::new("out").join("Frames (2)")
        );
    }

    #[test]
    fn the_suffix_counts_up_past_taken_and_existing_names() {
        let candidate = Path::new("out").join("gem.png");
        let exists = |path: &Path| path.ends_with("gem (2).png");
        let taken = [Path::new("out").join("gem (3).png")];
        assert_eq!(
            reserve_unique_file(&candidate, &[], &|p| p == candidate || exists(p)),
            Path::new("out").join("gem (3).png")
        );
        assert_eq!(
            reserve_unique_file(&candidate, &taken, &|p| p == candidate || exists(p)),
            Path::new("out").join("gem (4).png")
        );
        let folder = Path::new("out").join("frames");
        let folder_exists = |p: &Path| p == folder || p.ends_with("frames (2)");
        assert_eq!(
            reserve_unique_folder(&folder, &[], &folder_exists),
            Path::new("out").join("frames (3)")
        );
    }

    #[test]
    fn a_name_without_extension_gets_the_suffix_at_the_end() {
        let candidate = Path::new("out").join("notes");
        let taken = [candidate.clone()];
        assert_eq!(
            reserve_unique_file(&candidate, &taken, &never),
            Path::new("out").join("notes (2)")
        );
    }

    #[test]
    fn slugs_are_ascii_and_bounded() {
        assert_eq!(
            slug("Barion Oval · Current view · 1920×1080"),
            "barion-oval-current-view-1920-1080"
        );
        assert_eq!(slug("  --Hello,   World!--  "), "hello-world");
        assert_eq!(slug("Ünïcödé"), "n-c-d");
        assert_eq!(slug(""), "job");
        assert_eq!(slug("···"), "job");
        assert_eq!(slug("日本語"), "job");
        let long = "a".repeat(200);
        assert_eq!(slug(&long), "a".repeat(48));
        let dashed = format!("{}-{}", "a".repeat(47), "b".repeat(10));
        assert_eq!(slug(&dashed), "a".repeat(47));
    }

    #[test]
    fn job_file_names_are_numbered_with_enough_digits() {
        assert_eq!(
            job_file_name(1, 2, "Barion Oval · tilt video · axis 45°"),
            "01-barion-oval-tilt-video-axis-45.job.json"
        );
        assert_eq!(job_file_name(9, 99, ""), "09-job.job.json");
        assert_eq!(job_file_name(99, 100, "x"), "099-x.job.json");
        assert_eq!(job_file_name(100, 100, "x"), "100-x.job.json");
        assert_eq!(job_file_name(5, 1000, "x"), "0005-x.job.json");
        let long = "z".repeat(200);
        let name = job_file_name(1, 3, &long);
        assert_eq!(name, format!("01-{}.job.json", "z".repeat(48)));
    }

    #[test]
    fn marker_text_round_trips() {
        let token = "9f2c4a7d1e0b48c3a65d2f71c8e4b903";
        let text = marker_text(token);
        assert_eq!(
            text,
            "indicatrix render job\n9f2c4a7d1e0b48c3a65d2f71c8e4b903\n"
        );
        assert_eq!(marker_token(&text), Some(token));
        assert_eq!(marker_token(&text.replace('\n', "\r\n")), Some(token));
        assert_eq!(marker_token("something else\nabc\n"), None);
        assert_eq!(marker_token("indicatrix render job\n"), None);
        assert_eq!(marker_token("indicatrix render job\n  \n"), None);
        assert_eq!(marker_token(""), None);
    }
}
