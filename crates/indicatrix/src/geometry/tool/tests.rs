//! Tests for [`super`].

use super::*;

/// xorshift32, as in the other seeded generators in this workspace.
struct Rng(u32);

impl Rng {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in `[-1, 1)`.
    fn signed(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }

    fn unit_vec(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(self.signed(), self.signed(), self.signed());
            let l = v.length();
            if (0.1..=1.0).contains(&l) {
                return v / l;
            }
        }
    }
}

fn base_tool(kind: ToolKind) -> ToolPrimitive {
    let centre = Vec3::new(0.1, -0.2, 0.15);
    let axis = Vec3::new(0.3, 0.5, 0.8).normalize();
    match kind {
        ToolKind::Ball => ToolPrimitive::ball(centre, 0.4),
        ToolKind::Cylinder => ToolPrimitive::cylinder(centre, axis, 0.3, 0.5),
        ToolKind::Frustum => ToolPrimitive::frustum(centre, axis, 0.2, 0.45, 0.5),
        ToolKind::Bicone => ToolPrimitive::bicone(centre, axis, 0.45, 0.5),
    }
}

fn with_sweep_kind(t: ToolPrimitive, sweep: ToolSweep) -> ToolPrimitive {
    let axis = t.axis_vec();
    let dir = axis.any_orthonormal_vector();
    t.with_sweep(sweep, 0.3, dir)
}

#[test]
fn tool_primitive_layout_is_80_bytes_and_16_aligned() {
    assert_eq!(std::mem::size_of::<ToolPrimitive>(), 80);
    assert_eq!(std::mem::align_of::<ToolPrimitive>(), 16);
}

#[test]
fn tool_kind_and_sweep_round_trip_through_u32_and_reject_unknown_values() {
    for k in [
        ToolKind::Ball,
        ToolKind::Cylinder,
        ToolKind::Frustum,
        ToolKind::Bicone,
    ] {
        assert_eq!(ToolKind::try_from(k as u32), Ok(k));
    }
    for s in [ToolSweep::None, ToolSweep::AlongAxis, ToolSweep::AcrossAxis] {
        assert_eq!(ToolSweep::try_from(s as u32), Ok(s));
    }
    assert_eq!(ToolKind::try_from(4), Err(4));
    assert_eq!(ToolSweep::try_from(9), Err(9));
}

#[test]
fn tool_primitive_validate_rejects_each_bad_field() {
    let good = base_tool(ToolKind::Cylinder);
    assert_eq!(good.validate(), Ok(()));
    for sweep in [ToolSweep::AlongAxis, ToolSweep::AcrossAxis] {
        assert_eq!(with_sweep_kind(good, sweep).validate(), Ok(()));
    }

    let mut t = good;
    t.origin[1] = f32::NAN;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::NonFinite));
    let mut t = good;
    t.axis[3] = f32::INFINITY;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::NonFinite));

    let mut t = good;
    t.origin[3] = -0.1;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::NonPositiveRadius));
    let mut t = good;
    t.origin[3] = 0.0;
    t.profile[0] = 0.0;
    t.profile[1] = 0.0;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::NonPositiveRadius));
    let mut t = good;
    t.axis[3] = 0.0;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::NonPositiveRadius));
    let mut t = good;
    t.profile[2] = -1.0;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::NonPositiveRadius));

    let mut t = good;
    t.axis[0] *= 1.5;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::AxisNotUnit));
    let across = with_sweep_kind(good, ToolSweep::AcrossAxis);
    let mut t = across;
    t.sweep_dir = [t.axis[0], t.axis[1], t.axis[2], 0.0];
    assert_eq!(t.validate(), Err(ToolPrimitiveError::AxisNotUnit));

    let mut t = good;
    t.kind = 7;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::UnknownKind(7)));
    let mut t = good;
    t.sweep_kind = 5;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::UnknownSweep(5)));

    let mut t = good;
    t._pad[1] = 1;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::PaddingNotZero));
    let mut t = good;
    t.profile[3] = 1.0;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::PaddingNotZero));
    let mut t = good;
    t.sweep_dir[3] = 1.0;
    assert_eq!(t.validate(), Err(ToolPrimitiveError::PaddingNotZero));
}

#[test]
fn tool_interval_agrees_with_point_classification_on_seeded_rays() {
    let mut rng = Rng(0x9E37_79B9);
    for kind in [
        ToolKind::Ball,
        ToolKind::Cylinder,
        ToolKind::Frustum,
        ToolKind::Bicone,
    ] {
        for sweep in [ToolSweep::None, ToolSweep::AlongAxis, ToolSweep::AcrossAxis] {
            let tool = with_sweep_kind(base_tool(kind), sweep);
            assert_eq!(tool.validate(), Ok(()), "{kind:?} {sweep:?}");
            let centre = tool.centre();
            let scale = tool.origin[3].max(tool.profile[0]).max(tool.profile[1]);
            let mut hit_rays = 0usize;
            for _ in 0..10_000 {
                let from = centre + rng.unit_vec() * 3.0;
                let aim = centre + rng.unit_vec() * (0.9 * rng.signed().abs());
                let ray = Ray {
                    origin: from,
                    dir: (aim - from).normalize(),
                };
                let interval = tool.ray_interval(ray);
                hit_rays += usize::from(interval.is_some());
                for i in 0..64u32 {
                    let t = 1.0 + 4.0 * (i as f32 + 0.5) / 64.0;
                    let p = ray.origin + ray.dir * t;
                    if tool.boundary_gap(p) < 1e-4 * scale {
                        continue;
                    }
                    let in_interval = interval.is_some_and(|(c, d)| t >= c && t <= d);
                    assert_eq!(
                        in_interval,
                        tool.contains(p),
                        "{kind:?} {sweep:?} t={t} interval={interval:?}"
                    );
                }
            }
            assert!(
                hit_rays > 500,
                "{kind:?} {sweep:?}: only {hit_rays} rays hit, the test would be vacuous"
            );
        }
    }
}

#[test]
fn stone_geometry_counts_planes_and_tools_and_reports_convexity() {
    let planes = [GpuFacetPlane::new(Vec3::X, -1.0)];
    let tools = [ToolPrimitive::ball(Vec3::ZERO, 0.1)];
    let convex = StoneGeometry::planes_only(&planes);
    assert!(convex.is_convex());
    assert_eq!(convex.facet_count(), 1);
    let carved = StoneGeometry {
        planes: &planes,
        tools: &tools,
    };
    assert!(!carved.is_convex());
    assert_eq!(carved.facet_count(), 2);
}
