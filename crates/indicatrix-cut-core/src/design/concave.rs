//! Concave (fantasy-cut) tiers: the data model, validation and the derived
//! cutting order.
//!
//! A concave tier is authored exactly as the two-line faceting-diagram standard
//! writes it (a facet line plus a tool line) and is kept apart from
//! [`super::ConstraintTier`] on purpose: it has no mast, no meet constraint and
//! no target, so none of the flat solver's per-tier machinery applies to it.
//! Keeping it in its own list on [`Design`] also leaves the ~130
//! `ConstraintTier` literals and every flat-only code path untouched, so a
//! design without concave tiers behaves byte-for-byte as before.

use core::{
    fmt::{self, Write as _},
    str::FromStr,
};

use indicatrix::geometry::plane::tier_is_crown_side;

use super::{ConstraintTier, Design};

/// A tool's three-letter code in the faceting-diagram standard (plan §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConcaveTool {
    /// Long cylinder (a mandrel-type lap).
    Cylinder,
    /// Cone (frustum).
    Cone,
    /// Circle: the plain rim of a flat wheel.
    Circle,
    /// Disc: a wheel with a bevelled (V) rim.
    Disc,
    /// Sphere (a dimple).
    Sphere,
}

impl ConcaveTool {
    /// Every tool, in the order the standard lists them.
    pub const ALL: [Self; 5] = [
        Self::Cylinder,
        Self::Cone,
        Self::Circle,
        Self::Disc,
        Self::Sphere,
    ];

    /// The three-letter code the standard prints for this tool.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Cylinder => "CYL",
            Self::Cone => "CON",
            Self::Circle => "CIR",
            Self::Disc => "DSC",
            Self::Sphere => "SPH",
        }
    }

    /// Whether the tool's profile needs an angle: cones and discs do, the other
    /// shapes have no angle to give.
    #[must_use]
    pub const fn takes_angle(self) -> bool {
        matches!(self, Self::Cone | Self::Disc)
    }
}

impl fmt::Display for ConcaveTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// A string that is not one of the five tool codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownToolCode(pub String);

impl fmt::Display for UnknownToolCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown concave tool code {:?} (expected CYL, CON, CIR, DSC or SPH)",
            self.0
        )
    }
}

impl std::error::Error for UnknownToolCode {}

impl FromStr for ConcaveTool {
    type Err = UnknownToolCode;

    /// Case-insensitive and whitespace-trimmed: a hand-typed `" cyl "` is a
    /// cylinder, so a cutter never loses a sheet over capitalisation.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        Self::ALL
            .into_iter()
            .find(|tool| tool.code().eq_ignore_ascii_case(trimmed))
            .ok_or_else(|| UnknownToolCode(s.to_owned()))
    }
}

/// How the tool moves while it cuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolMotion {
    /// Stroked back and forth past the stone.
    Reciprocating,
    /// Pressed straight in with no lateral motion.
    Plunge,
}

impl ToolMotion {
    /// The motion a [`Self::word`] names, for reading a stored file back; `None`
    /// for any other text (a stored motion is never guessed).
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        [Self::Reciprocating, Self::Plunge]
            .into_iter()
            .find(|motion| motion.word() == word)
    }

    /// The full word the tool line prints (`reciprocating`, `plunge`) and the
    /// native file stores.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Reciprocating => "reciprocating",
            Self::Plunge => "plunge",
        }
    }
}

/// `value` with `decimals` places, an explicit `+` for positive values when
/// `plus` is set, and no sign at all when it rounds to zero.
fn fixed(value: f64, decimals: usize, plus: bool) -> String {
    let text = format!("{value:.decimals$}");
    let magnitude = text.trim_start_matches('-');
    if magnitude.chars().all(|c| c == '0' || c == '.') {
        magnitude.to_owned()
    } else if plus && value > 0.0 {
        format!("+{text}")
    } else {
        text
    }
}

/// The largest tool diameter ratio `D/W` a tier may carry. Ten stone widths is far
/// beyond any real tool; the bound only keeps the resolver's geometry finite.
pub const MAX_DIAMETER_RATIO: f64 = 10.0;

/// The largest `|X|`, `|Y|` or `|Z|` displacement (a ratio of the stone width) a
/// tier may carry, for the same reason as [`MAX_DIAMETER_RATIO`].
pub const MAX_DISPLACEMENT_RATIO: f64 = 10.0;

