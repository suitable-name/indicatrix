//! Topic ids: how the program points at a place in the manual.
//!
//! A topic id is `"<chapter file name without .md>#<section anchor>"` (the anchor is
//! optional: `"15-planning-a-rough"` is the chapter's first page). Anchors follow
//! [`slug`](super::markdown::slug), so `## The tier form` is `the-tier-form`.
//!
//! The named topics below are the places a panel's "?" button opens. The same ids are
//! repeated for the Slint side in `HelpTopics` (`ui/models/help.slint`) as
//! `HelpButton { topic: HelpTopics.tier-form }`; a test keeps the two lists equal, and
//! another resolves every id against the embedded manual, so renaming a manual heading
//! fails the tests instead of leaving a dead button.

use super::manual::{self, ManualChapter};

/// The manual's contents page.
pub const CONTENTS: &str = manual::CONTENTS_STEM;
/// The Tier tab's form: angle, Meets, name, indices.
pub const TIER_FORM: &str = "04-editing-tiers#the-tier-form";
/// The Tier tab while it shows the concave tier form.
pub const CONCAVE_TIER: &str = "16-concave-tiers#adding-a-concave-tier";
/// The Preform tab: proportions and yield.
pub const PREFORM_TAB: &str = "04-editing-tiers#preform-proportions-and-yield";
/// The Optimize tab.
pub const OPTIMIZE_TAB: &str = "08-deep-solve-optimize-adopt#optimize";
/// The Schedule tab.
pub const SCHEDULE_TAB: &str = "03-loading-and-tier-list#the-inspectors-schedule-tab";
/// The tier table and its columns.
pub const TIER_TABLE: &str = "03-loading-and-tier-list#reading-the-tier-list";
/// The Design Settings panel above the tier table.
pub const DESIGN_SETTINGS: &str = "06-materials-and-refractive-index#ri-override-vs-material";
/// The Solid viewport.
pub const SOLID_VIEW: &str = "13-solid-inspection-view#the-four-view-modes";
/// The Diagram view.
pub const DIAGRAM_VIEW: &str = "13-solid-inspection-view#diagram-view";
/// The Live Render viewport.
pub const LIVE_RENDER: &str = "02-catalogue-and-viewing#the-3d-viewport";
/// The Live Render settings dialog: quality, resolution, compute, lighting.
pub const RENDER_SETTINGS: &str = "02-catalogue-and-viewing#quality-resolution-and-gpu-options";
/// The Retarget dialog.
pub const RETARGET: &str = "14-retarget-snapshot-compare-tilt-curves#retarget";
/// The before/after Compare window.
pub const COMPARE: &str = "14-retarget-snapshot-compare-tilt-curves#visual-beforeafter-comparison";
/// The material editor.
pub const MATERIAL_EDITOR: &str = "06-materials-and-refractive-index#custom-materials";
/// The export dialog. (Named `EXPORT_DIALOG`: `export` is a Slint keyword and cannot be a
/// property name on the Slint side.)
pub const EXPORT_DIALOG: &str = "09-rendering-and-export#opening-the-export-dialog";
/// The Rough Planner window.
pub const ROUGH_PLANNER: &str = "15-planning-a-rough";
/// The Preferences dialog.
pub const PREFERENCES: &str = "17-preferences-and-accessibility";
/// Cutting mode.
pub const CUTTING_MODE: &str = "20-cutting-mode";
/// Angle sweeps.
pub const SWEEP: &str = "21-angle-sweeps";
/// Undo, redo and the history.
pub const HISTORY: &str = "18-history-and-variants";
/// The command palette.
pub const COMMAND_PALETTE: &str = "19-command-palette-and-keyboard";
/// Solving a design.
pub const SOLVE: &str = "05-solving#what-solve-does";
/// Deep Solve.
pub const DEEP_SOLVE: &str = "08-deep-solve-optimize-adopt#deep-solve";
/// The tilt-performance graph.
pub const TILT_CURVES: &str = "02-catalogue-and-viewing#reading-the-tilt-performance-graph";
/// Saving and the file formats.
pub const SAVING: &str = "11-saving-and-file-formats";
/// Setting up a remote worker.
pub const REMOTE_WORKER: &str = "10-remote-worker-setup";
/// The New Design dialog.
pub const NEW_DESIGN: &str = "07-new-design-worked-example#step-1-the-new-design-dialog";
/// The Import panel. (Named `IMPORT_DESIGNS`: `import` is a Slint keyword.)
pub const IMPORT_DESIGNS: &str = "01-getting-started#bringing-your-own-designs-in";
/// The Edit as Text window.
pub const EDIT_AS_TEXT: &str = "11-saving-and-file-formats#editing-the-instructions-as-text";
/// The tutorial browser and the guide panel.
pub const TUTORIALS: &str = "22-tutorials";
/// Anchors: the tier that fixes a block's size.
pub const ANCHORS: &str = "03-loading-and-tier-list#anchors-and-blocks";
/// What the library keeps about a design: previews, tilt curves, details.
pub const LIBRARY_DETAILS: &str = "02-catalogue-and-viewing#selecting-and-inspecting-a-design";
/// The overall Good / Check / Problem verdict in the status strip.
pub const VERDICT: &str = "05-solving#the-overall-verdict";
/// The editor's command bar: New, Open, Solve, Export and the other buttons.
pub const COMMAND_BAR: &str = "03-loading-and-tier-list#the-editors-toolbar";
/// The status strip under the editor: solver state, warnings, Details.
pub const STATUS_STRIP: &str = "05-solving#reading-the-status-strip-after-solve";
/// The library list and the search row above it.
pub const LIBRARY: &str = "02-catalogue-and-viewing#searching-and-filtering";
/// The Advanced Filters panel of the library.
pub const FILTERS: &str = "02-catalogue-and-viewing#advanced-filters-panel";
/// The editor's empty state: the four ways to start.
pub const EMPTY_STATE: &str = "01-getting-started#starting-from-nothing-the-empty-state";
/// The library's Cutting Instructions tab.
pub const CUTTING_TABLE: &str = "02-catalogue-and-viewing#the-cutting-instructions-tab";
/// The Simple / Advanced switch.
pub const SIMPLE_ADVANCED: &str = "17-preferences-and-accessibility#simple-and-advanced";
/// The keyboard shortcuts.
pub const SHORTCUTS: &str = "appendix-b-keyboard-shortcuts";
/// The glossary.
pub const GLOSSARY: &str = "appendix-a-glossary";
/// The Render Jobs window.
pub const RENDER_JOBS: &str = "23-render-jobs";

