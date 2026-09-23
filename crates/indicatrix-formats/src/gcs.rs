//! Reader for Gem Cut Studio's `.gcs` design format.
//!
//! Gem Cut Studio ("GCS") is a Windows faceting-design application distinct from
//! `GemCAD`; this module is not produced, endorsed, or affiliated with Gem Cut
//! Studio or its author (see the crate-level docs for the full affiliation note).
//! It exists for the same reason [`crate::asc`] exists: some real-world designs in
//! the wild are only ever published as a `.gcs` file, with no `.asc` counterpart,
//! and reading one shouldn't require Gem Cut Studio itself.
//!
//! # Format (reverse-engineered from a real-world corpus of 59 `.gcs` files, 56 of
//! which have a sibling `.asc` for the same design -- see "Verification" below)
//!
//! A `.gcs` file is plain-text XML (no prolog, no external DTD, no namespaces) with
//! one attribute-only root wrapping a flat, non-recursive element tree:
//!
//! ```text
//! <GemCutStudio version="1000">
//!     <index gear="64" base="0" symmetry="4" mirror="0"/>
//!     <tier angle="126.39" depth="0.6664" name="P1" instructions="" visible="true" guide="false">
//!         <facet nx="-0" ny="-0.805" nz="-0.593" index_angle="0">
//!             <vertex x="-0.0827" y="-0.8186" z="-0.0126"/>
//!             ...
//!         </facet>
//!         ...
//!     </tier>
//!     ...
//!     <render material="176 Corundum" refractive_index="1.76" dispersion="0.018" clarity="100" density="1.4" lighting_model="Random">
//!         <color r="0.71" g="0.73" b="0.95"/>
//!     </render>
//!     <info title="..." author="..." date="..." ri_min="1.7" ri_max="2.15" shape="Octagon" footer1="..." footer2="..."/>
//! </GemCutStudio>
//! ```
//!
//! Unlike [`crate::asc`]'s `.asc` (a cutting *schedule*: angle, mast, and index
//! positions the cutter still has to solve into a shape), a `.gcs` file is a
//! *solved* design: every `<tier>` already carries its facets as closed polygons
//! (`<facet>` children, each a list of real `<vertex>` points) on a stone
//! normalized so its own reference plane sits at radius 1. That difference in kind
//! -- schedule vs. solved geometry -- is exactly why `depth` and `.asc`'s `mast` are
//! **not interchangeable** even though both nominally mean "how far this facet
//! plane sits from center"; see "What does not carry over" below.
//!
//! ## What is confirmed, and how
//!
//! Every field below was checked against the 56 real `.gcs`/`.asc` sibling pairs in
//! the corpus (`facet_diagrams.sqlite`'s `attached_files` table, joined on
//! `detail_id`), using [`crate::asc::parse_asc`] on the `.asc` side as ground
//! truth. 44 of the 56 pairs (79%) matched on every check below with zero
//! discrepancy; the remaining 12 are understood corpus-data edge cases, not
//! parser gaps (see "Known discrepancies").
//!
//! - **`<index gear>`** matches `.asc`'s `g` line tooth count exactly (as a
//!   magnitude -- `.asc` occasionally signs it for handedness, `.gcs` never does).
//! - **[`GcsTier::angle_deg`]** uses a *different convention* from
//!   [`crate::asc::AscTier::angle_deg`], but a fully verified one: `.gcs` measures
//!   a single continuous polar angle from the crown apex/table-normal direction
//!   (`0`) through the girdle plane (`90`, vertical) to the culet direction
//!   (`180`), rather than `.asc`'s signed "negative = pavilion, non-negative =
//!   crown" split. For every crown tier (`.asc` angle `>= 0`) the two values are
//!   *identical*; for every pavilion tier (`.asc` angle `< 0`) `.gcs`'s angle
//!   equals `180.0 + asc_angle` to within float32 rounding (`.gcs` stores its
//!   trigonometric fields at `f32` precision even though the XML prints them as
//!   `f64`-width decimals -- hence the "give or take a few times `1e-5`" residue
//!   seen when cross-checking). [`GcsTier::to_signed_asc_angle`] applies this
//!   verified transform.
//! - **`<facet index_angle>`** matches `.asc`'s tooth-number indices exactly once
//!   converted to degrees: `index_angle = (tooth % gear) / gear * 360`. Checked
//!   facet-by-facet (not just tier-by-tier) across all 44 clean-passing pairs.
//! - **`<render refractive_index>`** matches `.asc`'s `I` line in every pair where
//!   the two files actually describe the same material (see "Known
//!   discrepancies" for the three that do not).
//! - Total facet-plane count (summed across every tier) matches `.asc`'s summed
//!   index count exactly in every clean-passing pair.
//!
//! ## What does not carry over
//!
//! - **[`GcsTier::depth`] is not `.asc`'s `mast`.** Comparing matched tiers within
//!   a single real design (`attached_files` id 1124, detail 553, "Octabar-X") shows
//!   `depth / mast` ranging smoothly from `0.90` (at the girdle, angle 90) up
//!   through `1.18` (at the table, angle 0) -- not a constant, and not a simple
//!   trig function of the tier's own angle either (the pavilion tiers of the same
//!   file give a completely different, non-overlapping ratio curve from the crown
//!   tiers). This module does not guess at a conversion. Reconciling the two
//!   requires the same full meet-point geometry solve `.asc`'s own mast values are
//!   solved from -- exactly the job of `indicatrix::geometry::meet_solver`, not
//!   this crate.
//! - **`<index base>`** is carried through as-is (presumably analogous to `.asc`'s
//!   `g` line reference-angle field) but every sample in the corpus has `base="0"`,
//!   so this module has no real, non-zero example to verify that analogy against.
//!   Treat it as unconfirmed.
//! - **`<index symmetry>` and `<index mirror>` do not reliably mirror `.asc`'s `y`
//!   line.** Cross-checking all 56 pairs: `symmetry` sometimes matches `.asc`'s
//!   symmetry order exactly, but is very often `1` even when the design's real
//!   rotational symmetry (per its own `.asc`) is 4, 8, or 16 -- `.gcs` appears to
//!   fully unroll the facet list (no compression via the index/symmetry mechanism)
//!   for many designs and not others, and this module could not determine the
//!   rule that decides which. `mirror` is stranger still: real corpus values
//!   include `0`, `1`, `2`, `3`, `4`, and `5`, which rules out a simple boolean
//!   matching `.asc`'s `y`/`n` mirror flag. Both fields are kept as raw integers
//!   ([`GcsIndex::symmetry`], [`GcsIndex::mirror`]) with no derived
//!   `is_mirrored()`-style helper, specifically so callers don't inherit a
//!   boolean assumption this module cannot back up.
//!
//! ## Known discrepancies (12 of 56 pairs)
//!
//! - **Flat, single-facet tiers (the table, and occasionally the culet) have an
//!   arbitrary `index_angle`.** A tier spanning the entire top or bottom of the
//!   stone has only one facet and no real azimuthal position, so `.gcs` appears to
//!   just pick something (observed: `0`, `11.25`, `22.5`, `45` across different
//!   files) rather than echo the design's own tooth number for that facet. 7 of
//!   the 12 discrepant pairs are exactly this.
//! - **A few "cut corner" shapes (2 of 56) merge tiers differently than this
//!   module's `.asc`-side grouping expects** when the design has more than one
//!   facet sharing an angle+mast at the `.asc` level; the exact grouping rule
//!   `.gcs` uses there was not pinned down.
//! - **3 of 56 pairs have a genuine refractive-index mismatch** between the
//!   `.gcs` and its catalogued `.asc` sibling (e.g. 1.76 vs. 1.54) -- almost
//!   certainly two different material variants of the same design filed under one
//!   catalog entry, not a parsing issue.
//!
//! None of these were "fixed" by loosening a check; they are reported here so a
//! caller knows exactly which 21% of real files might disagree with an `.asc`
//! sibling and why.
//!
//! # What this module does not attempt
//!
//! There is no `.gcs` *writer*: nothing downstream in this workspace produces
//! `.gcs` files (the editor's own save format is `.indicatrix.toml`, see
//! [`crate::native`], and its export path targets `.asc`), so a serializer would
//! have no real caller and no way to be verified against anything.

