//! Stored view identifiers. Startup deliberately opens both views in Solid.
/// The solid preview's Solid/Path-Traced/Both/Diagram/Ray-Traced view-mode selector -- `0` is Solid
/// (the software-rasterized inspection view), `1` Path-Traced (today's GPU/CPU render),
/// `2` Both (edges from the solid rasterizer composited over the path-traced image --
/// see `solid_preview::edges_layer`), `3` Diagram (the faceting diagram), `4` Ray-Traced. A plain `u8` since that's what a Slint `int`
/// property round-trips most directly. `0` (Solid) by default so a settings file
/// predating this control loads into the same view that already shipped as the sole
/// mode.
pub const DEFAULT_SOLID_VIEW_MODE: u8 = 0;
/// The Live Render tab's own Solid/Path-traced view-mode selector -- see
/// `ViewportModel.live_view_mode`'s own doc comment (`ui/models/viewport.slint`). `1`
/// (Path-traced) by default so a settings file predating this control loads into the
/// same spectral-tracer view that already shipped as the Live Render tab's sole mode --
/// unlike [`DEFAULT_SOLID_VIEW_MODE`], whose `0` default matches the EDIT tab's
/// pre-existing sole mode instead.
pub const DEFAULT_LIVE_VIEW_MODE: u8 = 1;

pub(super) const fn default_live_view_mode() -> u8 {
    DEFAULT_LIVE_VIEW_MODE
}
