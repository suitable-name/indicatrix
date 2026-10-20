# L3 Desktop Renderer Integration

Implemented the opt-in Ray-Traced engine in the live/edit render context, immutable frame inputs, scene identity, typed view interpretation and image dispatcher. The existing spectral dispatcher remains the Path-Traced branch. Startup still opens Solid. Engine and AA/Fire changes invalidate accumulation; fixed taps replace the path SPP target, and Ray-Traced never enables denoise or the spectral optical overlay worker.

New modules: `gui/startup_ray.rs`, `gui/render/ray_settings.rs`, `settings/model/ray_trace.rs`, `settings/model/view_settings.rs`, `bridge/render_thread/engine.rs`, `bridge/render_thread/whitted_scanline.rs` and the mechanically moved `pacing.rs`. Expensive Whitted preparation is cached by scene generation and preview dimensions. The independent small GPU facade is used for GPU/CPU+GPU selections, with complete CPU retry after a decline. UV and fluorescence are refused before claiming a frame range; an HDR map ignores a stale UV picker value.

Settings preserve backward compatibility: missing `ray_trace` loads eight taps and Fire on; view indices remain unchanged, with Edit 4 and Live 2 added. Unknown modes normalize to Solid. New settings callbacks have harness coverage. L6 adds the independent still/tilt renderer and thumbnail selectors on top of this integration.

Source review covered mode/persistence callbacks, effective sample target, scene identity, hidden-viewport suspension, accumulation reset, no-denoise and unsupported-physics handling. Standalone rustfmt parsed the named changed Rust files; no cargo, build, tests, Clippy or git commands were run. Regression tests were written but remain unrun. Existing oversized settings files received only the necessary fields/defaults, as permitted by the plan.

## Source Line Counts

| File | Lines |
|---|---:|
| `apps/indicatrix-cut/src/gui/startup_ray.rs` | 20 |
| `apps/indicatrix-cut/src/gui/render/ray_settings.rs` | 73 |
| `apps/indicatrix-cut/src/settings/model/ray_trace.rs` | 37 |
| `apps/indicatrix-cut/src/settings/model/view_settings.rs` | 20 |
| `apps/indicatrix-cut/src/bridge/render_thread/engine.rs` | 141 |
| `apps/indicatrix-cut/src/bridge/render_thread/whitted_scanline.rs` | 98 |
| `apps/indicatrix-cut/src/bridge/render_thread/context/frame.rs` | 185 |
| `apps/indicatrix-cut/src/bridge/render_thread/context/scene_identity.rs` | 292 |
| `apps/indicatrix-cut/src/bridge/render_thread/mod.rs` | 769 |
