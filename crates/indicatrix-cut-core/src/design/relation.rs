//! Parametric tier relations: a tier's ANGLE driven by an expression over other
//! tiers' angles and constants, for example "Crown break = C1 - 4" or
//! "P2 = (P1 + P3) / 2".
//!
//! # What a relation is
//!
//! A [`TierRelation`] lives in [`Design::tier_relations`], keyed by the driven tier's
//! [`TierId`] exactly like [`Design::tier_targets`] (so it survives add, remove, move
//! and undo). Its [`RelationExpr`] is a small syntax tree of numbers, tier references,
//! `+ - * /` and unary minus. Parentheses only group while parsing; they are not nodes.
//! Version 1 drives angles only.
//!
//! The values are angle MAGNITUDES in degrees: a pavilion angle is stored negative,
//! but a reference to it yields its absolute value, and the result must lie in
//! `(0, 90]`. The driven tier keeps its own side of the girdle (its current sign).
//! A table or culet (angle 0) and a girdle (90) cannot be driven.
//!
//! # Two text forms
//!
//! - USER text names tiers: `P1`, or `[Crown Main]` for names with spaces or symbols.
//!   A name resolves exactly first, then ASCII case-insensitively, like
//!   `MeetConstraint::MeetNamed`. `@3` (the tier's stable id) is accepted too.
//!   [`TierRelation::parse_user`] reads it and [`TierRelation::to_display`] writes it.
//! - CANONICAL text, the one a file stores, names tiers by id (`@3 - 2`) so a rename
//!   never breaks a saved relation. [`TierRelation::to_canonical`] writes it and
//!   [`TierRelation::parse_canonical`] reads it back.
//!
//! # Evaluation
//!
//! [`Design::evaluate_relations`] orders the driven tiers so a tier is computed after
//! the driven tiers it reads, reports a loop between them as
//! [`RelationError::Cycle`], and checks every result is in range. Nothing here mutates
//! a design: the editor session turns the result into an undoable edit (see
//! `indicatrix_editor::session`).
//!
//! The arithmetic parser is generic over what a name stands for ([`Expr`]), so the
//! editor's number fields reuse it for plain sums such as `41 + 0.5`.

use super::{Design, TierId};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

mod parse;
mod tree;

use parse::is_bare_name;
pub use tree::snap_noise;

/// The longest relation text a user may type, in characters.
pub const MAX_RELATION_CHARS: usize = 256;

/// The longest canonical relation text a file may hold, in characters. Larger than
/// [`MAX_RELATION_CHARS`] because a name such as `P1` becomes an id such as `@12345`.
pub const MAX_CANONICAL_CHARS: usize = 1024;

/// The deepest nesting of parentheses and unary minus signs the parser accepts.
pub const MAX_NESTING: usize = 32;

/// The most nodes an expression may have.
const MAX_NODES: usize = 1024;

/// The largest number a relation may contain.
const MAX_NUMBER: f64 = 1e9;

/// Below this angle magnitude a tier counts as horizontal (a table or a culet).
const HORIZONTAL_EPSILON_DEG: f64 = 1e-9;

/// Within this of 90 degrees a tier counts as a girdle tier, the solver's own rule.
const GIRDLE_EPSILON_DEG: f64 = 1e-6;

/// The largest angle magnitude a tier can have.
const MAX_ANGLE_DEG: f64 = 90.0;

/// Every tier relation of a design, keyed by the driven tier's [`TierId`].
pub type TierRelationMap = BTreeMap<TierId, TierRelation>;

/// Why a piece of arithmetic could not be read or calculated. The `Display` text is
/// plain English, ready to show next to a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprError {
    /// Nothing was typed.
    Empty,
    /// The text is longer than the limit.
    TooLong {
        /// The limit, in characters.
        max: usize,
    },
    /// Parentheses are nested deeper than [`MAX_NESTING`].
    TooDeep,
    /// The expression has more nodes, or a number larger, than a relation may hold.
    TooBig,
    /// The text is not a valid calculation; the message says what is wrong.
    Syntax(String),
    /// A name could not be used; the message says why.
    UnknownName(String),
    /// A division by zero.
    DivisionByZero,
    /// The result is not a usable number.
    NotFinite,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("nothing to calculate"),
            Self::TooLong { max } => write!(f, "it is too long (at most {max} characters)"),
            Self::TooDeep => f.write_str("it has too many brackets inside brackets"),
            Self::TooBig => f.write_str("it is too big"),
            Self::Syntax(message) | Self::UnknownName(message) => f.write_str(message),
            Self::DivisionByZero => f.write_str("it divides by zero"),
            Self::NotFinite => f.write_str("it does not give a usable number"),
        }
    }
}

