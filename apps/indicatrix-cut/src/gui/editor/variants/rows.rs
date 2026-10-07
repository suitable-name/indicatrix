//! The Variants list as plain data: the rows, the words on them, the default name of a new
//! variant and what the two compare boxes offer. No Slint types, so the tests cover it
//! directly.

use crate::bridge::export_thread::filename_template::civil_from_unix_seconds;
use indicatrix_vault::model::design_variant::VariantSummary;

const MINUTE_SECS: i64 = 60;
const HOUR_SECS: i64 = 3_600;
const DAY_SECS: i64 = 86_400;

/// A variant older than this many days shows its date instead of "N days ago".
const RELATIVE_DAYS: i64 = 30;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// What the first entry of the compare boxes is called.
pub(super) const CURRENT_LABEL: &str = "Current design";

/// The longest name, in characters.
pub(super) const NAME_LIMIT: usize = 80;

/// The longest note, in characters.
pub(super) const NOTE_LIMIT: usize = 400;

/// One row of the list: the Slint-free twin of `VariantRowData`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VariantRow {
    /// The variant's id in the library.
    pub(super) id: i64,
    pub(super) name: String,
    /// The note; empty when there is none.
    pub(super) note: String,
    /// "12 minutes ago", or a date for an old one.
    pub(super) age: String,
    /// The exact time in UTC, for the hover text.
    pub(super) exact: String,
    /// "from: Variant 2"; empty when the variant has no parent.
    pub(super) parent: String,
    /// New variants are saved as made from this one.
    pub(super) opened_from: bool,
}

/// `list` newest first (by time, then by id).
fn newest_first(list: &[VariantSummary]) -> Vec<&VariantSummary> {
    let mut ordered: Vec<&VariantSummary> = list.iter().collect();
    ordered.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then(b.variant_id.cmp(&a.variant_id))
    });
    ordered
}

/// The rows for `list`, newest first. `branch` is the variant the open design came from;
/// `now` is the current time in Unix seconds.
pub(super) fn build_rows(
    list: &[VariantSummary],
    branch: Option<i64>,
    now: i64,
) -> Vec<VariantRow> {
    newest_first(list)
        .into_iter()
        .map(|variant| VariantRow {
            id: variant.variant_id,
            name: variant.name.clone(),
            note: variant.note.clone().unwrap_or_default(),
            age: age_text(now, variant.created_at),
            exact: exact_text(variant.created_at),
            parent: parent_text(list, variant.parent_variant_id),
            opened_from: branch == Some(variant.variant_id),
        })
        .collect()
}

/// "from: Variant 2" for a parent that is in `list`, otherwise nothing.
fn parent_text(list: &[VariantSummary], parent: Option<i64>) -> String {
    parent
        .and_then(|id| list.iter().find(|variant| variant.variant_id == id))
        .map_or_else(String::new, |variant| format!("from: {}", variant.name))
}

