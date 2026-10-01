//! Putting the passes of a frame together, on plain RGBA buffers.
//!
//! The fit scene is drawn in separate passes: the stones opaque on the theme background,
//! the rough as a transparent volume (its edges, plus a coverage mask), the saw pieces as
//! edges. [`compose`] lays the rough and the saw pass over the stones; the outline and the
//! hover marks are drawn last. Everything here is a pure function of buffers, like the
//! compare overlay, so it is tested on tiny images.

/// The tint of the glass the rough is drawn as.
pub(super) const ROUGH_TINT: [u8; 3] = [150, 170, 200];

/// How much of [`ROUGH_TINT`] covered pixels take, in percent.
pub(super) const ROUGH_TINT_PERCENT: u32 = 10;

/// The colour of the rough's edges.
pub(super) const ROUGH_EDGE: [u8; 3] = [190, 205, 235];

/// How much of the rough's edge colour an edge pixel takes, in percent.
pub(super) const ROUGH_EDGE_PERCENT: u32 = 70;

/// The colour of the saw pieces' edges (`Theme.accent-amber`).
pub(super) const SAW_EDGE: [u8; 3] = [245, 158, 11];

/// How much of the saw colour an edge pixel takes, in percent.
pub(super) const SAW_EDGE_PERCENT: u32 = 55;

/// The colour of the outline around the stones of the selected design.
pub(super) const OUTLINE: [u8; 3] = [56, 189, 248];

/// The colour of the hovered edge or corner marker.
pub(super) const HOVER_MARK: [u8; 3] = [56, 189, 248];

/// The rough pass: its edges (alpha 255 on an edge pixel, 0 elsewhere) and which pixels
/// its faces cover (`0` uncovered), both from a transparent render.
pub(super) struct RoughLayer<'a> {
    /// RGBA8 edge pixels.
    pub(super) edges: &'a [u8],
    /// The pick buffer of the same render.
    pub(super) cover: &'a [u32],
}

/// The passes laid over the stones.
pub(super) struct Layers<'a> {
    /// The rough as a glass volume.
    pub(super) rough: Option<RoughLayer<'a>>,
    /// The saw pieces' edges, RGBA8 in the same way.
    pub(super) saw: Option<&'a [u8]>,
}

/// `dst` moved towards `src` by `weight / 25500` (`weight` in `0..=25500`), rounded.
fn mix(dst: u8, src: u8, weight: u32) -> u8 {
    let total = 25_500;
    let value = (u32::from(dst) * (total - weight) + u32::from(src) * weight + total / 2) / total;
    value as u8
}

/// Moves the covered pixels of `out` towards [`ROUGH_TINT`] by [`ROUGH_TINT_PERCENT`].
fn tint_covered(out: &mut [u8], cover: &[u32]) {
    let weight = ROUGH_TINT_PERCENT * 255;
    let (pixels, _) = out.as_chunks_mut::<4>();
    for (pixel, &covered) in pixels.iter_mut().zip(cover) {
        if covered != 0 {
            for (channel, tint) in pixel.iter_mut().zip(ROUGH_TINT) {
                *channel = mix(*channel, tint, weight);
            }
        }
    }
}

/// Blends the painted pixels of `edges` over `out` with `colour`, `percent` strong.
fn blend_edges(out: &mut [u8], edges: &[u8], colour: [u8; 3], percent: u32) {
    let (pixels, _) = out.as_chunks_mut::<4>();
    let (edge_pixels, _) = edges.as_chunks::<4>();
    for (pixel, edge) in pixels.iter_mut().zip(edge_pixels) {
        if edge[3] == 0 {
            continue;
        }
        let weight = percent * u32::from(edge[3]);
        for (channel, tint) in pixel.iter_mut().zip(colour) {
            *channel = mix(*channel, tint, weight);
        }
    }
}