/// Every named topic: its name (the Slint property is the same name in lower case with
/// hyphens, `TIER_FORM` is `HelpTopics.tier-form`) and its id.
pub const TOPICS: &[(&str, &str)] = &[
    ("CONTENTS", CONTENTS),
    ("TIER_FORM", TIER_FORM),
    ("CONCAVE_TIER", CONCAVE_TIER),
    ("PREFORM_TAB", PREFORM_TAB),
    ("OPTIMIZE_TAB", OPTIMIZE_TAB),
    ("SCHEDULE_TAB", SCHEDULE_TAB),
    ("TIER_TABLE", TIER_TABLE),
    ("DESIGN_SETTINGS", DESIGN_SETTINGS),
    ("SOLID_VIEW", SOLID_VIEW),
    ("DIAGRAM_VIEW", DIAGRAM_VIEW),
    ("LIVE_RENDER", LIVE_RENDER),
    ("RENDER_SETTINGS", RENDER_SETTINGS),
    ("RETARGET", RETARGET),
    ("COMPARE", COMPARE),
    ("MATERIAL_EDITOR", MATERIAL_EDITOR),
    ("EXPORT_DIALOG", EXPORT_DIALOG),
    ("ROUGH_PLANNER", ROUGH_PLANNER),
    ("PREFERENCES", PREFERENCES),
    ("CUTTING_MODE", CUTTING_MODE),
    ("SWEEP", SWEEP),
    ("HISTORY", HISTORY),
    ("COMMAND_PALETTE", COMMAND_PALETTE),
    ("SOLVE", SOLVE),
    ("DEEP_SOLVE", DEEP_SOLVE),
    ("TILT_CURVES", TILT_CURVES),
    ("SAVING", SAVING),
    ("REMOTE_WORKER", REMOTE_WORKER),
    ("NEW_DESIGN", NEW_DESIGN),
    ("IMPORT_DESIGNS", IMPORT_DESIGNS),
    ("EDIT_AS_TEXT", EDIT_AS_TEXT),
    ("TUTORIALS", TUTORIALS),
    ("ANCHORS", ANCHORS),
    ("LIBRARY_DETAILS", LIBRARY_DETAILS),
    ("VERDICT", VERDICT),
    ("COMMAND_BAR", COMMAND_BAR),
    ("STATUS_STRIP", STATUS_STRIP),
    ("LIBRARY", LIBRARY),
    ("FILTERS", FILTERS),
    ("EMPTY_STATE", EMPTY_STATE),
    ("CUTTING_TABLE", CUTTING_TABLE),
    ("SIMPLE_ADVANCED", SIMPLE_ADVANCED),
    ("SHORTCUTS", SHORTCUTS),
    ("GLOSSARY", GLOSSARY),
    ("RENDER_JOBS", RENDER_JOBS),
];

