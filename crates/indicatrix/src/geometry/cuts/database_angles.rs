//! Reconstructs facet planes from scraped `angle_settings` rows
//! ([`FacetSpec`]), which carry only angle and index text -- no real depth --
//! so proportions have to be fabricated from realistic gemological defaults.

use glam::Vec3;
use tracing::warn;

use super::{FacetSpec, StandardGemCuts};
use crate::geometry::{brep::GemPolyhedron, plane::GpuFacetPlane};

impl StandardGemCuts {
    /// Classifies whether a `FacetSpec` row belongs to the crown (true) or pavilion (false)
    /// side of the stone, using markers that are actually present in the scraped
    /// facetdiagrams.org data (verified against `facet_diagrams.sqlite`):
    fn classify_is_crown(item: &FacetSpec, angle_deg: f32, tier_idx: usize, total: usize) -> bool {
        let index_val = item.index.trim();
        if index_val.eq_ignore_ascii_case("table") {
            return true;
        }
        if index_val.eq_ignore_ascii_case("culet") {
            return false;
        }

        let facet = item.facet.trim();
        let mut chars = facet.chars();
        if let Some(first) = chars.next() {
            let rest = chars.as_str();
            let rest_is_all_digits = !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
            match first.to_ascii_lowercase() {
                'p' if rest_is_all_digits => return false,
                'c' if rest_is_all_digits || rest.is_empty() => return true,
                _ => {}
            }
        }
        if facet.eq_ignore_ascii_case("t") && angle_deg.abs() < 0.5 {
            return true;
        }

        if item.notes.to_lowercase().contains("crown") {
            return true;
        }

        // Last resort: no explicit marker available, guess from position in the list.
        tier_idx > total / 2
    }

    /// Parses the leading numeric angle prefix out of a scraped angle string.
    fn parse_angle_deg(s: &str) -> Option<f32> {
        let trimmed = s.trim();
        let bytes = trimmed.as_bytes();
        let mut end = 0;

        if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
            end += 1;
        }

        let mut saw_digit = false;
        let mut saw_dot = false;
        while end < bytes.len() {
            match bytes[end] {
                b'0'..=b'9' => {
                    saw_digit = true;
                    end += 1;
                }
                b'.' if !saw_dot => {
                    saw_dot = true;
                    end += 1;
                }
                _ => break,
            }
        }

        if !saw_digit {
            return None;
        }

