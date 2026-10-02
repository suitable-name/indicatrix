# indicatrix-formats

Readers and writers for gemstone faceting design file formats.

`indicatrix-formats` is an independent,
unaffiliated implementation of file formats originating with `GemCAD` (Robert
Strickland's faceting-design software) and Gem Cut Studio. It is not
produced, endorsed, or affiliated with either program or their authors. `asc`,
`gcs`, and `gem` have **zero runtime dependencies** between them — only `native`
(this crate's own `.indicatrix` design file format, plus the older `.indicatrix.toml`
sidecar it replaced) pulls in `serde`/`toml`/`sha2`.
A file-format reader should not force a dependency tree onto callers who just want
to parse text, and keeping it that way means every downstream crate that touches
`.asc`/`.gcs`/`.gem` files (`indicatrix`, `indicatrix-vault`, `apps/indicatrix-cut`)
pays nothing extra for it.

> **Note on this document:** `indicatrix-formats`'s internals (`src/asc/`) are under
> active development. This README describes the format's semantics and the
> crate's public API shape at a conceptual level, verified against the source at
> time of writing — treat exact struct layouts as the current state, not a frozen
> contract, and re-check `src/asc/` itself for anything load-bearing.

## Formats

- **`indicatrix_formats::asc`** — `GemCAD`'s `.asc` cutting-instructions text format. Read and
  write support, verified against a real-world corpus of 5,759 `.asc` files across
  2,881 distinct designs.
- **`indicatrix_formats::gcs`** — Gem Cut Studio's `.gcs` XML design format, following
  the format Gem Cut Studio publishes (User's Manual v1.1.0, pp. 58-61). Reads all 59
  corpus files (`parse_gcs_bytes` handles the Windows-1252 one), including the
  spec's optional attributes, XML comments and declarations. `gcs_to_asc_schedule`
  converts a design to cutting instructions; `to_gcs_string` is an **experimental**
  writer (untested in Gem Cut Studio itself; chiral designs are the known risk). The
  module doc records the measured conventions: GCS's normalised frame
  (`max(|x|,|y|) = 1`, z-range centred), `depth` as the mast in that frame, and the
  side-dependent `index_angle` winding (crown `90° + phi`, pavilion/girdle
  `270° - phi`).
- **`indicatrix_formats::gem`** — `GemCAD`'s native `.gem` binary save format. A
  structural decoder: facet plane vectors (`p·x = 1`), tier numbers, `name\tinstructions`
  labels, facet polygons, the trailer (symmetry, mirror, signed gear, RI, gear offset,
  headings, footnotes) and the optional `preform` section. It frames all 254 corpus
  files byte-exactly, and `gem_to_asc_schedule` reproduces three exact `.asc` exports
  (including a chiral negative-gear design) to 6 decimals. Still open: the constant
  trailer field `0x7FFF`, and the hypothesis that the 30 files with vertices at
  `p·v = 0.81` were exported by Gem Cut Studio.

Each format lives in its own module (`indicatrix_formats::asc`, `indicatrix_formats::gcs`,
`indicatrix_formats::gem`) so that reading or writing a design never requires pulling in a
particular renderer, database, or GUI toolkit. Anything genuinely shared across more
than one format's module belongs at the crate root: the Windows-1252 decoding that
`.gem` and `.gcs` share lives in a private `encoding` module.

## Quick start

```rust
use indicatrix_formats::asc::{parse_asc, parse_asc_bytes, to_asc_string};

// Raw bytes, not read_to_string: many real files are Windows-1252, not UTF-8.
let bytes = std::fs::read("design.asc")?;
let schedule = parse_asc_bytes(&bytes)?; // Err(AscParseError) on a malformed file

println!("{schedule}"); // AscSchedule(gear=96, order=6, mirror=true, RI=1.72, tiers=57)
for tier in &schedule.tiers {
    println!("{:>8.3} deg  mast {:>10.6}  {:?}  indices {:?}",
        tier.angle_deg, tier.mast, tier.names(), tier.indices);
}

// Round-trips semantically, not byte-for-byte:
let regenerated = to_asc_string(&schedule)?; // Err(AscWriteError) on an embedded newline
assert_eq!(parse_asc(&regenerated)?, schedule);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## The `.asc` format

```text
GemCad 5.0
g 96 0.0                                       <- gear teeth, reference angle
y 6 y                                           <- symmetry order, mirror flag (y/n)
I 1.72                                          <- refractive index
H PC 45.149  Round Trichecker-12                <- header/title lines (repeatable)
H by Fred W. Van Sant, X 51, Extra Designs 2000
a -41.000000 0.64991234 92 n 1 84 76 68 60 ...  <- tier: angle, mast, indices/name
F "For small stones"                            <- footnote (repeatable)
```

Each `a` record is one facet tier:

- **angle** — signed degrees from the girdle plane. `GemCAD`'s own convention is
  negative = pavilion, positive = crown. A zero angle is the table, unless the file
  marks it as the culet — with a negative distance (`a 0.00 -0.368 0`, the form the
  `GemCAD` manual documents) or a `-0.000000` angle token. `parse_asc` stores a culet
  as a sign-negative zero angle with a positive mast, so the side never depends on
  file order; `to_asc_string` writes it back as `a -0 -<mast> <index>`.
- **mast** — how far the facet plane sits from the stone's center. This is the
  field the crate exists to extract reliably: a design's angle/index metadata is
  sometimes available elsewhere (e.g. scraped from a catalog site) *without* the
  depth, and `.asc` is the only place that depth actually lives. Stored as
  `GemCAD` wrote it, except that a culet's negative distance moves onto the angle's
  zero (see above); callers turning a mast into a plane offset should take its
  magnitude.
- **gear / index** — the index-wheel tooth count (`AscSchedule::gear_teeth`,
  occasionally negative as an internal handedness convention —
  `gear_teeth_abs()` gives the unsigned magnitude used for azimuth math) and each
  tier's index-wheel position(s) (`AscTier::indices`, usually integers, occasionally
  fractional).
- **`n <name>`** — an optional facet name. Facet names in real files are sometimes
  themselves plain numbers, so a token cannot be classified as an index vs. a name
  by whether it parses as a number — only the token immediately following an `n`
  marker is unconditionally treated as a name. A name belongs to the index written
  just before it (`AscTier::index_names` keeps each position, and the writer puts
  it back there); `AscTier::name` is the tier's folded label. Real corpus names look like `P1`,
  `C7`, `G1`, `1`, `U` — the `P`/`C`/`G` prefixes are a common informal
  Pavilion/Crown/Girdle convention in how designers *name* facets, but `indicatrix-formats`
  itself stores names as opaque strings and does not classify them; that
  interpretation, where it matters, lives downstream (see below). A name with
  embedded whitespace (e.g. `"Crown Main"`) is never rejected on write:
  `to_asc_string` sanitises it via `asc_safe_tier_name` into a single
  whitespace-free token (`"Crown_Main"`) before writing the ` n` marker, since a
  name with embedded whitespace would otherwise split into extra tokens on
  re-parse. The true, human-typed name is not lost — it still lives in the
  `.indicatrix` design file (`indicatrix_formats::native::design`), which restores it
  on load; only a plain
  `.asc` export ever shows the sanitised form. `is_asc_safe_tier_name` checks,
  without allocating, whether a given name would survive unchanged.
- **`G <notes...>`** — an optional meet instruction, GemCAD's way of describing how
  a tier's mast distance was actually determined (e.g. "cut until this facet meets
  a named vertex," rather than a fixed depth). `AscTier::meet_instruction()` parses
  the raw `notes` text on demand into a `MeetInstruction`:

  ```rust
  pub enum MeetInstruction {
      Meet(Vec<String>),   // "Meet P1, P2, G1" -- meet the named facet(s)
      CutToCenterpoint,    // "Cut to centerpoint" / "TCP" / "PCP"
      GirdleMeetPoint,     // "GMP" / "Girdle meet point"
      LevelGirdle,         // "Level girdle."
      ScaleReference,      // "Set girdle width" / "Set stone size" / ...
      Other(String),       // anything unrecognized, kept verbatim
  }
  ```

  This is case-insensitive keyword matching over free text, not a strict grammar,
  since these are hand-typed designer notes. `Meet(names)` parsing tolerates both
  comma- and whitespace-separated lists and drops lowercase connector words
  (`"and"`, `"the"`, `"or"`) while staying case-sensitive so single-letter facet
  names like `a`/`b`/`c` are never filtered out.

  `indicatrix-formats` only extracts this text-level structure — a list of referenced name
  strings, or a fixed variant. Actually *resolving* those names against a design's
  own tier list (including stripping a `P`/`C` side prefix, or falling back to a
  design's one identified girdle tier) is a geometric problem solved downstream, in
  `indicatrix::geometry::meet_solver`.

### Corpus realities the parser absorbs

Built and verified against 5,759 real `.asc` files across 2,881 distinct designs.
Its lenient handling exists specifically because a naive reading of the format
sketch above misses real cases:

- Long index lists wrap onto continuation lines that don't start with `a`.
- A tier's indices can be split across more than one `n <name>` group on the same
  logical record; these fold into one tier rather than becoming duplicates.
- Index-wheel positions are occasionally fractional.
- The gear-teeth count is occasionally negative.
- At least one real file is missing its `g` keyword entirely.
- A rare tier carries a negative mast value on an otherwise all-positive-mast file.

Free text after the last facet tier that wasn't folded in as a continuation, a
stray index-position token that couldn't be classified as a number/name/marker,
and a duplicate `g` line whose value overrode an earlier one are all non-fatal —
`parse_asc` collects them into `AscSchedule::warnings` instead of failing the
whole parse (empty for a clean file). `parse_asc` still hard-rejects a file
outright for a value that would be actively dangerous downstream: a non-finite
(`NaN`/infinite) angle, mast, index position, reference angle, or refractive
index (`AscParseError::NonFiniteValue`); a symmetry order of exactly `0`
(`SymmetryOrderZero`); a gear-teeth count of exactly `0` (`GearTeethZero`); or a
refractive index not greater than `1.0` (`RefractiveIndexOutOfRange`) — as well
as missing/malformed required header lines and unparseable numeric tokens.

## Public API

```rust
pub struct AscTier {
    pub angle_deg: f64,
    pub mast: f64,
    pub name: String,       // '/'-joined when more than one name shares a tier
    pub indices: Vec<f64>,
    pub index_names: Vec<(usize, String)>, // (position in `indices`, name) per `n` group
    pub notes: String,      // raw text after a 'G' marker
}
impl AscTier {
    pub fn names(&self) -> Vec<&str>;                        // splits `name` on '/'
    pub const fn is_culet(&self) -> bool;                    // sign-negative zero angle
    pub fn meet_instruction(&self) -> Option<MeetInstruction>; // parses `notes`
}

pub struct AscSchedule {
    pub gemcad_version: String,
    pub gear_teeth: i32,            // sometimes negative (handedness)
    pub gear_reference_angle: f64,
    pub symmetry_order: u32,
    pub mirror: bool,
    pub refractive_index: f64,
    pub headers: Vec<String>,       // 'H' lines, leading 'H' stripped
    pub footnotes: Vec<String>,     // 'F' lines, leading 'F' stripped
    pub tiers: Vec<AscTier>,
    pub warnings: Vec<String>,      // non-fatal lenient-parse diagnostics; empty for a clean file
    pub line_ending: AscLineEnding, // Lf or CrLf, as read; CrLf (GemCAD's own) by default
}
impl AscSchedule {
    pub const fn gear_teeth_abs(&self) -> u32;
    pub fn facet_plane_count(&self) -> usize; // sum of tiers' index counts, min 1 each
}

pub fn parse_asc(content: &str) -> Result<AscSchedule, AscParseError>;
pub fn parse_asc_bytes(bytes: &[u8]) -> Result<AscSchedule, AscParseError>;
pub fn decode_asc_bytes(bytes: &[u8]) -> std::borrow::Cow<'_, str>; // BOM, UTF-8 or Windows-1252, lone CR
pub fn to_asc_string(schedule: &AscSchedule) -> Result<String, AscWriteError>;
pub fn mark_reconstructed(schedule: &mut AscSchedule, note: &str);
pub fn is_asc_safe_tier_name(name: &str) -> bool;
pub fn asc_safe_tier_name(name: &str) -> std::borrow::Cow<'_, str>;
```

`to_asc_string` is **not byte-identical** to hand-authored `GemCAD` output —
whitespace and numeric formatting are normalized, and a culet is always written
as `a -0 -<mast> <index>` — but it round-trips *semantically*:
`parse_asc(&to_asc_string(s)?)` reproduces a schedule equal to `s`. Numeric fields
lose no precision, since Rust's default `f64`/`i32` `Display` formatting is
guaranteed to round-trip exactly. `to_asc_string` returns `Err(AscWriteError)`
only for a header, footnote, or tier-notes string containing a newline (which
would otherwise split across physical lines and be misread on re-parse); an
unsafe tier *name* is never an error — it is sanitised via `asc_safe_tier_name`
instead (see the `n <name>` bullet above).

`mark_reconstructed(schedule, note)` prepends a `"RECONSTRUCTED -- mast distances
are solved, not original -- {note}"` header (idempotent — calling it twice doesn't
double the marker). Use this whenever a schedule's mast values were *computed*
(from meet constraints, or left at a placeholder because no original depth data
was available) rather than read literally from a real `.asc` file — a reconstructed
schedule must never be mistaken for hand-authored, verified cutting instructions
before someone cuts a stone from it. Two real call sites: `indicatrix::geometry::meet_solver`,
after solving mast distances geometrically, and `indicatrix_vault::local::reconstruct_asc_schedule`,
when rebuilding a schedule from a saved angle/index table that never had mast data
in the first place.

### `.gem` and `.gcs`

```rust
// indicatrix_formats::gem
pub fn parse_gem(content: &[u8]) -> Result<GemDesign, GemParseError>; // typed framing errors with byte offsets
pub fn gem_to_asc_schedule(design: &GemDesign) -> AscSchedule;
pub struct GemDesign { pub facets: Vec<GemFacet>, pub symmetry: i32, pub mirror: bool,
    pub gear: i32, pub refractive_index: f64, pub gear_offset: f64, pub unknown_7fff: u32,
    pub headings: [String; 4], pub footnotes: [String; 4],
    pub preform: Option<Box<GemDesign>>, pub vertex_scale: f64 }
pub struct GemFacet { pub plane: [f64; 3], pub tier: i32, pub name: Option<String>,
    pub instructions: String, pub vertices: Vec<[f64; 3]> }
// GemFacet::{normal, distance, angle_deg, is_flat, index(gear, offset), vertices_on_plane(scale)}
// GemDesign::{notes, title, facet_index}

// indicatrix_formats::gcs
pub fn parse_gcs(content: &str) -> Result<GcsDesign, GcsParseError>;
pub fn parse_gcs_bytes(bytes: &[u8]) -> Result<GcsDesign, GcsParseError>; // BOM, UTF-8 or Windows-1252
pub fn gcs_to_asc_schedule(design: &GcsDesign) -> AscSchedule;
pub fn to_gcs_string(schedule: &AscSchedule) -> Result<String, GcsWriteError>; // experimental
pub fn side_rule_index_angle(normal: [f64; 3]) -> Option<f64>;
pub fn normal_from_index_angle(angle_deg: f64, index_angle_deg: f64) -> [f64; 3];
// GcsFacet::{index(gear), is_frosted}, GcsTier::{to_signed_asc_angle, is_frosted}
```

## Real usage elsewhere in the workspace

**Importing a user's own `.asc` file into the local catalog**
(`crates/indicatrix-vault/src/local/`):

```rust
use indicatrix_formats::asc::{self, AscSchedule, AscTier};

pub fn import_asc(file_name: &str, content: &str) -> Result<ImportedAsc, String> {
    let schedule = asc::parse_asc(content)?;
    // ... build a FacetingDiagramEntry/FacetingDiagramDetail from schedule.tiers,
    // schedule.refractive_index, schedule.gear_teeth_abs(), etc.
}
```

**Exporting a stored diagram back out as `.asc`** (`apps/indicatrix-cut/src/gui/library/local/export.rs`):

```rust
let schedule = local::reconstruct_asc_schedule(
    &full.title,
    full.refractive_index.as_deref(),
    full.index_gear.as_deref(),
    &full.angle_settings,
).ok_or(/* ... */)?;
let text = indicatrix_formats::asc::to_asc_string(&schedule);
```

**Turning a schedule into real renderable geometry** (`crates/indicatrix/src/geometry/cuts/asc_schedule.rs`):

```rust
pub fn from_asc_schedule(schedule: &AscSchedule) -> Vec<GpuFacetPlane>
```

uses each tier's *real* `mast` value as a plane offset — this is the non-fabricated
path, as opposed to `StandardGemCuts::from_database_angles`, which only has
angle/index data and has to guess proportional offsets.

## What it does not do

`indicatrix-formats` only knows these text/binary formats. It has no opinion on 3D geometry,
rendering, or how a schedule's angle/mast fields become facet planes — that
conversion (sign conventions, plane offsets, and validation against a real B-Rep
solid) lives in `indicatrix`, which depends on this crate, not the other way around.

## Testing

```
cargo test -p indicatrix-formats
```

There is no `tests/` directory — every test lives inline in each format module's own
`#[cfg(test)] mod tests`, with fixtures embedded as string/byte constants (several are
verbatim excerpts of real corpus files, attributed by their `attached_files` row id
and filename). `src/asc/`'s coverage includes field-level parsing, continuation-line
handling, corpus-quirk tolerance (missing `g` keyword, fractional indices, multi-name
tiers), negative-input error messages, a no-panic-on-garbage smoke test, `MeetInstruction`
parsing against real note text, and round-trip equality (`parse_asc(&to_asc_string(parse_asc(x))) == parse_asc(x)`)
against five real fixture files spanning different gear counts, symmetry orders,
name-free designs, and negative-mast tiers. `src/gcs/`'s tests parse a real (trimmed)
`.gcs` excerpt and a spec-shaped example (comments, declaration, optional attributes),
check the side rule and the conversion to `.asc`, and round-trip the writer
(parse → write → parse gives equal tiers). `src/gem/`'s tests encode synthetic `.gem`
files with a test-only encoder and check the round trip, every framing error, varint
string lengths and the 0.81 vertex scale.

Corpus checks are `#[ignore]`d and read an exported corpus directory (`gem/`, `gcs/`,
`asc/` subfolders):

```
INDICATRIX_FORMAT_CORPUS_DIR=/path/to/corpus cargo test -p indicatrix-formats -- --ignored corpus
```

Broader, corpus-scale validation (the "5,759 files / 2,881 designs" figures, and
cross-checks of `.asc`-derived geometry against independently published
measurements) lives outside this crate, in
`crates/indicatrix/tests/optics_geometry_tests/`.
