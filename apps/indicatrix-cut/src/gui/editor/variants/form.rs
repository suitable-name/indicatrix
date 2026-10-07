//! The small form above the variants list: what it is for and the words it shows. No Slint
//! types, so the tests cover it directly.

use indicatrix_vault::model::design_variant::VariantSummary;

/// What the form is asking for. `Closed` is the form not showing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) enum Form {
    #[default]
    Closed,
    /// Save a new variant: of the design as it is now (`position` is `None`), or as it was at
    /// a history step. `revision` is the step's revision when the form was opened, so a step
    /// that has since been replaced is not saved under the old words. `design` is the UUID and
    /// the design epoch the form was opened for: revisions count from zero in every new
    /// history, so a step number and revision alone can name a step of another design.
    Save {
        position: Option<usize>,
        revision: Option<u64>,
        design: (String, u64),
    },
    /// Rename the variant with this id.
    Rename(i64),
    /// Change the note of the variant with this id.
    Note(i64),
    /// Ask before deleting the variant with this id.
    Delete(i64),
}

impl Form {
    /// `VariantsModel.form_mode`: 0 closed, 1 save, 2 rename, 3 note, 4 delete.
    pub(super) const fn code(&self) -> i32 {
        match self {
            Self::Closed => 0,
            Self::Save { .. } => 1,
            Self::Rename(_) => 2,
            Self::Note(_) => 3,
            Self::Delete(_) => 4,
        }
    }
}

/// What the form shows when it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FormView {
    pub(super) title: String,
    /// One sentence under the title; empty for none.
    pub(super) hint: String,
    /// The label of the confirm button.
    pub(super) ok: &'static str,
    /// The text the name field starts with.
    pub(super) name: String,
    /// The text the note field starts with.
    pub(super) note: String,
}

/// The save form. `step` names the history step being saved (its number and what it did),
/// `None` for the design as it is now. `name` is the name offered.
pub(super) fn save_view(step: Option<(usize, &str)>, name: String) -> FormView {
    let hint = match step {
        None => "Keeps the design as it is now.".to_owned(),
        Some((0, _)) => {
            "Keeps the design as it was at the start, before your first change.".to_owned()
        }
        Some((number, label)) => {
            format!("Keeps the design as it was after step {number}: {label}.")
        }
    };
    FormView {
        title: "Save as a variant".to_owned(),
        hint,
        ok: "Save",
        name,
        note: String::new(),
    }
}

/// The rename form for `variant`.
pub(super) fn rename_view(variant: &VariantSummary) -> FormView {
    FormView {
        title: "Rename variant".to_owned(),
        hint: String::new(),
        ok: "Rename",
        name: variant.name.clone(),
        note: String::new(),
    }
}

/// The note form for `variant`.
pub(super) fn note_view(variant: &VariantSummary) -> FormView {
    FormView {
        title: format!("Note for \"{}\"", variant.name),
        hint: String::new(),
        ok: "Save note",
        name: String::new(),
        note: variant.note.clone().unwrap_or_default(),
    }
}

/// The question before deleting `variant`.
pub(super) fn delete_view(variant: &VariantSummary) -> FormView {
    FormView {
        title: "Delete this variant?".to_owned(),
        hint: format!(
            "\"{}\" will be removed from your library. This cannot be undone. Your current design is not changed.",
            variant.name
        ),
        ok: "Delete",
        name: String::new(),
        note: String::new(),
    }
}
