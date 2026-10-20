//! Retarget analysis and manufacturing checks, with a mandatory original-meet gate.
//! Geometry is captured before Shift and retained independently of mutable analyses.

use glam::DVec3;
use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::{SolidMesh, SolidStatus, StoneProportions, build_solid_mesh, measure_solid},
    tool::ToolPrimitive,
};
use indicatrix_cut_core::{
    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, ManufacturabilityWarning,
    check_manufacturability,
    design::{
        girdle_band::girdle_band_in,
        hinge::{
            FacetSide, SolidFacets, TierHinge, solid_facets_in, tier_hinges_in, tier_plane_ranges,
        },
    },
    optics_hints::is_horizontal_angle_deg,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

/// The thinnest girdle a retarget may leave, as a fraction of the original thickness. It
/// applies to the overall girdle figure and, separately, to the band's thinnest point.
pub const MIN_GIRDLE_FRACTION: f64 = 0.5;

/// A thinnest point at or below this many percent of the width is a knife edge: the crown-side
/// and pavilion-side edges of the girdle meet. A backstop only.
///
/// [`analyze`] reads the thinnest point through
/// [`indicatrix_cut_core::design::girdle_band::corner_resolution`], which snaps every gap at or
/// under that resolution to exactly zero. So a reading is either `0.0` or larger than the
/// resolution (1e-5 of the stone's size, a few thousandths of a percent of its width), and the
/// real floor under which a corner counts as a knife edge is that resolution, not this figure.
pub const KNIFE_EDGE_PERCENT: f64 = 1e-4;

/// The headline names the thinnest point only when it differs from the girdle figure by more
/// than this many percentage points: otherwise there is nothing new to say.
const THINNEST_SHOWN_FROM: f64 = 0.05;

/// Tiers this close to vertical are girdle facets (matches the solver's own rule).
const GIRDLE_COS_EPS: f64 = 1e-6;

/// The narrowest stone width a percentage is read against (the same floor `StoneProportions`
/// uses).
const MIN_WIDTH: f64 = 1e-9;

/// The own facet ring of a flat tier (the table, the culet or any other horizontal plane).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlatRing {
    /// The tier's position in `design.tiers`.
    pub tier_index: usize,
    /// Which side of the girdle the tier faces.
    pub side: FacetSide,
    /// The ring's area, in the design's own mast units squared.
    pub area: f64,
    /// The ring's height (`y`).
    pub height: f64,
}

/// Everything the gate needs to know about one solved stone.
#[derive(Debug, Clone)]
pub struct StoneAnalysis {
    /// Immutable original network, captured before any geometric adjustment.
    pub preservation:
        Option<std::sync::Arc<indicatrix_cut_core::meet_preservation::MeetPreservationBaseline>>,
    /// Authored state that produced this analysis; never reconstructed from summary figures.
    pub authored: Option<std::sync::Arc<Design>>,
    /// The solved masts the figures below come from.
    pub solved: Vec<SolvedTier>,
    /// The stone's planes (`planes_from_solved`), as `(normal, offset)`.
    pub planes: Vec<(DVec3, f64)>,
    /// Which planes of `planes` each tier contributes, in tier order.
    pub ranges: Vec<Range<usize>>,
    /// The girdle thickness as a percentage of the stone's width, if there is a girdle.
    pub girdle_percent: Option<f64>,
    /// The girdle band's thinnest point as a percentage of the stone's width: the smallest
    /// vertical gap between its crown-side and pavilion-side edges at any corner, if a girdle
    /// wall is live. Never more than the overall figure in `girdle_percent`, which reads the
    /// highest and the lowest wall vertex.
    pub girdle_thinnest_percent: Option<f64>,
    /// The table width as a percentage of the stone's width, if there is a table.
    pub table_percent: Option<f64>,
    /// The crown height (the top of the solid above the girdle band), in the stone's own
    /// units, if there is a live girdle.
    pub crown_height: Option<f64>,
    /// The pavilion depth (the bottom of the solid below the girdle band), if there is a
    /// live girdle.
    pub pavilion_depth: Option<f64>,
    /// `crown_height / pavilion_depth`: the stone's silhouette. `None` unless both are known
    /// and the depth is a real length.
    pub crown_to_pavilion: Option<f64>,
    /// The total depth (table to culet) as a percentage of the stone's width (H/W).
    pub depth_percent: Option<f64>,
    /// Which facets of which tier reach the solid's surface.
    pub facets: SolidFacets,
    /// The own ring of every flat tier that has one.
    pub flats: Vec<FlatRing>,
    /// The lowest and highest height of the girdle tiers' rings, if any is live.
    pub girdle_band: Option<(f64, f64)>,
    /// Tiers whose facets are already too small to cut.
    pub undersized: BTreeSet<usize>,
    /// The girdle corners (hinge) of every flat crown or pavilion tier (empty unless asked for).
    pub hinges: BTreeMap<usize, TierHinge>,
}

