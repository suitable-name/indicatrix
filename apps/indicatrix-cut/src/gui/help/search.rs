//! Searching the whole manual.
//!
//! A search is case-insensitive and every word of the query has to appear in a section,
//! in its heading or in its text. Sections whose heading holds the words rank first, then
//! sections that use them most; the rest keeps the manual's reading order.

use super::manual::{self, ManualChapter};

/// The fewest characters a search needs before it looks at the manual.
pub const MIN_QUERY_CHARS: usize = 2;

/// How many characters of context a snippet shows around the first match.
const SNIPPET_CHARS: usize = 150;

/// One matching section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The index of the chapter in [`manual::chapters`].
    pub chapter: usize,
    /// The index of the section in that chapter.
    pub section: usize,
    /// How well the section matches; higher ranks first.
    pub score: u32,
    /// A line of the section's text around the first match.
    pub snippet: String,
}

/// Searches the embedded manual; returns at most `limit` hits, best first.
#[must_use]
pub fn search(query: &str, limit: usize) -> Vec<Hit> {
    search_in(manual::chapters(), query, limit)
}

/// [`search`] over any list of chapters.
#[must_use]
pub fn search_in(chapters: &[ManualChapter], query: &str, limit: usize) -> Vec<Hit> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let phrase = words.join(" ");
    if phrase.chars().count() < MIN_QUERY_CHARS {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (chapter_index, chapter) in chapters.iter().enumerate() {
        for (section_index, (section, text)) in chapter
            .parsed
            .sections
            .iter()
            .zip(&chapter.texts)
            .enumerate()
        {
            let title = section.title.to_lowercase();
            if !words
                .iter()
                .all(|word| title.contains(word.as_str()) || text.lower.contains(word.as_str()))
            {
                continue;
            }
            hits.push(Hit {
                chapter: chapter_index,
                section: section_index,
                score: score(&words, &phrase, &title, &text.lower),
                snippet: snippet(&text.plain, &text.lower, &words),
            });
        }
    }
    // A stable sort keeps the manual's order among equal scores.
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
    hits.truncate(limit);
    hits
}

/// The score of a section that holds every word.
fn score(words: &[String], phrase: &str, title: &str, text: &str) -> u32 {
    let mut score = 0;
    for word in words {
        if title.contains(word.as_str()) {
            score += 30;
        }
        let uses = text.matches(word.as_str()).count().min(5);
        score += u32::try_from(uses).unwrap_or(5);
    }
    if words.len() > 1 {
        if title.contains(phrase) {
            score += 50;
        }
        if text.contains(phrase) {
            score += 10;
        }
    }
    score
}

/// About [`SNIPPET_CHARS`] characters of `plain` around the first word that occurs in it,
/// cut at word boundaries and marked with "…" where text was left out. The start of the
/// text when no word occurs in it (the words are in the heading).
fn snippet(plain: &str, lower: &str, words: &[String]) -> String {
    let first = words
        .iter()
        .filter_map(|word| lower.find(word.as_str()))
        .min();
    // Lower-casing can change a character's length; then the offsets do not line up and
    // the snippet starts at the top instead.
    let aligned = plain.chars().count() == lower.chars().count();
    let at = match first {
        Some(byte) if aligned => lower[..byte].chars().count(),
        _ => 0,
    };
    let chars: Vec<char> = plain.chars().collect();
    let start = at.saturating_sub(SNIPPET_CHARS / 3);
    let end = (start + SNIPPET_CHARS).min(chars.len());
    let start = start.min(end);
    let mut text: String = chars[start..end].iter().collect();
    if start > 0 {
        // Begin at a word boundary.
        if let Some((_, rest)) = text.split_once(' ') {
            text = rest.to_owned();
        }
        text.insert(0, '\u{2026}');
    }
    if end < chars.len() {
        if let Some((head, _)) = text.rsplit_once(' ') {
            text = head.to_owned();
        }
        text.push('\u{2026}');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manual() -> Vec<ManualChapter> {
        vec![
            ManualChapter::from_source(
                "one",
                "# Solving\n\nSolve computes the masts of every tier.\n\n## Why the tier form matters\n\nThe tier form holds the angle.\n",
            ),
            ManualChapter::from_source(
                "two",
                "# Rendering\n\nA tier form is not needed to render. Rendering uses the masts.\n\n## Tier form\n\nUse the tier form to edit the angle of a tier form.\n",
            ),
        ]
    }

    #[test]
    fn a_short_or_empty_query_finds_nothing() {
        let manual = manual();
        assert_eq!(search_in(&manual, "", 10).len(), 0);
        assert_eq!(search_in(&manual, "   ", 10).len(), 0);
        assert_eq!(search_in(&manual, "t", 10).len(), 0);
    }

    #[test]
    fn matching_ignores_case_and_needs_every_word() {
        let manual = manual();
        let hits = search_in(&manual, "MASTS tier", 10);
        let places: Vec<(usize, usize)> = hits.iter().map(|h| (h.chapter, h.section)).collect();
        // Only the two sections that hold both words.
        assert_eq!(places.len(), 2);
        assert!(places.contains(&(0, 0)) && places.contains(&(1, 0)));
        assert_eq!(search_in(&manual, "masts nonsense", 10).len(), 0);
    }

    #[test]
    fn a_heading_match_ranks_above_a_text_match() {
        let manual = manual();
        let hits = search_in(&manual, "tier form", 10);
        // "Tier form" (chapter two) has the phrase in its heading and the most uses.
        assert_eq!((hits[0].chapter, hits[0].section), (1, 1));
        assert!(hits.windows(2).all(|pair| pair[0].score >= pair[1].score));
    }

    #[test]
    fn the_hit_limit_is_respected() {
        let manual = manual();
        assert_eq!(search_in(&manual, "tier", 2).len(), 2);
        assert_eq!(search_in(&manual, "tier", 0).len(), 0);
    }

    #[test]
    fn a_snippet_shows_the_text_around_the_match() {
        let long = format!("{} needle {}", "word ".repeat(60), "tail ".repeat(60));
        let manual = [ManualChapter::from_source("x", &format!("# T\n\n{long}\n"))];
        let hits = search_in(&manual, "needle", 5);
        assert_eq!(hits.len(), 1);
        let snippet = &hits[0].snippet;
        assert!(snippet.contains("needle"), "{snippet}");
        assert!(snippet.starts_with('\u{2026}') && snippet.ends_with('\u{2026}'));
        assert!(snippet.chars().count() <= SNIPPET_CHARS + 2);
    }

    #[test]
    fn a_title_only_match_gets_the_start_of_the_text() {
        let manual = [ManualChapter::from_source(
            "x",
            "# T\n\n## Glossary\n\nTerms used throughout.\n",
        )];
        let hits = search_in(&manual, "glossary", 5);
        assert_eq!(hits[0].snippet, "Terms used throughout.");
    }

    #[test]
    fn the_real_manual_answers_a_real_question() {
        let hits = search("tier form", 50);
        assert_ne!(hits.len(), 0);
        let chapters = manual::chapters();
        let top = &hits[0];
        let title = &chapters[top.chapter].parsed.sections[top.section].title;
        assert!(title.to_lowercase().contains("tier form"), "{title}");
        assert_eq!(search("zzzzqqqq-not-in-the-manual", 5).len(), 0);
    }
}
