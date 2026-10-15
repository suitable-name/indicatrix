//! Integration adapter for `indicatrix-editor`.
//!
//! The optimizer needs no adapter: `RlOptimizerModel::optimize` already returns cut-core's
//! `OptimizeResult`, which the editor applies with `apply_optimize_result`.

pub mod editor_adapter;

pub use editor_adapter::EditorRetargetAdapter;
