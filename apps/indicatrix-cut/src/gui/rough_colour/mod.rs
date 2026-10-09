//! Rough colour and colour zoning, application side (`zoning` feature only).
//!
//! * [`store`]: THE storage API. Every function the Rough colour UI (lane U1) calls to save,
//!   load and delete a plan's rough colour, its cached photos, its pose choices, a material's
//!   zones and a render job's zones lives there, with typed arguments and one `anyhow::Result`
//!   each. The vault tables underneath are opaque text and bytes
//!   (`indicatrix_vault::db::sqlite::Database::save_rough_colour` and friends); the formats are
//!   `indicatrix_cut_core::rough_plan::zoned_plan`'s.
//!
//! * [`preview`], [`adopt_link`]: what the Rough Planner shows and does with a plan's colour (the
//!   coloured previews, the "Stone orientation" option, "Use colour");
//! * [`handles`], [`mesh_paint`], [`zone_link`]: the 3D zone handles and the polished-window brush
//!   in the planner's rough view, and the wizard's side of them;
//! * [`material_zones`]: the "Zones..." list and the base-colour write-back of the material editor.
//!
//! Nothing here is reachable from a default build.
//!
//! # Flows
//!
//! **Planner.** A plan with a rough colour ([`store::load_rough_colour`]) previews each stone with
//! [`store::planner_stone_material`] (the host material plus the rough's zones moved into the
//! stone's frame). The pose of a stone is the planner's own unless the cutter asked for "best
//! colour": [`store::choose_and_save_poses`] scores the box-symmetric poses and stores the
//! non-canonical choices; [`store::posed_stone_pose`] reads one back. `PlacedStone` and the saved
//! plan are unchanged.
//!
//! **Adopt.** [`store::adopt_stone`] creates the custom material "<rough name> colour" (its row
//! holds the BASE zone, so a default build renders it) and its `material_zoning` row (the zones
//! in the stone frame, in mm), and returns the render-ready material plus the real stone width
//! the editor should be set to. The stone is rendered at that size; no slider is involved.
//!
//! **Material resolution.** [`store::attach_zoning`] / [`store::with_stored_zoning`] put the
//! stored zones back on a material resolved from a custom row. They are called wherever the
//! application builds its list of custom materials (startup and the material editor's save), so
//! every `resolve_material` afterwards returns a zoned material.
//!
//! **Render jobs.** A job freezes its scene as the wire's `SceneState`, which carries no zones.
//! [`store::save_job_zoning`] persists the zones beside the queued job and
//! [`store::attach_job_zoning`] puts them back when it runs. Tilt-curve requests carry no zones
//! either: a zoned material's curves are always computed locally (`gui::batch::tilt::engine`).

pub mod adopt_link;
pub mod handles;
pub mod material_zones;
pub mod mesh_paint;
pub mod preview;
pub mod store;
#[cfg(test)]
mod store_tests;
pub mod wizard;
pub mod zone_link;