use std::fmt;

/// Everything that can go wrong parsing a `.gcs` design with [`parse_gcs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GcsParseError {
    /// `content` was empty or contained only whitespace.
    EmptyInput,
    /// A `<` was never closed by a matching `>` before the file ended.
    UnterminatedTag {
        /// 1-based line the unterminated tag started on.
        line: usize,
    },
    /// An attribute inside a tag had a key but no closing quote for its value.
    MalformedAttribute {
        /// 1-based line the tag containing the bad attribute started on.
        line: usize,
        /// The attribute's key, if one was found before the parser gave up.
        key: String,
    },
    /// The file did not open with a `<GemCutStudio ...>` root element.
    MissingRootElement,
    /// The root element had no `<index .../>` child.
    MissingIndexElement,
    /// The `<index>` element was missing a required attribute.
    IndexAttributeMissing {
        /// The missing attribute's name (`"gear"`, `"base"`, `"symmetry"`, or
        /// `"mirror"`).
        attr: &'static str,
    },
    /// An `<index>` attribute's value did not parse as a number.
    IndexAttributeNotNumeric {
        /// The attribute's name.
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<tier>` element was missing a required attribute.
    TierAttributeMissing {
        /// 0-based position of this tier among the file's `<tier>` elements.
        tier_index: usize,
        /// The missing attribute's name (`"angle"` or `"depth"`).
        attr: &'static str,
    },
    /// A `<tier>` attribute's value did not parse as a number.
    TierAttributeNotNumeric {
        /// 0-based position of this tier among the file's `<tier>` elements.
        tier_index: usize,
        /// The attribute's name.
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<facet>` attribute's value did not parse as a number.
    FacetAttributeNotNumeric {
        /// 0-based position of this tier among the file's `<tier>` elements.
        tier_index: usize,
        /// The attribute's name (`"nx"`, `"ny"`, `"nz"`, or `"index_angle"`).
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<vertex>` attribute's value did not parse as a number.
    VertexAttributeNotNumeric {
        /// 0-based position of this tier among the file's `<tier>` elements.
        tier_index: usize,
        /// The attribute's name (`"x"`, `"y"`, or `"z"`).
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<render>` attribute was present but did not parse as a number. Absent
    /// entirely is not an error (these fields are optional and default to `0.0`) --
    /// only a garbled value present on the attribute goes through this fallible path,
    /// same as [`Self::TierAttributeNotNumeric`].
    RenderAttributeNotNumeric {
        /// The attribute's name (`"refractive_index"`, `"dispersion"`, `"clarity"`, or
        /// `"density"`).
        attr: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// A `<tier>` or `<facet>` or `<render>` element was opened but the file ended
    /// (or the root closed) before its matching close tag appeared.
    UnterminatedElement {
        /// The element name (`"tier"`, `"facet"`, or `"render"`).
        name: &'static str,
    },
}

impl fmt::Display for GcsParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "empty input"),
            Self::UnterminatedTag { line } => {
                write!(f, "line {line}: '<' was never closed by a matching '>'")
            }
            Self::MalformedAttribute { line, key } => write!(
                f,
                "line {line}: attribute {key:?} has no closing quote for its value"
            ),
            Self::MissingRootElement => {
                write!(f, "missing '<GemCutStudio ...>' root element")
            }
            Self::MissingIndexElement => write!(f, "missing '<index .../>' element"),
            Self::IndexAttributeMissing { attr } => {
                write!(f, "'<index>' is missing its {attr:?} attribute")
            }
            Self::IndexAttributeNotNumeric { attr, value } => write!(
                f,
                "'<index>' attribute {attr:?} value {value:?} is not numeric"
            ),
            Self::TierAttributeMissing { tier_index, attr } => {
                write!(f, "tier #{tier_index} is missing its {attr:?} attribute")
            }
            Self::TierAttributeNotNumeric {
                tier_index,
                attr,
                value,
            } => write!(
                f,
                "tier #{tier_index} attribute {attr:?} value {value:?} is not numeric"
            ),
            Self::FacetAttributeNotNumeric {
                tier_index,
                attr,
                value,
            } => write!(
                f,
                "tier #{tier_index}: facet attribute {attr:?} value {value:?} is not numeric"
            ),
            Self::VertexAttributeNotNumeric {
                tier_index,
                attr,
                value,
            } => write!(
                f,
                "tier #{tier_index}: vertex attribute {attr:?} value {value:?} is not numeric"
            ),
            Self::RenderAttributeNotNumeric { attr, value } => write!(
                f,
                "'<render>' attribute {attr:?} value {value:?} is not numeric"
            ),
            Self::UnterminatedElement { name } => {
                write!(f, "'<{name}>' was opened but never closed")
            }
        }
    }
}

