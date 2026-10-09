//! The capture checklist of the first step (plan 4.1) and the words of the consistency warnings
//! the photometry module raises when the frames of a view do not match its stone photo.

use indicatrix_cut_core::rough_plan::photometry::{
    ConsistencyReport, ConsistencyWarning, FrameRole,
};

/// What to do at the rig before the first photo, one line each.
pub const CHECKLIST: [&str; 5] = [
    "Fix the exposure, ISO and white balance (or shoot RAW). No HDR or scene modes, and fixed focus.",
    "White frame, one per view: the empty rig with the backlight on, holder in place if it stays. It gives the light and the flat field.",
    "Dark frame, one per view: backlight off, lens not capped (it catches stray light).",
    "Stone photos, one per view, with the same settings. Optionally a second exposure 2 EV lower for bright, clear areas.",
    "Optional: photograph a few reference filters on the backlight once, for the camera calibration (Calibration step).",
];

/// The headline over [`CHECKLIST`].
pub const CHECKLIST_TITLE: &str = "Before you photograph";

/// The name of a frame for a sentence.
#[must_use]
pub fn frame_name(role: FrameRole) -> String {
    match role {
        FrameRole::White(i) => format!("white frame {}", i + 1),
        FrameRole::Dark(i) => format!("dark frame {}", i + 1),
    }
}

fn seconds(value: f64) -> String {
    if value > 0.0 && value < 1.0 {
        format!("1/{:.0} s", (1.0 / value).round())
    } else {
        format!("{value:.2} s")
    }
}

/// One warning as a sentence about the view `view_name`.
#[must_use]
pub fn warning_text(view_name: &str, warning: &ConsistencyWarning) -> String {
    match warning {
        ConsistencyWarning::ExposureMismatch {
            frame,
            stone,
            other,
        } => format!(
            "{view_name}: the {} was exposed {} but the stone {}. Shoot all frames with the same exposure.",
            frame_name(*frame),
            seconds(*other),
            seconds(*stone)
        ),
        ConsistencyWarning::IsoMismatch {
            frame,
            stone,
            other,
        } => format!(
            "{view_name}: the {} is ISO {other} but the stone photo is ISO {stone}.",
            frame_name(*frame)
        ),
        ConsistencyWarning::WhiteBalanceMismatch { frame, .. } => format!(
            "{view_name}: the {} has another white balance mode than the stone photo. Fix the white balance or shoot RAW.",
            frame_name(*frame)
        ),
        ConsistencyWarning::ApertureMismatch {
            frame,
            stone,
            other,
        } => format!(
            "{view_name}: the {} was shot at f/{other:.1} but the stone photo at f/{stone:.1}.",
            frame_name(*frame)
        ),
        ConsistencyWarning::SizeMismatch {
            frame,
            stone,
            other,
        } => format!(
            "{view_name}: the {} is {} x {} px but the stone photo is {} x {} px.",
            frame_name(*frame),
            other[0],
            other[1],
            stone[0],
            stone[1]
        ),
        ConsistencyWarning::MissingExif { frame } => frame.as_ref().map_or_else(
            || {
                format!(
                    "{view_name}: the stone photo has no shooting data (EXIF), so the frames cannot be compared with it."
                )
            },
            |role| {
                format!(
                    "{view_name}: the {} has no shooting data, so it cannot be compared with the stone photo.",
                    frame_name(*role)
                )
            },
        ),
        ConsistencyWarning::NoWhiteFrame => {
            format!("{view_name}: no white frame. The photo cannot be calibrated without one.")
        }
        ConsistencyWarning::Saturation { fraction } => format!(
            "{view_name}: {:.1} % of the stone photo is clipped. Those pixels are left out. Shoot a second, darker exposure for bright areas.",
            f64::from(*fraction) * 100.0
        ),
        ConsistencyWarning::NonLinearSource => format!(
            "{view_name}: the stone photo is not linear (8-bit or tone-mapped), so colours will be less certain. RAW is best."
        ),
    }
}

/// Every warning of a report, one sentence each, in the report's order.
#[must_use]
pub fn report_lines(view_name: &str, report: &ConsistencyReport) -> Vec<String> {
    report
        .warnings
        .iter()
        .map(|warning| warning_text(view_name, warning))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checklist_covers_the_capture_protocol() {
        assert_eq!(CHECKLIST.len(), 5);
        assert!(CHECKLIST[1].contains("White frame"));
        assert!(CHECKLIST[2].contains("Dark frame"));
        assert!(CHECKLIST.iter().all(|line| !line.contains("GemRay")));
    }

    #[test]
    fn exposure_warnings_name_the_frame_and_the_times() {
        let text = warning_text(
            "+X upper",
            &ConsistencyWarning::ExposureMismatch {
                frame: FrameRole::White(0),
                stone: 1.0 / 125.0,
                other: 1.0 / 60.0,
            },
        );
        assert!(text.contains("+X upper"));
        assert!(text.contains("white frame 1"));
        assert!(text.contains("1/60 s"));
        assert!(text.contains("1/125 s"));
    }

    #[test]
    fn every_warning_kind_has_words() {
        let warnings = [
            ConsistencyWarning::IsoMismatch {
                frame: FrameRole::Dark(1),
                stone: 100,
                other: 400,
            },
            ConsistencyWarning::WhiteBalanceMismatch {
                frame: FrameRole::White(0),
                stone: 0,
                other: 1,
            },
            ConsistencyWarning::ApertureMismatch {
                frame: FrameRole::White(0),
                stone: 8.0,
                other: 5.6,
            },
            ConsistencyWarning::SizeMismatch {
                frame: FrameRole::White(0),
                stone: [6000, 4000],
                other: [3000, 2000],
            },
            ConsistencyWarning::MissingExif { frame: None },
            ConsistencyWarning::MissingExif {
                frame: Some(FrameRole::Dark(0)),
            },
            ConsistencyWarning::NoWhiteFrame,
            ConsistencyWarning::Saturation { fraction: 0.05 },
            ConsistencyWarning::NonLinearSource,
        ];
        for warning in &warnings {
            let text = warning_text("V", warning);
            assert!(text.starts_with("V: "), "{text}");
            assert!(text.len() > 20);
        }
        let text = warning_text("V", &warnings[7]);
        assert!(text.contains("5.0 %"), "{text}");
    }

    #[test]
    fn a_report_gives_one_line_per_warning() {
        let report = ConsistencyReport {
            warnings: vec![
                ConsistencyWarning::NoWhiteFrame,
                ConsistencyWarning::NonLinearSource,
            ],
            saturated_fraction: 0.0,
        };
        assert_eq!(report_lines("V", &report).len(), 2);
        assert_eq!(
            report_lines("V", &ConsistencyReport::default()),
            [] as [String; 0]
        );
    }
}