/// One concave tier exactly as the two-line standard writes it (plan §4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct ConcaveTier {
    /// Facet name, free text. Must not equal a flat tier's name.
    pub name: String,
    /// φ, signed like [`ConstraintTier::angle_deg`] (negative is pavilion-side).
    pub angle_deg: f64,
    /// Index-wheel positions of the placements; at least one.
    pub indices: Vec<f64>,
    /// Free-text cutting/meetpoint instructions, printed verbatim, never parsed.
    pub instructions: String,
    /// The tool shape.
    pub tool: ConcaveTool,
    /// θ: direction of the tool axis in the facet plane, degrees.
    pub tool_azimuth_deg: f64,
    /// X, Y, Z displacement as ratios of the stone width.
    pub displacement: [f64; 3],
    /// Tool diameter as a ratio of the stone width.
    pub diameter_ratio: f64,
    /// Included angle of a cone or disc; required for those, rejected otherwise.
    pub tool_angle_deg: Option<f64>,
    /// How the tool moves while it cuts.
    pub motion: ToolMotion,
}

/// Why a [`ConcaveTier`] (or its placement in a [`Design`]) is not acceptable.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ConcaveTierError {
    /// A numeric field is NaN or infinite.
    NonFinite {
        /// The offending field's name.
        field: &'static str,
        /// Its value.
        value: f64,
    },
    /// φ must satisfy `0 < |φ| < 90`: the frame is undefined on the table and
    /// ambiguous on the girdle (plan §11a, Q1).
    AngleOutOfRange {
        /// The rejected angle.
        angle_deg: f64,
    },
    /// A tier with no placements cuts nothing.
    NoIndices,
    /// An index outside `[0, |gear_teeth|)`.
    IndexOutOfRange {
        /// The rejected index.
        index: f64,
        /// The design's gear.
        gear_teeth: i32,
    },
    /// The tool diameter ratio must be positive.
    DiameterNotPositive {
        /// The rejected ratio.
        diameter_ratio: f64,
    },
    /// The tool diameter ratio exceeds [`MAX_DIAMETER_RATIO`].
    DiameterTooLarge {
        /// The rejected ratio.
        diameter_ratio: f64,
    },
    /// A displacement component exceeds [`MAX_DISPLACEMENT_RATIO`] in magnitude.
    DisplacementTooLarge {
        /// The rejected component.
        value: f64,
    },
    /// The tool primitive built from the tier is not a valid kernel primitive
    /// (found by the resolver, which narrows the geometry to `f32`).
    ToolGeometry {
        /// The kernel's reason.
        reason: String,
    },
    /// A cone or disc without its angle.
    ToolAngleMissing {
        /// The tool that needs one.
        tool: ConcaveTool,
    },
    /// An angle given to a tool that has none.
    ToolAngleUnexpected {
        /// The tool that rejects one.
        tool: ConcaveTool,
    },
    /// The tool angle must satisfy `0 < angle < 180`.
    ToolAngleOutOfRange {
        /// The rejected angle.
        tool_angle_deg: f64,
    },
    /// The tier has the same name as a flat tier.
    NameClash {
        /// The shared name.
        name: String,
    },
}

impl fmt::Display for ConcaveTierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinite { field, value } => write!(f, "{field} is not finite ({value})"),
            Self::AngleOutOfRange { angle_deg } => {
                write!(f, "angle {angle_deg} must satisfy 0 < |angle| < 90")
            }
            Self::NoIndices => f.write_str("no indices"),
            Self::IndexOutOfRange { index, gear_teeth } => {
                write!(
                    f,
                    "index {index} is outside [0, {})",
                    gear_teeth.unsigned_abs()
                )
            }
            Self::DiameterNotPositive { diameter_ratio } => {
                write!(f, "diameter ratio {diameter_ratio} must be positive")
            }
            Self::DiameterTooLarge { diameter_ratio } => write!(
                f,
                "diameter ratio {diameter_ratio} must not exceed {MAX_DIAMETER_RATIO}"
            ),
            Self::DisplacementTooLarge { value } => write!(
                f,
                "displacement {value} must not exceed {MAX_DISPLACEMENT_RATIO} in magnitude"
            ),
            Self::ToolGeometry { reason } => write!(f, "tool geometry is not valid: {reason}"),
            Self::ToolAngleMissing { tool } => write!(f, "{tool} needs a tool angle"),
            Self::ToolAngleUnexpected { tool } => write!(f, "{tool} takes no tool angle"),
            Self::ToolAngleOutOfRange { tool_angle_deg } => {
                write!(
                    f,
                    "tool angle {tool_angle_deg} must satisfy 0 < angle < 180"
                )
            }
            Self::NameClash { name } => write!(f, "name {name:?} is already a flat tier"),
        }
    }
}