impl std::error::Error for GcsParseError {}

/// One `<vertex .../>` point of a solved facet's polygon outline.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GcsVertex {
    /// X coordinate, in the design's normalized (reference-radius-1) frame.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
    /// Z coordinate (the stone's optical axis).
    pub z: f64,
}

/// One solved facet plane: a real polygon at one azimuthal ("index") position
/// within a [`GcsTier`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcsFacet {
    /// The facet plane's outward unit normal `(nx, ny, nz)`.
    pub normal: [f64; 3],
    /// This facet's azimuthal position, in degrees. Confirmed (see module docs)
    /// to equal `(tooth % gear) / gear * 360.0` for `.asc`'s corresponding tooth
    /// number on every non-flat tier checked -- but see [`GcsTier`]'s docs for the
    /// one confirmed exception (a tier with a single, full-width facet, e.g. the
    /// table, where this value is not meaningful).
    pub index_angle_deg: f64,
    /// The facet's polygon outline, in file order.
    pub vertices: Vec<GcsVertex>,
}

/// One `<tier>...</tier>` block: a single angle/depth setting, realized as one
/// solved facet per azimuthal ("index") position.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcsTier {
    /// Polar angle in degrees, `0` at the crown apex (table-normal direction)
    /// through `90` at the girdle plane (vertical) to `180` at the culet
    /// direction. See the module docs for the verified relationship to
    /// [`crate::asc::AscTier::angle_deg`]'s signed convention.
    pub angle_deg: f64,
    /// Distance from the design's center in its own normalized frame. **Not**
    /// directly comparable to `.asc`'s `mast` -- see the module docs.
    pub depth: f64,
    /// This tier's facet name, e.g. `"P1"`, `"C7"`, `"G1"`, `"T"`. Stored exactly
    /// as written; often does not match the name a sibling `.asc` uses for the
    /// same physical facet (the two formats appear to name tiers independently --
    /// see the module docs' angle-based, not name-based, verification method).
    pub name: String,
    /// Free-text cutting/meet instruction, in the same domain as
    /// [`crate::asc::AscTier::notes`] / [`crate::asc::MeetInstruction`] (real
    /// corpus values include `"Meet C1, C2, and C3"`, `"Level girdle"`, `"Cut to
    /// CP"`, and `"CAM PF1  35.760  64-16-32-48"`-style compound-angle-mast
    /// notes). Empty when the file leaves it blank, which is the common case.
    pub instructions: String,
    /// Whether Gem Cut Studio currently displays this tier. Always `true` in the
    /// sampled corpus, but the attribute is real and is not discarded.
    pub visible: bool,
    /// Whether this tier is a non-physical construction/guide line rather than a
    /// real cut facet. Always `false` in the sampled corpus (no real example to
    /// verify the `true` case's other fields against), but the attribute is real
    /// and is not discarded.
    pub guide: bool,
    /// Every solved facet at this tier's angle/depth, one per azimuthal position.
    pub facets: Vec<GcsFacet>,
}

impl GcsTier {
    /// Converts [`Self::angle_deg`] to `.asc`'s signed convention (negative =
    /// pavilion, non-negative = crown): identity for `angle_deg <= 90.0`,
    /// `angle_deg - 180.0` otherwise. Verified against 44 of 56 real `.gcs`/`.asc`
    /// sibling pairs in the corpus -- see the module docs for the other 12.
    #[must_use]
    pub fn to_signed_asc_angle(&self) -> f64 {
        if self.angle_deg <= 90.0 {
            self.angle_deg
        } else {
            self.angle_deg - 180.0
        }
    }

    /// Total number of solved facet planes in this tier (one per azimuthal
    /// position).
    #[must_use]
    pub const fn facet_plane_count(&self) -> usize {
        self.facets.len()
    }
}

/// The `<index .../>` element: the index-wheel setup shared by every tier.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GcsIndex {
    /// Index-wheel tooth count. Confirmed to match `.asc`'s `g` line tooth count
    /// (as a magnitude) -- see the module docs.
    pub gear: u32,
    /// Kept as-is; presumed analogous to `.asc`'s gear reference-angle field, but
    /// every sample in the corpus has `base = 0`, so this module has no non-zero
    /// real example to confirm that against. Treat as **unconfirmed**.
    pub base: f64,
    /// Kept as-is. Sometimes matches the design's real rotational symmetry order
    /// (per its `.asc` sibling), but is very often `1` regardless -- see the
    /// module docs. Treat as **unconfirmed** unless cross-checked against another
    /// source for the specific design at hand.
    pub symmetry: u32,
    /// Kept as-is. **Not a boolean.** Real corpus values include `0` through `5`,
    /// which rules out a simple flag matching `.asc`'s `y`/`n` mirror field. This
    /// module could not determine what the value represents -- see the module
    /// docs.
    pub mirror: u32,
}

