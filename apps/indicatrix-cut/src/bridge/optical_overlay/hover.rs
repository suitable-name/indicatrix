//! The hover read-out for the pixel under the pointer.
use super::{labels::label_for, snapshot::OpticalSnapshot};
use indicatrix::color::metrics::{OpticalImage, scintillation_index_at};

/// A facet, named with the tier label of the scene it was traced in when there is one.
fn facet_name(scene: Option<&OpticalSnapshot>, id: usize) -> String {
    let Some(scene) = scene else {
        return format!("Facet {}", id + 1);
    };
    label_for(scene, id).unwrap_or_else(|| {
        if id >= scene.planes.len() {
            format!(
                "Concave Tool Surface {} (Tier Label Unavailable)",
                id - scene.planes.len() + 1
            )
        } else {
            format!("Facet {} (Tier Label Unavailable)", id + 1)
        }
    })
}
fn entry_label(scene: Option<&OpticalSnapshot>, id: Option<usize>) -> String {
    id.map_or_else(
        || "No Center-Ray Hit".to_string(),
        |id| facet_name(scene, id),
    )
}
fn exit_label(scene: Option<&OpticalSnapshot>, id: Option<usize>) -> String {
    id.map_or_else(|| "No Exit".to_string(), |id| facet_name(scene, id))
}
/// Index of the map pixel under the normalized position `(u, v)`; `None` outside the map.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the position is inside [0, 1), so the scaled coordinate is non-negative and below the extent"
)]
fn pixel_under(map: &OpticalImage, u: f32, v: f32) -> Option<usize> {
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) || map.width == 0 || map.height == 0 {
        return None;
    }
    let x = ((u * map.width as f32) as u32).min(map.width - 1);
    let y = ((v * map.height as f32) as u32).min(map.height - 1);
    Some(y as usize * map.width as usize + x as usize)
}
/// The read-out text for the pixel at `(u, v)`, or an empty string outside the map.
/// `scene` is the one the map was traced for; without it facets are numbered only.
pub(super) fn hover_text(
    map: &OpticalImage,
    scene: Option<&OpticalSnapshot>,
    u: f32,
    v: f32,
) -> String {
    let Some((index, pixel)) =
        pixel_under(map, u, v).and_then(|i| map.pixels.get(i).map(|p| (i, p)))
    else {
        return String::new();
    };
    let f = pixel.fractions();
    let surfaces = if pixel.mixed_facets {
        " (Mixed Subpixel Surfaces)"
    } else {
        " (Center Ray)"
    };
    let fire = if map.fire_computed {
        format!(
            "{:.5} Weighted Deg; {} Qualifying / {} Rejected Pairs",
            pixel.fire_deg(),
            pixel.fire_qualified,
            pixel.fire_rejected
        )
    } else {
        "Not Computed".to_string()
    };
    let mut text = format!(
        "W {:.1}% · E {:.1}% · B {:.1}% · Unknown {:.1}% | {}{} | Exit {} · {} Reflections · {} | Fire {}",
        f[0] * 100.0,
        f[1] * 100.0,
        f[2] * 100.0,
        f[3] * 100.0,
        entry_label(scene, pixel.centre.entry_facet),
        surfaces,
        exit_label(scene, pixel.centre.exit_facet),
        pixel.centre.bounces,
        pixel.centre.reason,
        fire
    );
    if map.scint_computed {
        text.push_str(&scintillation_read_out(map, index));
    }
    text
}
/// The scintillation part of the read-out: the pixel's 0-100 index and how many of the five
/// yaw poses returned light at its centre ray.
fn scintillation_read_out(map: &OpticalImage, index: usize) -> String {
    // The same combination the overlay draws: the module's 60/40 of the pixel's spatial
    // and temporal terms.
    let Some(value) = scintillation_index_at(map, index) else {
        return " | Scint Not Computed".to_string();
    };
    format!(
        " | Scint {value:.0} ({}/{} poses)",
        map.pixels[index].scint_returned, map.pixels[index].scint_poses
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::color::metrics::{ExtinctionReason, OpticalPixel, RayDetail};
    fn map_with(pixel: OpticalPixel) -> OpticalImage {
        OpticalImage {
            width: 1,
            height: 1,
            samples: 1,
            fire_computed: false,
            scint_computed: false,
            origin: [0, 0],
            pixels: vec![pixel],
            total: OpticalPixel::default(),
        }
    }
    #[test]
    fn a_bounce_budget_ray_reads_in_words_and_without_an_exit() {
        let map = map_with(OpticalPixel {
            hits: 1,
            classes: [0, 1, 0, 0],
            centre: RayDetail {
                entry_facet: Some(2),
                exit_facet: None,
                bounces: 10,
                reason: ExtinctionReason::BounceBudget,
            },
            ..OpticalPixel::default()
        });
        let text = hover_text(&map, None, 0.5, 0.5);
        assert!(text.contains("Bounce Budget"), "{text}");
        assert!(!text.contains("BounceBudget"), "{text}");
        assert!(text.contains("Exit No Exit"), "{text}");
        assert!(text.contains("Facet 3 (Center Ray)"), "{text}");
        assert!(text.contains("Fire Not Computed"), "{text}");
        assert!(!text.contains("centre"), "{text}");
    }
    #[test]
    fn scintillation_reads_as_an_index_with_its_poses_only_when_the_map_computed_it() {
        let flicker = OpticalPixel {
            hits: 1,
            classes: [0, 0, 1, 0],
            scint_returned: 2,
            scint_poses: 5,
            ..OpticalPixel::default()
        };
        let mut map = map_with(flicker);
        assert!(!hover_text(&map, None, 0.5, 0.5).contains("Scint"));
        map.scint_computed = true;
        let text = hover_text(&map, None, 0.5, 0.5);
        // Uniform brilliance (spatial 0) and a flip at 2 of 5 poses (temporal at its cap).
        assert!(text.ends_with(" | Scint 40 (2/5 poses)"), "{text}");
        map.pixels[0].scint_poses = 0;
        map.pixels[0].scint_returned = 0;
        let text = hover_text(&map, None, 0.5, 0.5);
        assert!(text.ends_with(" | Scint Not Computed"), "{text}");
    }
    #[test]
    fn positions_outside_the_map_read_empty_and_the_edge_stays_in_range() {
        let map = map_with(OpticalPixel::default());
        assert_eq!(hover_text(&map, None, 1.0, 0.5), "");
        assert_eq!(hover_text(&map, None, -0.1, 0.5), "");
        assert_ne!(hover_text(&map, None, 0.999_999_9, 0.999_999_9), "");
    }
}
