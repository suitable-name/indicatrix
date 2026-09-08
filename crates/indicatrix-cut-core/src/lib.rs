//! `indicatrix-cut-core` is the editor-core library for a faceting-design editor: it owns the
//! preform, the editable cutting schedule, undo/redo, and live-solid validation --
//! everything a UI needs to answer "what does this schedule look like right now,
//! and is it a real stone" without itself touching a window, a database, or a
//! renderer.
//!
//! # Why this is a separate crate from the GUI
//!
//! Undo/redo is business logic, not view logic: it has to be correct independent of
//! whatever widget toolkit drives it, and testable without a window (see [`edit`]'s
//! module doc comment). Two dependencies only -- [`indicatrix`] for geometry and
//! [`indicatrix-formats`] for `.asc` read/write and the native `.indicatrix.toml` sidecar's own
//! on-disk format -- so `apps/indicatrix-cut` can wire a UI on top of it without this
//! crate ever needing to know Slint, wgpu, or SQLite exist.
//!
//! # The pieces
//!
//! - [`preform`]: the physical rough a design's facets are cut from -- always a
//!   closed plane set, so a brand-new design already renders as a real solid.
//! - [`design`]: [`design::Design`] pairs a [`preform::PreformSpec`] with
//!   [`design::ConstraintTier`]s -- authored meet constraints, not recorded masts.
//!   `Design::solve` derives masts on demand and builds the full plane arrangement
//!   feeding `indicatrix::geometry::stone_metrics::build_solid_mesh`.
//! - [`edit`]: [`edit::Edit`] (add/remove/modify a tier or its constraint, or
//!   replace the preform) and [`edit::History`], a command/inverse-pair undo/redo
//!   stack over a [`Design`].
//! - [`resolve`]: [`resolve::resolve_after_edit`] re-solves only the tiers an edit
//!   could actually change, instead of [`Design::solve`]'s whole-design re-solve.
//! - [`orbit`]: [`orbit::orbit_units`] derives a tier's symmetry-generated facet
//!   groups from schedule metadata (never stored, always recomputed), and the
//!   `Design::*_orbit_member` helpers keep membership changes orbit-consistent by
//!   construction.
//! - [`manufacturability`]: four checks (vanishing facets, undersized facets,
//!   gear-quantization error, out-of-order meets) over an already-solved
//!   [`design::Design`] -- warnings only, never silently corrected.
//! - [`material`]: [`material::MaterialSelection`] and a specific-gravity lookup
//!   table; [`material::MaterialSelection::resolve`] and
//!   [`design::Design::effective_refractive_index`] give a design one real
//!   refractive index every consumer can derive from.
//! - [`optics_hints`]: pure critical-angle math shared by the material-retarget
//!   path and the solid preview's risk overlay.
//! - [`yield_metrics`]: the real-unit binding ([`yield_metrics::mm_per_unit`]) and
//!   the figures built on it -- exact volumetric yield, an estimated carat weight,
//!   and the design-bigger-than-its-rough check.
//! - [`native`]: pairs a real `.asc` export with (never replacing it) the native
//!   `.indicatrix.toml` sidecar (legacy `.gemcut.toml` files still load), carrying
//!   design state `.asc` has no field for at all. The on-disk document itself --
//!   schema, TOML encode/decode, path rules, fingerprint -- lives in
//!   `indicatrix_formats::native`; this module owns only the conversions to and from
//!   [`Design`] and the `load_paired`/`save_paired` pairing built on top of them.
//! - [`optimize`]: the objective ([`optimize::evaluate_objective`]) and
//!   [`optimize::optimize_design`], a deterministic coordinate search over a
//!   design's free facet angles that never moves a pinned tier, never proposes
//!   geometry that fails to close, and never regresses a manufacturability warning.
//!
//! # Determinism
//!
//! Every public operation here is a plain, total-order computation over the
//! caller-supplied schedule/preform -- no `HashMap`/`HashSet` in any decision path, no
//! wall-clock or thread-count dependence: two runs over identical input produce
//! byte-identical output. [`optimize::optimize_design`] is the one place this crate
//! takes a seed at all -- identical seed, identical result, always caller-supplied,
//! never derived from wall-clock time.

pub mod design;
pub mod edit;
pub mod manufacturability;
pub mod material;
pub mod native;
pub mod optics_hints;
pub mod optimize;
pub mod orbit;
pub mod preform;
pub mod resolve;
pub mod yield_metrics;

pub use design::{ConstraintTier, Design, FreshDesignSpec, MissingAnchor, ScheduleMeta};
pub use edit::{Edit, EditError, History, RemapRounding};
pub use manufacturability::{
    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, ManufacturabilityWarning, check_manufacturability,
};
pub use material::{
    BuiltinMaterials, MaterialLookup, MaterialSelection, ResolvedMaterial, SpecificGravity,
    built_in_refractive_index, built_in_specific_gravity,
};
pub use native::{
    FingerprintCheck, LoadPairedError, LoadPairedResult, NativeDesignFile, PairedSave, SaveError,
    TierOverlay, asc_path_for_native, check_fingerprint, load_paired, native_path_for_asc,
    save_paired,
};
pub use optics_hints::{
    Risk, critical_angle_deg, retarget_angle_deg, tier_margin_deg, windowing_risk,
};
pub use optimize::{
    AngleChange, ObjectiveComponents, ObjectiveFidelity, ObjectiveWeights, OptimizeConfig,
    OptimizeOutcome, SearchHooks, apply_optimize_outcome, evaluate_objective, free_tier_indices,
    optimize_design,
};
pub use orbit::{OrbitUnit, orbit_units};
pub use preform::{PreformShape, PreformSpec};
pub use resolve::resolve_after_edit;
pub use yield_metrics::{
    PreformFit, YieldReport, carat_weight, exceeds_preform, mm_per_unit, volume_mm3,
    volumetric_yield,
};
