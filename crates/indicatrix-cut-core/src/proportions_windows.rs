//! Proportion-window verdicts for a solved design.
//!
//! Verdicts ("within" / "near" / "outside") for a solved design's proportion
//! readouts (table %, crown angle, pavilion angle, total depth %, girdle
//! thickness %) against published/rule-of-thumb reference windows, keyed by
//! shape class and material band in the tables below. [`crate::yield_metrics::proportions`]
//! ([`indicatrix::geometry::stone_metrics::StoneProportions`]) already reports
//! the raw numbers; this module only judges them.
//!
//! # Scope and honesty
//!
//! Only [`ShapeClass::Round`] carries real, shape-specific windows (the round
//! brilliant is the one shape this app ships verified, closing templates for
//! today -- see [`crate::templates`]'s own doc comment on why the gallery
//! stops at round-brilliant variants). Every other [`ShapeClass`] falls back
//! to the SAME wide, material-independent "widely published lapidary rule of
//! thumb" windows [`MaterialClass::Mid`]/[`MaterialClass::Low`] already use for
//! Round -- there is no dedicated cushion/oval/step/trillion reference data in
//! this crate, and claiming otherwise would be exactly the "never claim more
//! than the model supports" mistake this module exists to avoid. See each
//! table entry's own `source` field for its citation.
//!
//! Windows are one-sided or two-sided ranges with a "near" margin: a value
//! inside `[min, max]` is [`Verdict::Within`]; a value within `near_margin` of
//! either bound (but outside it) is [`Verdict::Near`]; anything further is
//! [`Verdict::Outside`]. [`classify`] implements exactly that.

/// The faceting style a design's schedule was authored for -- decides which
/// reference window row [`table_percent_window`] and friends look up.
///
/// Classifying a real [`crate::design::Design`] into one of these is an
/// app-layer job (symmetry order, mirror flag, and eventually facet-shape
/// heuristics), not this module's: this module only judges an already-decided
/// class against a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeClass {
    /// Radially symmetric, 6-fold or higher, mirrored -- the round brilliant
    /// family (this crate's only verified-closing template shape today).
    Round,
    /// A rounded, elongated outline with no dedicated reference data here --
    /// judged against the same generic lapidary windows as
    /// [`Self::StepEmerald`]/[`Self::Trillion`]/[`Self::Other`].
    CushionOval,
    /// Rectangular step facets (an emerald cut's ladder) -- no dedicated
    /// reference data here; see this module's own top doc comment.
    StepEmerald,
    /// 3-fold symmetry -- no dedicated reference data here; see this module's
    /// own top doc comment.
    Trillion,
    /// Anything this app cannot confidently classify -- the honest default,
    /// judged against the same generic lapidary windows as the three shapes
    /// above.
    Other,
}

/// The material band a design's effective refractive index falls into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialClass {
    /// `n_d < 1.6` -- quartz and similar low-index colored stones.
    Low,
    /// `1.6 <= n_d < 1.8` -- most colored gemstones (sapphire, spinel,
    /// tourmaline).
    Mid,
    /// `n_d >= 1.8` -- diamond, zircon, cubic zirconia, synthetic moissanite.
    High,
}

impl MaterialClass {
    /// Classifies `n_d` into a [`MaterialClass`] band. The two boundaries
    /// (1.6, 1.8) match the `Low`/`Mid`/`High` cutoffs documented on
    /// [`MaterialClass`]'s own variants; `n_d` is read as-is with no domain
    /// check (the same "every real caller's
    /// `n` traces back to a resolved material" assumption
    /// [`crate::optics_hints`] documents).
    #[must_use]
    pub fn from_ri(n_d: f64) -> Self {
        if n_d < 1.6 {
            Self::Low
        } else if n_d < 1.8 {
            Self::Mid
        } else {
            Self::High
        }
    }
}

/// A judged value's relationship to its reference [`ProportionWindow`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Inside `[min, max]`.
    Within,
    /// Outside `[min, max]` but within `near_margin` of the nearer bound.
    Near,
    /// Further than `near_margin` past either bound.
    Outside,
}

