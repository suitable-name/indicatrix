//! Read-only `facet_diagrams.sqlite` access: loads every `.asc`-schedule row from
//! `attached_files`, and locates the database file relative to the current working
//! directory.

use rusqlite::Connection;

use crate::types::AscRow;

/// Loads every `.asc` attachment row from the database in a stable order.
pub fn load_asc_rows(conn: &Connection) -> Vec<AscRow> {
    let mut stmt = conn
        .prepare("SELECT detail_id, content FROM attached_files WHERE name LIKE '%.asc' ORDER BY detail_id, id")
        .expect("prepare attached_files query");
    let rows = stmt
        .query_map([], |row| {
            Ok(AscRow {
                detail_id: row.get(0)?,
                content: row.get(1)?,
            })
        })
        .expect("query attached_files");
    rows.filter_map(Result::ok).collect()
}

/// Locates `facet_diagrams.sqlite` relative to the working directory.
pub fn find_db_path() -> String {
    for candidate in [
        "facet_diagrams.sqlite",
        "../../facet_diagrams.sqlite",
        "../facet_diagrams.sqlite",
    ] {
        if std::path::Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "facet_diagrams.sqlite".to_string()
}