impl std::error::Error for ExprError {}

/// A name as it was written in the text, before it is resolved to anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawName {
    /// A bare word such as `P1` or `Table`.
    Bare(String),
    /// A name in square brackets such as `[Crown Main]`, without the brackets.
    Bracketed(String),
    /// A tier id written as `@3`.
    Id(u64),
}

impl RawName {
    /// The name as the cutter typed it (`@3` for an id), for messages.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Bare(name) => name.clone(),
            Self::Bracketed(name) => format!("[{name}]"),
            Self::Id(id) => format!("@{id}"),
        }
    }
}

/// A small arithmetic syntax tree whose leaves are of type `L`: tier ids in a
/// relation ([`RelationExpr`]), names in the editor's number fields.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr<L> {
    /// A constant.
    Number(f64),
    /// A reference to something outside the expression (a tier).
    Ref(L),
    /// Unary minus.
    Neg(Box<Self>),
    /// Addition.
    Add(Box<Self>, Box<Self>),
    /// Subtraction.
    Sub(Box<Self>, Box<Self>),
    /// Multiplication.
    Mul(Box<Self>, Box<Self>),
    /// Division.
    Div(Box<Self>, Box<Self>),
}

/// The expression of a [`TierRelation`]: leaves are tier ids.
pub type RelationExpr = Expr<TierId>;

/// A tier's angle as an expression over other tiers' angle magnitudes -- see the
/// module documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct TierRelation {
    /// What the driven tier's angle magnitude equals.
    pub angle: RelationExpr,
}

impl TierRelation {
    /// A relation driving a tier's angle from `angle`.
    #[must_use]
    pub const fn new(angle: RelationExpr) -> Self {
        Self { angle }
    }

    /// Reads the text a cutter typed, with tier names resolved against `design`. A
    /// leading `=` is allowed and ignored (`=C1-4`).
    ///
    /// # Errors
    ///
    /// [`RelationError::Parse`] with a plain-English reason: the text is empty, too
    /// long, not valid arithmetic, or names a tier that does not exist or that more
    /// than one tier bears.
    pub fn parse_user(text: &str, design: &Design) -> Result<Self, RelationError> {
        let body = text.trim();
        let body = body.strip_prefix('=').unwrap_or(body).trim();
        let cannot_read = |error: ExprError| {
            RelationError::Parse(format!("This relation cannot be read: {error}."))
        };
        if body.is_empty() {
            return Err(RelationError::Parse(
                "Type a relation, for example C1 - 4 or (P1 + P3) / 2.".to_owned(),
            ));
        }
        let syntax_tree =
            Expr::<RawName>::parse_syntax(body, MAX_RELATION_CHARS).map_err(cannot_read)?;
        let angle = syntax_tree
            .try_map_refs(&mut |name| design.resolve_relation_name(&name).map_err(cannot_read))?;
        angle.validate()?;
        Ok(Self { angle })
    }

    /// The canonical text -- see [`RelationExpr::to_canonical`].
    #[must_use]
    pub fn to_canonical(&self) -> String {
        self.angle.to_canonical()
    }

    /// Reads canonical text -- see [`RelationExpr::parse_canonical`].
    ///
    /// # Errors
    ///
    /// As [`RelationExpr::parse_canonical`].
    pub fn parse_canonical(text: &str) -> Result<Self, RelationError> {
        RelationExpr::parse_canonical(text).map(Self::new)
    }

    /// The text for a cutter: tiers by their current names (`P1 - 2`). A tier is
    /// written as `@id` only when it has no name a reader could resolve back.
    #[must_use]
    pub fn to_display(&self, design: &Design) -> String {
        self.angle
            .render(|id, out| out.push_str(&design.relation_leaf_text(*id)))
    }