/// "1 minute", "2 minutes".
fn counted(count: i64, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// How long ago `created` was, at `now` (both Unix seconds): "Just now", "5 minutes ago",
/// "3 hours ago", "2 days ago", and for a variant more than a month old its date.
///
/// Only differences are used, so the words do not depend on the time zone. A time in the
/// future (the clock was set back) reads "Just now".
pub(super) fn age_text(now: i64, created: i64) -> String {
    let elapsed = now.saturating_sub(created);
    if elapsed < MINUTE_SECS {
        "Just now".to_owned()
    } else if elapsed < HOUR_SECS {
        format!("{} ago", counted(elapsed / MINUTE_SECS, "minute"))
    } else if elapsed < DAY_SECS {
        format!("{} ago", counted(elapsed / HOUR_SECS, "hour"))
    } else if elapsed < RELATIVE_DAYS * DAY_SECS {
        format!("{} ago", counted(elapsed / DAY_SECS, "day"))
    } else {
        date_text(created)
    }
}

/// "5 Oct 2026" (UTC).
fn date_text(created: i64) -> String {
    let (year, month, day, ..) = civil_from_unix_seconds(created);
    let month_name = usize::try_from(month.saturating_sub(1))
        .ok()
        .and_then(|index| MONTHS.get(index))
        .copied()
        .unwrap_or("?");
    format!("{day} {month_name} {year}")
}

/// "2026-10-05 14:20:31 UTC".
pub(super) fn exact_text(created: i64) -> String {
    let (year, month, day, hour, minute, second) = civil_from_unix_seconds(created);
    format!("{year}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

/// The number in a name written as "Variant 7".
fn numbered(name: &str) -> Option<u64> {
    name.strip_prefix("Variant ")?.trim().parse().ok()
}

/// The name a new variant is offered: "Variant N", with N one more than the number of
/// variants and than the highest "Variant N" there is, so it is never a name already used.
pub(super) fn default_name(list: &[VariantSummary]) -> String {
    let highest = list
        .iter()
        .filter_map(|variant| numbered(&variant.name))
        .max()
        .unwrap_or(0);
    let count = u64::try_from(list.len()).unwrap_or(u64::MAX);
    let mut number = highest.max(count).saturating_add(1);
    loop {
        let candidate = format!("Variant {number}");
        if !list
            .iter()
            .any(|variant| variant.name.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
        number = number.saturating_add(1);
    }
}

/// One entry of the compare boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Choice {
    /// The design as it is now.
    Current,
    /// A saved variant, by id.
    Variant(i64),
}

/// The compare boxes' entries: the current design first, then the variants newest first,
/// with the labels to show (equal names are told apart by their time).
pub(super) fn build_choices(list: &[VariantSummary]) -> (Vec<Choice>, Vec<String>) {
    let ordered = newest_first(list);
    let mut choices = vec![Choice::Current];
    let mut labels = vec![CURRENT_LABEL.to_owned()];
    for variant in &ordered {
        choices.push(Choice::Variant(variant.variant_id));
        let clashes = variant.name == CURRENT_LABEL
            || ordered
                .iter()
                .filter(|other| other.name == variant.name)
                .count()
                > 1;
        labels.push(if clashes {
            format!(
                "{} (saved {})",
                variant.name,
                exact_text(variant.created_at)
            )
        } else {
            variant.name.clone()
        });
    }
    make_unique(&mut labels, &choices);
    (choices, labels)
}

/// Adds the variant's id to any label that is still the same as another one (two variants
/// with one name saved in the same second).
fn make_unique(labels: &mut [String], choices: &[Choice]) {
    let snapshot = labels.to_vec();
    for (label, choice) in labels.iter_mut().zip(choices) {
        let repeated = snapshot
            .iter()
            .filter(|other| other.as_str() == label.as_str())
            .count()
            > 1;
        if let (true, Choice::Variant(id)) = (repeated, choice) {
            *label = format!("{label}, number {id}");
        }
    }
}

/// Where the two compare boxes stand after the entries changed.
///
/// Each keeps the design it was on if that is still there. The first falls back to the
/// current design and the second to the newest variant, and the two never name the same
/// entry unless there is only one.
pub(super) fn reselect(old: &[Choice], first: i32, second: i32, new: &[Choice]) -> (i32, i32) {
    let find = |index: i32| -> Option<i32> {
        let choice = old.get(usize::try_from(index).ok()?)?;
        let position = new.iter().position(|other| other == choice)?;
        i32::try_from(position).ok()
    };
    let other_than_current = i32::from(new.len() > 1);
    let first = find(first).unwrap_or(0);
    let second = find(second).unwrap_or(other_than_current);
    if second != first {
        return (first, second);
    }
    let instead = if first == 0 { other_than_current } else { 0 };
    (first, instead)
}

/// The two entries the boxes name, or the sentence that says why they cannot be compared.
///
/// # Errors
///
/// A plain sentence when a box names nothing or both name the same entry.
pub(super) fn resolve_pair(
    choices: &[Choice],
    first: i32,
    second: i32,
) -> Result<(Choice, Choice), &'static str> {
    let pick = |index: i32| usize::try_from(index).ok().and_then(|i| choices.get(i));
    match (pick(first), pick(second)) {
        (Some(a), Some(b)) if a != b => Ok((*a, *b)),
        (Some(_), Some(_)) => Err("Choose two different designs to compare."),
        _ => Err("Choose the two designs to compare."),
    }
}

/// The name typed into the form, trimmed.
///
/// # Errors
///
/// A plain sentence when it is empty or too long.
pub(super) fn clean_name(text: &str) -> Result<String, String> {
    let name = text.trim();
    if name.is_empty() {
        return Err("Give the variant a name.".to_owned());
    }
    if name.chars().count() > NAME_LIMIT {
        return Err(format!(
            "Use a shorter name ({NAME_LIMIT} characters at most)."
        ));
    }
    Ok(name.to_owned())
}

/// The note typed into the form, trimmed; `None` when it is empty.
///
/// # Errors
///
/// A plain sentence when it is too long.
pub(super) fn clean_note(text: &str) -> Result<Option<String>, String> {
    let note = text.trim();
    if note.chars().count() > NOTE_LIMIT {
        return Err(format!(
            "Use a shorter note ({NOTE_LIMIT} characters at most)."
        ));
    }
    Ok((!note.is_empty()).then(|| note.to_owned()))
}
