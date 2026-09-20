# indicatrix-formats

Readers and writers for gemstone faceting design file formats.

`indicatrix-formats` is an independent,
unaffiliated implementation of file formats originating with `GemCAD` (Robert
Strickland's faceting-design software) and Gem Cut Studio. It is not
produced, endorsed, or affiliated with either program or their authors. `asc`,
`gcs`, and `gem` have **zero runtime dependencies** between them — only `native`
(this crate's own `.indicatrix.toml` sidecar format) pulls in `serde`/`toml`/`sha2`.
A file-format reader should not force a dependency tree onto callers who just want
to parse text, and keeping it that way means every downstream crate that touches
`.asc`/`.gcs`/`.gem` files (`indicatrix`, `indicatrix-vault`, `apps/indicatrix-cut`)
pays nothing extra for it.

> **Note on this document:** `indicatrix-formats`'s internals (`src/asc.rs`) are under
> active development. This README describes the format's semantics and the
> crate's public API shape at a conceptual level, verified against the source at
> time of writing — treat exact struct layouts as the current state, not a frozen
> contract, and re-check `src/asc.rs` itself for anything load-bearing.

## Formats

- **`indicatrix_formats::asc`** — `GemCAD`'s `.asc` cutting-schedule text format. Read and
  write support, verified against a real-world corpus of 5,759 `.asc` files across
  2,881 distinct designs.
- **`indicatrix_formats::gcs`** — Gem Cut Studio's `.gcs` XML design format. Read-only
  (nothing downstream produces `.gcs`, so there is no writer). Verified against 44 of
  56 real `.gcs` files with a sibling `.asc` for the same design; see the module's own
  doc comment for exactly what matches, what does not (`depth` is not `.asc`'s
  `mast`), and the 12 files that disagree and why.
- **`indicatrix_formats::gem`** — `GemCAD`'s native `.gem` binary save format.
  **Partial, by necessity**: this is an unpublished binary format, and only its
  embedded cutting/meet-instruction text and facet-name labels could be confirmed
  against real data (a Pascal-style length-prefixed ASCII string scan, cross-checked
  against a scraped `angle_settings` table and this crate's own catalog metadata).
  The numeric encoding of facet angle, index position, and depth could not be
  reverse-engineered despite a systematic search (raw degrees/radians, normal-vector
  `cos`/`sin`, and literal tooth-number integers, at multiple widths and both
  `f32`/`f64` precision) and is not guessed at or exposed. See the module's own doc
  comment for the full account of what is and is not established.

Each format lives in its own module (`indicatrix_formats::asc`, `indicatrix_formats::gcs`,
`indicatrix_formats::gem`) so that reading or writing a design never requires pulling in a
particular renderer, database, or GUI toolkit. Anything genuinely shared across more
than one format's module would belong at the crate root — nothing has met that bar
yet.

## Quick start

```rust
use indicatrix_formats::asc::{parse_asc, to_asc_string};

let text = std::fs::read_to_string("design.asc")?;
let schedule = parse_asc(&text)?;

println!("{schedule}"); // AscSchedule(gear=96, order=6, mirror=true, RI=1.72, tiers=57)
for tier in &schedule.tiers {
    println!("{:>8.3} deg  mast {:>10.6}  {:?}  indices {:?}",
        tier.angle_deg, tier.mast, tier.names(), tier.indices);
}

// Round-trips semantically, not byte-for-byte:
let regenerated = to_asc_string(&schedule);
assert_eq!(parse_asc(&regenerated)?, schedule);
# Ok::<(), String>(())
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
  negative = pavilion, non-negative = crown.
- **mast** — how far the facet plane sits from the stone's center. This is the
  field the crate exists to extract reliably: a design's angle/index metadata is
  sometimes available elsewhere (e.g. scraped from a catalog site) *without* the
  depth, and `.asc` is the only place that depth actually lives. Always stored as
  `GemCAD` wrote it; a rare tier carries a negative mast on an otherwise
  all-positive-mast file — callers turning a mast into a plane offset should take
  its magnitude, not assume the sign is meaningful.
- **gear / index** — the index-wheel tooth count (`AscSchedule::gear_teeth`,
  occasionally negative as an internal handedness convention —
  `gear_teeth_abs()` gives the unsigned magnitude used for azimuth math) and each
  tier's index-wheel position(s) (`AscTier::indices`, usually integers, occasionally
  fractional).
- **`n <name>`** — an optional facet name. Facet names in real files are sometimes
  themselves plain numbers, so a token cannot be classified as an index vs. a name
  by whether it parses as a number — only the token immediately following an `n`
  marker is unconditionally treated as a name. Real corpus names look like `P1`,
  `C7`, `G1`, `1`, `U` — the `P`/`C`/`G` prefixes are a common informal
  Pavilion/Crown/Girdle convention in how designers *name* facets, but `indicatrix-formats`
  itself stores names as opaque strings and does not classify them; that
  interpretation, where it matters, lives downstream (see below).
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

## Public API

```rust
pub struct AscTier {
    pub angle_deg: f64,
    pub mast: f64,
    pub name: String,       // '/'-joined when more than one name shares a tier
    pub indices: Vec<f64>,
    pub notes: String,      // raw text after a 'G' marker
}
impl AscTier {
    pub fn names(&self) -> Vec<&str>;                        // splits `name` on '/'
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
}
impl AscSchedule {
    pub const fn gear_teeth_abs(&self) -> u32;
    pub fn facet_plane_count(&self) -> usize; // sum of tiers' index counts, min 1 each
}

pub fn parse_asc(content: &str) -> Result<AscSchedule, String>;
pub fn to_asc_string(schedule: &AscSchedule) -> String;
pub fn mark_reconstructed(schedule: &mut AscSchedule, note: &str);
```

`to_asc_string` is **not byte-identical** to hand-authored `GemCAD` output —
whitespace, token order within a tier line, and how repeated `n <name>` groups
collapse back down are all normalized — but it round-trips *semantically*:
`parse_asc(&to_asc_string(s))` reproduces a schedule equal to `s`. Numeric fields
lose no precision, since Rust's default `f64`/`i32` `Display` formatting is
guaranteed to round-trip exactly.

`mark_reconstructed(schedule, note)` prepends a `"RECONSTRUCTED -- mast distances
are solved, not original -- {note}"` header (idempotent — calling it twice doesn't
double the marker). Use this whenever a schedule's mast values were *computed*
(from meet constraints, or left at a placeholder because no original depth data
was available) rather than read literally from a real `.asc` file — a reconstructed
schedule must never be mistaken for a hand-authored, verified cutting schedule
before someone cuts a stone from it. Two real call sites: `indicatrix::geometry::meet_solver`,
after solving mast distances geometrically, and `indicatrix_vault::local::reconstruct_asc_schedule`,
when rebuilding a schedule from a saved angle/index table that never had mast data
in the first place.

## Real usage elsewhere in the workspace

**Importing a user's own `.asc` file into the local catalog**
(`crates/indicatrix-vault/src/local.rs`):

```rust
use indicatrix_formats::asc::{self, AscSchedule, AscTier};

pub fn import_asc(file_name: &str, content: &str) -> Result<ImportedAsc, String> {
    let schedule = asc::parse_asc(content)?;
    // ... build a FacetDiagramEntry/FacetDiagramDetail from schedule.tiers,
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

**Turning a schedule into real renderable geometry** (`crates/indicatrix/src/geometry/cuts.rs`):

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
and filename). `src/asc.rs`'s coverage includes field-level parsing, continuation-line
handling, corpus-quirk tolerance (missing `g` keyword, fractional indices, multi-name
tiers), negative-input error messages, a no-panic-on-garbage smoke test, `MeetInstruction`
parsing against real note text, and round-trip equality (`parse_asc(&to_asc_string(parse_asc(x))) == parse_asc(x)`)
against five real fixture files spanning different gear counts, symmetry orders,
name-free designs, and negative-mast tiers. `src/gcs.rs`'s tests parse a real (trimmed)
`.gcs` excerpt and check its angle-convention conversion against its sibling `.asc`;
`src/gem.rs`'s tests parse real extracted byte slices from an actual `.gem` file and
check the recovered note text against that same design's own catalog metadata.

Broader, corpus-scale validation (the "5,759 files / 2,881 designs" figures, and
cross-checks of `.asc`-derived geometry against independently published
measurements) lives outside this crate, in
`crates/indicatrix/tests/optics_geometry_tests.rs`.