/// The `<color .../>` child of [`GcsRender`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GcsColor {
    /// Red channel, `0.0..=1.0`.
    pub r: f64,
    /// Green channel, `0.0..=1.0`.
    pub g: f64,
    /// Blue channel, `0.0..=1.0`.
    pub b: f64,
}

/// The `<render>...</render>` element: material and display settings used by Gem
/// Cut Studio's own preview, not by anything this crate does.
///
/// Kept because [`GcsIndex::gear`] and [`Self::refractive_index`] are this
/// module's two strongest cross-checks against a sibling `.asc`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcsRender {
    /// Material name as Gem Cut Studio's own material library names it (e.g.
    /// `"176 Corundum"`, or the literal `"(from file)"` seen in some samples).
    pub material: String,
    /// Refractive index. Confirmed to match `.asc`'s `I` line in every pair
    /// checked where the two files describe the same material -- see the module
    /// docs' "Known discrepancies" for the ones that do not.
    pub refractive_index: f64,
    /// Dispersion value, as Gem Cut Studio's material library records it.
    pub dispersion: f64,
    /// Clarity setting used by the preview renderer.
    pub clarity: f64,
    /// Density setting used by the preview renderer.
    pub density: f64,
    /// Lighting model name (only `"Random"` seen in the sampled corpus).
    pub lighting_model: String,
    /// Preview display color.
    pub color: GcsColor,
}

/// The `<info .../>` element: free-text design metadata.
///
/// Every field is `Option` because the real corpus has at least four distinct
/// attribute sets -- e.g. some files carry `shape`/`header2`/`header3` and no
/// `size_min`/`size_max`, others the reverse -- so no attribute here is reliably
/// present.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GcsInfo {
    /// Design title.
    pub title: Option<String>,
    /// Designer/author name.
    pub author: Option<String>,
    /// Free-text publication date/venue (e.g. `"Star Cuts 1 1998"`); several real
    /// samples embed a trailing blank line inside the attribute value itself.
    pub date: Option<String>,
    /// Shape name (e.g. `"Octagon"`).
    pub shape: Option<String>,
    /// Second free-text header line.
    pub header2: Option<String>,
    /// Third free-text header line.
    pub header3: Option<String>,
    /// Minimum recommended refractive index, as a string (real samples format it
    /// inconsistently, e.g. `"1.7"` next to `"2.1500001"`, so this is not parsed
    /// to a number here).
    pub ri_min: Option<String>,
    /// Maximum recommended refractive index, as a string; see [`Self::ri_min`].
    pub ri_max: Option<String>,
    /// Minimum recommended finished size, as a string (an alternative to
    /// `shape`/`header2`/`header3` seen in some samples).
    pub size_min: Option<String>,
    /// Maximum recommended finished size, as a string; see [`Self::size_min`].
    pub size_max: Option<String>,
    /// First free-text footer line.
    pub footer1: Option<String>,
    /// Second free-text footer line.
    pub footer2: Option<String>,
    /// Third free-text footer line, present in some samples.
    pub footer3: Option<String>,
    /// Fourth free-text footer line, present in a few samples.
    pub footer4: Option<String>,
}

/// A fully parsed Gem Cut Studio `.gcs` design.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcsDesign {
    /// The `<GemCutStudio version="...">` root's version string (`"1000"` in
    /// every sampled file).
    pub version: String,
    /// The shared index-wheel setup.
    pub index: GcsIndex,
    /// Every tier, in file order.
    pub tiers: Vec<GcsTier>,
    /// Preview material/display settings, when present.
    pub render: Option<GcsRender>,
    /// Free-text design metadata, when present.
    pub info: Option<GcsInfo>,
}

impl GcsDesign {
    /// Total number of solved facet planes across every tier.
    #[must_use]
    pub fn facet_plane_count(&self) -> usize {
        self.tiers.iter().map(GcsTier::facet_plane_count).sum()
    }
}

impl fmt::Display for GcsDesign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GcsDesign(gear={}, tiers={}, facets={})",
            self.index.gear,
            self.tiers.len(),
            self.facet_plane_count()
        )
    }
}

/// One `<tag ...>`, `<tag .../>`, or `</tag>` construct, with its attributes
/// already split out. Owns its strings so the tokenizer's output can be consumed
/// by an ordinary `Vec` iterator without fighting borrow lifetimes against
/// `content` -- this format's files are small enough (tens of KB) that the extra
/// allocations are immaterial next to the clarity win.
struct RawTag {
    name: String,
    attrs: Vec<(String, String)>,
    self_closing: bool,
    closing: bool,
}

