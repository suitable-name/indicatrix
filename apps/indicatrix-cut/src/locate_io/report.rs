//! The words of the locate windows: what the alignment, the solution and the calibration
//! tell the user, as lines the windows show.
//!
//! A line is a [`Row`]: its text and whether it is a warning (shown in amber).

use std::fmt::Write as _;

use indicatrix_cut_core::rough_plan::locate::{
    AlignOptions, AlignResult, Calibrated, InsideState, Located, LocatedPolyline, ViewPose,
    ViewStatus,
};

use super::pipeline::Found;

/// One line of a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The text.
    pub text: String,
    /// Whether the line is a warning.
    pub warn: bool,
}

impl Row {
    const fn info(text: String) -> Self {
        Self { text, warn: false }
    }

    const fn warning(text: String) -> Self {
        Self { text, warn: true }
    }
}

/// The caveat of the calibration's scale check, in plain words. It is shown next to the result
/// and written in the manual.
pub const SCALE_CAVEAT: &str = "The scale check only means something when the camera distances and the focal lengths are entered precisely. The image scale is the focal length times the cube's edge divided by the distance, so with loose values the fit can hide a wrong cube size in them. Measure the distances, take the focal lengths from a trusted calibration, and then read the ratio.";

/// The tooltip of the disabled Accept button for a line or polygon.
pub const LINES_NOT_ADDED: &str =
    "Feathers and silk can be located but not yet added as inclusions";

/// What a view's status means for its mark, as a clause.
#[must_use]
pub const fn status_words(status: ViewStatus) -> &'static str {
    match status {
        ViewStatus::Used => "used",
        ViewStatus::NoSurfaceHit => "the ray misses the stone",
        ViewStatus::FromInside => {
            "the ray starts inside the stone or meets its surface from behind"
        }
        ViewStatus::TotalInternalReflection => {
            "beyond the critical angle at the surface, so light from there cannot leave the stone in this direction and the mark cannot be a direct image"
        }
        ViewStatus::NoExit => "the ray never leaves the stone again (a damaged mesh)",
        ViewStatus::NoSuchView => "the rig has no such view",
    }
}

fn view_name(views: &[ViewPose], view: usize) -> String {
    views
        .get(view)
        .map_or_else(|| format!("View {}", view + 1), |pose| pose.name.clone())
}

/// A length in mm to two decimals.
fn mm(value: f64) -> String {
    format!("{value:.2} mm")
}

/// A distance in pixels to one decimal.
fn px(value: f64) -> String {
    format!("{value:.1} px")
}

/// The per-view misfit lines of an alignment, in the order the outlines were given.
#[must_use]
pub fn alignment_rows(result: &AlignResult, views: &[ViewPose]) -> Vec<Row> {
    let limit = AlignOptions::default().max_misfit_px;
    result
        .misfit
        .iter()
        .map(|misfit| {
            let text = format!(
                "{}: {} RMS, worst {}",
                view_name(views, misfit.view),
                px(misfit.rms_px),
                px(misfit.max_px)
            );
            if misfit.rms_px > limit {
                Row::warning(text)
            } else {
                Row::info(text)
            }
        })
        .collect()
}

/// The headline of an alignment: accepted with its worst misfit, or refused with the reason.
#[must_use]
pub fn alignment_summary(result: &AlignResult) -> String {
    if result.accepted {
        format!(
            "Aligned. The worst view misses the outline by {} RMS.",
            px(result.worst_rms_px)
        )
    } else {
        let note = result
            .note
            .as_deref()
            .unwrap_or("The mesh does not match the photos' outlines well enough.");
        format!(
            "Not accepted: the worst view misses the outline by {} RMS. {note}",
            px(result.worst_rms_px)
        )
    }
}

/// The headline of a located point.
fn point_summary(located: &Located) -> String {
    let [x, y, z] = located.point;
    let mut text = format!(
        "Located at ({x:.2}, {y:.2}, {z:.2}) mm in the rough's own coordinates. Uncertainty {} RMS over {} views; suggested margin {}.",
        mm(located.rms_mm),
        located.used_views,
        mm(located.suggested_margin_mm())
    );
    match located.inside {
        InsideState::Inside => {}
        InsideState::MovedInward => text.push_str(
            " The rays did not meet inside the stone, so the point was moved inward. Check the marks.",
        ),
        InsideState::Outside => text.push_str(
            " The rays do not meet inside the stone and the point could not be moved in. The marks do not agree.",
        ),
    }
    text
}

/// The headline of a located line or polygon.
fn line_summary(line: &LocatedPolyline) -> String {
    let kind = if line.closed { "polygon" } else { "line" };
    format!(
        "Located {kind} of {} vertices. Worst uncertainty {} RMS; suggested margin {}.",
        line.vertices.len(),
        mm(line.worst_rms_mm()),
        mm(line.suggested_margin_mm())
    )
}