/// The id of the named topic, e.g. `topic_id("TIER_FORM")`.
#[must_use]
pub fn topic_id(name: &str) -> Option<&'static str> {
    TOPICS
        .iter()
        .find(|(topic, _)| *topic == name)
        .map(|&(_, id)| id)
}

/// Where a topic id points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// The index of the chapter in [`manual::chapters`].
    pub chapter: usize,
    /// The index of the section in that chapter; `0` is the chapter's own title.
    pub section: usize,
    /// `false` when the id named a section the chapter does not have, so the target is the
    /// top of the chapter instead.
    pub exact: bool,
}

/// Splits `"chapter#anchor"` into its parts. A trailing `.md` on the chapter is dropped,
/// so a link written for the plain files (`04-editing-tiers.md#the-tier-form`) works too.
#[must_use]
pub fn split_topic(id: &str) -> (&str, Option<&str>) {
    let (chapter, fragment) = id
        .split_once('#')
        .map_or((id, None), |(chapter, fragment)| (chapter, Some(fragment)));
    let chapter = chapter.trim();
    (
        chapter.strip_suffix(".md").unwrap_or(chapter),
        fragment
            .map(str::trim)
            .filter(|fragment| !fragment.is_empty()),
    )
}

/// Resolves a topic id against the embedded manual. `None` when the chapter is unknown.
#[must_use]
pub fn resolve(id: &str) -> Option<Target> {
    resolve_in(manual::chapters(), id)
}

/// [`resolve`] against any list of chapters.
#[must_use]
pub fn resolve_in(chapters: &[ManualChapter], id: &str) -> Option<Target> {
    let (stem, fragment) = split_topic(id);
    let (chapter, entry) = chapters
        .iter()
        .enumerate()
        .find(|(_, entry)| entry.stem == stem)?;
    let Some(fragment) = fragment else {
        return Some(Target {
            chapter,
            section: 0,
            exact: true,
        });
    };
    let found = entry.parsed.section_by_slug(fragment);
    Some(Target {
        chapter,
        section: found.unwrap_or(0),
        exact: found.is_some(),
    })
}

/// What a link inside the manual points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// Another place in the manual (a topic id).
    Topic(String),
    /// A web address to hand to the browser.
    External(String),
    /// Anything else, which the viewer does not open.
    Unsupported,
}

