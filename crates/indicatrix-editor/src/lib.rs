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
//! - [`files`]: file kinds by name, `.gem`/`.gcs` conversion to `.asc` text, native
//!   pairing among several opened files, and the desktop's default save names.
//! - [`tier_save`]: which `Edit` a parsed tier form becomes (insert after the selection,
//!   modify in place, the depth / girdle-thickness / table-width target batch).
//! - [`printed_proportions`]: the design settings' five printed figures (Vol/W^3, L/W,
//!   C/W, P/W, H/W) from their text fields.
//! - [`snapshot`]: "Compare to snapshot"'s rows -- the tier-by-tier diff of a remembered
//!   design against the design now.
//!
//! No GUI toolkit types, no threads, no clock and no filesystem anywhere in this
//! crate, so it links unchanged into a `wasm32-unknown-unknown` build. Each UI maps
//! the plain structs to its own row types, and supplies its own clock and threads
//! (or Workers).

pub mod cut_sheet;
pub mod edit_intent;
pub mod files;
pub mod guide;
pub mod loading;
pub mod manipulate;
pub mod material;
pub mod material_lookup;
pub mod optimize_view;
pub mod printed_proportions;
pub mod retarget;
pub mod scratch;
pub mod session;
pub mod snapshot;
pub mod solve_policy;
pub mod stale;
pub mod templates;
pub mod tier_save;
pub mod view_model;

pub use manipulate::{
    DragStart, DragValue, FacetFrame, HandleKind, HandleLayout, ScreenPoint, ScreenSize, SliceSide,
    SnapMode, SnappedFacet,
};
pub use session::{EditChange, EditorSession, NudgeOutcome, PinOutcome};
