//! What an [`Expr`] can do once it is built: map and read its references, calculate it,
//! write it back as text, and the limits a stored [`RelationExpr`] must keep.

use super::{
    Expr, ExprError, MAX_CANONICAL_CHARS, MAX_NODES, MAX_NUMBER, RawName, RelationError,
    RelationExpr, TierId,
};
use std::collections::BTreeSet;

// --- the tree -----------------------------------------------------------------------

impl<L> Expr<L> {
    /// This tree with every reference replaced through `map`; the first error wins.
    ///
    /// # Errors
    ///
    /// Whatever `map` returns.
    pub fn try_map_refs<M, E>(self, map: &mut impl FnMut(L) -> Result<M, E>) -> Result<Expr<M>, E> {
        Ok(match self {
            Self::Number(number) => Expr::Number(number),
            Self::Ref(leaf) => Expr::Ref(map(leaf)?),
            Self::Neg(inner) => Expr::Neg(Box::new((*inner).try_map_refs(map)?)),
            Self::Add(a, b) => Expr::Add(
                Box::new((*a).try_map_refs(map)?),
                Box::new((*b).try_map_refs(map)?),
            ),
            Self::Sub(a, b) => Expr::Sub(
                Box::new((*a).try_map_refs(map)?),
                Box::new((*b).try_map_refs(map)?),
            ),
            Self::Mul(a, b) => Expr::Mul(
                Box::new((*a).try_map_refs(map)?),
                Box::new((*b).try_map_refs(map)?),
            ),
            Self::Div(a, b) => Expr::Div(
                Box::new((*a).try_map_refs(map)?),
                Box::new((*b).try_map_refs(map)?),
            ),
        })
    }

    /// Calculates the expression; `lookup` supplies the value of each reference.
    ///
    /// # Errors
    ///
    /// [`ExprError::DivisionByZero`], [`ExprError::NotFinite`] for a result that is
    /// not a usable number, or whatever `lookup` returns.
    pub fn eval(
        &self,
        lookup: &mut dyn FnMut(&L) -> Result<f64, ExprError>,
    ) -> Result<f64, ExprError> {
        let value = match self {
            Self::Number(number) => *number,
            Self::Ref(leaf) => lookup(leaf)?,
            Self::Neg(inner) => -inner.eval(lookup)?,
            Self::Add(a, b) => a.eval(lookup)? + b.eval(lookup)?,
            Self::Sub(a, b) => a.eval(lookup)? - b.eval(lookup)?,
            Self::Mul(a, b) => a.eval(lookup)? * b.eval(lookup)?,
            Self::Div(a, b) => {
                let numerator = a.eval(lookup)?;
                let denominator = b.eval(lookup)?;
                if denominator == 0.0 {
                    return Err(ExprError::DivisionByZero);
                }
                numerator / denominator
            }
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(ExprError::NotFinite)
        }
    }

    /// Every reference in the tree, in reading order (repeats included).
    #[must_use]
    pub fn refs(&self) -> Vec<&L> {
        let mut found = Vec::new();
        self.collect_refs(&mut found);
        found
    }

    fn collect_refs<'a>(&'a self, found: &mut Vec<&'a L>) {
        match self {
            Self::Number(_) => {}
            Self::Ref(leaf) => found.push(leaf),
            Self::Neg(inner) => inner.collect_refs(found),
            Self::Add(a, b) | Self::Sub(a, b) | Self::Mul(a, b) | Self::Div(a, b) => {
                a.collect_refs(found);
                b.collect_refs(found);
            }
        }
    }

    /// Binding strength used to place brackets: a sum binds loosest (1), a product
    /// next (2), a minus sign (and a negative constant) next (3), an atom tightest (4).
    const fn precedence(&self) -> u8 {
        match self {
            Self::Number(number) if number.is_sign_negative() => 3,
            Self::Number(_) | Self::Ref(_) => 4,
            Self::Neg(_) => 3,
            Self::Add(..) | Self::Sub(..) => 1,
            Self::Mul(..) | Self::Div(..) => 2,
        }
    }

    /// Writes the tree with the fewest brackets that read back as the same tree.
    fn write(&self, out: &mut String, leaf: &mut dyn FnMut(&L, &mut String)) {
        match self {
            Self::Number(number) => out.push_str(&format_number(*number)),
            Self::Ref(reference) => leaf(reference, out),
            Self::Neg(inner) => {
                out.push('-');
                inner.write_wrapped(out, leaf, inner.precedence() <= 3);
            }
            Self::Add(a, b) => Self::write_binary(out, leaf, a, " + ", b, 1),
            Self::Sub(a, b) => Self::write_binary(out, leaf, a, " - ", b, 1),
            Self::Mul(a, b) => Self::write_binary(out, leaf, a, " * ", b, 2),
            Self::Div(a, b) => Self::write_binary(out, leaf, a, " / ", b, 2),
        }
    }

    /// Writes `left op right`. The left operand needs brackets only when it binds
    /// looser; the right one also when it binds equally, because the parser groups
    /// to the left (`a - (b - c)` is not `a - b - c`).
    fn write_binary(
        out: &mut String,
        leaf: &mut dyn FnMut(&L, &mut String),
        left: &Self,
        op: &str,
        right: &Self,
        precedence: u8,
    ) {
        left.write_wrapped(out, leaf, left.precedence() < precedence);
        out.push_str(op);
        right.write_wrapped(out, leaf, right.precedence() <= precedence);
    }

    fn write_wrapped(&self, out: &mut String, leaf: &mut dyn FnMut(&L, &mut String), wrap: bool) {
        if wrap {
            out.push('(');
        }
        self.write(out, leaf);
        if wrap {
            out.push(')');
        }
    }

    /// Writes the tree as text, calling `leaf` to write each reference.
    #[must_use]
    pub fn render(&self, mut leaf: impl FnMut(&L, &mut String)) -> String {
        let mut out = String::new();
        self.write(&mut out, &mut leaf);
        out
    }

    /// The number of nodes in the tree.
    #[must_use]
    pub fn node_count(&self) -> usize {
        match self {
            Self::Number(_) | Self::Ref(_) => 1,
            Self::Neg(inner) => 1 + inner.node_count(),
            Self::Add(a, b) | Self::Sub(a, b) | Self::Mul(a, b) | Self::Div(a, b) => {
                1 + a.node_count() + b.node_count()
            }
        }
    }

    /// Whether every constant is finite and no larger than a relation allows.
    fn constants_usable(&self) -> bool {
        match self {
            Self::Number(number) => number.is_finite() && number.abs() <= MAX_NUMBER,
            Self::Ref(_) => true,
            Self::Neg(inner) => inner.constants_usable(),
            Self::Add(a, b) | Self::Sub(a, b) | Self::Mul(a, b) | Self::Div(a, b) => {
                a.constants_usable() && b.constants_usable()
            }
        }
    }
}

