//! Fuzzy search over the command table.
//!
//! A query is split into words. Every word must match somewhere in a command's title,
//! keywords or category, as a case-insensitive subsequence: `nd` finds "New Design...",
//! `png` finds "Export Diagram (PNG)...". Matches score higher when the letters sit in one
//! run, when they start words, and when they are found in the title rather than in a
//! keyword. The listing is stable: equal scores keep the table's order.
//!
//! An empty query lists the recently used commands first (most recent first), then every
//! other command by category and title.

use std::cmp::Reverse;

/// What the search looks at for one command.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    /// The command's stable id, matched against the recent list.
    pub id: &'a str,
    /// The title shown in the palette.
    pub title: &'a str,
    /// Extra words that also match.
    pub keywords: &'a str,
    /// The category label, which also matches.
    pub category: &'a str,
}

/// Points for every matched letter.
const BASE: i32 = 16;
/// Extra points when a matched letter starts a word.
const WORD_START: i32 = 20;
/// Extra points when a matched letter directly follows the previous matched letter.
const CONSECUTIVE: i32 = 18;
/// Points lost for each letter skipped between two matched letters, up to [`GAP_CAP`].
const GAP: i32 = 2;
/// The most letters a skipped stretch can cost.
const GAP_CAP: usize = 6;
/// Points lost for each letter before the first match, up to [`LEADING_CAP`].
const LEADING: i32 = 1;
/// The most letters the leading stretch can cost.
const LEADING_CAP: usize = 8;
/// Extra points when the whole word appears as one unbroken run.
const RUN_BONUS: i32 = 30;
/// Extra points when that run starts a word.
const RUN_AT_WORD_START: i32 = 40;
/// Extra points when that run is at the very start of the field.
const RUN_AT_START: i32 = 20;

/// How much a match in each field counts. The title outweighs a keyword, which outweighs
/// the category.
const TITLE_WEIGHT: i32 = 10;
const KEYWORD_WEIGHT: i32 = 6;
const CATEGORY_WEIGHT: i32 = 5;

/// The most recently used commands the palette remembers.
pub const RECENT_LIMIT: usize = 8;

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The best score for matching `word` as a subsequence of `field`, or `None` when it does
/// not match. `word` is already folded to lower case.
fn score_word_in(word: &[char], field: &str) -> Option<i32> {
    let hay: Vec<char> = field.chars().map(fold).collect();
    let (m, n) = (word.len(), hay.len());
    if m == 0 || m > n {
        return None;
    }
    let starts_word =
        |j: usize| -> bool { j == 0 || hay.get(j - 1).is_none_or(|prev| !prev.is_alphanumeric()) };

    // best[j]: the best score of matching the word so far with its latest letter at hay[j].
    let mut best: Vec<Option<i32>> = vec![None; n];
    for (i, &letter) in word.iter().enumerate() {
        let mut next: Vec<Option<i32>> = vec![None; n];
        for j in i..n {
            if hay[j] != letter {
                continue;
            }
            let mut here = BASE + if starts_word(j) { WORD_START } else { 0 };
            let before = if i == 0 {
                let skipped = i32::try_from(j.min(LEADING_CAP)).unwrap_or(0);
                Some(-LEADING * skipped)
            } else {
                (i - 1..j)
                    .filter_map(|k| {
                        let prior = best[k]?;
                        let step = if k + 1 == j {
                            CONSECUTIVE
                        } else {
                            let skipped = i32::try_from((j - k - 1).min(GAP_CAP)).unwrap_or(0);
                            -GAP * skipped
                        };
                        Some(prior + step)
                    })
                    .max()
            };
            if let Some(before) = before {
                here += before;
                next[j] = Some(here);
            }
        }
        best = next;
    }
    let mut score = best.into_iter().flatten().max()?;

    // An unbroken run beats the same letters scattered across the field.
    if let Some(at) = hay.windows(m).position(|window| window == word) {
        score += RUN_BONUS;
        if starts_word(at) {
            score += RUN_AT_WORD_START;
        }
        if at == 0 {
            score += RUN_AT_START;
        }
    }
    Some(score)
}