/// One reference window: an inclusive `[min, max]` range, a "near" margin on
/// either side, a one-line reason shown alongside the verdict chip, and the
/// citation for where the range comes from.
#[derive(Debug, Clone, Copy)]
pub struct ProportionWindow {
    /// The reference range's lower bound.
    pub min: f64,
    /// The reference range's upper bound.
    pub max: f64,
    /// How far past `min`/`max` still reads as [`Verdict::Near`] rather than
    /// [`Verdict::Outside`].
    pub near_margin: f64,
    /// A one-line reason shown next to the verdict chip.
    pub reason: &'static str,
    /// Where this window's numbers come from, for the doc-comment citation
    /// and (eventually) a tooltip.
    pub source: &'static str,
}

/// Classifies `value` against `window` -- see this module's own top doc
/// comment for the three-band rule.
#[must_use]
pub fn classify(value: f64, window: &ProportionWindow) -> Verdict {
    if value >= window.min && value <= window.max {
        Verdict::Within
    } else if value >= window.min - window.near_margin && value <= window.max + window.near_margin {
        Verdict::Near
    } else {
        Verdict::Outside
    }
}

/// One metric this module carries a reference window for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// Table diameter as a percentage of the stone's own width.
    TablePercent,
    /// Crown angle from the girdle plane, in degrees.
    CrownAngle,
    /// Pavilion angle from the girdle plane, in degrees.
    PavilionAngle,
    /// Total depth as a percentage of the stone's own width.
    TotalDepthPercent,
    /// Girdle thickness as a percentage of the stone's own width.
    GirdleThicknessPercent,
}

/// One row of the static reference table -- see [`WINDOWS`].
struct Entry {
    shape: ShapeClass,
    material: MaterialClass,
    metric: Metric,
    window: ProportionWindow,
}

