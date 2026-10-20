//! The GPU packing of concave tools: how a stone's tool list rides in the `planes` storage
//! buffer of the megakernel (and, later, the event-wavefront kernels).
//!
//! # Layout (the contract `transport_physics/10_concave_tools.wgsl` reads)
//!
//! The buffer is `array<FacetPlane>`, 16 bytes per entry (`normal: vec3<f32>, d: f32`), and
//! holds the stone's real planes first, then [`TOOL_RECORD_VECS`] entries per tool, one
//! entry being one `vec4<f32>` of the record:
//!
//! | entry | x, y, z | w |
//! |-------|---------|---|
//! | 0 | `kind` (exact small float), `sweep_kind` (exact small float), 0 | 0 |
//! | 1 | tool origin | radius at the centre (`ToolPrimitive::origin[3]`) |
//! | 2 | unit axis | half length (`ToolPrimitive::axis[3]`) |
//! | 3 | radius at `-hl`, radius at `+hl`, half stroke | 0 |
//! | 4 | unit sweep direction | 0 |
//!
//! Entries 1 to 4 are [`ToolPrimitive`]'s own `origin`, `axis`, `profile` and `sweep_dir`
//! verbatim; only entry 0 differs, because the two `u32` discriminants are stored as exact
//! floats instead of bit-cast integers (so the whole record is read as `vec4<f32>`s and the
//! tools need no second storage binding). The shader's tool count is
//! `GpuTransportParams::num_tools`; its plane count is `arrayLength(&planes) - 5 * num_tools`.
//!
//! The facet finish buffer (`facet_finishes`) is sized to the packed entry count (so the
//! bind-group helpers, which size both buffers alike, are unchanged); a tool hit's facet id
//! is `planes.len() + k`, exactly as on the CPU, and indexes the first part of it.
//!
//! This module is not `gpu`-gated so the packing and the routing limit are unit-tested on
//! every host.

use std::borrow::Cow;

use super::ToolPrimitive;
use crate::geometry::plane::GpuFacetPlane;

/// `vec4`s (equivalently packed [`GpuFacetPlane`] entries) per tool record.
pub const TOOL_RECORD_VECS: usize = 5;

/// The most tools the GPU kernels trace in one stone: the CPU tracer's own bound.
///
/// That bound, [`super::MAX_TOOL_PRIMITIVES`], is also the most placements a design may hold
/// (`indicatrix-cut-core` refuses more), so every valid concave stone can go to the GPU.
///
/// A tool costs a bounding-sphere test per scan and, for a ray that reaches it, up to six
/// closed-form piece intervals; the carve repeats the scan once per tool interval the ray
/// passes before it settles (at most `tools + 1` times). Nothing in the GPU layout depends
/// on the figure: the records ride in the `planes` buffer and the kernels loop over
/// `params.num_tools`. It was 64 until 2026-10-20; the cost of a 128-tool stone on the
/// desktop GPU, a phone and the browser is the owner's measurement. The routing that
/// declines a longer list (`GpuRouteDecline::TooManyTools`) stays as a guard.
pub const GPU_MAX_TOOL_PRIMITIVES: usize = super::MAX_TOOL_PRIMITIVES;

/// Entry 0 of a tool record: the two discriminants as exact floats.
const fn discriminants(tool: &ToolPrimitive) -> [f32; 4] {
    // `kind` is at most 3 and `sweep_kind` at most 2 for a valid tool, and an out of range
    // value is still represented exactly (u32 below 2^24); the shader treats it as "no
    // pieces", the same as the CPU's `pieces()` returning `None`.
    let (kind, sweep) = (tool.kind as f32, tool.sweep_kind as f32);
    [kind, sweep, 0.0, 0.0]
}

/// The five `vec4`s of one tool record, in buffer order (see the module docs).
#[must_use]
pub const fn pack_tool_record(tool: &ToolPrimitive) -> [[f32; 4]; TOOL_RECORD_VECS] {
    [
        discriminants(tool),
        tool.origin,
        tool.axis,
        tool.profile,
        tool.sweep_dir,
    ]
}

/// Reinterprets one record `vec4` as the `FacetPlane` shape the buffer is declared with.
const fn as_plane_entry(v: [f32; 4]) -> GpuFacetPlane {
    GpuFacetPlane {
        normal: [v[0], v[1], v[2]],
        d: v[3],
    }
}