    /// The ids of every tier this relation reads.
    #[must_use]
    pub fn references(&self) -> BTreeSet<TierId> {
        self.angle.tier_refs()
    }
}

// --- errors -------------------------------------------------------------------------

/// Why a relation cannot be set or satisfied. The `Display` text is plain English.
#[derive(Debug, Clone, PartialEq)]
pub enum RelationError {
    /// These tiers (by name, in tier order) read each other in a loop. One name
    /// means a tier that reads itself.
    Cycle(Vec<String>),
    /// A relation reads a tier that is no longer in the design.
    MissingTier {
        /// The driven tier's label.
        tier: String,
    },
    /// A relation's result is outside `(0, 90]` degrees.
    OutOfRange {
        /// The driven tier's label.
        tier: String,
        /// The result, in degrees.
        value: f64,
    },
    /// The text could not be read; the message says why.
    Parse(String),
    /// The tier is flat (a table or culet), so its angle cannot follow a relation.
    HorizontalTier {
        /// The tier's label.
        tier: String,
    },
    /// The tier is a girdle tier (90 degrees), so its angle cannot follow a relation.
    GirdleTier {
        /// The tier's label.
        tier: String,
    },
    /// A relation divides by zero.
    DivisionByZero {
        /// The driven tier's label.
        tier: String,
    },
    /// A relation does not give a usable number.
    NotFinite {
        /// The driven tier's label.
        tier: String,
    },
    /// There is no tier at this position.
    NoSuchTier {
        /// The position asked for.
        index: usize,
    },
}

impl fmt::Display for RelationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cycle(names) => match names.as_slice() {
                [] => f.write_str("Some tiers refer to each other in a loop."),
                [only] => write!(f, "{only} refers to itself."),
                [first, second] => write!(f, "{first} and {second} refer to each other in a loop."),
                [rest @ .., last] => {
                    write!(
                        f,
                        "{} and {last} refer to each other in a loop.",
                        rest.join(", ")
                    )
                }
            },
            Self::MissingTier { tier } => {
                write!(
                    f,
                    "{tier} refers to a tier that is no longer in the design."
                )
            }
            Self::OutOfRange { tier, value } => write!(
                f,
                "{tier} would come out at {value:.2}\u{b0}, but a facet angle must be more \
                 than 0\u{b0} and at most {MAX_ANGLE_DEG}\u{b0}."
            ),
            Self::Parse(message) => f.write_str(message),
            Self::HorizontalTier { tier } => {
                write!(f, "{tier} is flat, so its angle cannot follow a relation.")
            }
            Self::GirdleTier { tier } => write!(
                f,
                "{tier} is a girdle facet (90\u{b0}), so its angle cannot follow a relation."
            ),
            Self::DivisionByZero { tier } => write!(f, "The relation for {tier} divides by zero."),
            Self::NotFinite { tier } => {
                write!(f, "The relation for {tier} does not give a usable number.")
            }
            Self::NoSuchTier { index } => write!(f, "There is no tier {}.", index + 1),
        }
    }
}

impl std::error::Error for RelationError {}

// --- the design side ----------------------------------------------------------------

impl Design {
    /// Whether both designs give every tier slot the same relation -- [`Self::eq`]'s
    /// building block, comparing by position so id numbering does not matter.
    pub(super) fn tier_relations_eq(&self, other: &Self) -> bool {
        if self.tier_relations.is_empty() && other.tier_relations.is_empty() {
            return true;
        }
        self.tiers.len() == other.tiers.len()
            && (0..self.tiers.len())
                .all(|index| self.tier_relation(index) == other.tier_relation(index))
    }

    /// The relation driving the tier CURRENTLY at `index`, if any.
    #[must_use]
    pub fn tier_relation(&self, index: usize) -> Option<&TierRelation> {
        self.tier_ids
            .get(index)
            .and_then(|id| self.tier_relations.get(id))
    }

    /// The relation driving the tier known by `id`, wherever it sits now.
    #[must_use]
    pub fn tier_relation_for_id(&self, id: TierId) -> Option<&TierRelation> {
        self.tier_relations.get(&id)
    }