impl StoneAnalysis {
    /// The concave tools cut out of this stone's [`Self::planes`]: the design this analysis
    /// measured ([`Self::authored`]) resolves its concave tiers against them
    /// ([`Design::tools_for_scoring`], the placement the Compare window draws). Empty for a
    /// design without concave tiers, for tiers that do not resolve, and for an analysis built
    /// without its design.
    ///
    /// What Retarget measures its optical columns on, so the figures describe the stone with
    /// its tools and not its flat shell.
    #[must_use]
    pub fn tools(&self) -> Vec<ToolPrimitive> {
        self.authored
            .as_ref()
            .map(|design| design.tools_for_scoring(&self.planes))
            .unwrap_or_default()
    }

    /// This stone with its girdle band thickened by `delta` (in the stone's own units): the
    /// crown half is translated up by `delta / 2` and the pavilion half down by `delta / 2`.
    ///
    /// Only what re-anchoring reads is moved: every crown-side hinge point `y += delta / 2`,
    /// every pavilion-side one `y -= delta / 2`, the offset of every tier plane that leans up
    /// or down by `normal.y * shift` (so a hinge still lies on the planes it lay on), the
    /// flat rings' heights and the girdle band's ends. Vertical (girdle) planes, the preform's
    /// planes and the figures measured on the stone (percentages, areas, `solved`) are left
    /// as they were: the plan view, the table and culet area and the crown-to-pavilion ratio
    /// are all translation invariant, and the gate always judges against the UNSPLIT stone.
    #[must_use]
    pub fn split_at_girdle(&self, delta: f64) -> Self {
        let half = 0.5 * delta;
        let shift_of = |side: FacetSide| match side {
            FacetSide::Crown => half,
            FacetSide::Pavilion => -half,
        };
        let mut split = self.clone();
        for hinge in split.hinges.values_mut() {
            hinge.point.y += shift_of(hinge.side);
        }
        for flat in &mut split.flats {
            flat.height += shift_of(flat.side);
        }
        for (tier_index, range) in self.ranges.iter().enumerate() {
            // The tier's own side decides: a tier with a hinge (or a flat ring) moves with
            // that hinge, so a plane leaning just off vertical still lies where the
            // translated hinge does. Only a tier with neither falls back to the lean.
            let tier_side = self
                .hinges
                .get(&tier_index)
                .map(|hinge| hinge.side)
                .or_else(|| {
                    self.flats
                        .iter()
                        .find(|flat| flat.tier_index == tier_index)
                        .map(|flat| flat.side)
                });
            for index in range.clone() {
                if let Some((normal, offset)) = split.planes.get_mut(index) {
                    let shift = if let Some(side) = tier_side {
                        shift_of(side)
                    } else if normal.y > MIN_VERTICAL_NORMAL {
                        half
                    } else if normal.y < -MIN_VERTICAL_NORMAL {
                        -half
                    } else {
                        continue;
                    };
                    *offset = normal.y.mul_add(shift, *offset);
                }
            }
        }
        split.girdle_band = split
            .girdle_band
            .map(|(low, high)| (low - half, high + half));
        split
    }
}

