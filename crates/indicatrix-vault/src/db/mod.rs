/// The SQLite-backed [`Database`](sqlite::Database): schema creation/migration.
///
/// Also the two ways to open one: [`sqlite::Database::new`]/[`sqlite::Database::open_read_only`].
pub mod sqlite;