/// The reference-window table. A linear `const` slice, scanned by
/// [`window_for`] -- not a `HashMap`, per this crate's own "no HashMap/HashSet
/// in a decision path, static tables as const slices" house rule.
///
/// [`ShapeClass::Round`] carries three material-specific rows per metric
/// (diamond's own published AGS/GIA "Excellent" round-brilliant ranges for
/// [`MaterialClass::High`]; the widely published colored-stone lapidary rule
/// of thumb, 40-43 degrees pavilion / 30-40 degrees crown, for
/// [`MaterialClass::Low`]/[`MaterialClass::Mid`]). Every other shape class
/// reuses the SAME lapidary rule-of-thumb rows regardless of material band --
/// see this module's own top doc comment for why.
const WINDOWS: &[Entry] = &[
    // --- Round, High RI (diamond): AGS/GIA published "Excellent"/"Ideal"
    // round-brilliant ranges. ---
    Entry {
        shape: ShapeClass::Round,
        material: MaterialClass::High,
        metric: Metric::TablePercent,
        window: ProportionWindow {
            min: 53.0,
            max: 58.0,
            near_margin: 3.0,
            reason: "AGS/GIA \"Excellent\" round-brilliant table range for diamond.",
            source: "AGS/GIA published round-brilliant proportion ranges (diamond).",
        },
    },
    Entry {
        shape: ShapeClass::Round,
        material: MaterialClass::High,
        metric: Metric::CrownAngle,
        window: ProportionWindow {
            min: 34.0,
            max: 35.0,
            near_margin: 2.0,
            reason: "AGS/GIA \"Excellent\" round-brilliant crown-angle range for diamond.",
            source: "AGS/GIA published round-brilliant proportion ranges (diamond).",
        },
    },
    Entry {
        shape: ShapeClass::Round,
        material: MaterialClass::High,
        metric: Metric::PavilionAngle,
        window: ProportionWindow {
            min: 40.6,
            max: 41.8,
            near_margin: 1.5,
            reason: "AGS/GIA \"Excellent\" round-brilliant pavilion-angle range for diamond.",
            source: "AGS/GIA published round-brilliant proportion ranges (diamond).",
        },
    },
    Entry {
        shape: ShapeClass::Round,
        material: MaterialClass::High,
        metric: Metric::TotalDepthPercent,
        window: ProportionWindow {
            min: 59.0,
            max: 62.6,
            near_margin: 3.0,
            reason: "AGS/GIA \"Excellent\" round-brilliant total-depth range for diamond.",
            source: "AGS/GIA published round-brilliant proportion ranges (diamond).",
        },
    },
    Entry {
        shape: ShapeClass::Round,
        material: MaterialClass::High,
        metric: Metric::GirdleThicknessPercent,
        window: ProportionWindow {
            min: 1.5,
            max: 4.5,
            near_margin: 2.0,
            reason: "AGS/GIA thin-to-slightly-thick girdle range for diamond.",
            source: "AGS/GIA published round-brilliant proportion ranges (diamond).",
        },
    },
    // --- Round, Low/Mid RI (colored stones): widely published lapidary rule
    // of thumb. Both bands share the same numbers -- the rule of thumb does
    // not distinguish within this range. ---
    Entry {
        shape: ShapeClass::Round,
        material: MaterialClass::Mid,
        metric: Metric::TablePercent,
        window: ProportionWindow {
            min: 53.0,
            max: 65.0,
            near_margin: 5.0,
            reason: "Widely published lapidary rule of thumb for colored round brilliants.",
            source: "Lapidary rule of thumb (colored stones), not a formal grading standard.",
        },
    },
    Entry {
        shape: ShapeClass::Round,
        material: MaterialClass::Mid,
        metric: Metric::CrownAngle,
        window: ProportionWindow {
            min: 30.0,
            max: 40.0,
            near_margin: 3.0,
            reason: "Widely published 30-40 degree lapidary crown-angle rule of thumb.",
            source: "Lapidary rule of thumb (colored stones), not a formal grading standard.",
        },
    },
    Entry {
        shape: ShapeClass::Round,
        material: MaterialClass::Mid,
        metric: Metric::PavilionAngle,
        window: ProportionWindow {
            min: 40.0,
            max: 43.0,
            near_margin: 2.0,
            reason: "Widely published 40-43 degree lapidary pavilion-angle rule of thumb.",
            source: "Lapidary rule of thumb (colored stones), not a formal grading standard.",
        },
    },
    Entry {
        shape: ShapeClass::Round,
        material: MaterialClass::Mid,
        metric: Metric::TotalDepthPercent,
        window: ProportionWindow {
            min: 55.0,
            max: 70.0,
            near_margin: 5.0,
            reason: "Loose rule-of-thumb total-depth range for colored round brilliants.",
            source: "Lapidary rule of thumb (colored stones), not a formal grading standard.",
        },
    },
    Entry {
        shape: ShapeClass::Round,
        material: MaterialClass::Mid,
        metric: Metric::GirdleThicknessPercent,
        window: ProportionWindow {
            min: 1.0,
            max: 4.0,
            near_margin: 2.0,
            reason: "Loose rule-of-thumb girdle-thickness range for colored stones.",
            source: "Lapidary rule of thumb (colored stones), not a formal grading standard.",
        },
    },
];

/// The reference window for `shape`/`material`/`metric`, or `None` when this
/// table carries no row for that exact combination.
///
/// Fallback order (documented, not silent): an exact `(shape, material)` row
/// first; failing that, [`ShapeClass::Round`]'s own [`MaterialClass::Mid`] row
/// for the same metric -- the generic lapidary rule of thumb every non-Round
/// shape and every non-High material uses, per this module's own top doc
/// comment. [`MaterialClass::Low`] deliberately maps to the SAME `Mid` rows
/// (the rule of thumb does not distinguish the two bands); only
/// [`MaterialClass::High`] on [`ShapeClass::Round`] has its own dedicated row.
#[must_use]
pub fn window_for(
    shape: ShapeClass,
    material: MaterialClass,
    metric: Metric,
) -> Option<&'static ProportionWindow> {
    WINDOWS
        .iter()
        .find(|e| e.shape == shape && e.material == material && e.metric == metric)
        .or_else(|| {
            WINDOWS.iter().find(|e| {
                e.shape == ShapeClass::Round
                    && e.material == MaterialClass::Mid
                    && e.metric == metric
            })
        })
        .map(|e| &e.window)
}