/// A plane whose normal leans less than this from the horizontal is a girdle wall: it does not
/// move when a half of the stone is translated vertically.
const MIN_VERTICAL_NORMAL: f64 = 1e-9;

/// Why a retargeted design is not acceptable.
#[derive(Debug, Clone, PartialEq)]
pub enum InvalidReason {
    /// A hard original-meet or shape-preservation failure.
    Preservation(String),
    /// The design does not solve.
    DoesNotSolve(String),
    /// The facets no longer enclose a finite stone.
    NotClosed,
    /// The girdle has no facet left.
    GirdleGone,
    /// The girdle is much thinner than before.
    GirdleTooThin {
        /// The original thickness, percent of the stone's width.
        was_percent: f64,
        /// The new thickness, percent of the stone's width.
        now_percent: f64,
    },
    /// The girdle gets too thin at its corners: its thinnest point is much thinner than
    /// before, or runs to a knife edge. The overall girdle figure can stay exactly as it was.
    GirdleThinAtCorners {
        /// The original thinnest point, percent of the stone's width.
        was_percent: f64,
        /// The new thinnest point, percent of the stone's width.
        now_percent: f64,
    },
    /// A table or culet ended up on the wrong side of the girdle.
    FlatOffGirdle {
        /// The side the flat facet should be on.
        side: FacetSide,
    },
    /// A tier lost every facet.
    TierLost {
        /// The tier's name.
        name: String,
    },
    /// A tier lost some of its facets.
    TierPartlyLost {
        /// The tier's name.
        name: String,
        /// How many facets disappear.
        lost: usize,
        /// How many facets the tier has.
        total: usize,
    },
    /// A tier's facets became too small to cut.
    FacetsTooSmall {
        /// The tier's name.
        name: String,
    },
    /// A tier that follows a relation cannot follow it once the tiers it reads have moved
    /// (the result is not a facet angle, or the relations read each other in a loop).
    RelationFails(String),
}

impl InvalidReason {
    /// The reason as one plain sentence for the dialog.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Preservation(why) => why.clone(),
            Self::DoesNotSolve(why) => {
                format!("The retargeted design cannot be solved ({why}).")
            }
            Self::NotClosed => "The retargeted facets no longer enclose a stone.".to_string(),
            Self::GirdleGone => "The girdle disappears.".to_string(),
            Self::GirdleTooThin {
                was_percent,
                now_percent,
            } => format!(
                "The girdle gets much thinner: {now_percent:.1} % of the width, was {was_percent:.1} %."
            ),
            Self::GirdleThinAtCorners {
                was_percent,
                now_percent,
            } if *now_percent <= KNIFE_EDGE_PERCENT => format!(
                "The girdle runs to a knife edge at its corners (was {was_percent:.2} % of the width at its thinnest point)."
            ),
            Self::GirdleThinAtCorners {
                was_percent,
                now_percent,
            } => format!(
                "The girdle gets too thin at its corners: {now_percent:.2} % of the width at its thinnest point, was {was_percent:.2} %."
            ),
            Self::FlatOffGirdle {
                side: FacetSide::Crown,
            } => "The table would sit below the top of the girdle.".to_string(),
            Self::FlatOffGirdle {
                side: FacetSide::Pavilion,
            } => "The culet would sit above the bottom of the girdle.".to_string(),
            Self::TierLost { name } => format!("{name}: all its facets disappear."),
            Self::TierPartlyLost { name, lost, total } => {
                format!("{name}: {lost} of {total} facets disappear.")
            }
            Self::FacetsTooSmall { name } => {
                format!("{name}: its facets become too small to cut.")
            }
            Self::RelationFails(why) => format!(
                "A tier that follows a relation cannot follow it after this change ({}).",
                why.trim_end_matches('.')
            ),
        }
    }
}

