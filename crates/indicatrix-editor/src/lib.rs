//! GUI-free faceting-design editor logic shared by the desktop editor
//! (`apps/indicatrix-cut`) and the wasm web app (`apps/indicatrix-web`), so both
//! edit, validate, format and export a design identically -- see this crate's
//! README for the full picture.
//!
//! - [`session`]: [`EditorSession`] -- the design, its undo/redo history (nudge
//!   coalescing on caller-supplied timestamps), the multi-selection and the
//!   generation/saved-generation pair behind "unsaved changes".
//! - [`loading`]: the tier, index-shorthand, preform, New Design and material-form
//!   parsers, and a design from bare `.asc` text.
//! - [`material`] / [`material_lookup`]: the material combo, RI-source text, gear
//!   presets and remap preview; the custom-over-built-in material lookup and the
//!   nearest-built-in search.
//! - [`view_model`]: plain-struct tier rows, index chips, cut-order rows,
//!   yield/proportion texts and verdicts, and the validation banner.
//! - [`optimize_view`]: Optimize's hint, result tables, preview design and weight
//!   parser.
//! - [`retarget`]: "Retarget for material" proposals and their view model.
//! - [`cut_sheet`]: the printable cutting-sheet HTML and diagram PNG bytes.
//! - [`guide`]: the guided walkthrough's steps and completion predicates.
//! - [`solve_policy`]: when to solve synchronously, auto-solve eligibility, banner
//!   texts, the cancellable solve and the plane-cap diagnosis.
//! - [`scratch`], [`stale`], [`edit_intent`], [`templates`]: form re-seed change
//!   detection, result staleness stamps, UI-intent coalescing, gallery cards.
//! - [`manipulate`]: mouse-driven direct manipulation -- the pick-frame projection, the
//!   angle/depth/index drag handles of the selected facet, the Slice tool's plane
//!   snapping, the tiers a drag drags along, and the hint and toast wording.
//! - [`files`]: file kinds by name and content, `.gem`/`.gcs` conversion to `.asc` text,
//!   older-sidecar pairing among several opened files, and the desktop's default save names.
//! - [`tier_save`]: which `Edit` a parsed tier form becomes (insert after the selection,
//!   modify in place, the depth / girdle-thickness / table-width target batch).
//! - [`printed_proportions`]: the design settings' five printed figures (Vol/W^3, L/W,
//!   C/W, P/W, H/W) from their text fields.
//! - [`snapshot`]: "Compare to snapshot"'s rows -- the tier-by-tier diff of a remembered
//!   design against the design now.
//! - [`raw_text`]: the design's cutting instructions as editable `.asc` text -- generating
//!   it, reading an edited copy back with line-numbered problems, merging it into the design
//!   as one undoable edit with a summary of what is lost, and a plain line diff.
//! - [`sweep`]: "Angle Sweep" -- one tier's angle over a range, every angle solved and
//!   scored, as rows, a chart and CSV text.
//! - [`cutting_mode`]: the cutting instructions as one page per step -- the steps in sheet
//!   order with a stable key and a values fingerprint each, the done marks and index ticks
//!   (a mark whose step changed since reads "changed", not "done"), the page texts and the
//!   index wheel with a step's indices marked.
//! - [`slider_ranges`]: the sliders beside the main number fields -- the angle ranges per side
//!   and material, the marked pavilion band over the critical angle, the "Typical" presets
//!   with their reasons, the Preform ranges, and the text a slider writes and reads back.
//! - [`metric_deltas`]: the compare window's optical figures of two stones, the table rows
//!   beside them and a few plain sentences about what changed, with named noise thresholds
//!   so a change lost in the measurement noise reads "about the same".
//! - [`verdict`]: the overall Good / Check / Problem verdict for the open design -- the
//!   reasons behind it with named thresholds, and a validated one-edit "Fix" for each
//!   reason where a safe tool exists (snap to the gear, remove cut-away facets, move a tier
//!   behind the one it meets, steepen a windowing pavilion about its girdle edge, add a
//!   table).
//!
//! No GUI toolkit types, no clock and no filesystem anywhere in this crate, and no
//! threads except the angle sweep's worker threads, which are compiled out of a
//! `wasm32-unknown-unknown` build (it runs the same loop inline there), so it links
//! unchanged into that build. Each UI maps
//! the plain structs to its own row types, and supplies its own clock and threads
//! (or Workers).

pub mod cut_sheet;
pub mod cutting_mode;
pub mod edit_intent;
pub mod files;
pub mod guide;
pub mod lch_color;
pub mod loading;
pub mod manipulate;
pub mod material;
pub mod material_lookup;
pub mod metric_deltas;
pub mod optimize_view;
pub mod printed_proportions;
pub mod raw_text;
pub mod retarget;
pub mod scratch;
pub mod session;
pub mod slider_ranges;
pub mod snapshot;
pub mod solve_policy;
pub mod stale;
pub mod sweep;
pub mod templates;
pub mod tier_save;
pub mod verdict;
pub mod view_model;

pub use manipulate::{
    DragStart, DragValue, FacetFrame, HandleKind, HandleLayout, ScreenPoint, ScreenSize, SliceSide,
    SnapMode, SnappedFacet,
};
pub use session::{
    EditChange, EditorSession, HistorySnapshot, JumpFailure, JumpOutcome, NudgeOutcome, PinOutcome,
};