impl std::error::Error for ConcaveTierError {}

const fn finite(field: &'static str, value: f64) -> Result<(), ConcaveTierError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(ConcaveTierError::NonFinite { field, value })
    }
}

impl ConcaveTier {
    /// Checks every field in isolation (the name-clash check needs the whole
    /// design and lives in [`Design::validate_concave_tiers`]).
    ///
    /// `gear_teeth` is `design.meta.gear_teeth`; indices must lie in
    /// `[0, |gear_teeth|)`. Finiteness is checked first for every number so no
    /// later comparison silently passes on a NaN.
    ///
    /// # Errors
    ///
    /// The first violated rule, as a [`ConcaveTierError`].
    pub fn validate(&self, gear_teeth: i32) -> Result<(), ConcaveTierError> {
        finite("angle_deg", self.angle_deg)?;
        for &index in &self.indices {
            finite("indices", index)?;
        }
        finite("tool_azimuth_deg", self.tool_azimuth_deg)?;
        for value in self.displacement {
            finite("displacement", value)?;
        }
        finite("diameter_ratio", self.diameter_ratio)?;
        if let Some(angle) = self.tool_angle_deg {
            finite("tool_angle_deg", angle)?;
        }

        let magnitude = self.angle_deg.abs();
        if magnitude <= 0.0 || magnitude >= 90.0 {
            return Err(ConcaveTierError::AngleOutOfRange {
                angle_deg: self.angle_deg,
            });
        }
        if self.indices.is_empty() {
            return Err(ConcaveTierError::NoIndices);
        }
        let gear = f64::from(gear_teeth.unsigned_abs());
        if let Some(&index) = self.indices.iter().find(|&&i| i < 0.0 || i >= gear) {
            return Err(ConcaveTierError::IndexOutOfRange { index, gear_teeth });
        }
        if self.diameter_ratio <= 0.0 {
            return Err(ConcaveTierError::DiameterNotPositive {
                diameter_ratio: self.diameter_ratio,
            });
        }
        if self.diameter_ratio > MAX_DIAMETER_RATIO {
            return Err(ConcaveTierError::DiameterTooLarge {
                diameter_ratio: self.diameter_ratio,
            });
        }
        if let Some(&value) = self
            .displacement
            .iter()
            .find(|v| v.abs() > MAX_DISPLACEMENT_RATIO)
        {
            return Err(ConcaveTierError::DisplacementTooLarge { value });
        }
        match (self.tool.takes_angle(), self.tool_angle_deg) {
            (true, None) => return Err(ConcaveTierError::ToolAngleMissing { tool: self.tool }),
            (false, Some(_)) => {
                return Err(ConcaveTierError::ToolAngleUnexpected { tool: self.tool });
            }
            (true, Some(angle)) if angle <= 0.0 || angle >= 180.0 => {
                return Err(ConcaveTierError::ToolAngleOutOfRange {
                    tool_angle_deg: angle,
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// The four columns of the tier's tool line, exactly as the faceting-diagram
    /// reference template prints them (plan §1a):
    /// `["CYL", "+10.00°", "X = 0.000, Y = 0.000, Z = 0.000", "D/W = 0.400, reciprocating"]`.
    ///
    /// One formatter feeds every output (text and HTML sheets, view models and
    /// the `.asc` footnotes), so they can never disagree on a sign or a
    /// decimal. θ carries an explicit `+` when positive and none at zero; `CON`
    /// and `DSC` print their included angle before the motion; the motion is
    /// always printed because the standard lists it as a tool detail (plan
    /// §11a). A value that rounds to zero prints without a sign, so `-0.000`
    /// never reaches a sheet.
    #[must_use]
    pub fn second_line_fields(&self) -> [String; 4] {
        let [x, y, z] = self.displacement;
        let mut size = format!("D/W = {}", fixed(self.diameter_ratio, 3, false));
        if let (true, Some(angle)) = (self.tool.takes_angle(), self.tool_angle_deg) {
            let _ = write!(size, ", angle = {}°", fixed(angle, 2, false));
        }
        let _ = write!(size, ", {}", self.motion.word());
        [
            self.tool.code().to_owned(),
            format!("{}°", fixed(self.tool_azimuth_deg, 2, true)),
            format!(
                "X = {}, Y = {}, Z = {}",
                fixed(x, 3, false),
                fixed(y, 3, false),
                fixed(z, 3, false)
            ),
            size,
        ]
    }

    /// Whether the tier belongs to the crown-side block of the cutting order;
    /// reads the sign of φ via [`tier_is_crown_side`].
    #[must_use]
    pub const fn is_crown_side(&self) -> bool {
        tier_is_crown_side(self.angle_deg)
    }
}

/// A position in either tier list (plan §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TierRef {
    /// Index into [`Design::tiers`].
    Flat(usize),
    /// Index into [`Design::concave_tiers`].
    Concave(usize),
}

impl ConstraintTier {
    /// Whether this is the table: a crown-side tier at exactly `+0`. A
    /// sign-negative zero is the culet, so the sign matters
    /// ([`tier_is_crown_side`]).
    ///
    /// A method, not a field, so the many `ConstraintTier` literals stay valid.
    #[must_use]
    pub const fn is_table(&self) -> bool {
        self.angle_deg == 0.0 && tier_is_crown_side(self.angle_deg)
    }
}

impl Design {
    /// The order tiers are cut in (plan §4.2): flat pavilion and girdle tiers,
    /// concave pavilion-side tiers, flat crown tiers except the table, concave
    /// crown-side tiers, then the table.
    ///
    /// Within each group the stored order is kept; the author controls the order
    /// inside the concave groups. A flat tier with `|angle| == 90` is a girdle
    /// and joins the first group even though `+90` reads as crown-side.
    #[must_use]
    pub fn cutting_order(&self) -> Vec<TierRef> {
        let flat = |keep: fn(&ConstraintTier) -> bool| {
            self.tiers
                .iter()
                .enumerate()
                .filter(move |(_, tier)| keep(tier))
                .map(|(i, _)| TierRef::Flat(i))
        };
        let concave = |crown: bool| {
            self.concave_tiers
                .iter()
                .enumerate()
                .filter(move |(_, tier)| tier.is_crown_side() == crown)
                .map(|(i, _)| TierRef::Concave(i))
        };
        let pavilion_or_girdle =
            |t: &ConstraintTier| !tier_is_crown_side(t.angle_deg) || t.angle_deg.abs() == 90.0;
        let crown_not_table = |t: &ConstraintTier| {
            tier_is_crown_side(t.angle_deg) && t.angle_deg.abs() != 90.0 && !t.is_table()
        };

        let mut order = Vec::with_capacity(self.tiers.len() + self.concave_tiers.len());
        order.extend(flat(pavilion_or_girdle));
        order.extend(concave(false));
        order.extend(flat(crown_not_table));
        order.extend(concave(true));
        order.extend(flat(ConstraintTier::is_table));
        order
    }

    /// Validates every concave tier against this design's gear and flat names.
    ///
    /// # Errors
    ///
    /// The position of the first bad concave tier and what is wrong with it.
    pub fn validate_concave_tiers(&self) -> Result<(), (usize, ConcaveTierError)> {
        for (i, tier) in self.concave_tiers.iter().enumerate() {
            tier.validate(self.meta.gear_teeth).map_err(|e| (i, e))?;
            if !tier.name.is_empty()
                && self
                    .tiers
                    .iter()
                    .any(|flat| flat.names().contains(&tier.name.as_str()))
            {
                return Err((
                    i,
                    ConcaveTierError::NameClash {
                        name: tier.name.clone(),
                    },
                ));
            }
        }
        Ok(())
    }

    /// Grows [`Self::concave_tier_ids`] with fresh ids until it is as long as
    /// [`Self::concave_tiers`], mirroring [`Design::ensure_tier_ids`]. Existing
    /// ids are never changed, so a consistent design is left as it is.
    pub fn ensure_concave_tier_ids(&mut self) {
        while self.concave_tier_ids.len() < self.concave_tiers.len() {
            let id = self.allocate_tier_id();
            self.concave_tier_ids.push(id);
        }
    }

    /// Total number of concave facets cut: the sum of every tier's index count.
    #[must_use]
    pub fn concave_placement_count(&self) -> usize {
        self.concave_tiers.iter().map(|t| t.indices.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TierRef::{Concave as C, Flat as F},
        *,
    };
    use crate::{design::ScheduleMeta, preform::PreformSpec};
    use indicatrix::geometry::meet_solver::MeetConstraint;

    fn sample() -> ConcaveTier {
        ConcaveTier {
            name: "Groove".to_owned(),
            angle_deg: -40.0,
            indices: vec![0.0, 24.0, 48.0, 72.0],
            instructions: String::new(),
            tool: ConcaveTool::Cylinder,
            tool_azimuth_deg: 0.0,
            displacement: [0.0, 0.0, 0.1],
            diameter_ratio: 0.5,
            tool_angle_deg: None,
            motion: ToolMotion::Reciprocating,
        }
    }

    fn flat(name: &str, angle_deg: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_owned(),
            indices: vec![0.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    fn design(tiers: Vec<ConstraintTier>) -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            tiers,
        )
    }

    #[test]
    fn concave_tool_codes_round_trip_through_from_str_case_insensitively() {
        for tool in ConcaveTool::ALL {
            assert_eq!(tool.code().parse::<ConcaveTool>(), Ok(tool));
            assert_eq!(tool.to_string(), tool.code());
            let lower = tool.code().to_ascii_lowercase();
            assert_eq!(format!("  {lower}\t").parse::<ConcaveTool>(), Ok(tool));
        }
        assert_eq!(
            "XYZ".parse::<ConcaveTool>(),
            Err(UnknownToolCode("XYZ".to_owned()))
        );
    }

    #[test]
    fn second_line_fields_match_the_reference_template() {
        let cylinder = ConcaveTier {
            tool_azimuth_deg: 10.0,
            displacement: [0.0; 3],
            diameter_ratio: 0.4,
            ..sample()
        };
        assert_eq!(
            cylinder.second_line_fields(),
            [
                "CYL",
                "+10.00°",
                "X = 0.000, Y = 0.000, Z = 0.000",
                "D/W = 0.400, reciprocating"
            ]
        );
        let cone = ConcaveTier {
            tool: ConcaveTool::Cone,
            tool_angle_deg: Some(60.0),
            tool_azimuth_deg: 0.0,
            displacement: [-0.25, 0.12, 0.05],
            diameter_ratio: 0.6,
            ..sample()
        };
        assert_eq!(
            cone.second_line_fields(),
            [
                "CON",
                "0.00°",
                "X = -0.250, Y = 0.120, Z = 0.050",
                "D/W = 0.600, angle = 60.00°, reciprocating"
            ]
        );
        let negative = ConcaveTier {
            tool: ConcaveTool::Disc,
            tool_angle_deg: Some(90.0),
            tool_azimuth_deg: -15.0,
            displacement: [-0.0, 0.0, 0.0],
            ..sample()
        };
        assert_eq!(negative.second_line_fields()[1], "-15.00°");
        assert_eq!(
            negative.second_line_fields()[2],
            "X = 0.000, Y = 0.000, Z = 0.000"
        );
        let plunge = ConcaveTier {
            tool: ConcaveTool::Sphere,
            motion: ToolMotion::Plunge,
            diameter_ratio: 0.25,
            ..sample()
        };
        assert_eq!(plunge.second_line_fields()[0], "SPH");
        assert_eq!(plunge.second_line_fields()[3], "D/W = 0.250, plunge");
    }

    #[test]
    fn concave_tier_validate_rejects_each_hostile_field() {
        const GEAR: i32 = 96;
        assert_eq!(sample().validate(GEAR), Ok(()));
        let bad = |edit: &dyn Fn(&mut ConcaveTier)| {
            let mut tier = sample();
            edit(&mut tier);
            tier.validate(GEAR)
                .expect_err("hostile tier must be rejected")
        };
        assert!(matches!(
            bad(&|t| t.angle_deg = f64::NAN),
            ConcaveTierError::NonFinite {
                field: "angle_deg",
                ..
            }
        ));
        assert!(matches!(
            bad(&|t| t.displacement[1] = f64::INFINITY),
            ConcaveTierError::NonFinite {
                field: "displacement",
                ..
            }
        ));
        assert!(matches!(
            bad(&|t| t.angle_deg = 0.0),
            ConcaveTierError::AngleOutOfRange { .. }
        ));
        assert!(matches!(
            bad(&|t| t.angle_deg = 90.0),
            ConcaveTierError::AngleOutOfRange { .. }
        ));
        assert_eq!(bad(&|t| t.indices.clear()), ConcaveTierError::NoIndices);
        assert!(matches!(
            bad(&|t| t.indices = vec![96.0]),
            ConcaveTierError::IndexOutOfRange { gear_teeth: 96, .. }
        ));
        assert!(matches!(
            bad(&|t| t.diameter_ratio = 0.0),
            ConcaveTierError::DiameterNotPositive { .. }
        ));
        assert!(matches!(
            bad(&|t| t.diameter_ratio = 1e300),
            ConcaveTierError::DiameterTooLarge { .. }
        ));
        assert!(matches!(
            bad(&|t| t.diameter_ratio = MAX_DIAMETER_RATIO + 0.5),
            ConcaveTierError::DiameterTooLarge { .. }
        ));
        for axis in 0..3 {
            for value in [1e300, -1e300, -(MAX_DISPLACEMENT_RATIO + 0.5)] {
                assert!(matches!(
                    bad(&|t| t.displacement[axis] = value),
                    ConcaveTierError::DisplacementTooLarge { .. }
                ));
            }
        }
        // The bounds themselves are allowed.
        let mut at_limit = sample();
        at_limit.diameter_ratio = MAX_DIAMETER_RATIO;
        at_limit.displacement = [MAX_DISPLACEMENT_RATIO, -MAX_DISPLACEMENT_RATIO, 0.0];
        assert_eq!(at_limit.validate(GEAR), Ok(()));
        assert_eq!(
            bad(&|t| t.tool = ConcaveTool::Cone),
            ConcaveTierError::ToolAngleMissing {
                tool: ConcaveTool::Cone
            }
        );
        assert_eq!(
            bad(&|t| {
                t.tool = ConcaveTool::Sphere;
                t.tool_angle_deg = Some(60.0);
            }),
            ConcaveTierError::ToolAngleUnexpected {
                tool: ConcaveTool::Sphere
            }
        );
        assert!(matches!(
            bad(&|t| {
                t.tool = ConcaveTool::Disc;
                t.tool_angle_deg = Some(180.0);
            }),
            ConcaveTierError::ToolAngleOutOfRange { .. }
        ));

        let mut clashing = design(vec![flat("Pavilion Main", -41.0)]);
        clashing.concave_tiers.push(ConcaveTier {
            name: "Pavilion Main".to_owned(),
            ..sample()
        });
        assert_eq!(
            clashing.validate_concave_tiers(),
            Err((
                0,
                ConcaveTierError::NameClash {
                    name: "Pavilion Main".to_owned()
                }
            ))
        );
    }

    #[test]
    fn cutting_order_places_concave_groups_at_the_end_of_each_section_before_the_table() {
        let mut d = design(vec![
            flat("table", 0.0),
            flat("crown", 35.0),
            flat("pav", -41.0),
            flat("girdle", 90.0),
            flat("culet", -0.0),
            flat("crown2", 20.0),
        ]);
        let concave_at = |angle_deg: f64| ConcaveTier {
            angle_deg,
            ..sample()
        };
        d.concave_tiers = vec![
            concave_at(30.0),
            concave_at(-30.0),
            concave_at(10.0),
            concave_at(-10.0),
        ];
        assert_eq!(
            d.cutting_order(),
            vec![F(2), F(3), F(4), C(1), C(3), F(1), F(5), C(0), C(2), F(0)]
        );
        assert_eq!(d.concave_placement_count(), 16);
        assert!(d.tiers[0].is_table());
        assert!(!d.tiers[4].is_table());
    }

    #[test]
    fn design_equality_ignores_concave_tier_ids() {
        let mut a = design(vec![flat("pav", -41.0)]);
        a.concave_tiers.push(sample());
        let mut b = a.clone();
        a.ensure_concave_tier_ids();
        assert_eq!(a.concave_tier_ids.len(), 1);
        assert!(b.concave_tier_ids.is_empty());
        assert_eq!(a, b);
        b.concave_tiers[0].diameter_ratio = 0.6;
        assert_ne!(a, b);
    }
}