/// The headline of a solution.
#[must_use]
pub fn found_summary(found: &Found) -> String {
    match found {
        Found::Point(located) => point_summary(located),
        Found::Line(line) => line_summary(line),
    }
}

/// The per-view lines of a located point: the residual, and the leave-one-out warning.
#[must_use]
pub fn point_rows(located: &Located, views: &[ViewPose]) -> Vec<Row> {
    located
        .views
        .iter()
        .map(|report| {
            let name = view_name(views, report.view);
            if report.status != ViewStatus::Used {
                return Row::warning(format!("{name}: not used, {}.", status_words(report.status)));
            }
            let mut text = format!(
                "{name}: {} from the solution",
                report.distance_mm.map_or_else(|| "?".to_owned(), mm)
            );
            let mut warn = false;
            if report.likely_outlier {
                warn = true;
                let _ = write!(
                    text,
                    " - this view's mark may be a reflected copy: it is {} from where the other views agree, which they do to {}",
                    report.leave_one_out_mm.map_or_else(|| "?".to_owned(), mm),
                    report.rms_without_mm.map_or_else(|| "?".to_owned(), mm)
                );
            }
            if report.beyond_segment {
                warn = true;
                text.push_str(" - the point lies beyond the stretch of this ray inside the stone");
            }
            if warn {
                Row::warning(text)
            } else {
                Row::info(text)
            }
        })
        .collect()
}

/// The per-vertex lines of a located line or polygon.
#[must_use]
pub fn line_rows(line: &LocatedPolyline) -> Vec<Row> {
    line.vertices
        .iter()
        .enumerate()
        .map(|(index, vertex)| {
            let [x, y, z] = vertex.point;
            let text = format!(
                "Vertex {}: ({x:.2}, {y:.2}, {z:.2}) mm, uncertainty {} RMS",
                index + 1,
                mm(vertex.rms_mm)
            );
            if vertex.outlier_views().is_empty() {
                Row::info(text)
            } else {
                Row::warning(format!(
                    "{text} - a view of this vertex may be a reflected copy"
                ))
            }
        })
        .collect()
}

/// The per-view lines of a solution.
#[must_use]
pub fn found_rows(found: &Found, views: &[ViewPose]) -> Vec<Row> {
    match found {
        Found::Point(located) => point_rows(located, views),
        Found::Line(line) => line_rows(line),
    }
}

/// The label drawn beside a reprojected point: the error against the click.
#[must_use]
pub fn reprojection_label(error_px: Option<f64>) -> String {
    error_px.map_or_else(String::new, px)
}

/// The label of a predicted ghost: how many reflections it takes.
#[must_use]
pub fn ghost_label(bounces: u8) -> String {
    if bounces == 1 {
        "ghost, 1 reflection".to_owned()
    } else {
        format!("ghost, {bounces} reflections")
    }
}

