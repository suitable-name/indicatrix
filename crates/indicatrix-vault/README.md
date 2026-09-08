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
there is no system SQLite dependency). `Database` (`db::sqlite::Database`) wraps one
`rusqlite::Connection`, opened with `PRAGMA foreign_keys = ON`. Six tables:

| Table | Purpose |
|---|---|
| `diagram_entries` | One row per design: `title`, `url` (**unique** — the dedup key within one source), `design_id`, `source_id` |
| `diagram_details` | One row per entry (1:1, cascade-deleted with it): shape, refractive index, gear, ratios, volume, facet counts, symmetry, thumbnail image, PDF/GEM attachment names, etc. |
| `angle_settings` | The cutting-schedule rows for one detail: facet, angle, index, notes — ordered |
| `attached_files` | Raw file bytes attached to one detail (an original `.asc`, an image, a PDF) |
| `custom_gem_materials` | User-defined materials (name, RI, dispersion, birefringence, absorption) |
| `shape_vocabulary` | The canonical shape picker list (`name`, `sort_order`) — seeded from `DEFAULT_SHAPES`, see below |

## Migrations

There is no separate schema-version table and no external migration framework.
`Database::new` runs a fixed, ordered sequence of private migration methods on
every open (`create_tables_if_not_exist`, then `migrate_numeric_columns`,
`migrate_source_id_column`, `migrate_proportions_columns`,
`migrate_designer_and_attachment_columns`, `migrate_crystal_optics_columns`,
`migrate_shape_vocabulary`). Most are self-gating: they check `PRAGMA table_info`
for a column that would already exist if they had already run, and return
immediately if so. On a brand-new database, `create_tables_if_not_exist` already
creates every column (and table) the later migrations would add, so each of them
is a no-op the very first time; on an older database file, each migration actually
runs its `ALTER TABLE`/backfill exactly once. The more involved migrations
(retyping a `TEXT` column to `REAL`/`INTEGER`, splitting a packed `"55+6"`-style
facet-count string into separate columns) run inside one transaction each, so a
crash partway through rolls back cleanly rather than leaving the schema
half-migrated. `migrate_shape_vocabulary` is the one migration that is *not*
column-gated — see "Shape vocabulary" below for why and how it stays idempotent
instead.

**Adding a new migration**: write a new `migrate_*` method following the same
"check a representative new column via `column_exists`, then `ALTER TABLE`"
pattern, and call it at the end of `Database::new`'s migration chain (order
matters if a later migration depends on an earlier one's columns existing).
`migrate_shape_vocabulary` (below) is the one exception to the `column_exists`
gate — a data-seeding migration rather than a schema-altering one, so it's
idempotent by `CREATE TABLE IF NOT EXISTS` + `INSERT OR IGNORE` instead.

### Shape vocabulary

```rust
pub const DEFAULT_SHAPES: &[&str] = &[
    "Round", "Oval", "Cushion", "Square", "Rectangle", "Emerald", "Pear",
    "Marquise", "Heart", "Triangle", "Trillion", "Hexagon", "Octagon",
    "Pentagon", "Kite", "Rhombus", "Shield", "Star", "Barion", "Briolette",
    "Freeform",
];
```

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
    pub entry: FacetDiagramEntry,
    pub detail: FacetDiagramDetail,
}

pub fn import_asc(file_name: &str, content: &str) -> Result<ImportedAsc, String>;

