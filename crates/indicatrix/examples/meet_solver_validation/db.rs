//! Read-only `facet_diagrams.sqlite` access: loads every `.asc`-schedule row from
//! `attached_files` together with its printed proportions, and locates the
//! database file relative to the current working directory.

use rusqlite::Connection;

use crate::types::AscRow;

pub fn load_asc_rows(conn: &Connection) -> Vec<AscRow> {
    let mut stmt = conn
        .prepare(
            "SELECT af.detail_id, af.content, dd.cw_ratio, dd.pw_ratio, \
                    dd.volume, dd.lw_ratio, dd.hw_ratio \
             FROM attached_files af \
             LEFT JOIN diagram_details dd ON af.detail_id = dd.id \
             WHERE af.name LIKE '%.asc' \
             ORDER BY af.detail_id, af.id",
        )
        .expect("prepare attached_files query");
    let rows = stmt
        .query_map([], |row| {
            Ok(AscRow {
                detail_id: row.get(0)?,
                content: row.get(1)?,
                cw_ratio: row.get(2)?,
                pw_ratio: row.get(3)?,
                volume: row.get(4)?,
                lw_ratio: row.get(5)?,
                hw_ratio: row.get(6)?,
            })
        })
        .expect("query attached_files");
    rows.filter_map(Result::ok).collect()
}

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