/// A constant as text that reads back exactly: plain digits in the everyday range,
/// scientific notation outside it.
fn format_number(number: f64) -> String {
    let magnitude = number.abs();
    if magnitude < f64::MIN_POSITIVE || (1e-6..1e15).contains(&magnitude) {
        format!("{number}")
    } else {
        format!("{number:e}")
    }
}

/// Pulls a result back onto the nearest 1e-9 grid point when it sits within float
/// noise of one, so `40 + 3 * 0.1` is `40.3` and not `40.300000000000004`.
#[must_use]
pub fn snap_noise(value: f64) -> f64 {
    let snapped = (value * 1e9).round() / 1e9;
    if (snapped - value).abs() < 1e-12 {
        snapped
    } else {
        value
    }
}

impl RelationExpr {
    /// `reference + delta` (written as a subtraction for a negative `delta`, and just
    /// `reference` for zero) -- what a linked step series stores for each rung.
    #[must_use]
    pub fn offset_from(reference: TierId, delta: f64) -> Self {
        let base = Self::Ref(reference);
        if delta == 0.0 {
            base
        } else if delta.is_sign_negative() {
            Self::Sub(Box::new(base), Box::new(Self::Number(-delta)))
        } else {
            Self::Add(Box::new(base), Box::new(Self::Number(delta)))
        }
    }

    /// The ids of every tier the expression reads.
    #[must_use]
    pub fn tier_refs(&self) -> BTreeSet<TierId> {
        self.refs().into_iter().copied().collect()
    }

    /// Checks the tree is within the limits a stored relation must keep.
    ///
    /// # Errors
    ///
    /// [`RelationError::Parse`] when a constant is not finite or is larger than
    /// 1,000,000,000, or the tree has more than 1,024 nodes.
    pub fn validate(&self) -> Result<(), RelationError> {
        if self.node_count() > MAX_NODES || !self.constants_usable() {
            return Err(RelationError::Parse(
                "This relation is too big, or uses a number that is too large.".to_owned(),
            ));
        }
        Ok(())
    }

    /// The canonical text: tiers by id (`@3 - 2`), the form a file stores.
    #[must_use]
    pub fn to_canonical(&self) -> String {
        self.render(|id, out| {
            out.push('@');
            out.push_str(&id.value().to_string());
        })
    }

    /// Reads canonical text back.
    ///
    /// # Errors
    ///
    /// [`RelationError::Parse`] for text that is not canonical: too long, not valid
    /// arithmetic, or naming a tier by anything but its `@id`.
    pub fn parse_canonical(text: &str) -> Result<Self, RelationError> {
        let syntax_tree =
            Expr::<RawName>::parse_syntax(text, MAX_CANONICAL_CHARS).map_err(|error| {
                RelationError::Parse(format!("The saved relation cannot be read: {error}."))
            })?;
        let expr = syntax_tree.try_map_refs(&mut |name| match name {
            RawName::Id(id) => Ok(TierId(id)),
            other => Err(RelationError::Parse(format!(
                "The saved relation names tiers by number (@3), not '{}'.",
                other.text()
            ))),
        })?;
        expr.validate()?;
        Ok(expr)
    }
}