        trimmed[..end].parse::<f32>().ok()
    }

    /// Recognizes the `"N girdle facets"` / `"N girdle facet"` form seen in
    /// `angle_settings.index_val` (e.g. "48 girdle facets", "96 girdle facets") and extracts `N`.
    fn parse_girdle_facet_count(index_val: &str) -> Option<u32> {
        let trimmed = index_val.trim();
        if !trimmed.to_lowercase().contains("girdle facet") {
            return None;
        }
        trimmed.split_whitespace().next()?.parse::<u32>().ok()
    }

    /// Reconstructs facet planes from parsed Angle items in database with realistic gemological proportions.
    ///
    /// Azimuth uses [`Self::index_to_azimuth`] with a reference angle of `0.0`:
    /// `angle_settings`-sourced [`FacetSpec`] rows (angle + index text, scraped
    /// independently of any `.asc` file) carry no `gear_reference_angle`
    /// equivalent, so there is nothing to apply here -- unlike
    /// [`Self::from_asc_schedule`](super::StandardGemCuts::from_asc_schedule), which reads it
    /// straight off the parsed schedule.
    pub fn from_database_angles(angles: &[FacetSpec], gear_teeth: u32) -> Vec<GpuFacetPlane> {
        const MAX_UNPARSEABLE_FRACTION: f32 = 0.1;

        if angles.is_empty() {
            return Self::standard_round_brilliant();
        }

        let gear_teeth_f = if gear_teeth > 0 {
            gear_teeth as f32
        } else {
            96.0
        };

        let unparseable = angles
            .iter()
            .filter(|item| Self::parse_angle_deg(&item.angle).is_none())
            .count();
        if unparseable as f32 > angles.len() as f32 * MAX_UNPARSEABLE_FRACTION {
            warn!(
                "from_database_angles: {}/{} angle values are unparseable; refusing to fabricate a solid, falling back to standard_round_brilliant()",
                unparseable,
                angles.len()
            );
            return Self::standard_round_brilliant();
        }

        let mut planes = Vec::new();

        for (tier_idx, item) in angles.iter().enumerate() {
            let Some(angle_deg) = Self::parse_angle_deg(&item.angle) else {
                warn!(
                    "from_database_angles: skipping facet '{}' with unparseable angle '{}'",
                    item.facet, item.angle
                );
                continue;
            };
            let theta = angle_deg.to_radians();
            let is_crown = Self::classify_is_crown(item, angle_deg, tier_idx, angles.len());

            // Exact proportional plane offset matching realistic cutting geometry
            let offset = if angle_deg >= 88.0 {
                -1.0
            } else if is_crown {
                if angle_deg < 5.0 {
                    -0.32 // Crown Table flat
                } else {
                    // Taper from girdle (r=1.0, y=0.03) to table
                    -0.04f32.mul_add(
                        -(1.0 - theta.sin()),
                        0.03f32.mul_add(theta.cos(), 1.0 * theta.sin()),
                    )
                }
            } else if angle_deg < 5.0 {
                -0.88 // Pavilion Culet
            } else {
                // Taper from girdle (r=1.0, y=-0.02) to culet
                -0.02f32.mul_add(theta.cos(), 1.0 * theta.sin())
            };

            // Parse index numbers from string e.g. "96, 12, 24, 36, 48, 60, 72, 84" or "96-12-24"
            let indices: Vec<f32> = Self::parse_girdle_facet_count(&item.index).map_or_else(
                || {
                    item.index
                        .split([',', '-', ' ', ';'])
                        .filter_map(|s| s.trim().parse::<f32>().ok())
                        .collect()
                },
                |n| {
                    (0..n)
                        .map(|i| (i as f32) * gear_teeth_f / (n as f32))
                        .collect()
                },
            );

            if indices.is_empty() {
                // Single default orientation (e.g. Table or Culet)
                let n = if is_crown {
                    Vec3::new(0.0, theta.cos(), theta.sin())
                } else {
                    Vec3::new(0.0, -theta.cos(), theta.sin())
                };
                planes.push(GpuFacetPlane::new(n, offset));
            } else {
                for g in indices {
                    let phi = Self::index_to_azimuth(g, gear_teeth_f, 0.0);
                    let n = if is_crown {
                        Vec3::new(
                            theta.sin() * phi.cos(),
                            theta.cos(),
                            theta.sin() * phi.sin(),
                        )
                    } else {
                        Vec3::new(
                            theta.sin() * phi.cos(),
                            -theta.cos(),
                            theta.sin() * phi.sin(),
                        )
                    };
                    planes.push(GpuFacetPlane::new(n, offset));
                }
            }
        }

        if planes.len() < 4 {
            return Self::standard_round_brilliant();
        }

        planes
    }

    /// Reconstructs a validated boundary-representation solid from a cutting
    /// schedule's angle rows.
    ///
    /// This builds on [`Self::from_database_angles`] by using
    /// [`GemPolyhedron::from_planes`] as a plausibility gate on its output: a schedule
    /// whose planes don't actually bound a finite solid (or bound one only by leaving
    /// some of the schedule's own planes untouched -- an over-constrained, internally
    /// inconsistent schedule) renders silently wrong under the implicit half-space
    /// intersection alone, with no signal that anything is amiss. Here, either failure
    /// mode falls back to the same known-good `standard_round_brilliant()` cut that
    /// `from_database_angles` itself falls back to on badly unparseable input, and logs
    /// why.
    ///
    /// # Panics
    ///
    /// Panics only if `standard_round_brilliant()` itself ever failed to reconstruct
    /// into a valid polyhedron, which would indicate that reference cut regressed.
    #[must_use]
    pub fn reconstruct_validated_brep(angles: &[FacetSpec], gear_teeth: u32) -> GemPolyhedron {
        let planes = Self::from_database_angles(angles, gear_teeth);
        let plane_count = planes.len();

        match GemPolyhedron::from_planes(planes) {
            Ok(hull) => {
                let untouched = hull.untouched_planes();
                if untouched.is_empty() {
                    return hull;
                }
                warn!(
                    "reconstruct_validated_brep: {} of {plane_count} reconstructed planes contribute no facet \
                     (over-constrained/redundant schedule, plane indices {untouched:?}); falling back to \
                     standard_round_brilliant()",
                    untouched.len()
                );
            }
            Err(e) => {
                warn!(
                    "reconstruct_validated_brep: B-Rep reconstruction failed ({e}); falling back to \
                     standard_round_brilliant()"
                );
            }
        }

        GemPolyhedron::from_planes(Self::standard_round_brilliant()).expect(
            "standard_round_brilliant() must always reconstruct into a valid, finite polyhedron",
        )
    }
}