/// Lays `layers` over `out` (the stones on their background), in order: the glass tint of
/// the rough, the rough's edges, the saw pieces' edges. A buffer of the wrong length
/// leaves `out` as it is.
pub(super) fn compose(out: &mut [u8], layers: &Layers<'_>) {
    if let Some(rough) = &layers.rough
        && rough.edges.len() == out.len()
        && rough.cover.len() * 4 == out.len()
    {
        tint_covered(out, rough.cover);
        blend_edges(out, rough.edges, ROUGH_EDGE, ROUGH_EDGE_PERCENT);
    }
    if let Some(saw) = layers.saw
        && saw.len() == out.len()
    {
        blend_edges(out, saw, SAW_EDGE, SAW_EDGE_PERCENT);
    }
}

/// A grid of `width x height` cells in which `inside` says which belong to a region.
struct Region<'a> {
    width: usize,
    height: usize,
    inside: &'a dyn Fn(usize, usize) -> bool,
}

impl Region<'_> {
    /// Whether the cell at `(x, y)` (`None` for a coordinate below zero) is outside the
    /// region; everything beyond the grid is.
    fn outside(&self, x: Option<usize>, y: Option<usize>) -> bool {
        match (x, y) {
            (Some(x), Some(y)) if x < self.width && y < self.height => !(self.inside)(x, y),
            _ => true,
        }
    }

    /// Whether an inside cell has an outside cell `distance` steps away along an axis.
    fn is_border(&self, x: usize, y: usize, distance: usize) -> bool {
        self.outside(x.checked_sub(distance), Some(y))
            || self.outside(Some(x + distance), Some(y))
            || self.outside(Some(x), y.checked_sub(distance))
            || self.outside(Some(x), Some(y + distance))
    }
}

/// Paints the border of the region of pixels whose `pick` value passes `selected`,
/// `thickness` pixels wide and inside the region. A region touching the image border is
/// outlined there too.
pub(super) fn outline_region(
    out: &mut [u8],
    pick: &[u32],
    size: (u32, u32),
    selected: &dyn Fn(u32) -> bool,
    thickness: usize,
) {
    let (width, height) = (size.0 as usize, size.1 as usize);
    if pick.len() != width * height || out.len() != width * height * 4 {
        return;
    }
    let inside = |x: usize, y: usize| pick[y * width + x] != 0 && selected(pick[y * width + x]);
    let region = Region {
        width,
        height,
        inside: &inside,
    };
    for y in 0..height {
        for x in 0..width {
            if inside(x, y) && (1..=thickness).any(|d| region.is_border(x, y, d)) {
                let at = (y * width + x) * 4;
                out[at..at + 3].copy_from_slice(&OUTLINE);
            }
        }
    }
}

/// Paints the pixel `(x, y)` when it is inside the image.
fn put(out: &mut [u8], size: (u32, u32), x: i32, y: i32, colour: [u8; 3]) {
    if x < 0 || y < 0 || x as u32 >= size.0 || y as u32 >= size.1 {
        return;
    }
    let at = ((y as usize) * (size.0 as usize) + x as usize) * 4;
    if let Some(pixel) = out.get_mut(at..at + 3) {
        pixel.copy_from_slice(&colour);
    }
}

/// The part of the segment `from`-`to` inside the box `low..=high` (Liang-Barsky), as
/// the two end points, or `None` when no part is inside or a coordinate is not finite.
fn clip_segment(
    from: (f32, f32),
    to: (f32, f32),
    low: (f32, f32),
    high: (f32, f32),
) -> Option<((f32, f32), (f32, f32))> {
    let all_finite = [from.0, from.1, to.0, to.1].iter().all(|v| v.is_finite());
    if !all_finite {
        return None;
    }
    // In f64, so that a segment millions of pixels long still resolves its visible end
    // points to a fraction of a pixel.
    let widen = |p: (f32, f32)| (f64::from(p.0), f64::from(p.1));
    let (from, to, low, high) = (widen(from), widen(to), widen(low), widen(high));
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let (mut near, mut far) = (0.0_f64, 1.0_f64);
    // Each border as `(direction towards it, distance of the start from it)`.
    let borders = [
        (-dx, from.0 - low.0),
        (dx, high.0 - from.0),
        (-dy, from.1 - low.1),
        (dy, high.1 - from.1),
    ];
    for (direction, distance) in borders {
        if direction == 0.0 {
            if distance < 0.0 {
                return None;
            }
            continue;
        }
        let at = distance / direction;
        if direction < 0.0 {
            near = near.max(at);
        } else {
            far = far.min(at);
        }
    }
    if near > far {
        return None;
    }
    let point = |t: f64| (t.mul_add(dx, from.0) as f32, t.mul_add(dy, from.1) as f32);
    Some((point(near), point(far)))
}

