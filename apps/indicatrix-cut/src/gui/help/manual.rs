//! The user manual, built into the program.
//!
//! Every chapter file of `docs/manual/` is compiled in with `include_str!`, so Help works
//! in an installed build whether or not the plain files were copied next to the program.
//!
//! To add a chapter, put one `chapter!("<file name without .md>")` line in [`CHAPTERS`], in
//! reading order. A test fails while a `.md` file of the manual folder is missing from the
//! list.

use super::markdown::{Chapter, parse_chapter};
use std::sync::OnceLock;

/// One chapter file, as compiled in.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedChapter {
    /// The file name without `.md`. It names the chapter in topic ids.
    pub stem: &'static str,
    /// The file's text.
    pub source: &'static str,
}

/// Embeds `docs/manual/<stem>.md`.
macro_rules! chapter {
    ($stem:literal) => {
        EmbeddedChapter {
            stem: $stem,
            source: include_str!(concat!("../../../docs/manual/", $stem, ".md")),
        }
    };
}

/// The file name (without `.md`) of the manual's contents page.
pub const CONTENTS_STEM: &str = "README";

/// How the contents page is named in the chapter list.
const CONTENTS_LABEL: &str = "Contents";

/// Every chapter of the manual in reading order: the contents page, the numbered chapters,
/// then the appendices.
pub const CHAPTERS: &[EmbeddedChapter] = &[
    chapter!("README"),
    chapter!("01-getting-started"),
    chapter!("02-catalogue-and-viewing"),
    chapter!("03-loading-and-tier-list"),
    chapter!("04-editing-tiers"),
    chapter!("05-solving"),
    chapter!("06-materials-and-refractive-index"),
    chapter!("07-new-design-worked-example"),
    chapter!("08-deep-solve-optimize-adopt"),
    chapter!("09-rendering-and-export"),
    chapter!("10-remote-worker-setup"),
    chapter!("11-saving-and-file-formats"),
    chapter!("12-troubleshooting-and-limitations"),
    chapter!("13-solid-inspection-view"),
    chapter!("14-retarget-snapshot-compare-tilt-curves"),
    chapter!("15-planning-a-rough"),
    chapter!("16-concave-tiers"),
    chapter!("17-preferences-and-accessibility"),
    chapter!("18-history-and-variants"),
    chapter!("19-command-palette-and-keyboard"),
    chapter!("20-cutting-mode"),
    chapter!("21-angle-sweeps"),
    chapter!("22-tutorials"),
    chapter!("23-render-jobs"),
    chapter!("appendix-a-glossary"),
    chapter!("appendix-b-keyboard-shortcuts"),
    chapter!("appendix-c-render-materials"),
];

/// The words of one section in two spellings, made once for the search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionText {
    /// The section's own text without formatting marks.
    pub plain: String,
    /// `plain` in lower case.
    pub lower: String,
}

/// A chapter, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct ManualChapter {
    /// The file name without `.md`.
    pub stem: &'static str,
    /// The name shown in the chapter list: the chapter's title, or "Contents" for the
    /// contents page.
    pub label: String,
    /// The parsed chapter.
    pub parsed: Chapter,
    /// The text of each section of `parsed`, by section index.
    pub texts: Vec<SectionText>,
}

impl ManualChapter {
    /// Parses a chapter file's text.
    #[must_use]
    pub fn from_source(stem: &'static str, source: &str) -> Self {
        let parsed = parse_chapter(source);
        let label = if stem == CONTENTS_STEM {
            CONTENTS_LABEL.to_owned()
        } else if parsed.title.is_empty() {
            stem.to_owned()
        } else {
            parsed.title.clone()
        };
        let texts = (0..parsed.sections.len())
            .map(|index| {
                let plain = parsed.section_plain_text(index);
                SectionText {
                    lower: plain.to_lowercase(),
                    plain,
                }
            })
            .collect();
        Self {
            stem,
            label,
            parsed,
            texts,
        }
    }
}

/// The parsed manual, in reading order. Parsed on first use and kept.
#[must_use]
pub fn chapters() -> &'static [ManualChapter] {
    static MANUAL: OnceLock<Vec<ManualChapter>> = OnceLock::new();
    MANUAL.get_or_init(|| {
        CHAPTERS
            .iter()
            .map(|chapter| ManualChapter::from_source(chapter.stem, chapter.source))
            .collect()
    })
}