impl RawTag {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Splits `content` into a flat stream of [`RawTag`]s. This is not a general XML
/// tokenizer: it only needs to respect double-quoted attribute values (so a `>`
/// inside a quoted value, or a literal newline inside one -- both real, seen in
/// the `<info>` element's multi-line `date`/`header2`/`header3`/`footer1`
/// attributes -- do not end the tag early), which is all `.gcs` files ever need.
fn tokenize(content: &str) -> Result<Vec<RawTag>, GcsParseError> {
    let bytes = content.as_bytes();
    let mut tags = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            if bytes[i] == b'\n' {
                line += 1;
            }
            i += 1;
            continue;
        }
        let tag_line = line;
        let mut j = i + 1;
        let mut in_quotes = false;
        while j < bytes.len() {
            match bytes[j] {
                b'"' => in_quotes = !in_quotes,
                b'\n' => line += 1,
                b'>' if !in_quotes => break,
                _ => {}
            }
            j += 1;
        }
        if j >= bytes.len() {
            return Err(GcsParseError::UnterminatedTag { line: tag_line });
        }
        let inner = &content[i + 1..j];
        let closing = inner.starts_with('/');
        let self_closing = inner.trim_end().ends_with('/');
        let core = inner
            .strip_prefix('/')
            .unwrap_or(inner)
            .trim_end_matches('/')
            .trim();
        let (name, attr_str) = core
            .find(char::is_whitespace)
            .map_or((core, ""), |p| (&core[..p], core[p..].trim_start()));
        tags.push(RawTag {
            name: name.to_string(),
            attrs: parse_attrs(attr_str, tag_line)?,
            self_closing,
            closing,
        });
        i = j + 1;
    }
    Ok(tags)
}

/// Parses `key="value"` pairs out of one tag's attribute text, decoding the five
/// standard XML entities in each value. Order-independent (see module docs: the
/// corpus's `<index>` attribute order happens to be consistent, but this crate
/// does not rely on that).
fn parse_attrs(s: &str, line: usize) -> Result<Vec<(String, String)>, GcsParseError> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = s[key_start..i].to_string();
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i >= bytes.len() {
            return Err(GcsParseError::MalformedAttribute { line, key });
        }
        i += 1; // opening quote
        let val_start = i;
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i >= bytes.len() {
            return Err(GcsParseError::MalformedAttribute { line, key });
        }
        let raw_value = &s[val_start..i];
        i += 1; // closing quote
        out.push((key, decode_entities(raw_value)));
    }
    Ok(out)
}

/// Decodes the five standard XML entities. None appear in the sampled corpus, but
/// a real title or footnote containing `&` or `"` would need this, so it costs
/// nothing to handle correctly rather than assume it never happens.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn required_index_attr<'a>(tag: &'a RawTag, key: &'static str) -> Result<&'a str, GcsParseError> {
    tag.attr(key)
        .ok_or(GcsParseError::IndexAttributeMissing { attr: key })
}

fn parse_index_u32(tag: &RawTag, key: &'static str) -> Result<u32, GcsParseError> {
    let raw = required_index_attr(tag, key)?;
    // Values are written as plain integers in every sample, but parse via f64
    // first so a stray "4.0"-style decimal (as `.asc` tolerates on its own
    // integer fields) would not need a second code path here.
    //
    // Upper-bounded at `u32::MAX`: `as u32` on a finite `f64` outside that range
    // SATURATES rather than erroring (stable Rust's documented float-to-int cast
    // behavior), so e.g. "1e20" must be rejected rather than silently becoming
    // `u32::MAX`.
    raw.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && (0.0..=f64::from(u32::MAX)).contains(v))
        .map(|v| v.round() as u32)
        .ok_or_else(|| GcsParseError::IndexAttributeNotNumeric {
            attr: key,
            value: raw.to_string(),
        })
}

fn parse_index_f64(tag: &RawTag, key: &'static str) -> Result<f64, GcsParseError> {
    let raw = required_index_attr(tag, key)?;
    raw.parse()
        .map_err(|_| GcsParseError::IndexAttributeNotNumeric {
            attr: key,
            value: raw.to_string(),
        })
}

fn parse_index(tag: &RawTag) -> Result<GcsIndex, GcsParseError> {
    Ok(GcsIndex {
        gear: parse_index_u32(tag, "gear")?,
        base: parse_index_f64(tag, "base")?,
        symmetry: parse_index_u32(tag, "symmetry")?,
        mirror: parse_index_u32(tag, "mirror")?,
    })
}

fn tier_f64(tag: &RawTag, tier_index: usize, key: &'static str) -> Result<f64, GcsParseError> {
    let raw = tag.attr(key).ok_or(GcsParseError::TierAttributeMissing {
        tier_index,
        attr: key,
    })?;
    raw.parse()
        .map_err(|_| GcsParseError::TierAttributeNotNumeric {
            tier_index,
            attr: key,
            value: raw.to_string(),
        })
}

fn facet_f64(tag: &RawTag, tier_index: usize, key: &'static str) -> Result<f64, GcsParseError> {
    let raw = tag.attr(key).unwrap_or("0");
    raw.parse()
        .map_err(|_| GcsParseError::FacetAttributeNotNumeric {
            tier_index,
            attr: key,
            value: raw.to_string(),
        })
}

fn vertex_f64(tag: &RawTag, tier_index: usize, key: &'static str) -> Result<f64, GcsParseError> {
    let raw = tag.attr(key).unwrap_or("0");
    raw.parse()
        .map_err(|_| GcsParseError::VertexAttributeNotNumeric {
            tier_index,
            attr: key,
            value: raw.to_string(),
        })
}

/// Parses one `<render>` numeric attribute: `Ok(0.0)` when `key` is absent (these
/// fields are optional -- not every real `.gcs` file's `<render>` element sets all of
/// them), but returns an error rather than silently defaulting to `0.0` when `key` IS
/// present and fails to parse. Garbage masquerading as "no data" deserves the same
/// treatment as the corruption [`tier_f64`]'s fallible path catches for `<tier>`.
fn render_f64(tag: &RawTag, key: &'static str) -> Result<f64, GcsParseError> {
    tag.attr(key).map_or(Ok(0.0), |raw| {
        raw.parse()
            .map_err(|_| GcsParseError::RenderAttributeNotNumeric {
                attr: key,
                value: raw.to_string(),
            })
    })
}

