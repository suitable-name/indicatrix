# indicatrix-vault

Plain data models, a SQLite-backed store, and local import/export for the user's own
faceting-design library.

Entirely local and offline: the whole dependency list is `anyhow`, `rusqlite`,
`serde`, `tracing`, and `indicatrix-formats` (for reading and writing `.asc` files). There is
no network client here and nothing that reaches outside the process. A library is
built by importing your own `.asc` files, which is the only way rows get into it.

## Quick start

```rust
use indicatrix_vault::db::sqlite::Database;
use indicatrix_vault::model::filter::RangeFilter;

let db = Database::new(Some("facet_diagrams.sqlite"))?;

let total = db.get_total_count()?;
let shapes = db.get_unique_shapes()?;

let results = db.search_diagrams("emerald", "", "", &RangeFilter::default())?;
for item in results {
    println!("{}: {}", item.id, item.title);
}
# Ok::<(), anyhow::Error>(())
```

`Database::new(None)` opens (creating if necessary) `facet_diagrams.sqlite` in the
process's current working directory — there is no fixed config-directory location;
the caller decides where the database file lives.

## Architecture

SQLite via `rusqlite` (the workspace-pinned version with the `bundled` feature, so
there is no system SQLite dependency, plus the `functions` feature for the `fold()`
search function below). `Database` (`db::sqlite::Database`) wraps one
`rusqlite::Connection`, opened with `PRAGMA foreign_keys = ON`, WAL journal mode, a 5s
`busy_timeout`, and -- only when WAL was actually obtained, since `NORMAL` is crash-safe
only under WAL -- `synchronous = NORMAL`. Tables:

| Table | Purpose |
|---|---|
| `diagram_entries` | One row per design: `title`, `url` (**unique** — the dedup key within one source), `design_id`, `source_id`, `ignored`, `created_at`/`updated_at` (a revision stamp: every write strictly advances it, even within one second), `derived_from_entry_id` |
| `diagram_details` | One row per entry (1:1, cascade-deleted with it): shape, refractive index, gear, ratios, volume, facet counts, symmetry, designer split, PDF/GEM attachment names, `concave_tiers`/`concave_facets` counts (`0` for a planar design; `facets_count` keeps its two-component `"55+6"` text), etc. Blob column (`diagram_image_data`) declared last — see "Blob columns last" below |
| `angle_settings` | The cutting-instructions rows for one detail: facet, angle, index, notes, plus nullable `tool`/`tool_line` (the concave tool and its formatted second line; `NULL` for a flat tier) — ordered |
| `attached_files` | Raw file bytes attached to one detail (an original `.asc`, an image, a PDF) — indexed on `detail_id` |
| `custom_gem_materials` | User-defined materials (name, RI, dispersion, birefringence, absorption, crystal system/optical character, per-axis dispersion, specific gravity) |
| `shape_vocabulary` | The canonical shape picker list (`name`, `sort_order`) — seeded from `DEFAULT_SHAPES`, see below |
| `diagram_previews` | Cached preview renders, keyed by `entry_id` (survives a `diagram_details` re-sync), with a `params_fingerprint` naming what they were rendered with (`NULL` on rows that predate it); blob columns last |
| `diagram_tilt_curves` | Cached tilt-performance curves plus 6 precomputed global-min/max columns, keyed by `entry_id`, with a `params_fingerprint` naming what the sweep was computed with (`NULL` on rows that predate it); blob columns last |
| `diagram_solid_extents` | Cached finished-solid extents (widths, length, height, volume) for the Rough Planner, keyed by `entry_id`, with the measurement `source` and an `extents_version`; cascade-deleted with the entry |
| `diagram_solid_hull` | Cached finished-solid convex hull (packed little-endian `f32` vertex triples, BLOB last) for the Rough Planner, keyed by `entry_id`, with a `hull_version`; cascade-deleted with the entry |
| `saved_rough_plans` | Saved Rough Planner plans: `name`, `created_at`/`updated_at`, and a versioned text `payload`; not tied to any entry |
| `diagram_planner_exclusions` | The designs the Rough Planner leaves out of its candidate set: one row per excluded `entry_id`, so the mark is the row's presence, cascade-deleted with the entry. Written by `set_planner_excluded` without touching `diagram_entries.updated_at`, and read back with `planner_excluded_ids` and `planner_excluded_among`; independent of the library's `ignored` flag |
| `tags` / `diagram_tag_links` | A flat, case-insensitive tag set and its many-to-many join table |
| `library_mirror_state` | Pull-mirror sync bookkeeping, keyed by `url`, including the `deleted_locally` tombstone — see "Mirror-state semantics" below |