/// The `planes` storage buffer contents for a stone: its planes followed by the packed tool
/// records.
///
/// With no tools this is the plane slice itself, borrowed (no copy, byte-identical to the
/// planar upload, so a convex stone's GPU path is untouched). Tools are not validated here;
/// the caller routes only tool lists within [`GPU_MAX_TOOL_PRIMITIVES`].
#[must_use]
pub fn pack_planes_and_tools<'a>(
    planes: &'a [GpuFacetPlane],
    tools: &[ToolPrimitive],
) -> Cow<'a, [GpuFacetPlane]> {
    if tools.is_empty() {
        return Cow::Borrowed(planes);
    }
    let mut out = Vec::with_capacity(planes.len() + TOOL_RECORD_VECS * tools.len());
    out.extend_from_slice(planes);
    for tool in tools {
        out.extend(pack_tool_record(tool).into_iter().map(as_plane_entry));
    }
    Cow::Owned(out)
}

/// Number of buffer entries [`pack_planes_and_tools`] produces.
#[must_use]
pub const fn packed_entry_count(plane_count: usize, tool_count: usize) -> usize {
    plane_count + TOOL_RECORD_VECS * tool_count
}

#[cfg(test)]
mod tests {
    use super::super::ToolSweep;
    use super::*;
    use glam::Vec3;

    fn plane(i: u32) -> GpuFacetPlane {
        let x = i as f32;
        GpuFacetPlane {
            normal: [x, 0.0, 1.0],
            d: -x,
        }
    }

    #[test]
    fn no_tools_borrows_the_planes_unchanged() {
        let planes = [plane(1), plane(2)];
        let packed = pack_planes_and_tools(&planes, &[]);
        assert!(matches!(packed, Cow::Borrowed(_)));
        assert_eq!(&*packed, &planes);
    }

    #[test]
    fn tools_follow_the_planes_five_entries_each() {
        let planes = [plane(1), plane(2), plane(3)];
        let ball = ToolPrimitive::ball(Vec3::new(0.1, 0.2, 0.3), 0.25);
        let cyl = ToolPrimitive::cylinder(Vec3::new(1.0, 2.0, 3.0), Vec3::X, 0.5, 0.75).with_sweep(
            ToolSweep::AcrossAxis,
            0.125,
            Vec3::Y,
        );
        let packed = pack_planes_and_tools(&planes, &[ball, cyl]);
        assert_eq!(packed.len(), packed_entry_count(3, 2));
        assert_eq!(&packed[..3], &planes);

        let entry = |i: usize| {
            let e = packed[i];
            [e.normal[0], e.normal[1], e.normal[2], e.d]
        };
        // Tool 0 (ball) starts right after the planes.
        assert_eq!(entry(3), [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(entry(4), ball.origin);
        assert_eq!(entry(5), ball.axis);
        assert_eq!(entry(6), ball.profile);
        assert_eq!(entry(7), ball.sweep_dir);
        // Tool 1 (swept cylinder) is five entries later.
        assert_eq!(entry(8), [1.0, 2.0, 0.0, 0.0]);
        assert_eq!(entry(9), cyl.origin);
        assert_eq!(entry(10), cyl.axis);
        assert_eq!(entry(11), cyl.profile);
        assert_eq!(entry(12), cyl.sweep_dir);
    }

    #[test]
    fn every_kind_and_sweep_discriminant_round_trips_through_f32() {
        for kind in 0..=3_u32 {
            for sweep in 0..=2_u32 {
                let mut tool = ToolPrimitive::ball(Vec3::ZERO, 1.0);
                tool.kind = kind;
                tool.sweep_kind = sweep;
                let rec = pack_tool_record(&tool);
                // The shader reads `u32(v0.x)` and `u32(v0.y)`.
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "exact small non-negative floats, as the shader's `u32(...)` conversion"
                )]
                let (k, s) = (rec[0][0] as u32, rec[0][1] as u32);
                assert_eq!((k, s), (kind, sweep));
            }
        }
    }

    #[test]
    fn a_record_is_five_sixteen_byte_entries() {
        assert_eq!(size_of::<GpuFacetPlane>() * TOOL_RECORD_VECS, 80);
        assert_eq!(size_of::<ToolPrimitive>(), 80);
    }
}