/// Consumes tags from `tags[*pos..]` until (and including) the `</facet>` that
/// closes `open`, building the facet's vertex list.
fn parse_facet(
    tags: &[RawTag],
    pos: &mut usize,
    tier_index: usize,
) -> Result<GcsFacet, GcsParseError> {
    let open = &tags[*pos];
    let normal = [
        facet_f64(open, tier_index, "nx")?,
        facet_f64(open, tier_index, "ny")?,
        facet_f64(open, tier_index, "nz")?,
    ];
    let index_angle_deg = facet_f64(open, tier_index, "index_angle")?;
    let self_closing = open.self_closing;
    *pos += 1;

    let mut vertices = Vec::new();
    if self_closing {
        // Not seen in the sampled corpus (every real facet has at least one
        // vertex), but a self-closing `<facet .../>` has no `</facet>` to look
        // for -- treat it as a facet with no vertices rather than scanning past
        // whatever tag happens to come next.
        return Ok(GcsFacet {
            normal,
            index_angle_deg,
            vertices,
        });
    }
    loop {
        let Some(tag) = tags.get(*pos) else {
            return Err(GcsParseError::UnterminatedElement { name: "facet" });
        };
        *pos += 1;
        if tag.name == "facet" && tag.closing {
            break;
        }
        if tag.name == "vertex" {
            vertices.push(GcsVertex {
                x: vertex_f64(tag, tier_index, "x")?,
                y: vertex_f64(tag, tier_index, "y")?,
                z: vertex_f64(tag, tier_index, "z")?,
            });
        }
    }
    Ok(GcsFacet {
        normal,
        index_angle_deg,
        vertices,
    })
}

/// Consumes tags from `tags[*pos..]` until (and including) the `</tier>` that
/// closes `open`, building the tier's facet list.
fn parse_tier(
    tags: &[RawTag],
    pos: &mut usize,
    tier_index: usize,
) -> Result<GcsTier, GcsParseError> {
    let open = &tags[*pos];
    let angle_deg = tier_f64(open, tier_index, "angle")?;
    let depth = tier_f64(open, tier_index, "depth")?;
    let name = open.attr("name").unwrap_or("").to_string();
    let instructions = open.attr("instructions").unwrap_or("").to_string();
    let visible = open.attr("visible") != Some("false");
    let guide = open.attr("guide") == Some("true");
    let self_closing = open.self_closing;
    *pos += 1;

    let mut facets = Vec::new();
    if self_closing {
        // Not seen in the sampled corpus, but a self-closing `<tier .../>` (a
        // tier with no facets at all) has no `</tier>` to scan for.
        return Ok(GcsTier {
            angle_deg,
            depth,
            name,
            instructions,
            visible,
            guide,
            facets,
        });
    }
    loop {
        let Some(tag) = tags.get(*pos) else {
            return Err(GcsParseError::UnterminatedElement { name: "tier" });
        };
        if tag.name == "tier" && tag.closing {
            *pos += 1;
            break;
        }
        if tag.name == "facet" && !tag.closing {
            facets.push(parse_facet(tags, pos, tier_index)?);
        } else {
            *pos += 1;
        }
    }
    Ok(GcsTier {
        angle_deg,
        depth,
        name,
        instructions,
        visible,
        guide,
        facets,
    })
}

/// Consumes tags from `tags[*pos..]` until (and including) the `</render>` that
/// closes `open`.
fn parse_render(tags: &[RawTag], pos: &mut usize) -> Result<GcsRender, GcsParseError> {
    let open = &tags[*pos];
    let material = open.attr("material").unwrap_or("").to_string();
    let refractive_index = render_f64(open, "refractive_index")?;
    let dispersion = render_f64(open, "dispersion")?;
    let clarity = render_f64(open, "clarity")?;
    let density = render_f64(open, "density")?;
    let lighting_model = open.attr("lighting_model").unwrap_or("").to_string();
    let self_closing = open.self_closing;
    *pos += 1;

    let mut color = GcsColor::default();
    if self_closing {
        // Not seen in the sampled corpus (every real `<render>` wraps a
        // `<color>`), but a self-closing `<render .../>` has no `</render>` to
        // scan for.
        return Ok(GcsRender {
            material,
            refractive_index,
            dispersion,
            clarity,
            density,
            lighting_model,
            color,
        });
    }
    loop {
        let Some(tag) = tags.get(*pos) else {
            return Err(GcsParseError::UnterminatedElement { name: "render" });
        };
        *pos += 1;
        if tag.name == "render" && tag.closing {
            break;
        }
        if tag.name == "color" {
            color = GcsColor {
                r: tag.attr("r").unwrap_or("0").parse().unwrap_or(0.0),
                g: tag.attr("g").unwrap_or("0").parse().unwrap_or(0.0),
                b: tag.attr("b").unwrap_or("0").parse().unwrap_or(0.0),
            };
        }
    }
    Ok(GcsRender {
        material,
        refractive_index,
        dispersion,
        clarity,
        density,
        lighting_model,
        color,
    })
}

fn opt_string(tag: &RawTag, key: &str) -> Option<String> {
    tag.attr(key).map(str::to_string)
}

fn parse_info(tag: &RawTag) -> GcsInfo {
    GcsInfo {
        title: opt_string(tag, "title"),
        author: opt_string(tag, "author"),
        date: opt_string(tag, "date"),
        shape: opt_string(tag, "shape"),
        header2: opt_string(tag, "header2"),
        header3: opt_string(tag, "header3"),
        ri_min: opt_string(tag, "ri_min"),
        ri_max: opt_string(tag, "ri_max"),
        size_min: opt_string(tag, "size_min"),
        size_max: opt_string(tag, "size_max"),
        footer1: opt_string(tag, "footer1"),
        footer2: opt_string(tag, "footer2"),
        footer3: opt_string(tag, "footer3"),
        footer4: opt_string(tag, "footer4"),
    }
}

