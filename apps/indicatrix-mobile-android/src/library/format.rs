//! Pure text helpers of the Library page: the card subtitle and the import summary.

use indicatrix_mobile_core::library::{ImportReport, LibraryCard};

/// What a card shows when its design has no title.
pub(super) const UNTITLED: &str = "Untitled Design";

/// "Round · 96 · 57 Facets": the shape, the index gear and the facet count, leaving out
/// whatever the library does not know.
pub(super) fn subtitle(card: &LibraryCard) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(3);
    let shape = capitalize(card.shape.trim());
    if !shape.is_empty() {
        parts.push(shape);
    }
    if card.gear > 0 {
        parts.push(card.gear.to_string());
    }
    if card.facets > 0 {
        parts.push(format!(
            "{} {}",
            card.facets,
            if card.facets == 1 { "Facet" } else { "Facets" }
        ));
    }
    parts.join(" \u{b7} ")
}

/// The title to show for a card.
pub(super) fn title(card: &LibraryCard) -> String {
    let title = card.title.trim();
    if title.is_empty() {
        UNTITLED.to_owned()
    } else {
        title.to_owned()
    }
}

/// `text` with its first letter in upper case ("round" becomes "Round").
pub(super) fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `count` followed by the singular or the plural noun.
fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The toast after an import: "Imported 3 designs, 1 duplicate".
pub(super) fn import_message(report: &ImportReport) -> String {
    let mut text = if report.added > 0 {
        format!("Imported {}", counted(report.added, "design", "designs"))
    } else {
        "Nothing imported".to_owned()
    };
    if report.duplicates > 0 {
        text.push_str(", ");
        text.push_str(&counted(report.duplicates, "duplicate", "duplicates"));
    }
    if !report.failed.is_empty() {
        text.push_str(", ");
        text.push_str(&counted(report.failed.len(), "failure", "failures"));
    }
    text
}

/// The body of the dialog that lists the files an import could not read: one line per
/// file (at most `max`), then a count of the rest.
pub(super) fn failure_details(report: &ImportReport, max: usize) -> String {
    let mut lines: Vec<String> = report
        .failed
        .iter()
        .take(max)
        .map(|(name, sentence)| format!("{name}: {sentence}"))
        .collect();
    if report.failed.len() > max {
        lines.push(format!("and {} more.", report.failed.len() - max));
    }
    lines.join("\n")
}

/// The busy line while an import runs.
pub(super) fn importing_text(files: usize) -> String {
    format!("Importing {}", counted(files, "file", "files"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(shape: &str, gear: u32, facets: u32) -> LibraryCard {
        LibraryCard {
            id: 1,
            title: "Barion".to_owned(),
            designer: "Jane".to_owned(),
            shape: shape.to_owned(),
            gear,
            facets,
            ri: None,
            tags: Vec::new(),
            mine: false,
            ignored: false,
            updated_unix: 0,
        }
    }

    #[test]
    fn subtitle_joins_what_is_known() {
        assert_eq!(
            subtitle(&card("round", 96, 57)),
            "Round \u{b7} 96 \u{b7} 57 Facets"
        );
        assert_eq!(subtitle(&card("", 96, 1)), "96 \u{b7} 1 Facet");
        assert_eq!(subtitle(&card("Oval", 0, 0)), "Oval");
        assert_eq!(subtitle(&card("", 0, 0)), "");
    }

    #[test]
    fn title_falls_back_when_blank() {
        let mut untitled = card("", 0, 0);
        untitled.title = "   ".to_owned();
        assert_eq!(title(&untitled), UNTITLED);
        assert_eq!(title(&card("", 0, 0)), "Barion");
    }

    #[test]
    fn capitalize_handles_empty_and_unicode() {
        assert_eq!(capitalize(""), "");
        assert_eq!(capitalize("square"), "Square");
        assert_eq!(capitalize("\u{e9}mile"), "\u{c9}mile");
    }

    #[test]
    fn import_message_counts_each_outcome() {
        let report = ImportReport {
            added: 3,
            duplicates: 1,
            failed: Vec::new(),
        };
        assert_eq!(import_message(&report), "Imported 3 designs, 1 duplicate");
        let report = ImportReport {
            added: 1,
            duplicates: 0,
            failed: vec![("a.asc".to_owned(), "bad".to_owned())],
        };
        assert_eq!(import_message(&report), "Imported 1 design, 1 failure");
        assert_eq!(
            import_message(&ImportReport {
                added: 0,
                duplicates: 2,
                failed: Vec::new(),
            }),
            "Nothing imported, 2 duplicates"
        );
    }

    #[test]
    fn failure_details_lists_and_truncates() {
        let report = ImportReport {
            added: 0,
            duplicates: 0,
            failed: (0..7)
                .map(|i| (format!("f{i}.asc"), "Not a design.".to_owned()))
                .collect(),
        };
        let text = failure_details(&report, 5);
        assert_eq!(text.lines().count(), 6);
        assert!(text.starts_with("f0.asc: Not a design."));
        assert!(text.ends_with("and 2 more."));
    }

    #[test]
    fn importing_text_pluralizes() {
        assert_eq!(importing_text(1), "Importing 1 file");
        assert_eq!(importing_text(12), "Importing 12 files");
    }
}