### Blob columns last

`diagram_details`/`diagram_previews`/`diagram_tilt_curves` all declare their BLOB
column(s) after every other column, both in `create_tables_if_not_exist` and (for a
database created before this convention existed) via the idempotent
`migrate_blob_columns_last` rebuild. SQLite reads a row's columns in physical storage
order, so a query that never touches a BLOB still pays to skip its bytes if it sits
earlier — measured on the real catalogue: a `diagram_details` text search cost
40.9/39.2 ms with `diagram_image_data` at its original column 5, 16.5 ms with it moved
last.

### Search indexes

`idx_angle_settings_detail_id`, `idx_diagram_tag_links_tag_id`, and
`idx_attached_files_detail_id` back the library search predicate's notes/tag-chip/
attachment lookups — see `SEARCH_INDEXES_SQL`'s own doc comment for the measurements
that motivated each. `idx_diagram_details_designer` (an equality index for an "every
design by X" lookup nothing in this workspace ever calls) was dropped.

## Migrations

There is no separate schema-version table and no external migration framework.
`Database::new` runs a fixed, ordered sequence of private migration methods on every
open — `create_tables_if_not_exist`, then (in order) `migrate_numeric_columns`,
`migrate_source_id_column`, `migrate_proportions_columns`,
`migrate_designer_and_attachment_columns`, `migrate_drop_unused_designer_index`,
`migrate_crystal_optics_columns`, `migrate_per_axis_dispersion_column`,
`migrate_custom_material_specific_gravity`, `migrate_shape_vocabulary`,
`migrate_ignored_column`, `migrate_diagram_entries_timestamps`,
`migrate_diagram_entries_provenance`, `migrate_diagram_previews_table`,
`migrate_diagram_tilt_curves_table`, `migrate_diagram_solid_extents_table`,
`migrate_diagram_solid_hull_table`, `migrate_saved_rough_plans_table`,
`migrate_planner_exclusion_table`,
`migrate_mirror_state_tombstone_column`, `migrate_prune_tilt_curve_aggregate_columns`,
`migrate_concave_columns` (the four concave columns above, each gated on its own `column_exists`, one transaction; it must precede the rebuild, which names `diagram_details`' columns),
`migrate_blob_columns_last`, `migrate_tag_tables`, and finally
`migrate_search_indexes` (see `db/sqlite/migrations/mod.rs` for the full list and
why the order matters — e.g. `migrate_search_indexes` must run after
`migrate_tag_tables` since one of its indexes targets `diagram_tag_links`). Most are
self-gating: they check `PRAGMA table_info` for a column that would already exist if
they had already run, and return immediately if so. On a brand-new database,
`create_tables_if_not_exist` already creates every column (and table) the later
migrations would add, so each of them is a no-op the very first time; on an older
database file, each migration actually runs its `ALTER TABLE`/backfill exactly once.
Which migrations are transactional:

- One transaction each, so a crash partway through rolls back cleanly: the numeric
  retype plus `facets_count` split (`migrate_numeric_columns`), the table rebuild
  (`migrate_blob_columns_last`), the tilt-curve column prune, the mirror-state
  tombstone column plus its backfill, and every multi-column `ADD COLUMN` batch —
  `migrate_proportions_columns` (7 columns), `migrate_designer_and_attachment_columns`
  (5), `migrate_crystal_optics_columns` (3) and `migrate_diagram_entries_timestamps`
  (2). The batches run through one helper that skips a column that already exists and
  are gated on the LAST column of the batch, so a table an older build left with only
  the first few columns is completed on the next open instead of skipped.
- Single statement, atomic by nature: the one-column `ALTER TABLE ... ADD COLUMN`
  migrations (`source_id`, `ignored`, per-axis dispersion, specific gravity,
  provenance).
- Idempotent but not wrapped in a transaction: the `CREATE TABLE IF NOT EXISTS`
  migrations for the side tables (previews, tilt curves, solid extents, solid hull,
  saved plans, planner exclusions, tags), `migrate_search_indexes` and
  `migrate_drop_unused_designer_index` (`CREATE/DROP INDEX IF EXISTS`), and
  `migrate_shape_vocabulary` (`CREATE TABLE IF NOT EXISTS` + `INSERT OR IGNORE`) — see
  "Shape vocabulary" below.

`migrate_planner_exclusion_table` creates `diagram_planner_exclusions` for a database that
predates it, from the same SQL constant `create_tables_if_not_exist` uses for a fresh one.
It runs right after `migrate_saved_rough_plans_table`, adds no column to an existing table,
and leaves every pre-existing design unexcluded.

The numeric retype turns text that does not begin with a number (`'n/a'`, `'?'`, empty)
into `NULL` rather than a fabricated `0`; text that does begin with one (`'96 index'`)
keeps the parsed leading number.

A table rebuild (`migrate_blob_columns_last`) that must survive a mid-crash and never
cascade-drop a child table (e.g. `diagram_details` is the FK parent of
`angle_settings`/`attached_files`) follows SQLite's documented 12-step procedure:
`PRAGMA foreign_keys = OFF` OUTSIDE any transaction, then inside one transaction —
create a `__reordered` staging table, `INSERT INTO ... SELECT` naming every column
explicitly, `DROP TABLE`, `ALTER TABLE ... RENAME TO`, and a closing
`PRAGMA foreign_key_check` before committing — with `foreign_keys` restored
unconditionally afterward, even on failure.

**Adding a new migration**: write a new `migrate_*` method following the same
"check the last new column via `column_exists`, then add the missing columns in one
transaction" pattern (`Database::add_missing_columns`), and call it at the end of
`Database::new`'s migration chain (order
matters if a later migration depends on an earlier one's columns existing).
`migrate_shape_vocabulary` (below) is the one exception to the `column_exists`
gate — a data-seeding migration rather than a schema-altering one, so it's
idempotent by `CREATE TABLE IF NOT EXISTS` + `INSERT OR IGNORE` instead.

### Shape vocabulary

```rust
pub const DEFAULT_SHAPES: &[&str] = &[
    "Round", "Oval", "Square", "Rectangle", "Emerald", "Pear", "Marquise",
    "Heart", "Triangle", "Hexagon", "Octagon", "Pentagon", "Kite", "Shield",
    "Star", "Freeform",
];
```

`"Cushion"`, `"Trillion"`, `"Barion"`, `"Briolette"`, and `"Rhombus"` were dropped from
the seed list: the shape filter (`build_search_predicate`) matches `dd.shape` with a
plain `=`, never a substring/prefix match, and none of these five ever appears as an
*exact* scraped `shape` string on the real catalogue — offering them in a picker
silently returns zero results. A hand-edited or already-seeded row for one of these
five is left in place on an existing install; this only changes what a fresh seed
offers.

`Database::get_unique_shapes()` used to be a plain `SELECT DISTINCT shape FROM
diagram_details` — on a fresh database, with no design yet imported, that returned
nothing, leaving a shape picker with no vocabulary to offer. `migrate_shape_vocabulary`
seeds a `shape_vocabulary` table (`name TEXT PRIMARY KEY`, `sort_order INTEGER`)
from `DEFAULT_SHAPES` on every open, fresh database included, and `get_unique_shapes`
now returns the **union** of that seeded vocabulary with whatever `shape` values
actually appear in `diagram_details`, deduplicated and sorted alphabetically. Plain
alphabetical (not canonical-list-first) is deliberate: the real catalogue holds
scraped shape strings `DEFAULT_SHAPES` doesn't and never will exhaustively cover
(e.g. `"Portuguese Round"`), so there's no principled way to split "seeded" from
"discovered" entries in the output — alphabetical is the one ordering a dropdown
reader can always predict regardless of which side of the union an entry came from.

`shape_vocabulary` is a plain lookup list, not a foreign-key target for
`diagram_details.shape` — a FK constraint would either reject the catalogue's
free-text scraped shapes or force a lossy migration of real data, so `shape` stays
free text and `shape_vocabulary` stays purely additive. `DEFAULT_SHAPES` is `pub`
specifically so another crate (e.g. `apps/indicatrix-cut`'s import flow) can offer the
same list as a picker without a round trip through the database — there is exactly
one definition of this list.

## Local `.asc` import / export (`local` module)

```rust
pub const LOCAL_SOURCE_ID: &str = "local-import";

pub struct ImportedAsc {
    pub entry: FacetingDiagramEntry,
    pub detail: FacetingDiagramDetail,
    pub derived_from_entry_id: Option<i64>,
}

pub fn import_asc(
    file_name: &str,
    content: &str,
    native_sidecar: Option<(&str, &[u8])>,
) -> Result<ImportedAsc, String>;

pub fn reconstruct_asc_schedule(
    title: &str,
    refractive_index: Option<&str>,
    index_gear: Option<&str>,
    angle_settings: &[AngleSetting],
) -> Result<Option<AscSchedule>, String>;
```

`import_asc` parses raw `.asc` text via `indicatrix_formats::asc::parse_asc`, derives a title
from the file's first `H` header line (falling back to the filename with `.asc`
stripped), and synthesizes `url: "local://{file_name}"` — this is what the
`diagram_entries.url` uniqueness constraint dedupes a repeat import of the same
file against. The parsed tiers become `angle_settings` rows, and the raw file
bytes are stored as received (`import_asc_bytes` keeps the file's raw bytes, not a re-encoded
UTF-8 copy) as an `attached_files` entry, so the original can be re-exported exactly later.
Rows imported by earlier versions hold the already-decoded UTF-8 text instead; readers
decode either form with `decode_asc_bytes`. `native_sidecar`, when the caller found a
`.indicatrix` design file (or an older `.indicatrix.toml`/`.gemcut.toml` sidecar)
beside the `.asc` on disk, is attached as a second `attached_files` entry, so a
design round-tripped through Save and back through Import doesn't lose the fields
only the design file carries. `derived_from_entry_id` recovers the
catalogue row id an exported `.asc` recorded itself as derived from (an
`Indicatrix-Source-Entry-Id:` footnote written by `gui::editor::native_io`), so an
export-then-reimport can be linked back to its source row instead of landing as an
unrelated duplicate — the caller is responsible for verifying that id still names a
real row before recording it via `Database::set_derived_from_entry_id`; this crate
never guesses provenance from a title/filename match.

`reconstruct_asc_schedule` is the inverse: for a design that has an
`angle_settings` table but no attached original `.asc` file, it rebuilds an
`AscSchedule` from those stored rows alone. Mast (depth) distances are left at
`0.0` — that
information genuinely does not exist anywhere except a real `.asc` file — and the
result always gets `indicatrix_formats::asc::mark_reconstructed` called on it before being
returned, so a reconstructed export can never be mistaken for original,
mast-accurate data. Returns `Ok(None)` if there are no angle-settings rows to
work with; returns `Err` naming the offending tier if a tier's angle text fails to
parse, or naming the offending text if a *present* refractive index fails to parse
(never a silent `0.0` for either) — a *missing* refractive index still defaults to
`0.0`, since that's a genuinely unknown value on some designs, not unparsable text.
The index-wheel text separator set includes `-` alongside `,`/` `/`;`, matching the
real catalogue's scraped format (`"96-08-16-24-32-40-48-56-64-72-80-88"`).

## Querying and filtering

```rust
pub struct RangeFilter {
    pub ri_min: Option<f64>, pub ri_max: Option<f64>,
    pub lw_min: Option<f64>, pub lw_max: Option<f64>,
    pub volume_min: Option<f64>, pub volume_max: Option<f64>,
    pub facets_min: Option<i64>, pub facets_max: Option<i64>,
    pub ri_tolerance: Option<(f64, f64)>,     // (center, tolerance) band, ANDed with the above
    pub include_ignored: bool,                // false: ignored designs are excluded
    pub performance: Vec<PerformanceFilter>,  // tilt-performance predicates, ANDed
}
```

`Database::search_diagrams(query, shape_filter, gear_filter, range)` builds one
dynamic, parameterized SQL query: free-text `LIKE` match against title/designer/
design-id/notes, optional exact shape/gear equality, and the numeric range bounds
below — all filtering happens in that one query, capped at `SEARCH_RESULT_CAP` rows,
nothing is filtered back in application code. A caller that needs every matching
row, not just the first page — e.g. `apps/indicatrix-cut`'s `bridge::library_mirror` —
uses `Database::search_diagrams_page(.., after_id, limit)` instead: the same
filters plus a keyset cursor (`id > after_id`, over `diagram_entries.id`'s unique,
strictly-increasing `INTEGER PRIMARY KEY AUTOINCREMENT`), walked page by page until
a short page signals the end. `search_diagrams` is exactly
`search_diagrams_page(.., None, SEARCH_RESULT_CAP)` — one query-building path, so the
two can never disagree. `Database::get_attribute_ranges()`
computes each numeric column's real minimum alongside a **99th-percentile** (not
the raw maximum) as the usable upper bound, so a single outlier row can't compress
a UI slider's whole usable range — it logs a warning if the raw maximum is more
than 5x the derived bound, since that's a sign worth investigating rather than
silently absorbing.

The title/designer-name match is wrapped in a registered `fold(text)` SQL function
(lowercased Unicode-aware, plus a small fixed map of curly-quote/dash punctuation —
U+2018/U+2019/U+201C/U+201D/U+2013/U+2014 — to their plain ASCII equivalents), applied
to both the column and the bound pattern: SQLite's own `LIKE` only case-folds ASCII,
so `"TORBJÖRN"` wouldn't otherwise match `"torbjörn"`, and a plain typed `"Cam's"`
wouldn't match a scraped `"Cam's"` (typographic apostrophe). Both `dd.designer_info`
(the free-text field) and the machine-split `dd.designer` column are matched, so a
query that only appears in the split column doesn't silently return nothing. Every
`LIKE` pattern is also escaped (`\`, `%`, `_`) with `ESCAPE '\'`, so a literal `%`/`_`
typed by a user is matched as text, not read as a SQL wildcard.

`entry_ids_missing_previews`/`entry_ids_missing_tilt_curves` return every
non-ignored entry id with no cached preview render / tilt curve yet, via a
`LEFT JOIN` that never selects a BLOB column — for a startup batch pass that used to
load both cached images for every catalogue entry just to test one timestamp. Each takes a
fingerprint closure (material in, current `params_fingerprint` out) and also lists a row
whose stored fingerprint differs, so a changed renderer or setting marks old results
outdated.

`save_preview_images`/`save_tilt_curves` take the caller's `params_fingerprint` and an
`expected_updated_at` (read with `Database::entry_updated_at` together with the record that
was rendered) and return `Ok(false)` without writing when the entry changed in between.
`Database::tags_for_entries(ids)` is `tags_by_entry` restricted to the given entries. The
separate `get_tilt_curve_image` accessor has been removed (the `curve_image` column is never written).

`Database::diagram_entry_id_for_url(url)` looks up an entry's id by its dedup `url`
alone (undocumented until now), without touching `diagram_details` or committing to
`save_diagram_entry`'s upsert-and-report-id shape — for a caller (e.g. a mirror sync)
that only needs to know whether a row already exists. `Database::diagram_entry_for_url`
returns the owner's `(id, title)` instead, for a caller that declines to write over a
row it does not own and wants to name it (Save's catalogue write-back does). `Database::get_preview_material
(entry_id)` (also undocumented until now) is the plain read half of the preview-material
pair: it returns `entry_id`'s persisted `preview_material`, if any, without running
`ensure_preview_material`'s RNG/candidate-selection logic — for a caller (e.g. a batch
preview scan) that only wants to know what's already on file.

## Narrow metadata edits (`update_diagram_metadata`)

```rust
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

db.update_diagram_metadata(entry_id, &update)?;
```

`Database::get_diagram_full` returns a `FullDiagramRecord`, which is a strict subset of
`FacetingDiagramDetail` — it has no `hw_ratio`/`tw_ratio`/`uw_ratio`/`pw_ratio`/`cw_ratio`/
`symmetry_order`/`mirror_symmetry`/`designer`/`source_citation`/`pdf_file`/`gem_file`/
`shape_category` fields at all. `save_diagram_detail` fully *replaces* a design's detail
row (delete, then reinsert, including every `angle_settings`/`attached_files` child
row), so building a fresh `FacetingDiagramDetail` from a `FullDiagramRecord` and saving it
back would silently zero every one of those fields on every edit — for a locally
imported design that means erasing the very proportions its own import step measured.

`update_diagram_metadata` is the real fix for editing metadata a user might legitimately
hand-correct (title, designer, shape, refractive index, index gear, facet count,
symmetry order, mirror symmetry, and the proportion ratios): one `UPDATE` naming exactly
`MetadataUpdate`'s fields (plus `facets`/`girdle_facets`, kept in sync with
`facets_count` as a deterministic re-parse of the same string — not the geometry
recomputation this crate otherwise never does) and nothing else. Every other
`diagram_details` column, and `angle_settings`/`attached_files` in their entirety, are
never touched — there is no delete, so a child row's own id and an attachment's bytes
survive a metadata edit completely unchanged. Title lives in `diagram_entries`, not
`diagram_details`, and already has its own narrow setter, `rename_diagram_entry`, with
none of this trap to begin with — it isn't part of `MetadataUpdate`. A caller that
edits the title and the metadata together uses `Database::rename_and_update_metadata`,
which does both in one transaction, so a rejected numeric field leaves the old title
in place.

## Saving a design atomically (`save_design`)

`Database::save_design(entry, detail, source_id) -> Result<i64>` runs
`save_diagram_entry` and `save_diagram_detail` in a single transaction, returning the
saved `diagram_entries.id`. Calling the two separately risked leaving a new entry
row with no matching detail row if the process died in between (observed on the
real catalogue); `save_design` is the atomic replacement for that pattern, and the
desktop app's three catalogue writers (Save's write-back, the mirror sync and
the `.asc` import) all use it.

`save_diagram_entry` is a `url`-keyed upsert: a different design saved under a taken
`url` lands on the owner's row and renames it, and the detail save then replaces that
row's angle table and attachments. A caller that must not do that checks
`diagram_entry_for_url` first and declines to write.

## Mirror-state semantics: "local delete wins"

`library_mirror_state` is keyed by `url`, not `entry_id`, and has no `FOREIGN KEY`
back to `diagram_entries`. `Database::delete_diagram_entry` deliberately keeps a
deleted design's `library_mirror_state` row and sets its `deleted_locally` flag (a
tombstone) in the same transaction as the delete: if a later sync saw no mirror-state
row at all for that url, it would treat the design as never seen before and
re-download it, silently undoing the deletion. The tombstone, not the stored hashes,
is what holds the deletion — a mirror pass skips every tombstoned url whatever the
remote hashes are, so a later change to the remote design cannot bring it back. A
database that predates the column has its already-orphaned rows tombstoned by the
migration. The cost is a permanently orphaned mirror-state row (no `diagram_entries`
row will ever match its `url` again unless the exact design is re-imported) —
invisible to a sync UI unless it asks, via
`Database::count_mirror_states_without_entry() -> Result<u64>`.

A stored hash that is not exactly 32 bytes makes `get_mirror_state` return an error
rather than a zero-padded state that could compare equal to a real hash.

## No network code

This crate has none — no HTTP client, no `Read`/`Write` over a socket. Grepping the
source for anything network-shaped turns up only `url`-shaped string data that gets
stored, never fetched: `diagram_entries.url` exists as a dedup key and provenance
field, not as something this crate ever dereferences.

## Testing

```
cargo test -p indicatrix-vault
```

No `tests/` directory — everything is inline `#[cfg(test)]` coverage, concentrated
in `db/sqlite/` (schema creation, every migration's correctness *and* idempotency
across two opens, using hand-seeded "pre-migration" fixture schemas; entry/detail
save-and-search round trips; custom-material CRUD; `update_diagram_metadata` proven, column by column, to leave every field
outside `MetadataUpdate` — including every field `FullDiagramRecord` can't even see —
byte-for-byte unchanged), plus `local/` (`import_asc`/`reconstruct_asc_schedule`), and `model/facets.rs` (parsing a packed `"55+6"`-style
facet-count string). Every test opens its own temporary SQLite file — none of them
touch the real `facet_diagrams.sqlite` working database that lives at the
workspace root.
