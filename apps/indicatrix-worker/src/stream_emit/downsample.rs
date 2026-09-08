//! Preview downsampling: [`downsample_preview`] box-averages a full-resolution running
//! total down to `PREVIEW`'s reduced resolution.

use glam::Vec3;

/// Downsamples `running_total` (a `width x height` radiance sum) to a
/// `target_width x target_height` box-averaged buffer -- the payload behind a `PREVIEW`
/// message.
///
/// Averages (not sums) each output pixel's source box, so the result stays directly
/// comparable in scale to the full-resolution total it was built from, consistent with
/// `PREVIEW` being a cumulative, display-only snapshot rather than something summed
/// further.
#[must_use]
pub(super) fn downsample_preview(
    running_total: &[Vec3],
    width: u32,
    height: u32,
    target_width: u32,
    target_height: u32,
) -> Vec<Vec3> {
    let (w, h) = (width as usize, height as usize);
    let (tw, th) = (target_width.max(1) as usize, target_height.max(1) as usize);
    let mut out = vec![Vec3::ZERO; tw * th];
    if w == 0 || h == 0 {
        return out;
    }
    for oy in 0..th {
        let y0 = oy * h / th;
        let y1 = ((oy + 1) * h / th).max(y0 + 1).min(h);
        for ox in 0..tw {
            let x0 = ox * w / tw;
            let x1 = ((ox + 1) * w / tw).max(x0 + 1).min(w);
            let mut sum = Vec3::ZERO;
            let mut count: u32 = 0;
            for y in y0..y1 {
                let row = y * w;
                for x in x0..x1 {
                    sum += running_total[row + x];
                    count += 1;
                }
            }
            out[oy * tw + ox] = if count == 0 {
                Vec3::ZERO
            } else {
                sum / count as f32
            };
        }
    }
    out
}