/// One row of the viewer's chapter list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavRow {
    /// The text of the row.
    pub label: String,
    /// The topic id the row opens: `<chapter>` or `<chapter>#<section>`.
    pub topic: String,
    /// 0 for a chapter, 1 and 2 for the sections of the open chapter.
    pub level: u8,
    /// Whether the row's chapter is the open one.
    pub open: bool,
    /// Whether the row is the part of the manual on screen.
    pub current: bool,
}

/// The chapter list for the viewer: every chapter, and under the open one its `##` and
/// `###` sections.
#[must_use]
pub fn nav_rows(chapters: &[ManualChapter], chapter: usize, section: usize) -> Vec<NavRow> {
    let mut rows = Vec::new();
    for (index, entry) in chapters.iter().enumerate() {
        let open = index == chapter;
        rows.push(NavRow {
            label: entry.label.clone(),
            topic: entry.stem.to_owned(),
            level: 0,
            open,
            current: open && section == 0,
        });
        if !open {
            continue;
        }
        for (at, part) in entry.parsed.sections.iter().enumerate() {
            if !(2..=3).contains(&part.level) || part.title.is_empty() {
                continue;
            }
            rows.push(NavRow {
                label: part.title.clone(),
                topic: format!("{}#{}", entry.stem, part.slug),
                level: part.level - 1,
                open: false,
                current: at == section,
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::help::markdown::BlockKind;

    #[test]
    fn every_chapter_file_of_the_manual_folder_is_embedded() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/manual");
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(&folder).expect("the manual folder can be read") {
            let path = entry.expect("a directory entry can be read").path();
            if path.extension().is_some_and(|extension| extension == "md") {
                let stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_owned();
                if !CHAPTERS.iter().any(|chapter| chapter.stem == stem) {
                    missing.push(stem);
                }
            }
        }
        assert!(
            missing.is_empty(),
            "add a chapter!(\"...\") line to CHAPTERS in src/gui/help/manual.rs for: {missing:?}"
        );
    }

    #[test]
    fn the_contents_page_comes_first_and_stems_are_unique() {
        assert_eq!(CHAPTERS[0].stem, CONTENTS_STEM);
        let mut stems: Vec<&str> = CHAPTERS.iter().map(|chapter| chapter.stem).collect();
        stems.sort_unstable();
        stems.dedup();
        assert_eq!(stems.len(), CHAPTERS.len());
    }

    #[test]
    fn every_chapter_starts_with_a_title_and_has_sections() {
        for chapter in chapters() {
            assert!(
                !chapter.parsed.title.is_empty(),
                "{} has no heading",
                chapter.stem
            );
            let first = &chapter.parsed.blocks[0];
            assert_eq!(first.kind, BlockKind::Heading, "{}", chapter.stem);
            assert_eq!(first.level, 1, "{}", chapter.stem);
            assert_eq!(chapter.texts.len(), chapter.parsed.sections.len());
        }
        assert_eq!(chapters()[0].label, "Contents");
    }

    #[test]
    fn no_embedded_chapter_leaves_raw_markdown_headings_in_its_text() {
        for chapter in chapters() {
            for block in &chapter.parsed.blocks {
                if matches!(block.kind, BlockKind::Paragraph | BlockKind::ListItem) {
                    assert!(
                        !block.text.starts_with("# ") && !block.text.starts_with("## "),
                        "{}: {:?}",
                        chapter.stem,
                        block.text
                    );
                }
            }
        }
    }

    #[test]
    fn the_chapter_list_expands_the_open_chapter_only() {
        let manual = [
            ManualChapter::from_source("a", "# One\n\n## A1\n\n### A1x\n\n#### Deep\n"),
            ManualChapter::from_source("b", "# Two\n\n## B1\n"),
        ];
        let rows = nav_rows(&manual, 0, 2);
        let shown: Vec<(&str, &str, u8, bool, bool)> = rows
            .iter()
            .map(|r| {
                (
                    r.label.as_str(),
                    r.topic.as_str(),
                    r.level,
                    r.open,
                    r.current,
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("One", "a", 0, true, false),
                ("A1", "a#a1", 1, false, false),
                ("A1x", "a#a1x", 2, false, true),
                ("Two", "b", 0, false, false),
            ]
        );
        // On the chapter's own title the chapter row is the current one.
        let second = nav_rows(&manual, 1, 0);
        assert_eq!(second.len(), 3);
        assert!(second[1].current && second[1].open);
    }
}