/// Classifies a link clicked in the chapter `current_stem`.
#[must_use]
pub fn classify_link(link: &str, current_stem: &str) -> Link {
    let link = link.trim();
    if link.starts_with("http://") || link.starts_with("https://") {
        Link::External(link.to_owned())
    } else if let Some(fragment) = link.strip_prefix('#') {
        Link::Topic(format!("{current_stem}#{fragment}"))
    } else if link.is_empty() || link.contains(':') {
        Link::Unsupported
    } else {
        Link::Topic(link.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Slint side of the registry, which must list the same ids.
    const SLINT: &str = include_str!("../../../ui/models/help.slint");

    #[test]
    fn every_named_topic_resolves_exactly_against_the_embedded_manual() {
        for (name, id) in TOPICS {
            let target = resolve(id).unwrap_or_else(|| panic!("{name}: no chapter for {id:?}"));
            assert!(
                target.exact,
                "{name}: the chapter has no section for {id:?}; was a heading renamed?"
            );
        }
    }

    #[test]
    fn topic_names_and_ids_are_unique() {
        let mut names: Vec<&str> = TOPICS.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TOPICS.len());
        assert_eq!(topic_id("TIER_FORM"), Some(TIER_FORM));
        assert_eq!(topic_id("NO_SUCH_TOPIC"), None);
    }

    #[test]
    fn the_slint_topic_list_matches_the_registry() {
        for (name, id) in TOPICS {
            let property = name.to_lowercase().replace('_', "-");
            let line = format!("out property <string> {property}: \"{id}\";");
            assert!(
                SLINT.contains(&line),
                "ui/models/help.slint is missing or differs: {line}"
            );
        }
        // Count only inside `HelpTopics`: `HelpModel` below it has `in-out property <string>`
        // fields (`search_text`, `glossary_query`) that also contain the substring
        // `out property <string> ` and are not topics.
        let block = SLINT
            .split_once("export global HelpTopics {")
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once("\n}"))
            .map(|(block, _)| block)
            .expect("ui/models/help.slint has an `export global HelpTopics` block");
        let declared = block.matches("out property <string> ").count();
        assert_eq!(
            declared,
            TOPICS.len(),
            "HelpTopics in ui/models/help.slint lists a topic the registry does not"
        );
    }

    #[test]
    fn topic_ids_split_at_the_anchor_and_drop_the_md_suffix() {
        assert_eq!(split_topic("04-editing-tiers"), ("04-editing-tiers", None));
        assert_eq!(
            split_topic("04-editing-tiers.md#the-tier-form"),
            ("04-editing-tiers", Some("the-tier-form"))
        );
        assert_eq!(split_topic(" a# "), ("a", None));
    }

    #[test]
    fn resolving_is_lenient_about_unknown_anchors_but_not_chapters() {
        let top = resolve("04-editing-tiers").expect("the chapter exists");
        assert_eq!((top.section, top.exact), (0, true));
        let missing = resolve("04-editing-tiers#no-such-section").expect("the chapter exists");
        assert_eq!(
            (missing.chapter, missing.section, missing.exact),
            (top.chapter, 0, false)
        );
        let with_suffix = resolve("04-editing-tiers.md#the-tier-form").expect("resolves");
        assert!(with_suffix.exact && with_suffix.section > 0);
        assert_eq!(resolve("99-no-such-chapter"), None);
        assert_eq!(resolve(""), None);
    }

    #[test]
    fn the_contents_page_is_the_first_chapter() {
        let contents = resolve(CONTENTS).expect("the contents page exists");
        assert_eq!(contents.chapter, 0);
    }

    #[test]
    fn links_are_classified_by_where_they_point() {
        assert_eq!(
            classify_link("05-solving.md#what-solve-does", "README"),
            Link::Topic("05-solving.md#what-solve-does".to_owned())
        );
        assert_eq!(
            classify_link("#next-steps", "05-solving"),
            Link::Topic("05-solving#next-steps".to_owned())
        );
        assert_eq!(
            classify_link(" https://example.com/a?b=1 ", "README"),
            Link::External("https://example.com/a?b=1".to_owned())
        );
        for refused in ["", "mailto:a@b.c", "file:///C:/x", "ms-settings:network"] {
            assert_eq!(
                classify_link(refused, "README"),
                Link::Unsupported,
                "{refused}"
            );
        }
    }
}