pub fn reconstruct_asc_schedule(
    title: &str,
    refractive_index: Option<&str>,
    index_gear: Option<&str>,
    angle_settings: &[AngleSetting],
) -> Option<AscSchedule>;
```

`import_asc` parses raw `.asc` text via `indicatrix_formats::asc::parse_asc`, derives a title
from the file's first `H` header line (falling back to the filename with `.asc`
stripped), and synthesizes `url: "local://{file_name}"` — this is what the
`diagram_entries.url` uniqueness constraint dedupes a repeat import of the same
file against. The parsed tiers become `angle_settings` rows, and the raw file
bytes are kept as an `attached_files` entry so the original can always be
re-exported byte-for-byte later.

`reconstruct_asc_schedule` is the inverse: for a design that has an
`angle_settings` table but no attached original `.asc` file, it rebuilds an
`AscSchedule` from those stored rows alone. Mast (depth) distances are left at
`0.0` — that
information genuinely does not exist anywhere except a real `.asc` file — and the
result always gets `indicatrix_formats::asc::mark_reconstructed` called on it before being
returned, so a reconstructed export can never be mistaken for original,
mast-accurate data. Returns `None` if there are no angle-settings rows to work
with.

## Querying and filtering

```rust
pub struct RangeFilter {
    pub ri_min: Option<f64>, pub ri_max: Option<f64>,
    pub lw_min: Option<f64>, pub lw_max: Option<f64>,
    pub volume_min: Option<f64>, pub volume_max: Option<f64>,
    pub facets_min: Option<i64>, pub facets_max: Option<i64>,
}
```

`Database::search_diagrams(query, shape_filter, gear_filter, range)` builds one
dynamic, parameterized SQL query: free-text `LIKE` match against title/designer/
design-id, optional exact shape/gear equality, and up to eight optional numeric
range bounds — all filtering happens in that one query, capped at 1000 rows,
nothing is filtered back in application code. A caller that needs every matching
row, not just the first page — e.g. `apps/indicatrix-cut`'s `bridge::library_mirror` —
uses `Database::search_diagrams_page(.., after_id, limit)` instead: the same
filters plus a keyset cursor (`id > after_id`, over `diagram_entries.id`'s unique,
strictly-increasing `INTEGER PRIMARY KEY AUTOINCREMENT`), walked page by page until
a short page signals the end. `search_diagrams` is exactly
`search_diagrams_page(.., None, 1000)` — one query-building path, so the two can
never disagree. `Database::get_attribute_ranges()`
computes each numeric column's real minimum alongside a **99th-percentile** (not
the raw maximum) as the usable upper bound, so a single outlier row can't compress
a UI slider's whole usable range — it logs a warning if the raw maximum is more
than 5x the derived bound, since that's a sign worth investigating rather than
silently absorbing.

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
`FacetDiagramDetail` — it has no `hw_ratio`/`tw_ratio`/`uw_ratio`/`pw_ratio`/`cw_ratio`/
`symmetry_order`/`mirror_symmetry`/`designer`/`source_citation`/`pdf_file`/`gem_file`/
`shape_category` fields at all. `save_diagram_detail` fully *replaces* a design's detail
row (delete, then reinsert, including every `angle_settings`/`attached_files` child
row), so building a fresh `FacetDiagramDetail` from a `FullDiagramRecord` and saving it
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
none of this trap to begin with — it isn't part of `MetadataUpdate`.

## Deduplication (`model::dedup`)

`diagram_entries.url` being unique only prevents a re-imported row from duplicating
itself *within one `source_id`*. The schema tracks `source_id` per entry precisely
because more than one source can name the same physical design differently — each
gets its own URL and would otherwise live as two permanent, unrelated rows.

```rust
pub fn normalize_for_dedup(s: &str) -> String; // lowercase, trim, collapse whitespace
```

`Database::find_cross_source_duplicates` uses this normalization to surface
candidate duplicates (same normalized title + designer, comparable facet count)
across different `source_id`s — for a caller to review, never to auto-merge:
two different designers really do sometimes publish same-named designs, and
silently collapsing those would destroy data.

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
in `db/sqlite.rs` (schema creation, every migration's correctness *and* idempotency
across two opens, using hand-seeded "pre-migration" fixture schemas; entry/detail
save-and-search round trips; custom-material CRUD; cross-source duplicate
detection; `update_diagram_metadata` proven, column by column, to leave every field
outside `MetadataUpdate` — including every field `FullDiagramRecord` can't even see —
byte-for-byte unchanged), plus `local.rs` (`import_asc`/`reconstruct_asc_schedule`), `dedup.rs`
(`normalize_for_dedup`), and `facets.rs` (parsing a packed `"55+6"`-style
facet-count string). Every test opens its own temporary SQLite file — none of them
touch the real `facet_diagrams.sqlite` working database that lives at the
workspace root.
