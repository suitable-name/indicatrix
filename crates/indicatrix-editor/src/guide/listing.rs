//! The tutorial browser's list: which guides show, in which order, how they are marked,
//! and the small bookkeeping behind the "Done" marks.

use super::{
    model::{Guide, GuideCategory},
    start::{StartPlan, plan_start},
};
use std::collections::BTreeSet;

/// One row of the tutorial browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserRow {
    /// The guide's id.
    pub id: String,
    /// The guide's title.
    pub title: String,
    /// The one-line summary.
    pub summary: String,
    /// The section the row sits in.
    pub category: GuideCategory,
    /// Whether the row is the first of its section, which then shows the heading.
    pub first_in_category: bool,
    /// Whether the guide has been finished before.
    pub done: bool,
    /// Why the guide cannot start right now, or `None` when it can.
    pub blocked: Option<&'static str>,
    /// How many steps the guide has.
    pub step_count: usize,
}

/// The lower-case words of a search query.
#[must_use]
pub fn query_terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether every search word appears in the guide's title, summary or section name.
/// No words matches everything.
#[must_use]
pub fn matches_terms(guide: &Guide, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let haystack = format!(
        "{} {} {}",
        guide.title,
        guide.summary,
        guide.category.label()
    )
    .to_lowercase();
    terms.iter().all(|term| haystack.contains(term.as_str()))
}

/// The rows the browser shows for `query`: the guides that match, grouped by section in
/// the order of [`GuideCategory::ALL`] (the catalogue's own order inside a section), each
/// marked done or blocked.
#[must_use]
pub fn browser_rows(
    guides: &[Guide],
    completed: &BTreeSet<String>,
    has_design: bool,
    query: &str,
) -> Vec<BrowserRow> {
    let terms = query_terms(query);
    let mut rows = Vec::new();
    for category in GuideCategory::ALL {
        let mut first = true;
        for guide in guides
            .iter()
            .filter(|guide| guide.category == category && matches_terms(guide, &terms))
        {
            rows.push(BrowserRow {
                id: guide.id.clone(),
                title: guide.title.clone(),
                summary: guide.summary.clone(),
                category,
                first_in_category: first,
                done: completed.contains(&guide.id),
                blocked: match plan_start(&guide.starting_state, has_design) {
                    StartPlan::Blocked(reason) => Some(reason),
                    _ => None,
                },
                step_count: guide.steps.len(),
            });
            first = false;
        }
    }
    rows
}

/// How many of `guides` have been finished.
#[must_use]
pub fn completed_count(guides: &[Guide], completed: &BTreeSet<String>) -> usize {
    guides
        .iter()
        .filter(|guide| completed.contains(&guide.id))
        .count()
}

/// The line under the list: how many tutorials are finished.
#[must_use]
pub fn progress_text(done: usize, total: usize) -> String {
    match (done, total) {
        (_, 0) => String::new(),
        (0, _) => format!("No tutorials finished yet ({total} available)."),
        _ if done >= total => format!("All {total} tutorials finished."),
        _ => format!("{done} of {total} tutorials finished."),
    }
}

/// Notes that guide `id` was finished. Returns whether that changed anything, so a caller
/// saves only when it did.
pub fn record_completion(completed: &mut BTreeSet<String>, id: &str) -> bool {
    completed.insert(id.to_owned())
}

/// The line on a row's Start button: "Restart" for a finished guide.
#[must_use]
pub const fn start_label(done: bool) -> &'static str {
    if done { "Restart" } else { "Start" }
}