/// How a retarget's masts were chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetargetStrategy {
    /// Only angles changed: nothing in the design was a pinned mast that could be
    /// re-anchored (every moving facet re-solves from its meets).
    AnglesOnly,
    /// Every pinned facet turns about its girdle corners, so the girdle keeps its outline
    /// and height.
    Anchored,
    /// As [`Self::Anchored`], and the table and culet heights are refitted to keep their size.
    AnchoredRefit,
    /// The Optimize search: every crown and pavilion facet may take any angle within the
    /// range, and turns about its girdle corners, so the girdle stays; the table and culet
    /// heights are refitted to keep their size.
    Optimized,
}

impl RetargetStrategy {
    /// One sentence for the dialog.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::AnglesOnly => "Only the angles change; the facets re-solve from their meets.",
            Self::Anchored => "Each facet turns about its girdle edge, so the girdle stays.",
            Self::AnchoredRefit => {
                "Each facet turns about its girdle edge, and the table and culet heights are refitted to keep their size."
            }
            Self::Optimized => {
                "Each crown and pavilion angle was searched within your range. Every facet turns about its girdle edge, so the girdle stays, and the table and culet heights are refitted to keep their size."
            }
        }
    }
}

/// The overall verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidityStatus {
    /// The retargeted stone passes every check.
    Valid,
    /// At least one check fails: Apply is disabled.
    Invalid,
    /// The current design itself does not solve, so nothing could be compared. Apply is
    /// disabled too: an unchecked check has no candidate stone and no baseline, so there is
    /// no change that could be applied ([`RetargetValidity::allows_apply`] is `Valid` only).
    Unchecked,
}

/// The girdle and table figures the verdict line quotes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ValidityFigures {
    /// Girdle thickness before, percent of the width.
    pub girdle_was: Option<f64>,
    /// Girdle thickness after.
    pub girdle_now: Option<f64>,
    /// The girdle's thinnest point before, percent of the width.
    pub thinnest_was: Option<f64>,
    /// The girdle's thinnest point after.
    pub thinnest_now: Option<f64>,
    /// Table width before, percent of the width.
    pub table_was: Option<f64>,
    /// Table width after.
    pub table_now: Option<f64>,
    /// Crown height over pavilion depth before: the silhouette.
    pub ratio_was: Option<f64>,
    /// Crown height over pavilion depth after.
    pub ratio_now: Option<f64>,
    /// Total depth before, percent of the width (H/W).
    pub depth_was: Option<f64>,
    /// Total depth after.
    pub depth_now: Option<f64>,
    /// How much the girdle band was thickened to keep its corners, as a percentage of the
    /// original band thickness (the girdle allowance's rung that was used), or `None` when
    /// the girdle keeps its thickness.
    pub girdle_thickened_percent: Option<f64>,
}

/// The gate's result for one retarget candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct RetargetValidity {
    /// The verdict.
    pub status: ValidityStatus,
    /// Every reason the candidate is invalid (empty unless `status` is `Invalid`).
    pub reasons: Vec<InvalidReason>,
    /// Things worth knowing that do not make the candidate invalid.
    pub warnings: Vec<String>,
    /// The figures behind the verdict line.
    pub figures: ValidityFigures,
    /// How the masts were chosen.
    pub strategy: RetargetStrategy,
}

fn percent(value: Option<f64>, decimals: usize) -> String {
    value.map_or_else(|| "none".to_string(), |v| format!("{v:.decimals$} %"))
}

impl RetargetValidity {
    /// A verdict for a design that could not be compared at all.
    #[must_use]
    pub fn unchecked(note: &InvalidReason) -> Self {
        Self {
            status: ValidityStatus::Unchecked,
            reasons: vec![note.clone()],
            warnings: Vec::new(),
            figures: ValidityFigures::default(),
            strategy: RetargetStrategy::AnglesOnly,
        }
    }

