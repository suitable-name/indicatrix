//! `indicatrix-cut-core` is the editor-core library for a faceting-design editor: it owns the
//! preform, the editable cutting instructions, undo/redo, and live-solid validation --
//! everything a UI needs to answer "what does this schedule look like right now,
//! and is it a real stone" without itself touching a window, a database, or a
//! renderer.
//!
//! # Why this is a separate crate from the GUI
//!
//! Undo/redo is business logic, not view logic: it has to be correct independent of
//! whatever widget toolkit drives it, and testable without a window (see [`edit`]'s
//! module doc comment). Two dependencies only -- [`indicatrix`] for geometry and
//! [`indicatrix-formats`] for `.asc` read/write and the `.indicatrix` design file's own
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
//! - [`native`]: converts a [`Design`] to and from the self-contained `.indicatrix`
//!   design file (design state `.asc` has no field for at all), and still reads the
//!   older `.indicatrix.toml` sidecar (legacy `.gemcut.toml` too) paired with a `.asc`. The on-disk document itself --
//!   schema, TOML encode/decode, path rules, fingerprint -- lives in
//!   `indicatrix_formats::native`; this module owns only the conversions to and from
//!   [`Design`] and the `load_paired`/`save_paired` pairing built on top of them.
//! - [`optimize`]: the objective ([`optimize::evaluate_objective`]) and
//!   [`optimize::optimize_design`], a deterministic coordinate search over a
//!   design's free facet angles that never moves a pinned tier, never proposes
//!   geometry that fails to close, and never regresses a manufacturability warning.
//! - [`cutting_sheet`]: [`design::Design::cutting_sheet`] builds a
//!   [`cutting_sheet::CuttingSheet`] (every tier in cutting order, with angle,
//!   indices, solved mast and a printable meet instruction) from a design and
//!   its already-solved masts; [`design::Design::facet_meets`] and
//!   [`cutting_sheet::diff_tiers`] are the companion per-facet and
//!   before/after reads built alongside it.
//!
//! # Determinism
//!
//! Every public operation here is a plain, total-order computation over the
//! caller-supplied schedule/preform -- no `HashMap`/`HashSet` in any decision path, no
//! wall-clock or thread-count dependence: two runs over identical input produce
//! byte-identical output. [`optimize::optimize_design`] is the one place this crate
//! takes a seed at all -- identical seed, identical result, always caller-supplied,
//! never derived from wall-clock time.

/// [`design::Design::cutting_sheet`]'s own output type.
///
/// [`cutting_sheet::CuttingSheet`]: every tier in cutting order, with angle,
/// indices, solved mast and a printable meet instruction) plus the
/// before/after diff over it ([`cutting_sheet::diff_tiers`]).
pub mod cutting_sheet;
/// A design's preform plus editable cutting instructions.
///
/// The derivation from authored constraints down to a renderable solid --
/// see this module's own doc comment ("Constraints, not masts"/ "Scale
/// anchoring").
pub mod design;
/// Reversible edits over a [`design::Design`].
///
/// [`edit::Edit`]/[`edit::History`]: the command/inverse-pair undo/redo
/// stack built on them.
pub mod edit;
/// Warnings-only checks over an already-solved [`design::Design`].
///
/// Vanishing/undersized facets, gear-quantization error, out-of-order meets.
pub mod manufacturability;
/// A design's material choice and the specific-gravity lookup table.
///
/// [`material::MaterialSelection`] and the built-in refractive-index/specific-
/// gravity lookup tables a design resolves its optics against.
pub mod material;
/// The `.indicatrix` design file's conversions, plus the older `.asc` + `.indicatrix.toml`
/// sidecar pairing, which stays readable.
///
/// See this module's own doc comment for the split with
/// `indicatrix_formats::native`.
pub mod native;
/// Pure critical-angle math shared by several consumers.
///
/// The material-retarget path and the solid preview's risk overlay.
pub mod optics_hints;
/// A deterministic coordinate search over a design's free facet angles.
///
/// The objective ([`optimize::evaluate_objective`]) and
/// [`optimize::optimize_design`] itself.
pub mod optimize;
/// Derives a tier's symmetry-generated facet groups from schedule metadata.
///
/// The `Design::*_orbit_member` helpers keep membership changes
/// orbit-consistent by construction.
pub mod orbit;
/// The physical rough a design's facets are cut from.
///
/// Always a closed plane set, so a brand-new design already renders as a
/// real solid.
pub mod preform;
/// Shape/material classification for the proportion-verdict windows.
///
/// The windows a solved design's measurements are judged against.
pub mod proportions_windows;
/// Re-solves only the tiers an edit could actually change.
///
/// [`resolve::resolve_after_edit`], instead of
/// [`design::Design::solve`]'s whole-design re-solve.
pub mod resolve;
/// Plans how many stones of which designs fit one rough block.
///
/// [`rough_plan::plan_rough`]: staged guillotine cuts (six cut orders), a
/// mixed-design DP over a shared piece table, single-design layouts and a
/// continuous refinement of the cut positions, ranked by finished volume.
pub mod rough_plan;
/// The built-in "New Design" template gallery.
///
/// See this module's own doc comment for the `ScaleReference`-only
/// restriction every entry follows.
pub mod templates;
/// The real-unit binding and the figures built on it.
///
/// [`yield_metrics::mm_per_unit`]: exact volumetric yield, an estimated
/// carat weight, and the design-bigger-than-its-rough check.
pub mod yield_metrics;

pub use cutting_sheet::{CutSheetRow, CuttingSheet, TierDelta, diff_tiers};
pub use design::{
    ConstraintTier, Design, DesignSolveError, FreshDesignSpec, MissingAnchor, ScheduleMeta,
    SolveMismatch, TargetResolveError, TierId, TierTarget,
};
pub use edit::{Edit, EditError, History, RemapRounding, remap_ratio};
pub use manufacturability::{
    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, ManufacturabilityWarning, check_manufacturability,
    degenerate_suspects,
};
pub use material::{
    BuiltinMaterials, MaterialCatalogue, MaterialEntry, MaterialKind, MaterialLookup,
    MaterialSelection, ResolvedMaterial, SpecificGravity, built_in_refractive_index,
    built_in_specific_gravity,
};
pub use native::{
    FingerprintCheck, LoadPairedError, LoadPairedResult, NativeDesignFile, PairedSave, SaveError,
    TierOverlay, asc_path_for_native, check_fingerprint, load_paired, native_path_for_asc,
    save_paired,
};
pub use optics_hints::{
    Risk, critical_angle_deg, crown_window_margin_deg, crown_windowing_risk, retarget_angle_deg,
    tier_margin_deg, windowing_risk,
};
pub use optimize::{
    AngleChange, CANONICAL_LIGHTING_PRESET, ObjectiveComponents, ObjectiveFidelity,
    ObjectiveWeights, OptimizeConfig, OptimizeOutcome, SearchHooks, apply_optimize_outcome,
    evaluate_objective, evaluate_objective_under, free_tier_indices, optimize_design,
};
pub use orbit::{OrbitUnit, expected_orbit, mirror_indices, orbit_units, rotate_indices};
pub use preform::{PreformShape, PreformSpec};
pub use proportions_windows::{MaterialClass, Metric as ProportionMetric, ShapeClass, Verdict};
pub use resolve::resolve_after_edit;
pub use yield_metrics::{
    PreformFit, StoneProportions, YieldReport, carat_weight, exceeds_preform, mm_per_unit,
    volume_mm3, volumetric_yield,
};