/// The score of one query word against an entry, or `None` when it matches nowhere.
fn score_word(word: &[char], entry: &Entry<'_>) -> Option<i32> {
    // A short title beats a long one with the same match, so "Save" ranks above "Save As...".
    let title = score_word_in(word, entry.title).map(|score| {
        let length = i32::try_from(entry.title.chars().count()).unwrap_or(0);
        (score - length / 2) * TITLE_WEIGHT
    });
    let keywords = score_word_in(word, entry.keywords).map(|score| score * KEYWORD_WEIGHT);
    let category = score_word_in(word, entry.category).map(|score| score * CATEGORY_WEIGHT);
    [title, keywords, category].into_iter().flatten().max()
}

/// The score of `query` against `entry`, or `None` when some word of the query matches
/// nothing. An empty query has no score.
#[must_use]
pub fn score(query: &str, entry: &Entry<'_>) -> Option<i32> {
    let words: Vec<Vec<char>> = query
        .split_whitespace()
        .map(|word| word.chars().map(fold).collect())
        .collect();
    if words.is_empty() {
        return None;
    }
    words
        .iter()
        .map(|word| score_word(word, entry))
        .sum::<Option<i32>>()
}

/// The order to list `entries` in for `query`, as indices into `entries`.
///
/// A non-empty query keeps only the entries it matches, best first, with ties in table
/// order. An empty query lists `recent` ids first (the slice is most recent first; ids
/// that are not in `entries` are skipped), then every other entry by category and title.
#[must_use]
pub fn rank(query: &str, entries: &[Entry<'_>], recent: &[&str]) -> Vec<usize> {
    if query.split_whitespace().next().is_none() {
        let mut order: Vec<usize> = recent
            .iter()
            .filter_map(|id| entries.iter().position(|entry| entry.id == *id))
            .collect();
        let mut rest: Vec<usize> = (0..entries.len())
            .filter(|index| !order.contains(index))
            .collect();
        rest.sort_by(|&a, &b| {
            (entries[a].category, entries[a].title).cmp(&(entries[b].category, entries[b].title))
        });
        order.extend(rest);
        return order;
    }
    let mut scored: Vec<(usize, i32)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| Some((index, score(query, entry)?)))
        .collect();
    // `sort_by_key` is stable, so equal scores keep the table's order.
    scored.sort_by_key(|&(_, points)| Reverse(points));
    scored.into_iter().map(|(index, _)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn entry<'a>(
        id: &'a str,
        title: &'a str,
        category: &'a str,
        keywords: &'a str,
    ) -> Entry<'a> {
        Entry {
            id,
            title,
            keywords,
            category,
        }
    }

    fn sample() -> Vec<Entry<'static>> {
        vec![
            entry("file.save", "Save", "File", "write store"),
            entry("file.save_as", "Save As...", "File", "write copy"),
            entry("file.new", "New Design...", "File", "create blank"),
            entry(
                "file.export_png",
                "Export Diagram (PNG)...",
                "File",
                "picture",
            ),
            entry("file.export_asc", "Export Edited .asc", "File", "text"),
            entry("edit.undo", "Undo", "Edit", "history revert"),
            entry("edit.redo", "Redo", "Edit", "undo again"),
            entry("tiers.delete", "Delete Selected Tier", "Tiers", "remove"),
            entry("tiers.add_concave", "Add Concave Tier", "Tiers", "tool cut"),
            entry("solve.run", "Solve", "Solve", "calculate depths"),
        ]
    }

    fn titles(entries: &[Entry<'_>], order: &[usize]) -> Vec<String> {
        order
            .iter()
            .map(|&i| entries[i].title.to_string())
            .collect()
    }

    #[test]
    fn a_word_must_match_as_a_subsequence() {
        let entries = sample();
        assert!(score("save", &entries[0]).is_some());
        assert!(score("sve", &entries[0]).is_some());
        assert!(score("saved", &entries[0]).is_none());
        assert!(score("zzz", &entries[0]).is_none());
    }

    #[test]
    fn matching_ignores_case() {
        let entries = sample();
        assert_eq!(score("SAVE", &entries[0]), score("save", &entries[0]));
        assert_eq!(score("sAvE", &entries[0]), score("save", &entries[0]));
    }

    #[test]
    fn an_exact_short_title_ranks_above_a_longer_one() {
        let entries = sample();
        let order = rank("save", &entries, &[]);
        let names = titles(&entries, &order);
        assert_eq!(names[0], "Save");
        assert_eq!(names[1], "Save As...");
    }

    #[test]
    fn a_title_match_outranks_a_keyword_match() {
        let entries = sample();
        let order = rank("undo", &entries, &[]);
        let names = titles(&entries, &order);
        // "Redo" only has "undo" in its keywords.
        assert_eq!(names, vec!["Undo", "Redo"]);
    }

    #[test]
    fn letters_in_one_run_beat_letters_scattered_over_the_title() {
        let entries = [
            entry("a", "Add Concave Tier", "Tiers", ""),
            entry("b", "Delete Selected Tier", "Tiers", ""),
        ];
        // "de" is an unbroken run at the start of "Delete", but scattered across
        // "Add Concave Tier".
        let order = rank("de", &entries, &[]);
        let names = titles(&entries, &order);
        assert_eq!(names[0], "Delete Selected Tier");
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn word_starts_beat_the_middle_of_a_word() {
        let entries = [
            entry("a", "Border", "Edit", ""),
            entry("b", "Open Document", "File", ""),
        ];
        // "od": the letters start two words in "Open Document", but sit inside "Border".
        let order = rank("od", &entries, &[]);
        assert_eq!(titles(&entries, &order), vec!["Open Document", "Border"]);
    }

    #[test]
    fn every_word_of_a_query_has_to_match() {
        let entries = sample();
        let order = rank("export png", &entries, &[]);
        assert_eq!(titles(&entries, &order), vec!["Export Diagram (PNG)..."]);
        assert_eq!(rank("export zzz", &entries, &[]), Vec::<usize>::new());
    }

    #[test]
    fn the_category_and_the_keywords_are_searched_too() {
        let entries = sample();
        let by_category = rank("tiers", &entries, &[]);
        assert_eq!(
            titles(&entries, &by_category),
            vec!["Delete Selected Tier", "Add Concave Tier"]
        );
        let by_keyword = rank("revert", &entries, &[]);
        assert_eq!(titles(&entries, &by_keyword), vec!["Undo"]);
    }

    #[test]
    fn equal_scores_keep_the_table_order() {
        let entries = [
            entry("a", "Alpha", "X", ""),
            entry("b", "Alpha", "X", ""),
            entry("c", "Alpha", "X", ""),
        ];
        assert_eq!(rank("alpha", &entries, &[]), vec![0, 1, 2]);
    }

    #[test]
    fn an_empty_query_lists_recent_commands_then_categories_alphabetically() {
        let entries = sample();
        let order = rank("", &entries, &["solve.run", "file.new"]);
        let names = titles(&entries, &order);
        assert_eq!(&names[..2], ["Solve", "New Design..."]);
        // The rest: Edit, File, Solve, Tiers; titles alphabetical within a category.
        assert_eq!(
            &names[2..],
            [
                "Redo",
                "Undo",
                "Export Diagram (PNG)...",
                "Export Edited .asc",
                "Save",
                "Save As...",
                "Add Concave Tier",
                "Delete Selected Tier",
            ]
        );
        assert_eq!(order.len(), entries.len());
    }

    #[test]
    fn a_recent_id_that_no_longer_exists_is_skipped() {
        let entries = sample();
        let order = rank("  ", &entries, &["gone.away", "edit.undo"]);
        assert_eq!(entries[order[0]].id, "edit.undo");
        assert_eq!(order.len(), entries.len());
    }

    #[test]
    fn a_search_ignores_the_recent_list() {
        let entries = sample();
        let with_recent = rank("save", &entries, &["solve.run"]);
        assert_eq!(with_recent, rank("save", &entries, &[]));
    }

    #[test]
    fn an_empty_word_list_has_no_score() {
        let entries = sample();
        assert_eq!(score("", &entries[0]), None);
        assert_eq!(score("   ", &entries[0]), None);
    }
}