    /// The verdict for a candidate [`judge`] returned `reasons` for (empty means valid),
    /// with the girdle and table figures read from `original` and the candidate's own
    /// analysis.
    #[must_use]
    pub fn judged(
        original: &StoneAnalysis,
        candidate: &Result<StoneAnalysis, InvalidReason>,
        reasons: Vec<InvalidReason>,
        strategy: RetargetStrategy,
    ) -> Self {
        let now = candidate.as_ref().ok();
        let girdle_now = now.and_then(|analysis| analysis.girdle_percent);
        let thinnest_now = now.and_then(|analysis| analysis.girdle_thinnest_percent);
        let table_now = now.and_then(|analysis| analysis.table_percent);
        let ratio_now = now.and_then(|analysis| analysis.crown_to_pavilion);
        let depth_now = now.and_then(|analysis| analysis.depth_percent);
        let warnings = now.map_or_else(Vec::new, |analysis| warnings_for(original, analysis));
        Self {
            status: if reasons.is_empty() {
                ValidityStatus::Valid
            } else {
                ValidityStatus::Invalid
            },
            reasons,
            warnings,
            figures: ValidityFigures {
                girdle_was: original.girdle_percent,
                girdle_now,
                thinnest_was: original.girdle_thinnest_percent,
                thinnest_now,
                table_was: original.table_percent,
                table_now,
                ratio_was: original.crown_to_pavilion,
                ratio_now,
                depth_was: original.depth_percent,
                depth_now,
                girdle_thickened_percent: None,
            },
            strategy,
        }
    }

    /// This verdict with the girdle allowance's rung recorded: the band was thickened by
    /// `fraction` of its original thickness (`None` or a non-positive value records nothing).
    #[must_use]
    pub fn with_girdle_thickened(mut self, fraction: Option<f64>) -> Self {
        self.figures.girdle_thickened_percent = fraction
            .filter(|fraction| fraction.is_finite() && *fraction > 0.0)
            .map(|fraction| 100.0 * fraction);
        self
    }

    /// `true` when Apply may go ahead: only a valid verdict. An invalid one fails a check, and
    /// an unchecked one has no candidate stone to apply.
    #[must_use]
    pub fn allows_apply(&self) -> bool {
        self.status == ValidityStatus::Valid
    }

    /// The thinnest-point clause of the headline, `, thinnest point 0.40 % (was 0.25 %)`, or
    /// nothing when the figures are missing or the thinnest point is no different from the
    /// girdle figure the headline already quotes.
    fn thinnest_clause(&self) -> String {
        let figures = &self.figures;
        let (Some(now), Some(was)) = (figures.thinnest_now, figures.thinnest_was) else {
            return String::new();
        };
        let adds_nothing = figures
            .girdle_now
            .is_some_and(|girdle| (girdle - now).abs() <= THINNEST_SHOWN_FROM);
        if adds_nothing {
            String::new()
        } else {
            format!(", thinnest point {now:.2} % (was {was:.2} %)")
        }
    }

    /// The depth clause of the headline, `, depth 61 % (was 58 %)`, or nothing when either
    /// depth figure is missing.
    fn depth_clause(&self) -> String {
        match (self.figures.depth_now, self.figures.depth_was) {
            (Some(now), Some(was)) => format!(", depth {now:.0} % (was {was:.0} %)"),
            _ => String::new(),
        }
    }