/// How long a guide is, for its row: "1 step", "8 steps".
#[must_use]
pub fn steps_text(count: usize) -> String {
    if count == 1 {
        "1 step".to_owned()
    } else {
        format!("{count} steps")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::model::{GuideStep, StartingState},
        *,
    };

    fn guide(id: &str, title: &str, category: GuideCategory, start: StartingState) -> Guide {
        Guide::new(id, title, format!("{title} summary."), category)
            .starting(start)
            .step(GuideStep::new("Only step", "Read it."))
    }

    fn sample() -> Vec<Guide> {
        vec![
            guide(
                "a",
                "Slicing a facet",
                GuideCategory::Tiers,
                StartingState::RequiresOpenDesign,
            ),
            guide(
                "b",
                "First look",
                GuideCategory::GettingStarted,
                StartingState::CurrentDesign,
            ),
            guide(
                "c",
                "Solving",
                GuideCategory::Solving,
                StartingState::RequiresOpenDesign,
            ),
            guide(
                "d",
                "More tiers",
                GuideCategory::Tiers,
                StartingState::NewEmpty,
            ),
        ]
    }

    #[test]
    fn rows_follow_the_section_order_then_the_catalogue_order() {
        let rows = browser_rows(&sample(), &BTreeSet::new(), true, "");
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "d", "c"]);
    }

    #[test]
    fn only_the_first_row_of_a_section_carries_the_heading() {
        let rows = browser_rows(&sample(), &BTreeSet::new(), true, "");
        let firsts: Vec<bool> = rows.iter().map(|row| row.first_in_category).collect();
        assert_eq!(firsts, [true, true, false, true]);
    }

    #[test]
    fn a_search_needs_every_word_and_looks_at_title_summary_and_section() {
        let all = sample();
        let titles = |query: &str| -> Vec<String> {
            browser_rows(&all, &BTreeSet::new(), true, query)
                .into_iter()
                .map(|row| row.id)
                .collect()
        };
        assert_eq!(titles("slic"), ["a"]);
        assert_eq!(
            titles("TIERS more"),
            ["d"],
            "section name and title, any case"
        );
        assert_eq!(
            titles("summary"),
            ["b", "a", "d", "c"],
            "summary text counts"
        );
        assert!(
            titles("slicing solving").is_empty(),
            "every word must match"
        );
        assert_eq!(
            titles("   "),
            ["b", "a", "d", "c"],
            "blank search shows everything"
        );
        assert!(titles("zzz").is_empty(), "no guide has these letters");
    }

    #[test]
    fn a_guide_that_needs_a_design_is_blocked_only_while_none_is_open() {
        let all = sample();
        let blocked_ids = |has_design: bool| -> Vec<String> {
            browser_rows(&all, &BTreeSet::new(), has_design, "")
                .into_iter()
                .filter(|row| row.blocked.is_some())
                .map(|row| row.id)
                .collect()
        };
        assert_eq!(blocked_ids(false), ["a", "c"]);
        assert!(
            blocked_ids(true).is_empty(),
            "with a design open nothing is blocked"
        );
    }

    #[test]
    fn finished_guides_are_marked_done() {
        let done: BTreeSet<String> = ["c".to_owned()].into();
        let rows = browser_rows(&sample(), &done, true, "");
        let marks: Vec<(&str, bool)> = rows.iter().map(|r| (r.id.as_str(), r.done)).collect();
        assert_eq!(
            marks,
            [("b", false), ("a", false), ("d", false), ("c", true)]
        );
        assert_eq!(completed_count(&sample(), &done), 1);
    }

    #[test]
    fn a_finished_id_without_a_guide_is_not_counted() {
        let done: BTreeSet<String> = ["gone".to_owned(), "a".to_owned()].into();
        assert_eq!(completed_count(&sample(), &done), 1);
    }

    #[test]
    fn recording_a_completion_reports_whether_it_was_new() {
        let mut done = BTreeSet::new();
        assert!(record_completion(&mut done, "welcome-tour"));
        assert!(
            !record_completion(&mut done, "welcome-tour"),
            "saving twice is a no-op"
        );
        assert!(record_completion(&mut done, "slice"));
        assert_eq!(done.len(), 2);
    }

    #[test]
    fn the_progress_line_reads_plainly() {
        assert_eq!(progress_text(0, 0), "");
        assert_eq!(
            progress_text(0, 5),
            "No tutorials finished yet (5 available)."
        );
        assert_eq!(progress_text(2, 5), "2 of 5 tutorials finished.");
        assert_eq!(progress_text(5, 5), "All 5 tutorials finished.");
    }

    #[test]
    fn a_finished_guide_offers_restart() {
        assert_eq!(start_label(false), "Start");
        assert_eq!(start_label(true), "Restart");
    }

    #[test]
    fn a_rows_length_reads_plainly() {
        assert_eq!(steps_text(1), "1 step");
        assert_eq!(steps_text(8), "8 steps");
    }
}