/// Looks up `metric`'s window and classifies `value` against it.
///
/// Convenience wrapper: looks up `metric`'s window for `shape`/`material`
/// (via [`window_for`]) and classifies `value` against it in one call. `None`
/// when [`window_for`] finds nothing at all (never happens today: the
/// `Round`/`Mid` fallback always resolves for every metric this table
/// lists).
#[must_use]
pub fn verdict_for(
    shape: ShapeClass,
    material: MaterialClass,
    metric: Metric,
    value: f64,
) -> Option<(Verdict, &'static ProportionWindow)> {
    let window = window_for(shape, material, metric)?;
    Some((classify(value, window), window))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_class_boundaries() {
        assert_eq!(MaterialClass::from_ri(1.54), MaterialClass::Low);
        assert_eq!(MaterialClass::from_ri(1.599), MaterialClass::Low);
        assert_eq!(MaterialClass::from_ri(1.6), MaterialClass::Mid);
        assert_eq!(MaterialClass::from_ri(1.76), MaterialClass::Mid);
        assert_eq!(MaterialClass::from_ri(1.799), MaterialClass::Mid);
        assert_eq!(MaterialClass::from_ri(1.8), MaterialClass::High);
        assert_eq!(MaterialClass::from_ri(2.417), MaterialClass::High);
    }

    #[test]
    fn classify_boundaries_are_inclusive_on_within() {
        let window = ProportionWindow {
            min: 40.0,
            max: 43.0,
            near_margin: 2.0,
            reason: "",
            source: "",
        };
        assert_eq!(classify(40.0, &window), Verdict::Within);
        assert_eq!(classify(43.0, &window), Verdict::Within);
        assert_eq!(classify(39.999, &window), Verdict::Near);
        assert_eq!(classify(38.0, &window), Verdict::Near);
        assert_eq!(classify(37.999, &window), Verdict::Outside);
        assert_eq!(classify(45.001, &window), Verdict::Outside);
        assert_eq!(classify(45.0, &window), Verdict::Near);
    }

    /// A round brilliant in sapphire (`n_d` 1.76, `MaterialClass::Mid`) at the
    /// standard 41-degree pavilion / 34.5-degree crown must read Within on
    /// both angle metrics -- the acceptance case this module was built for.
    #[test]
    fn round_brilliant_in_sapphire_reads_within_for_standard_angles() {
        let material = MaterialClass::from_ri(1.76);
        assert_eq!(material, MaterialClass::Mid);
        let (pavilion_verdict, _) =
            verdict_for(ShapeClass::Round, material, Metric::PavilionAngle, 41.0).unwrap();
        assert_eq!(pavilion_verdict, Verdict::Within);
        let (crown_verdict, _) =
            verdict_for(ShapeClass::Round, material, Metric::CrownAngle, 34.5).unwrap();
        assert_eq!(crown_verdict, Verdict::Within);
    }

    /// A round brilliant diamond at the standard 41-degree pavilion must read
    /// Within against diamond's own tighter AGS/GIA window, and a shallow
    /// 38-degree pavilion must read Outside it (still a real cut, just not an
    /// "Excellent" one).
    #[test]
    fn round_brilliant_diamond_uses_its_own_tighter_window() {
        let material = MaterialClass::from_ri(2.417);
        assert_eq!(material, MaterialClass::High);
        let (good, window) =
            verdict_for(ShapeClass::Round, material, Metric::PavilionAngle, 41.2).unwrap();
        assert_eq!(good, Verdict::Within);
        assert!((window.min - 40.6).abs() < 1e-9);
        let (shallow, _) =
            verdict_for(ShapeClass::Round, material, Metric::PavilionAngle, 38.0).unwrap();
        assert_eq!(shallow, Verdict::Outside);
    }

    /// Every non-Round shape falls back to the generic Round/Mid lapidary
    /// window for every metric -- never silently `None`.
    #[test]
    fn non_round_shapes_fall_back_to_the_generic_lapidary_window() {
        for shape in [
            ShapeClass::CushionOval,
            ShapeClass::StepEmerald,
            ShapeClass::Trillion,
            ShapeClass::Other,
        ] {
            for metric in [
                Metric::TablePercent,
                Metric::CrownAngle,
                Metric::PavilionAngle,
                Metric::TotalDepthPercent,
                Metric::GirdleThicknessPercent,
            ] {
                let window = window_for(shape, MaterialClass::High, metric);
                assert!(window.is_some(), "{shape:?}/{metric:?} must fall back");
            }
        }
    }
}