    /// The one-line verdict, e.g. `Valid: girdle 2.1 % (was 2.3 %), table 56 % (was 56 %),
    /// depth 61 % (was 58 %)` or `Not valid: The girdle disappears.`
    ///
    /// A valid verdict also names the girdle's thinnest point, `girdle 5.9 % (was 5.9 %),
    /// thinnest point 0.40 % (was 0.25 %), table ...`, when that differs from the girdle
    /// figure: the overall figure cannot show a thin corner. The depth clause closes the
    /// line when both depth figures are known, so the cutter sees the silhouette change.
    #[must_use]
    pub fn headline(&self) -> String {
        match self.status {
            ValidityStatus::Valid => format!(
                "Valid: girdle {} (was {}){}, table {} (was {}){}",
                percent(self.figures.girdle_now, 1),
                percent(self.figures.girdle_was, 1),
                self.thinnest_clause(),
                percent(self.figures.table_now, 0),
                percent(self.figures.table_was, 0),
                self.depth_clause(),
            ),
            ValidityStatus::Invalid => {
                let first = self
                    .reasons
                    .first()
                    .map_or_else(String::new, InvalidReason::message);
                format!("Not valid: {first}")
            }
            ValidityStatus::Unchecked => {
                let why = self
                    .reasons
                    .first()
                    .map_or_else(String::new, InvalidReason::message);
                format!("Could not check this change. The current design has a problem: {why}")
            }
        }
    }

    /// The reasons after the first, one sentence each (the headline already quotes the
    /// first), then the crown-to-pavilion ratio line when both ratios are known, then the
    /// warnings.
    #[must_use]
    pub fn detail_lines(&self) -> Vec<String> {
        let skip = usize::from(self.status == ValidityStatus::Invalid);
        let mut lines: Vec<String> = if self.status == ValidityStatus::Unchecked {
            Vec::new()
        } else {
            self.reasons
                .iter()
                .skip(skip)
                .map(InvalidReason::message)
                .collect()
        };
        if self.status != ValidityStatus::Unchecked
            && let (Some(now), Some(was)) = (self.figures.ratio_now, self.figures.ratio_was)
        {
            lines.push(format!("Crown-to-pavilion ratio {now:.2} (was {was:.2})."));
        }
        if self.status == ValidityStatus::Valid
            && let Some(percent) = self.figures.girdle_thickened_percent
        {
            lines.push(format!(
                "The girdle was thickened by {percent:.0} % so its corners keep their thickness; plan view, table and culet size and crown-to-pavilion ratio are unchanged."
            ));
        }
        lines.extend(self.warnings.iter().cloned());
        lines
    }
}

mod geometry;
pub use geometry::polygon_area;
use geometry::{mean_height, rings_by_plane};

mod analysis;
pub use analysis::{analyze, analyze_controlled, analyze_solved};

/// Whether a girdle whose thinnest point was `was_percent` of the width and is now `now_percent`
/// has thinned out at its corners: it runs to a knife edge, or it is under
/// [`MIN_GIRDLE_FRACTION`] of what it was. A girdle that was a knife edge already has nothing to
/// lose.
fn thins_out_at_corners(was_percent: f64, now_percent: f64) -> bool {
    was_percent > KNIFE_EDGE_PERCENT
        && (now_percent <= KNIFE_EDGE_PERCENT || now_percent < MIN_GIRDLE_FRACTION * was_percent)
}

/// Compares a candidate with the original and returns every reason it is not acceptable.
///
/// `design` is the original design (for tier names; a retarget never adds or removes
/// tiers, so the candidate's tier positions are the same). An empty result means valid.
#[must_use]
pub fn judge(
    design: &Design,
    original: &StoneAnalysis,
    candidate: &Result<StoneAnalysis, InvalidReason>,
) -> Vec<InvalidReason> {
    let candidate = match candidate {
        Ok(analysis) => analysis,
        Err(reason) => return vec![reason.clone()],
    };
    let mut reasons = Vec::new();
    match (&original.preservation, &candidate.authored) {
        (Some(baseline), Some(authored)) => {
            if let Err(error) = baseline.validate(
                authored,
                indicatrix_cut_core::meet_preservation::PreservationPolicy::RETARGET,
            ) {
                reasons.push(InvalidReason::Preservation(error.to_string()));
            }
        }
        _ => reasons.push(InvalidReason::Preservation(
            "Original meet validation is missing; recompute this candidate.".into(),
        )),
    }
    reasons.extend(stone_reasons(design, original, candidate));
    reasons
}

