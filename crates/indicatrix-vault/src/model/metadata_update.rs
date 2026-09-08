/// Fields a user might legitimately hand-correct on an already-imported design.
///
/// Used by the detail view's metadata editor (see `apps/indicatrix-cut`'s `gui::detail`/
/// `detail_header.slint`). Deliberately NOT title -- that lives in `diagram_entries`,
/// not `diagram_details`, with its own setter
/// ([`crate::db::sqlite::Database::rename_diagram_entry`]).
///
/// Every field is `Option<...>` because the underlying column is nullable and a blank
/// field in the editor is a valid edit (clears that value); there is no separate "leave
/// unchanged" state since [`crate::db::sqlite::Database::update_diagram_metadata`] is a
/// single `UPDATE` naming exactly these columns.
///
/// Deliberately excludes every other `diagram_details` column (`page_url`,
/// `diagram_image_name`/`diagram_image_data`, `competition_diagram`, `tw_ratio`,
/// `uw_ratio`, the `designer`/`source_citation` split, `pdf_file`, `gem_file`,
/// `shape_category`, `angle_settings`/`attached_files`) -- see
/// `update_diagram_metadata`'s doc for what a naive read-modify-write through
/// [`crate::model::detail::FacetDiagramDetail`] would silently zero instead.
///
/// `designer` here is the free-text `designer_info` column (what `detail_header.slint`
/// displays as "Designed by ...") -- not the separate machine-split `designer`/
/// `source_citation` pair `FacetDiagramDetail` also carries; there's no UI for editing
/// that split, and this update path leaves it as the original import produced it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataUpdate {
    pub designer_info: Option<String>,
    pub shape: Option<String>,
    pub refractive_index: Option<String>,
    pub index_gear: Option<String>,
    pub facets_count: Option<String>,
    pub symmetry_order: Option<String>,
    pub mirror_symmetry: Option<bool>,
    pub lw_ratio: Option<String>,
    pub hw_ratio: Option<String>,
    pub cw_ratio: Option<String>,
    pub pw_ratio: Option<String>,
    pub volume: Option<String>,
}