    /// Whether the tier CURRENTLY at `index` has its angle driven by a relation.
    #[must_use]
    pub fn is_tier_driven(&self, index: usize) -> bool {
        self.tier_relation(index).is_some()
    }

    /// The tiers (positions, ascending) the relation of the tier at `index` reads;
    /// empty when the tier is not driven.
    #[must_use]
    pub fn relation_drivers(&self, index: usize) -> Vec<usize> {
        let Some(relation) = self.tier_relation(index) else {
            return Vec::new();
        };
        let mut positions: Vec<usize> = relation
            .references()
            .into_iter()
            .filter_map(|id| self.index_of_tier_id(id))
            .collect();
        positions.sort_unstable();
        positions
    }

    /// The driven tiers (positions, ascending) whose relation reads the tier at
    /// `index` directly.
    #[must_use]
    pub fn relation_dependants(&self, index: usize) -> Vec<usize> {
        let Some(id) = self.tier_ids.get(index) else {
            return Vec::new();
        };
        let mut positions: Vec<usize> = self
            .tier_relations
            .iter()
            .filter(|(_, relation)| relation.references().contains(id))
            .filter_map(|(driven, _)| self.index_of_tier_id(*driven))
            .collect();
        positions.sort_unstable();
        positions
    }

    /// The relation of the tier at `index` as text for a cutter (`P1 - 2`).
    #[must_use]
    pub fn relation_text(&self, index: usize) -> Option<String> {
        self.tier_relation(index)
            .map(|relation| relation.to_display(self))
    }

    /// Reads relation text typed by a cutter against this design.
    ///
    /// # Errors
    ///
    /// As [`TierRelation::parse_user`].
    pub fn parse_relation(&self, text: &str) -> Result<TierRelation, RelationError> {
        TierRelation::parse_user(text, self)
    }

    /// The id the next [`crate::edit::Edit::AddTier`] will give its tier (counting
    /// ids a direct push onto `tiers` has not been given yet).
    #[must_use]
    pub const fn peek_next_tier_id(&self) -> TierId {
        let unsynced = self.tiers.len().saturating_sub(self.tier_ids.len());
        TierId(self.next_tier_id + unsynced as u64)
    }

    /// Checks the tier at `index` may have a relation at all.
    ///
    /// # Errors
    ///
    /// [`RelationError::NoSuchTier`], [`RelationError::HorizontalTier`] for a table
    /// or culet, [`RelationError::GirdleTier`] for a 90 degree tier.
    pub fn check_relation_target(&self, index: usize) -> Result<(), RelationError> {
        let Some(tier) = self.tiers.get(index) else {
            return Err(RelationError::NoSuchTier { index });
        };
        let magnitude = tier.angle_deg.abs();
        if magnitude < HORIZONTAL_EPSILON_DEG {
            Err(RelationError::HorizontalTier {
                tier: self.relation_label(index),
            })
        } else if (magnitude - MAX_ANGLE_DEG).abs() < GIRDLE_EPSILON_DEG {
            Err(RelationError::GirdleTier {
                tier: self.relation_label(index),
            })
        } else {
            Ok(())
        }
    }

    /// Every driven tier's angle as its relation gives it now, as
    /// `(position, signed angle)` in an order where a tier comes after the driven
    /// tiers it reads. Empty when the design has no relations. The driven tier keeps
    /// the side (sign) of its current angle. This changes nothing; the caller applies
    /// the result.
    ///
    /// # Errors
    ///
    /// [`RelationError::MissingTier`] for a relation reading a removed tier,
    /// [`RelationError::Cycle`] for tiers that read each other in a loop, and
    /// [`RelationError::OutOfRange`], [`RelationError::DivisionByZero`] or
    /// [`RelationError::NotFinite`] for a result that cannot be an angle.
    pub fn evaluate_relations(&self) -> Result<Vec<(usize, f64)>, RelationError> {
        if self.tier_relations.is_empty() {
            return Ok(Vec::new());
        }
        let positions = self.relation_positions();
        let driven = self.driven_tiers(&positions)?;
        let order = self.evaluation_order(&driven, &positions)?;
        let mut magnitudes: BTreeMap<usize, f64> = BTreeMap::new();
        let mut updates = Vec::with_capacity(order.len());
        for position in order {
            let Some(relation) = driven.get(&position) else {
                continue;
            };
            let magnitude = self.evaluate_one(position, relation, &positions, &magnitudes)?;
            magnitudes.insert(position, magnitude);
            let negative = self.tiers[position].angle_deg.is_sign_negative();
            updates.push((position, if negative { -magnitude } else { magnitude }));
        }
        Ok(updates)
    }