/// The lines of a calibration: the edge fit, the scale against the datasheet, the diagonal check
/// and the warnings, with the per-view misfit; the caveat is [`SCALE_CAVEAT`].
#[must_use]
pub fn calibration_rows(calibrated: &Calibrated, views: &[ViewPose]) -> Vec<Row> {
    let result = &calibrated.result;
    let mut rows = vec![Row::info(format!(
        "Edge fit: {} RMS over {} lines.",
        px(result.edge_rms_px),
        result.lines_used
    ))];
    for (view, rms) in calibrated.view_rms_px.iter().enumerate() {
        let name = view_name(views, view);
        let dot = calibrated
            .mark_error_px
            .get(view)
            .copied()
            .flatten()
            .map_or_else(String::new, |error| {
                format!(", dot {} from its corner", px(error))
            });
        rows.push(Row::info(rms.map_or_else(
            || format!("{name}: no edge lines{dot}"),
            |rms| format!("{name}: {} RMS{dot}", px(rms)),
        )));
    }
    let percent = (result.scale_ratio - 1.0) * 100.0;
    let scale = format!(
        "Scale: the fitted edge is {:.3} mm against the datasheet's {:.3} +/- {:.3} mm ({percent:+.2} %), {}.",
        result.fitted_edge_mm,
        result.datasheet_edge_mm,
        result.tolerance_mm,
        if result.scale_within_tolerance {
            "within the tolerance"
        } else {
            "outside the tolerance"
        }
    );
    rows.push(if result.scale_within_tolerance {
        Row::info(scale)
    } else {
        Row::warning(scale)
    });
    match (result.diagonal_rms_mm, result.diagonal_plane_rms_mm) {
        (Some(edges), Some(plane)) => rows.push(Row::info(format!(
            "Diagonal check: the triangulated edges are {} RMS from the true ones and {} RMS from the true plane. This is the rig's measured accuracy, and it is saved with the rig.",
            mm(edges),
            mm(plane)
        ))),
        _ => rows.push(Row::info(
            "No diagonal check was run: mark the coated diagonal's edges to measure the rig's accuracy.".to_owned(),
        )),
    }
    rows.extend(calibrated.warnings.iter().cloned().map(Row::warning));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::locate::{
        AlignResult, CalibrationResult, Projection, RigProfile, Rigid, ViewMisfit,
    };

    fn views() -> Vec<ViewPose> {
        RigProfile::side_layout(
            150.0,
            30.0,
            Projection::Pinhole { focal_px: 4000.0 },
            [4000, 3000],
        )
    }

    fn aligned(rms: f64, accepted: bool) -> AlignResult {
        AlignResult {
            transform: Rigid::IDENTITY,
            misfit: vec![
                ViewMisfit {
                    view: 0,
                    rms_px: 1.3,
                    max_px: 3.0,
                },
                ViewMisfit {
                    view: 3,
                    rms_px: rms,
                    max_px: rms * 2.0,
                },
            ],
            worst_rms_px: rms.max(1.3),
            accepted,
            note: (!accepted).then(|| "Check the outlines.".to_owned()),
            iterations: 5,
        }
    }

    #[test]
    fn alignment_lines_name_the_view_and_flag_a_misfit_over_the_limit() {
        let rows = alignment_rows(&aligned(6.0, false), &views());
        assert_eq!(rows[0].text, "+X upper: 1.3 px RMS, worst 3.0 px");
        assert!(!rows[0].warn);
        assert!(rows[1].warn, "6 px is over the 4 px limit");
        assert!(rows[1].text.starts_with("-X lower"), "{}", rows[1].text);
    }

    #[test]
    fn an_alignment_summary_says_accepted_or_why_not() {
        assert!(alignment_summary(&aligned(2.0, true)).starts_with("Aligned."));
        let refused = alignment_summary(&aligned(6.0, false));
        assert!(refused.starts_with("Not accepted"), "{refused}");
        assert!(refused.contains("Check the outlines."), "{refused}");
    }

    #[test]
    fn every_status_has_words() {
        for status in [
            ViewStatus::Used,
            ViewStatus::NoSurfaceHit,
            ViewStatus::FromInside,
            ViewStatus::TotalInternalReflection,
            ViewStatus::NoExit,
            ViewStatus::NoSuchView,
        ] {
            assert_ne!(status_words(status), "");
        }
        assert!(status_words(ViewStatus::NoSurfaceHit).contains("misses the stone"));
    }

    #[test]
    fn ghost_and_reprojection_labels() {
        assert_eq!(ghost_label(1), "ghost, 1 reflection");
        assert_eq!(ghost_label(2), "ghost, 2 reflections");
        assert_eq!(reprojection_label(Some(3.26)), "3.3 px");
        assert_eq!(reprojection_label(None), "");
    }

    #[test]
    fn a_calibration_reports_the_scale_and_the_diagonal_and_keeps_its_warnings() {
        let rig = RigProfile::new("t", views(), 1.5);
        let result = CalibrationResult {
            datasheet_edge_mm: 25.4,
            tolerance_mm: 0.1,
            cube_n_d: 1.5168,
            fitted_edge_mm: 25.55,
            scale_ratio: 25.55 / 25.4,
            scale_within_tolerance: false,
            edge_rms_px: 0.8,
            lines_used: 30,
            diagonal_rms_mm: Some(0.12),
            diagonal_plane_rms_mm: Some(0.05),
        };
        let calibrated = Calibrated {
            rig: rig.clone(),
            result,
            view_rms_px: vec![Some(0.7), None],
            mark_error_px: vec![Some(4.0), None],
            diagonal: None,
            fit_cost: 1.0,
            iterations: 7,
            warnings: vec!["A line was rejected.".to_owned()],
        };
        let rows = calibration_rows(&calibrated, &rig.views);
        let text: Vec<&str> = rows.iter().map(|row| row.text.as_str()).collect();
        assert!(text[0].contains("30 lines"), "{text:?}");
        assert!(
            text[1].starts_with("+X upper: 0.7 px RMS, dot 4.0 px"),
            "{text:?}"
        );
        assert!(text[2].contains("no edge lines"), "{text:?}");
        let scale = rows
            .iter()
            .find(|row| row.text.starts_with("Scale"))
            .unwrap();
        assert!(
            scale.warn && scale.text.contains("outside the tolerance"),
            "{scale:?}"
        );
        assert!(scale.text.contains("+0.59 %"), "{scale:?}");
        assert!(
            text.iter()
                .any(|t| t.contains("0.12 mm RMS from the true ones"))
        );
        assert!(rows.last().unwrap().warn, "the warnings come last");
    }
}
