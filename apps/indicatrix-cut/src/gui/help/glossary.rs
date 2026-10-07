//! The glossary: Appendix A of the manual, read as a list of terms.
//!
//! Each entry of the appendix is one paragraph, `**Term** — what it means.` The dialog
//! behind Help > Glossary lists and filters them; [`glossary_lookup`] finds one term for a
//! tooltip that wants to add "More in the glossary".

use super::{
    manual,
    markdown::{BlockKind, Chapter, plain_text},
    topics,
};
use std::sync::OnceLock;

/// One term of the glossary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlossaryEntry {
    /// The term as the appendix prints it, e.g. `Refractive index (RI)`.
    pub term: String,
    /// The explanation, as inline Markdown.
    pub text: String,
    /// The explanation without formatting marks.
    pub plain: String,
    /// Every name the term answers to, lower case: the whole term, each part of
    /// `Detach / Reattach`, and the words inside and outside brackets.
    pub aliases: Vec<String>,
    /// `plain` in lower case, for filtering.
    lower: String,
}

impl GlossaryEntry {
    /// The first sentence of the explanation, shortened to fit a tooltip.
    #[must_use]
    pub fn summary(&self) -> String {
        const LIMIT: usize = 160;
        let sentence_end = self
            .plain
            .match_indices(". ")
            .next()
            .map_or(self.plain.len(), |(at, _)| at + 1);
        let sentence = &self.plain[..sentence_end];
        if sentence.chars().count() <= LIMIT {
            return sentence.to_owned();
        }
        let cut: String = sentence.chars().take(LIMIT).collect();
        let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
        format!("{cut}\u{2026}")
    }
}

/// Reads the entries of the glossary chapter.
///
/// An entry is every top-level paragraph that opens with a bold term followed by a dash.
/// The appendix's introductory notes open with a bold phrase too, but no dash follows it,
/// so they are left out.
#[must_use]
pub fn parse_glossary(chapter: &Chapter) -> Vec<GlossaryEntry> {
    chapter
        .blocks
        .iter()
        .filter(|block| block.kind == BlockKind::Paragraph && block.indent == 0)
        .filter_map(|block| entry_of(&block.text))
        .collect()
}

fn entry_of(paragraph: &str) -> Option<GlossaryEntry> {
    let rest = paragraph.strip_prefix("**")?;
    let (term, after) = rest.split_once("**")?;
    let after = after.trim_start();
    let text = ["\u{2014}", "\u{2013}", "--", "- "]
        .iter()
        .find_map(|dash| after.strip_prefix(dash))?
        .trim();
    let term = term.trim();
    if term.is_empty() || text.is_empty() {
        return None;
    }
    let plain = plain_text(text);
    Some(GlossaryEntry {
        term: term.to_owned(),
        text: text.to_owned(),
        aliases: aliases_of(term),
        lower: plain.to_lowercase(),
        plain,
    })
}

/// Every name a term answers to, lower case.
fn aliases_of(term: &str) -> Vec<String> {
    let lower = term.trim().to_lowercase();
    let mut aliases: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        let name = name.trim();
        if !name.is_empty() && !aliases.iter().any(|alias| alias == name) {
            aliases.push(name.to_owned());
        }
    };
    add(&lower);
    for part in lower.split(" / ") {
        add(part);
        if let Some((outside, inside)) = part.split_once('(') {
            add(outside);
            add(inside.trim_end_matches(')'));
        }
    }
    aliases
}

/// The glossary of the embedded manual, in the appendix's order.
#[must_use]
pub fn entries() -> &'static [GlossaryEntry] {
    static ENTRIES: OnceLock<Vec<GlossaryEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        manual::chapters()
            .iter()
            .find(|chapter| chapter.stem == topics::GLOSSARY)
            .map(|chapter| parse_glossary(&chapter.parsed))
            .unwrap_or_default()
    })
}

/// The entry for `term`: any of its names, in any case, singular or plural
/// (`RI`, `tiers`, `Reattach`). For a tooltip's "More in the glossary".
#[must_use]
pub fn glossary_lookup(term: &str) -> Option<&'static GlossaryEntry> {
    lookup_in(entries(), term)
}

/// [`glossary_lookup`] in any list of entries.
#[must_use]
pub fn lookup_in<'a>(entries: &'a [GlossaryEntry], term: &str) -> Option<&'a GlossaryEntry> {
    let wanted = term.trim().to_lowercase();
    if wanted.is_empty() {
        return None;
    }
    let singular = wanted.strip_suffix('s');
    entries.iter().find(|entry| {
        entry.aliases.iter().any(|alias| {
            alias.as_str() == wanted || singular.is_some_and(|singular| alias.as_str() == singular)
        })
    })
}