    /// Tier id -> current position.
    fn relation_positions(&self) -> BTreeMap<TierId, usize> {
        (0..self.tiers.len())
            .map(|index| (self.tier_id_at_or_synthetic(index), index))
            .collect()
    }

    /// The relations of tiers that still exist, by tier position; fails when one
    /// reads a tier that does not.
    fn driven_tiers<'a>(
        &'a self,
        positions: &BTreeMap<TierId, usize>,
    ) -> Result<BTreeMap<usize, &'a TierRelation>, RelationError> {
        let mut driven = BTreeMap::new();
        for (id, relation) in &self.tier_relations {
            let Some(&position) = positions.get(id) else {
                continue;
            };
            if relation
                .references()
                .iter()
                .any(|reference| !positions.contains_key(reference))
            {
                return Err(RelationError::MissingTier {
                    tier: self.relation_label(position),
                });
            }
            driven.insert(position, relation);
        }
        Ok(driven)
    }

    /// The driven tiers' positions with every tier after the driven tiers it reads;
    /// ties broken by position so the order is deterministic.
    fn evaluation_order(
        &self,
        driven: &BTreeMap<usize, &TierRelation>,
        positions: &BTreeMap<TierId, usize>,
    ) -> Result<Vec<usize>, RelationError> {
        // `reads[d]` is the set of driven tiers `d` reads.
        let reads: BTreeMap<usize, BTreeSet<usize>> = driven
            .iter()
            .map(|(&position, relation)| {
                let among_driven = relation
                    .references()
                    .iter()
                    .filter_map(|id| positions.get(id).copied())
                    .filter(|other| driven.contains_key(other))
                    .collect();
                (position, among_driven)
            })
            .collect();
        let mut waiting: BTreeMap<usize, usize> = reads
            .iter()
            .map(|(&position, deps)| (position, deps.len()))
            .collect();
        let mut ready: BTreeSet<usize> = waiting
            .iter()
            .filter(|&(_, &count)| count == 0)
            .map(|(&position, _)| position)
            .collect();
        let mut order = Vec::with_capacity(driven.len());
        while let Some(position) = ready.pop_first() {
            order.push(position);
            waiting.remove(&position);
            for (&reader, deps) in &reads {
                if deps.contains(&position)
                    && let Some(count) = waiting.get_mut(&reader)
                {
                    *count -= 1;
                    if *count == 0 {
                        ready.insert(reader);
                    }
                }
            }
        }
        if waiting.is_empty() {
            Ok(order)
        } else {
            Err(self.cycle_error(&reads, &waiting))
        }
    }

    /// The loop among the tiers that could not be ordered, named in tier order.
    fn cycle_error(
        &self,
        reads: &BTreeMap<usize, BTreeSet<usize>>,
        waiting: &BTreeMap<usize, usize>,
    ) -> RelationError {
        // Every tier still waiting reads another tier still waiting, so following
        // "the first waiting tier this one reads" must come back to a tier seen before.
        let mut path: Vec<usize> = Vec::new();
        let mut current = waiting.keys().next().copied();
        while let Some(position) = current {
            if let Some(start) = path.iter().position(|&seen| seen == position) {
                let mut members: Vec<usize> = path[start..].to_vec();
                members.sort_unstable();
                return RelationError::Cycle(
                    members
                        .into_iter()
                        .map(|member| self.relation_label(member))
                        .collect(),
                );
            }
            path.push(position);
            current = reads
                .get(&position)
                .and_then(|deps| deps.iter().find(|dep| waiting.contains_key(dep)).copied());
        }
        RelationError::Cycle(Vec::new())
    }

    /// One tier's new angle magnitude.
    fn evaluate_one(
        &self,
        position: usize,
        relation: &TierRelation,
        positions: &BTreeMap<TierId, usize>,
        computed: &BTreeMap<usize, f64>,
    ) -> Result<f64, RelationError> {
        let mut lookup = |id: &TierId| -> Result<f64, ExprError> {
            let reference = positions
                .get(id)
                .copied()
                .ok_or_else(|| ExprError::UnknownName("a tier is missing".to_owned()))?;
            Ok(computed
                .get(&reference)
                .copied()
                .unwrap_or_else(|| self.tiers[reference].angle_deg.abs()))
        };
        let tier = || self.relation_label(position);
        let value = relation
            .angle
            .eval(&mut lookup)
            .map_err(|error| match error {
                ExprError::DivisionByZero => RelationError::DivisionByZero { tier: tier() },
                ExprError::NotFinite => RelationError::NotFinite { tier: tier() },
                other => RelationError::Parse(other.to_string()),
            })?;
        let value = snap_noise(value);
        if value > 0.0 && value <= MAX_ANGLE_DEG {
            Ok(value)
        } else {
            Err(RelationError::OutOfRange {
                tier: tier(),
                value,
            })
        }
    }

    /// The tier at `index` as a name for messages: its name, else `tier N` (counted
    /// from 1, like the tier table).
    #[must_use]
    pub fn relation_label(&self, index: usize) -> String {
        match self.tiers.get(index) {
            Some(tier) if !tier.name.is_empty() => tier.name.clone(),
            _ => format!("tier {}", index + 1),
        }
    }

    /// The position of the one tier a typed name stands for: an exact name first, then
    /// ASCII case-insensitively. A name may be one of the `/`-joined names of a tier.
    ///
    /// # Errors
    ///
    /// [`ExprError::UnknownName`] when no tier bears the name or more than one does.
    pub fn tier_position_by_name(&self, name: &str) -> Result<usize, ExprError> {
        let matching = |same: &dyn Fn(&str, &str) -> bool| -> Vec<usize> {
            self.tiers
                .iter()
                .enumerate()
                .filter(|(_, tier)| {
                    !tier.name.is_empty()
                        && (same(&tier.name, name) || tier.names().iter().any(|n| same(n, name)))
                })
                .map(|(index, _)| index)
                .collect()
        };
        let mut found = matching(&|a, b| a == b);
        if found.is_empty() {
            found = matching(&|a, b| a.eq_ignore_ascii_case(b));
        }
        match found.as_slice() {
            [] => Err(ExprError::UnknownName(format!(
                "there is no tier called '{name}'"
            ))),
            [only] => Ok(*only),
            _ => Err(ExprError::UnknownName(format!(
                "more than one tier is called '{name}' -- rename one of them first"
            ))),
        }
    }

    /// The id of the tier a typed name stands for -- see [`Self::tier_position_by_name`].
    fn resolve_tier_name(&self, name: &str) -> Result<TierId, ExprError> {
        self.tier_position_by_name(name)
            .map(|position| self.tier_id_at_or_synthetic(position))
    }

    /// Resolves one name written in relation text to a tier id.
    fn resolve_relation_name(&self, name: &RawName) -> Result<TierId, ExprError> {
        match name {
            RawName::Bare(text) | RawName::Bracketed(text) => self.resolve_tier_name(text),
            RawName::Id(id) => {
                let tier = TierId(*id);
                if self.index_of_tier_id(tier).is_some() {
                    Ok(tier)
                } else {
                    Err(ExprError::UnknownName(format!("there is no tier @{id}")))
                }
            }
        }
    }

    /// How a reference to `id` is written for a cutter: a bare name, a bracketed
    /// name, or `@id` when no name would read back as this tier.
    fn relation_leaf_text(&self, id: TierId) -> String {
        let Some(index) = self.index_of_tier_id(id) else {
            return format!("@{}", id.value());
        };
        let name = &self.tiers[index].name;
        let reads_back = !name.is_empty()
            && self
                .resolve_tier_name(name)
                .is_ok_and(|resolved| resolved == id);
        if reads_back && is_bare_name(name) {
            name.clone()
        } else if reads_back && !name.contains(']') && name.trim() == name {
            format!("[{name}]")
        } else {
            format!("@{}", id.value())
        }
    }
}

#[cfg(test)]
mod tests;