/// [`judge`] without the meet-preservation check: every reason the finished STONE is not
/// acceptable (girdle, flats, lost or partly lost tiers, undersized facets).
///
/// For a change that is not a retarget and cannot keep the original meet network by its
/// nature, such as the verdict's "Add a table", which cuts the crown's apex away and adds a
/// facet. Those fixes judge the stone alone; a retarget or an optimizer result must go
/// through [`judge`].
#[must_use]
pub fn judge_stone(
    design: &Design,
    original: &StoneAnalysis,
    candidate: &Result<StoneAnalysis, InvalidReason>,
) -> Vec<InvalidReason> {
    match candidate {
        Ok(analysis) => stone_reasons(design, original, analysis),
        Err(reason) => vec![reason.clone()],
    }
}

/// The stone checks of [`judge_stone`] for a candidate that did analyse.
fn stone_reasons(
    design: &Design,
    original: &StoneAnalysis,
    candidate: &StoneAnalysis,
) -> Vec<InvalidReason> {
    let name_of = |index: usize| {
        design
            .tiers
            .get(index)
            .map_or_else(String::new, |tier| tier.name.clone())
    };
    let mut reasons = Vec::new();

    if let Some(was) = original.girdle_percent {
        match candidate.girdle_percent {
            None => reasons.push(InvalidReason::GirdleGone),
            Some(now) if now < MIN_GIRDLE_FRACTION * was => {
                reasons.push(InvalidReason::GirdleTooThin {
                    was_percent: was,
                    now_percent: now,
                });
            }
            Some(_) => {}
        }
    }

    if let (Some(was), Some(now)) = (
        original.girdle_thinnest_percent,
        candidate.girdle_thinnest_percent,
    ) && thins_out_at_corners(was, now)
    {
        reasons.push(InvalidReason::GirdleThinAtCorners {
            was_percent: was,
            now_percent: now,
        });
    }

    if let Some((low, high)) = candidate.girdle_band {
        for flat in &original.flats {
            let Some(now) = candidate
                .flats
                .iter()
                .find(|other| other.tier_index == flat.tier_index)
            else {
                continue;
            };
            let off_side = match flat.side {
                FacetSide::Crown => now.height <= high,
                FacetSide::Pavilion => now.height >= low,
            };
            if off_side {
                reasons.push(InvalidReason::FlatOffGirdle { side: flat.side });
            }
        }
    }

    let mut reported = BTreeSet::new();
    for (index, (was, now)) in original
        .facets
        .tiers
        .iter()
        .zip(&candidate.facets.tiers)
        .enumerate()
    {
        if was.alive > 0 && now.alive == 0 {
            reported.insert(index);
            reasons.push(InvalidReason::TierLost {
                name: name_of(index),
            });
        } else if now.alive < was.alive {
            reported.insert(index);
            reasons.push(InvalidReason::TierPartlyLost {
                name: name_of(index),
                lost: was.alive - now.alive,
                total: was.total,
            });
        }
    }

    for &index in &candidate.undersized {
        if !original.undersized.contains(&index) && !reported.contains(&index) {
            reasons.push(InvalidReason::FacetsTooSmall {
                name: name_of(index),
            });
        }
    }
    reasons
}

/// Notes that do not make a candidate invalid.
#[must_use]
pub fn warnings_for(original: &StoneAnalysis, candidate: &StoneAnalysis) -> Vec<String> {
    let mut warnings = Vec::new();
    if candidate.facets.preform_alive > original.facets.preform_alive {
        warnings.push(
            "The retargeted stone touches the rough in places it did not before.".to_string(),
        );
    }
    warnings
}

#[cfg(test)]
mod tests;