/// The entries that match `query`, best first.
///
/// An empty query lists everything in the appendix's order. A term whose name is the query
/// ranks first, then names that start with it, names that contain it, and last
/// explanations that mention it.
#[must_use]
pub fn filter<'a>(entries: &'a [GlossaryEntry], query: &str) -> Vec<&'a GlossaryEntry> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return entries.iter().collect();
    }
    let mut ranked: Vec<(u8, &GlossaryEntry)> = entries
        .iter()
        .filter_map(|entry| rank(entry, &query).map(|rank| (rank, entry)))
        .collect();
    // A stable sort keeps the appendix's order among equal ranks.
    ranked.sort_by_key(|&(rank, _)| rank);
    ranked.into_iter().map(|(_, entry)| entry).collect()
}

fn rank(entry: &GlossaryEntry, query: &str) -> Option<u8> {
    if entry.aliases.iter().any(|alias| alias == query) {
        Some(0)
    } else if entry.aliases.iter().any(|alias| alias.starts_with(query)) {
        Some(1)
    } else if entry.aliases.iter().any(|alias| alias.contains(query)) {
        Some(2)
    } else if entry.lower.contains(query) {
        Some(3)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::help::markdown::parse_chapter;

    const SAMPLE: &str = "# Glossary\n\nIntro text.\n\n**A note on things.** More words follow.\n\n\
        **Anchor** \u{2014} A tier whose scale is stated directly. It needs no neighbours.\n\n\
        **Refractive index (RI)** \u{2014} A measure of how strongly a material bends\nlight.\n\n\
        **Detach / Reattach** - The toggle.\n\n**Tier** \u{2014} One row of the cutting instructions.\n\n\
        **Pinned tier** \u{2014} A tier with a stated scale.\n\n- **Not** \u{2014} a list item, ignored.\n";

    fn sample() -> Vec<GlossaryEntry> {
        parse_glossary(&parse_chapter(SAMPLE))
    }

    #[test]
    fn entries_are_the_bold_term_paragraphs_only() {
        let terms: Vec<String> = sample().into_iter().map(|entry| entry.term).collect();
        assert_eq!(
            terms,
            [
                "Anchor",
                "Refractive index (RI)",
                "Detach / Reattach",
                "Tier",
                "Pinned tier"
            ]
        );
    }

    #[test]
    fn wrapped_text_is_joined_and_marks_are_stripped_in_plain() {
        let entries = sample();
        assert_eq!(
            entries[1].plain,
            "A measure of how strongly a material bends light."
        );
        assert_eq!(
            entries[0].text,
            "A tier whose scale is stated directly. It needs no neighbours."
        );
    }

    #[test]
    fn aliases_cover_parts_and_brackets() {
        let entries = sample();
        assert_eq!(
            entries[1].aliases,
            ["refractive index (ri)", "refractive index", "ri"]
        );
        assert_eq!(
            entries[2].aliases,
            ["detach / reattach", "detach", "reattach"]
        );
    }

    #[test]
    fn lookup_finds_any_name_in_any_case_and_a_plural() {
        let entries = sample();
        let term = |wanted: &str| lookup_in(&entries, wanted).map(|e| e.term.as_str());
        assert_eq!(term("RI"), Some("Refractive index (RI)"));
        assert_eq!(term("  refractive INDEX "), Some("Refractive index (RI)"));
        assert_eq!(term("Reattach"), Some("Detach / Reattach"));
        assert_eq!(term("tiers"), Some("Tier"));
        assert_eq!(term("pinned tiers"), Some("Pinned tier"));
        assert_eq!(term("no such term"), None);
        assert_eq!(term(""), None);
    }

    #[test]
    fn filtering_ranks_names_before_explanations() {
        let entries = sample();
        let terms = |query: &str| -> Vec<&str> {
            filter(&entries, query)
                .into_iter()
                .map(|e| e.term.as_str())
                .collect()
        };
        // The exact name first, then a name that contains the word, then the explanation
        // that merely mentions it.
        assert_eq!(terms("tier"), ["Tier", "Pinned tier", "Anchor"]);
        assert_eq!(terms("bends"), ["Refractive index (RI)"]);
        assert_eq!(terms("").len(), 5);
        assert_eq!(terms("zzzz").len(), 0);
    }

    #[test]
    fn a_summary_is_the_first_sentence_and_fits_a_tooltip() {
        let entries = sample();
        assert_eq!(
            entries[0].summary(),
            "A tier whose scale is stated directly."
        );
        let long = parse_glossary(&parse_chapter(&format!(
            "**Long** \u{2014} {}\n",
            "word ".repeat(80)
        )));
        let summary = long[0].summary();
        assert!(summary.ends_with('\u{2026}'));
        assert!(summary.chars().count() <= 161);
    }

    #[test]
    fn the_real_glossary_has_the_known_terms() {
        let entries = entries();
        assert!(entries.len() > 20, "{} entries", entries.len());
        for term in [
            "Anchor",
            "Tier",
            "Mast",
            "Yield",
            "Culet",
            "Refractive index (RI)",
        ] {
            assert!(
                entries.iter().any(|entry| entry.term == term),
                "{term} is missing from the glossary"
            );
        }
        assert!(glossary_lookup("RI").is_some());
        assert!(glossary_lookup("masts").is_some());
        // The two introductory notes are not terms.
        assert!(!entries.iter().any(|entry| entry.term.starts_with("A note")));
    }
}