/// Parses a Gem Cut Studio `.gcs` design.
///
/// # Errors
///
/// Returns `Err` if `content` is empty, is not well-formed enough for
/// [`tokenize`] to find matching `<`/`>` pairs and quoted attribute values, is
/// missing the `<GemCutStudio>` root or its `<index>` child, or has a tier,
/// facet, or vertex with a missing or non-numeric required attribute. Unknown
/// elements and unknown attributes are ignored rather than rejected, so a future
/// Gem Cut Studio version that adds fields this module does not know about should
/// still parse.
pub fn parse_gcs(content: &str) -> Result<GcsDesign, GcsParseError> {
    if content.trim().is_empty() {
        return Err(GcsParseError::EmptyInput);
    }
    let tags = tokenize(content)?;

    let mut pos = 0usize;
    let root = tags.first().ok_or(GcsParseError::MissingRootElement)?;
    if root.name != "GemCutStudio" || root.closing {
        return Err(GcsParseError::MissingRootElement);
    }
    let version = root.attr("version").unwrap_or("").to_string();
    pos += 1;

    let mut index = None;
    let mut tiers = Vec::new();
    let mut render = None;
    let mut info = None;

    while pos < tags.len() {
        let tag = &tags[pos];
        if tag.name == "GemCutStudio" && tag.closing {
            break;
        }
        match tag.name.as_str() {
            "index" => {
                index = Some(parse_index(tag)?);
                pos += 1;
            }
            "tier" if !tag.closing => {
                let tier_index = tiers.len();
                tiers.push(parse_tier(&tags, &mut pos, tier_index)?);
            }
            "render" if !tag.closing => {
                render = Some(parse_render(&tags, &mut pos)?);
            }
            "info" => {
                info = Some(parse_info(tag));
                pos += 1;
            }
            _ => pos += 1,
        }
    }

    Ok(GcsDesign {
        version,
        index: index.ok_or(GcsParseError::MissingIndexElement)?,
        tiers,
        render,
        info,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real excerpt of `attached_files` id 1124 (detail 553), `"L1000-h0724-
    /// f085-i064-b83-ri170-215-Octagon-pc21086F-FVS-044-Octabar-X-by-Van-Sant-
    /// Fred-W.gcs"` -- "FVS-044 Octabar-X PC 21.086F" by Fred W. Van Sant. The
    /// root, `<index>`, and `<render>`/`<info>` lines are copied verbatim; the
    /// first `<tier>` is trimmed from its real 8 facets down to the first 2 (the
    /// bytes of each kept facet, including every vertex, are untouched) purely to
    /// keep this fixture short -- see `crates/indicatrix-formats/src/gcs.rs`'s
    /// module docs for the full-file cross-check results this trimmed excerpt
    /// can't exercise on its own (tier-to-tier angle/index matching against the
    /// sibling `.asc`).
    const GCS_OCTABAR_X_EXCERPT: &str = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <tier angle="126.38999938964842" depth="0.6664250328253426" name="P1" instructions="" visible="true" guide="false">
        <facet nx="-0" ny="-0.8049973625502862" nz="-0.59327838852184989" index_angle="0">
            <vertex x="-0.082702055286700479" y="-0.81860733222343418" z="-0.012554459365542562"/>
            <vertex x="-0.24803731741144758" y="-0.87723469910015384" z="0.066994832538740542"/>
            <vertex x="-0.41421356237309492" y="-0.99999999999999944" z="0.23357049979554395"/>
            <vertex x="0.41421356237309509" y="-1" z="0.23357049979554453"/>
            <vertex x="0.24803731773169496" y="-0.87723469921371255" z="0.066994832692824094"/>
            <vertex x="0.082702055286700812" y="-0.81860733222343418" z="-0.012554459365542562"/>
        </facet>
        <facet nx="-0.56921909389659309" ny="-0.56921909389659309" nz="-0.59327838852184989" index_angle="45">
            <vertex x="-0.99999999999999944" y="-0.41421356237309503" z="0.23357049979554412"/>
            <vertex x="-0.41421356237309492" y="-0.99999999999999944" z="0.23357049979554395"/>
            <vertex x="-0.57116930351496131" y="-0.5711693029784638" z="-0.027279076108571262"/>
        </facet>
    </tier>
    <render material="176 Corundum" refractive_index="1.76" dispersion="0.017999999" clarity="100" density="1.4" lighting_model="Random">
        <color r="0.70980394" g="0.73333335" b="0.94901967"/>
    </render>
    <info title="FVS-044 Octabar-X PC 21.086F" author="Van Sant, Fred W" date="Star Cuts 1 1998

" header2="This design released into the public domain

" header3="by Keith Wyman in memory of Charles L. Moon

" ri_min="1.7" ri_max="2.1500001" shape="Octagon" footer1="Entered into GCS, filename, shapename, cut sequence and meetpoints revised by Kevin Kane kane2002@telus.net

" footer2="L1000 h0724 f085 i064 b83 ri170-215 Octagon pc21086F FVS-044 Octabar-X by Van Sant, Fred W"/>
</GemCutStudio>
"#;

    #[test]
    fn parses_real_index_element() {
        let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
        assert_eq!(design.version, "1000");
        assert_eq!(design.index.gear, 64);
        assert_eq!(design.index.symmetry, 4);
        assert_eq!(design.index.mirror, 0);
    }

    #[test]
    fn parses_real_tier_and_facets() {
        let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
        assert_eq!(design.tiers.len(), 1);
        let tier = &design.tiers[0];
        assert_eq!(tier.name, "P1");
        assert!((tier.angle_deg - 126.389_999_389_648_42).abs() < 1e-9);
        assert!((tier.depth - 0.666_425_032_825_342_6).abs() < 1e-9);
        assert!(tier.visible);
        assert!(!tier.guide);
        assert_eq!(tier.facets.len(), 2);
        assert_eq!(tier.facets[0].vertices.len(), 6);
        assert_eq!(tier.facets[1].vertices.len(), 3);
        assert!((tier.facets[1].index_angle_deg - 45.0).abs() < 1e-9);
    }

    #[test]
    fn converts_pavilion_angle_to_signed_asc_convention() {
        // Verified against the real sibling `.asc` (attached_files id 1118,
        // detail 553): tier "2" there is angle -53.61435, and 180 + (-53.61435) =
        // 126.38565 -- matching this tier's 126.39 (`.gcs` stores trig fields at
        // float32 precision, hence the small residual).
        let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
        let signed = design.tiers[0].to_signed_asc_angle();
        assert!(
            (signed - (-53.610_001)).abs() < 1e-3,
            "expected roughly -53.61, got {signed}"
        );
    }

    #[test]
    fn parses_real_render_and_info() {
        let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
        let render = design.render.expect("render element must be present");
        assert_eq!(render.material, "176 Corundum");
        assert!((render.refractive_index - 1.76).abs() < 1e-9);

        let info = design.info.expect("info element must be present");
        assert_eq!(info.title.as_deref(), Some("FVS-044 Octabar-X PC 21.086F"));
        assert_eq!(info.author.as_deref(), Some("Van Sant, Fred W"));
        assert_eq!(info.shape.as_deref(), Some("Octagon"));
        // The real file embeds a trailing blank line inside this attribute value;
        // the tokenizer must not treat the embedded newline as ending the tag.
        assert!(
            info.date
                .as_deref()
                .unwrap_or("")
                .starts_with("Star Cuts 1 1998")
        );
    }

    #[test]
    fn facet_plane_count_sums_across_tiers() {
        let design = parse_gcs(GCS_OCTABAR_X_EXCERPT).expect("real excerpt must parse");
        assert_eq!(design.facet_plane_count(), 2);
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse_gcs("").is_err());
        assert!(parse_gcs("   \n  ").is_err());
    }

    #[test]
    fn rejects_missing_root_element() {
        let err = parse_gcs("<index gear=\"64\" base=\"0\" symmetry=\"4\" mirror=\"0\"/>")
            .expect_err("must reject a file with no GemCutStudio root");
        assert_eq!(err, GcsParseError::MissingRootElement);
    }

    #[test]
    fn rejects_missing_index_element() {
        let err = parse_gcs("<GemCutStudio version=\"1000\"></GemCutStudio>")
            .expect_err("must reject a file with no index element");
        assert_eq!(err, GcsParseError::MissingIndexElement);
    }

    #[test]
    fn rejects_unterminated_tag() {
        let err = parse_gcs("<GemCutStudio version=\"1000\"\n<index gear=\"64\"")
            .expect_err("must reject an unterminated tag");
        assert!(matches!(err, GcsParseError::UnterminatedTag { .. }));
    }

    #[test]
    fn rejects_non_numeric_tier_angle() {
        let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <tier angle="not-a-number" depth="1.0" name="T" instructions="" visible="true" guide="false">
    </tier>
</GemCutStudio>"#;
        let err = parse_gcs(content).expect_err("must reject a non-numeric tier angle");
        assert!(matches!(
            err,
            GcsParseError::TierAttributeNotNumeric { attr: "angle", .. }
        ));
    }

    /// `parse_index_u32` must reject a value bigger than `u32::MAX` rather
    /// than silently saturating to it via `as u32`.
    #[test]
    fn rejects_an_index_attribute_larger_than_u32_max() {
        let content = r#"<GemCutStudio version="1000">
    <index gear="1e20" base="0" symmetry="4" mirror="0"/>
</GemCutStudio>"#;
        let err = parse_gcs(content).expect_err("must reject a gear value beyond u32::MAX");
        assert!(
            matches!(
                err,
                GcsParseError::IndexAttributeNotNumeric { attr: "gear", .. }
            ),
            "{err:?}"
        );
    }

    /// A garbled (present but non-numeric) `<render>` attribute must be
    /// reported, not silently treated as `0.0`.
    #[test]
    fn rejects_a_non_numeric_render_attribute() {
        let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <render material="Diamond" refractive_index="not-a-number" dispersion="0.02" clarity="100" density="3.5" lighting_model="Random">
    </render>
</GemCutStudio>"#;
        let err = parse_gcs(content).expect_err("must reject a garbled refractive_index");
        assert!(
            matches!(
                err,
                GcsParseError::RenderAttributeNotNumeric {
                    attr: "refractive_index",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    /// An ABSENT `<render>` numeric attribute is still not an error -- only a present
    /// but garbled one is (see [`render_f64`]'s doc comment).
    #[test]
    fn a_missing_render_attribute_defaults_to_zero_without_erroring() {
        let content = r#"<GemCutStudio version="1000">
    <index gear="64" base="0" symmetry="4" mirror="0"/>
    <render material="Diamond" lighting_model="Random">
    </render>
</GemCutStudio>"#;
        let design = parse_gcs(content).expect("an absent render attribute must not error");
        let render = design.render.expect("render element must still be parsed");
        assert_eq!(render.refractive_index, 0.0);
        assert_eq!(render.dispersion, 0.0);
        assert_eq!(render.clarity, 0.0);
        assert_eq!(render.density, 0.0);
    }

    #[test]
    fn does_not_panic_on_arbitrary_garbage() {
        let samples = [
            "\u{0}\u{1}\u{2}garbage\u{ff}",
            "<<<>>>",
            "<GemCutStudio",
            "<GemCutStudio version=\"1000\">",
        ];
        for s in samples {
            let _ = parse_gcs(s);
        }
    }
}
