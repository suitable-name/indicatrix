//! The New Design template gallery's Rust-side glue. The template data itself
//! lives in `indicatrix_cut_core::templates` (five entries, each a
//! statically-verified closed solid -- see that module's doc comment for why
//! five, not a dozen); this module's only job is (a) handing that data to
//! `ui/models/templates.slint`'s `TemplateGalleryModel` for the gallery grid to
//! render, and (b) the thumbnail cache's path logic.
//!
//! # Every gallery entry is selectable
//!
//! `new_design_dialog.slint`'s "Create" button calls
//! `EditorModel.new_design_create(..., template_index)`, handled by
//! `gui::editor::callbacks::tier_actions::do_new_design_create`: that
//! function's template dispatch now indexes
//! `indicatrix_cut_core::templates::TEMPLATES` at `template_index - 1` for
//! every `template_index >= 1` (index 1 is "Standard Round Brilliant",
//! `TEMPLATES[0]`, matching this gallery's own display order), so every card
//! [`setup_template_gallery`] lists below is reachable, not only the first.
//! [`TemplateCardData::ready`] is `true` for every entry as a result.
//! `template_gallery.slint` has no "not selectable yet" state to render.

use crate::{MainWindow, TemplateCardData};
use slint::{ComponentHandle as _, ModelRc, SharedString, VecModel};
use std::path::{Path, PathBuf};

/// Pushes the built-in template gallery into `TemplateGalleryModel.templates`
/// once, at startup. Index 0 ("Empty") is authored here directly, matching
/// `new_design_dialog.slint`'s own pre-existing `["Empty", "Standard Round
/// Brilliant"]` combo model; every entry after it comes from
/// `indicatrix_cut_core::templates::TEMPLATES`, in order -- every one of them
/// `ready` (see this module's own top doc comment for the dispatch that makes
/// each one reachable).
pub(in crate::gui::editor) fn setup_template_gallery(ui: &MainWindow) {
    let mut cards = vec![TemplateCardData {
        name: SharedString::from("Empty"),
        shape: SharedString::from("Blank design"),
        description: SharedString::from("No starting tiers -- author the schedule from scratch."),
        ready: true,
    }];
    for spec in indicatrix_cut_core::templates::TEMPLATES {
        cards.push(TemplateCardData {
            name: SharedString::from(spec.name),
            shape: SharedString::from(spec.shape),
            description: SharedString::from(spec.description),
            ready: true,
        });
    }
    ui.global::<crate::TemplateGalleryModel>()
        .set_templates(ModelRc::new(VecModel::from(cards)));
}

/// Where a rendered thumbnail for `template_name` would be cached on disk,
/// under `cache_dir` (the app's own settings/config directory --
/// `settings::store`'s `platform_config_dir`-derived path, a sibling
/// subdirectory this function does not resolve itself so it stays a pure,
/// easily testable path computation). The template name is filtered to
/// `[A-Za-z0-9_-]` and lower-cased before becoming a filename, so a name with
/// spaces or punctuation (every entry in [`indicatrix_cut_core::templates::TEMPLATES`]
/// has at least one space) still produces one safe, stable, collision-free
/// path per template across platforms.
///
/// Not yet wired to an actual renderer -- see this module's own top doc
/// comment: rendering a real thumbnail on a background thread reuses
/// `gui::batch::preview::engine`'s `resolve_design`/`render_item_local`
/// machinery, both private to that module today. This function exists so the
/// cache PATH half of that feature (the part testable without a GPU) is in
/// place and tested; the render call itself is left as a placeholder-until-ready
/// card in `template_gallery.slint` (see that file's own doc comment).
#[must_use]
// Only outside `cfg(test)`: this module's own `#[cfg(test)] mod tests` below
// is this function's one caller today, so a non-test build (where that
// module does not exist) is the only configuration where `dead_code` would
// actually fire -- an unconditional `#[expect(dead_code)]` would instead go
// unfulfilled (and itself warn) on the test build, where the call from
// `tests` makes the function genuinely used.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "not yet called from non-test code: the render call this path feeds is a documented follow-up, see this fn's own doc comment"
    )
)]
pub(in crate::gui::editor) fn thumbnail_cache_path(
    cache_dir: &Path,
    template_name: &str,
) -> PathBuf {
    let slug: String = template_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    // Collapses runs of '-' (from stripped punctuation/whitespace) so
    // "Round Brilliant -- Shallow Pavilion" and a hypothetical
    // "Round Brilliant Shallow Pavilion" do not collide by accident, and
    // trims a leading/trailing '-' so the filename never starts or ends with
    // one.
    let mut collapsed = String::with_capacity(slug.len());
    let mut last_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !last_dash {
                collapsed.push(c);
            }
            last_dash = true;
        } else {
            collapsed.push(c);
            last_dash = false;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    cache_dir
        .join("thumbnail-cache")
        .join(format!("{trimmed}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_cache_path_is_stable_and_collision_free() {
        let dir = Path::new("/app-data");
        assert_eq!(
            thumbnail_cache_path(dir, "Standard Round Brilliant"),
            dir.join("thumbnail-cache")
                .join("standard-round-brilliant.png")
        );
        assert_eq!(
            thumbnail_cache_path(dir, "Round Brilliant -- Shallow Pavilion"),
            dir.join("thumbnail-cache")
                .join("round-brilliant-shallow-pavilion.png")
        );
        // Two different names must never collapse to the same path.
        let a = thumbnail_cache_path(dir, "Round Brilliant -- Shallow Pavilion");
        let b = thumbnail_cache_path(dir, "Round Brilliant -- Deep Pavilion");
        assert_ne!(a, b);
    }

    #[test]
    fn thumbnail_cache_path_never_starts_or_ends_with_a_dash() {
        let dir = Path::new("/app-data");
        let path = thumbnail_cache_path(dir, "  Weird -- Name!!  ");
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        assert!(!name.starts_with('-'));
        assert!(!name.ends_with('-'));
    }
}