/// Draws the line from `from` to `to` (pixel coordinates), `width` pixels wide. Only the
/// part that can reach the image is walked, so a line that starts far outside it (a point
/// just in front of the camera projects that far) costs no more than one across it.
pub(super) fn draw_line(
    out: &mut [u8],
    size: (u32, u32),
    from: (f32, f32),
    to: (f32, f32),
    width: i32,
    colour: [u8; 3],
) {
    let reach = width / 2;
    let margin = reach as f32;
    let low = (-margin, -margin);
    let high = (size.0 as f32 + margin, size.1 as f32 + margin);
    let Some((from, to)) = clip_segment(from, to, low, high) else {
        return;
    };
    let steps = (to.0 - from.0)
        .abs()
        .max((to.1 - from.1).abs())
        .ceil()
        .max(1.0) as i32;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let x = t.mul_add(to.0 - from.0, from.0).round() as i32;
        let y = t.mul_add(to.1 - from.1, from.1).round() as i32;
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                put(out, size, x + dx, y + dy, colour);
            }
        }
    }
}

/// Draws a filled disc of `radius` pixels around `centre`.
pub(super) fn draw_dot(
    out: &mut [u8],
    size: (u32, u32),
    centre: (f32, f32),
    radius: i32,
    colour: [u8; 3],
) {
    let (cx, cy) = (centre.0.round() as i32, centre.1.round() as i32);
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy <= radius * radius {
                put(out, size, cx + dx, cy + dy, colour);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An opaque image of `count` pixels, all `rgb`.
    fn solid(count: usize, rgb: [u8; 3]) -> Vec<u8> {
        (0..count)
            .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
            .collect()
    }

    /// A transparent image of `count` pixels with the pixels in `painted` opaque white.
    fn edges(count: usize, painted: &[usize]) -> Vec<u8> {
        let mut out = vec![0u8; count * 4];
        for &pixel in painted {
            out[pixel * 4..pixel * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
        out
    }

    #[test]
    fn the_glass_tint_moves_only_covered_pixels_ten_percent() {
        let mut out = solid(2, [0, 0, 0]);
        tint_covered(&mut out, &[7, 0]);
        // 10 % of (150, 170, 200) is (15, 17, 20).
        assert_eq!(&out[0..4], &[15, 17, 20, 255]);
        assert_eq!(&out[4..8], &[0, 0, 0, 255]);
    }

    #[test]
    fn edge_pixels_take_their_percentage_of_the_colour() {
        let mut out = solid(2, [0, 0, 0]);
        blend_edges(&mut out, &edges(2, &[1]), [200, 100, 0], 70);
        assert_eq!(&out[0..4], &[0, 0, 0, 255], "an unpainted pixel stays");
        // 70 % of 200 is 140, of 100 is 70.
        assert_eq!(&out[4..8], &[140, 70, 0, 255]);
    }

    #[test]
    fn compose_lays_the_glass_then_the_rough_edges_then_the_saw() {
        let mut out = solid(3, [0, 0, 0]);
        let rough_edges = edges(3, &[1]);
        let cover = [5u32, 5, 0];
        let saw = edges(3, &[1, 2]);
        compose(
            &mut out,
            &Layers {
                rough: Some(RoughLayer {
                    edges: &rough_edges,
                    cover: &cover,
                }),
                saw: Some(&saw),
            },
        );
        // Pixel 0: covered only. Pixel 1: covered, a rough edge and a saw edge. Pixel 2:
        // a saw edge only.
        assert_eq!(&out[0..3], &[15, 17, 20]);
        let after_glass = [15u8, 17, 20];
        let after_rough: Vec<u8> = after_glass
            .iter()
            .zip(ROUGH_EDGE)
            .map(|(&d, s)| mix(d, s, ROUGH_EDGE_PERCENT * 255))
            .collect();
        let expected: Vec<u8> = after_rough
            .iter()
            .zip(SAW_EDGE)
            .map(|(&d, s)| mix(d, s, SAW_EDGE_PERCENT * 255))
            .collect();
        assert_eq!(&out[4..7], expected.as_slice());
        // Pixel 2: 55 % of the saw colour over black.
        assert_eq!(&out[8..11], &[135, 87, 6]);
        assert_eq!(out[7], 255);
    }

    #[test]
    fn compose_with_no_layers_or_mismatched_buffers_changes_nothing() {
        let mut out = solid(2, [9, 9, 9]);
        let before = out.clone();
        compose(
            &mut out,
            &Layers {
                rough: None,
                saw: None,
            },
        );
        assert_eq!(out, before);
        let short = [0u8; 4];
        compose(
            &mut out,
            &Layers {
                rough: Some(RoughLayer {
                    edges: &short,
                    cover: &[1],
                }),
                saw: Some(&short),
            },
        );
        assert_eq!(out, before);
    }

    #[test]
    fn the_outline_runs_along_the_inside_of_the_region() {
        // A 3 x 3 region in the middle of a 5 x 5 image.
        let mut pick = vec![0u32; 25];
        for y in 1..4 {
            for x in 1..4 {
                pick[y * 5 + x] = 3;
            }
        }
        let mut out = solid(25, [0, 0, 0]);
        outline_region(&mut out, &pick, (5, 5), &|p| p == 3, 1);
        for y in 0..5 {
            for x in 0..5 {
                let painted = out[(y * 5 + x) * 4..(y * 5 + x) * 4 + 3] == OUTLINE;
                let on_ring = (1..4).contains(&x) && (1..4).contains(&y) && !(x == 2 && y == 2);
                assert_eq!(painted, on_ring, "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn a_region_at_the_image_border_is_outlined_there_too() {
        let pick = vec![4u32; 9];
        let mut out = solid(9, [0, 0, 0]);
        outline_region(&mut out, &pick, (3, 3), &|p| p == 4, 1);
        // Everything but the centre pixel touches the border.
        for pixel in 0..9 {
            let painted = out[pixel * 4..pixel * 4 + 3] == OUTLINE;
            assert_eq!(painted, pixel != 4, "pixel {pixel}");
        }
    }

    #[test]
    fn an_outline_of_the_wrong_buffer_size_is_skipped() {
        let mut out = solid(4, [0, 0, 0]);
        outline_region(&mut out, &[1, 1, 1], (2, 2), &|_| true, 1);
        assert_eq!(out, solid(4, [0, 0, 0]));
    }

    #[test]
    fn a_line_touches_both_ends_and_stays_inside_the_image() {
        let mut out = solid(16, [0, 0, 0]);
        draw_line(&mut out, (4, 4), (0.0, 0.0), (3.0, 3.0), 1, HOVER_MARK);
        for pixel in [0usize, 5, 10, 15] {
            assert_eq!(out[pixel * 4..pixel * 4 + 3], HOVER_MARK, "pixel {pixel}");
        }
        assert_eq!(out[4..7], [0, 0, 0]);
        // A line running off the image does not panic.
        draw_line(&mut out, (4, 4), (-5.0, 2.0), (9.0, 2.0), 3, HOVER_MARK);
        assert_eq!(out[(2 * 4) * 4..(2 * 4) * 4 + 3], HOVER_MARK);
    }

    #[test]
    fn a_line_from_far_outside_walks_only_the_part_near_the_image() {
        // An end point just in front of the camera projects enormously far away; the
        // segment must still be cheap and the visible part drawn.
        let mut out = solid(16, [0, 0, 0]);
        draw_line(&mut out, (4, 4), (-2.0e9, 1.0), (2.0e9, 1.0), 1, HOVER_MARK);
        for x in 0..4 {
            assert_eq!(
                out[(4 + x) * 4..(4 + x) * 4 + 3],
                HOVER_MARK,
                "pixel ({x}, 1)"
            );
        }
        assert_eq!(out[0..3], [0, 0, 0], "the rows above and below stay empty");
        assert_eq!(out[(2 * 4) * 4..(2 * 4) * 4 + 3], [0, 0, 0]);
    }

    #[test]
    fn a_line_that_misses_the_image_or_is_not_finite_draws_nothing() {
        let blank = solid(16, [0, 0, 0]);
        let mut out = blank.clone();
        draw_line(
            &mut out,
            (4, 4),
            (-50.0, -50.0),
            (-40.0, 90.0),
            1,
            HOVER_MARK,
        );
        draw_line(&mut out, (4, 4), (0.0, 9.0), (3.0, 9.0), 1, HOVER_MARK);
        draw_line(&mut out, (4, 4), (f32::NAN, 1.0), (3.0, 1.0), 1, HOVER_MARK);
        draw_line(
            &mut out,
            (4, 4),
            (0.0, 0.0),
            (f32::INFINITY, 3.0),
            1,
            HOVER_MARK,
        );
        assert_eq!(out, blank);
    }

    #[test]
    fn clipping_keeps_the_direction_and_the_inside_end_points() {
        let near = |a: (f32, f32), b: (f32, f32), tolerance: f32| {
            (a.0 - b.0).abs() < tolerance && (a.1 - b.1).abs() < tolerance
        };
        let (start, end) =
            clip_segment((-5.0, 2.0), (9.0, 2.0), (0.0, 0.0), (4.0, 4.0)).expect("crosses");
        assert!(near(start, (0.0, 2.0), 1e-5) && near(end, (4.0, 2.0), 1e-5));
        // A segment inside is returned as it is.
        let (start, end) =
            clip_segment((1.0, 1.0), (3.0, 2.0), (0.0, 0.0), (4.0, 4.0)).expect("inside");
        assert!(near(start, (1.0, 1.0), 1e-5) && near(end, (3.0, 2.0), 1e-5));
        // One that is hundreds of millions of pixels long still resolves its visible part.
        let (start, end) =
            clip_segment((-2.0e9, 1.0), (2.0e9, 1.0), (0.0, 0.0), (4.0, 4.0)).expect("crosses");
        assert!(
            near(start, (0.0, 1.0), 1e-3) && near(end, (4.0, 1.0), 1e-3),
            "{start:?} {end:?}"
        );
        // A diagonal crossing a corner region is cut on both borders.
        let (start, end) =
            clip_segment((-2.0, -2.0), (6.0, 6.0), (0.0, 0.0), (4.0, 4.0)).expect("crosses");
        assert!(near(start, (0.0, 0.0), 1e-5) && near(end, (4.0, 4.0), 1e-5));
        // Parallel to a border and outside it.
        assert_eq!(
            clip_segment((0.0, 5.0), (4.0, 5.0), (0.0, 0.0), (4.0, 4.0)),
            None
        );
    }

    #[test]
    fn a_dot_is_a_disc() {
        let mut out = solid(49, [0, 0, 0]);
        draw_dot(&mut out, (7, 7), (3.0, 3.0), 2, HOVER_MARK);
        let painted = |x: usize, y: usize| out[(y * 7 + x) * 4..(y * 7 + x) * 4 + 3] == HOVER_MARK;
        assert!(painted(3, 3));
        assert!(painted(5, 3));
        assert!(!painted(5, 5), "a corner of the square is outside the disc");
        assert!(!painted(0, 0));
    }
}
