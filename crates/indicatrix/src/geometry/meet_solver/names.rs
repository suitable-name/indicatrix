//! Resolving stated `"Meet <names>"` text against a design's tiers.
//!
//! [`MeetNameResolver`] is the one place the corpus's informal reference styles
//! (unnamed girdle/culet/table references, compound `"1-2-G1"` vertex specs,
//! connective prose, case and side-prefix mismatches) get handled, so the solve
//! pipeline itself only ever deals with resolved tier indices.

use super::{
    MeetConstraint, MeetTierInput,
    anchors::explicit_table_or_culet,
    blocks::{Block, classify_blocks},
};

/// Outcome of resolving one stated `"Meet ..."` token against a design's tiers.
///
/// Produced by [`MeetNameResolver::resolve_token`]; see that type's doc comment for
/// the rule set and the corpus measurements each rule rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenResolution {
    /// The token names one or more real tiers. A simple token (`"P1"`) names one; a
    /// compound vertex spec (`"1-2-G1"`) names each of its components' tiers.
    Tiers(Vec<usize>),
    /// A compound token in which only some components named real tiers -- the
    /// resolved subset is still returned (a partial constraint is still a
    /// constraint), but the token as a whole did not fully resolve.
    Partial(Vec<usize>),
    /// A recognized non-facet word: a meet-*point* reference (`"PCP"`, `"TCP"`,
    /// `"centerpoint"`, `"apex"`, `"keel"`) or connective prose (`"at"`,
    /// `"corner"`, `"index"`, `"level"`). Names no tier, but its presence should
    /// not count against the rest of the instruction resolving.
    Ignorable,
    /// Not a recognized word and no tier bears the name under any matching rule.
    Unresolved,
}

/// Everything [`MeetNameResolver::resolve_names`] extracts from one tier's stated
/// meet-name list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNames {
    /// Every distinct tier index the names resolved to, ascending.
    pub refs: Vec<usize>,
    /// True iff every token either resolved to a tier or was [ignorable]
    /// (`TokenResolution::Ignorable`), *and* at least one token actually resolved
    /// to a tier -- the definition of the `MeetNamed-resolved` reporting bucket.
    ///
    /// [ignorable]: TokenResolution::Ignorable
    pub fully: bool,
}

/// Non-facet words that appear inside real hand-typed `"Meet ..."` prose. Two
/// kinds: meet-*point* references that name a point rather than a facet
/// (`"PCP"` = pavilion culet point, `"TCP"` = table center point, `"apex"`,
/// `"keel"`), and connective prose (`"Meet at corner of 1-2-G1"`, `"Meet 1 and
/// 2 at index 96"`). Matched case-insensitively, only after exact and
/// case-insensitive *name* matching has already failed -- none of these words
/// is ever a real facet name in the corpus, so the name-match-first order
/// keeps even a future collision safe.
const IGNORABLE_MEET_WORDS: &[&str] = &[
    "all",
    "and",
    "apex",
    "at",
    "between",
    "center",
    "centerpoint",
    "corner",
    "corners",
    "cp",
    "cut",
    "edge",
    "facet",
    "facets",
    "form",
    "frosted",
    "gmp",
    "index",
    "keel",
    "leave",
    "level",
    "line",
    "lines",
    "meet",
    "mp",
    "note",
    "of",
    "on",
    "or",
    "pcp",
    "point",
    "points",
    "preform",
    "see",
    "tcp",
    "the",
    "tip",
    "to",
    "top",
    "with",
];

/// Resolves the hand-typed facet-name tokens of `"Meet <names>"` instructions
/// against a design's tiers.
///
/// Real `.asc` meet text is free prose, and a majority of its name tokens do not
/// literally match any stored tier name. The failing tokens are dominated by a
/// few systematic forms, each of which gets a rule here (applied in order, most
/// literal first -- an exact match always wins, so none of the fallbacks can ever
/// shadow a real name):
///
/// 1. **Exact name match** (first tier in file order), then
///    **ASCII-case-insensitive** match (`"b"` vs a tier named `"B"`).
/// 2. **Girdle references**: the girdle tier routinely carries no name at all,
///    while the text calls it `"girdle"`/`"Girdle"`, bare `"g"`/`"G"`, or
///    `"G1"`/`"g2"`. Falls back to the design's girdle tier -- unless a tier
///    really bears that name (rule 1 catches those first).
/// 3. **Culet / table references**: resolved to the design's explicit flat culet
///    or table tier when it has one. A `"culet"` reference on a pointed pavilion
///    names the pavilion's closing *point*, not a facet -- ignorable, same as
///    `"PCP"`.
/// 4. **Ignorable words** ([`IGNORABLE_MEET_WORDS`]): meet-point references and
///    connective prose.
/// 5. **Side-prefix stripping**: `"P1"` stated where the pavilion tiers are named
///    bare `"1"`, `"2"`, ... (`P`/`C` prefix + rest matches a tier of that block).
/// 6. **Plural stripping**: `"Stars"` stated for a tier named `"Star"`.
/// 7. **Compound vertex specs**: `"1-2-G1"` names the meet vertex of several
///    facets joined by hyphens; each component is resolved by rules 1-6, only
///    after the whole token fails rule 1 (a handful of real names contain
///    hyphens, e.g. `"c4-m"`).
///
/// Deterministic: every lookup is a positional scan over the tier list in file
/// order -- no hashing anywhere.
pub struct MeetNameResolver<'a> {
    tiers: &'a [MeetTierInput],
    blocks: Vec<Block>,
    /// The design's girdle tier: first tier at >= 85 degrees, else the first
    /// scale-reference tier (same convention `solve_meet_points` has always used).
    girdle_tier: Option<usize>,
    /// Explicit flat culet tier ([`explicit_table_or_culet`]), if the design has
    /// one (most pavilions are pointed and have none).
    culet_tier: Option<usize>,
    /// Explicit flat table tier, if the design has one.
    table_tier: Option<usize>,
}

impl<'a> MeetNameResolver<'a> {
    /// Builds the resolver for one design's tier list (schedule file order).
    #[must_use]
    pub fn new(tiers: &'a [MeetTierInput]) -> Self {
        let blocks = classify_blocks(tiers);
        let girdle_tier = tiers
            .iter()
            .position(|t| t.angle_deg.abs() >= 85.0)
            .or_else(|| {
                tiers
                    .iter()
                    .position(|t| matches!(t.constraint, MeetConstraint::ScaleReference(_)))
            });
        let culet_tier = explicit_table_or_culet(tiers, &blocks, Block::Pavilion);
        let table_tier = explicit_table_or_culet(tiers, &blocks, Block::Crown);
        Self {
            tiers,
            blocks,
            girdle_tier,
            culet_tier,
            table_tier,
        }
    }

    /// First tier (file order) bearing `token` as a name, exact then
    /// ASCII-case-insensitive.
    fn name_match(&self, token: &str) -> Option<usize> {
        self.tiers
            .iter()
            .position(|t| t.names.iter().any(|n| n == token))
            .or_else(|| {
                self.tiers
                    .iter()
                    .position(|t| t.names.iter().any(|n| n.eq_ignore_ascii_case(token)))
            })
    }

    /// Resolves one non-compound token (no hyphen splitting). See the type-level
    /// doc comment for the rule order.
    fn resolve_simple(&self, token: &str) -> TokenResolution {
        if let Some(i) = self.name_match(token) {
            return TokenResolution::Tiers(vec![i]);
        }
        let lower = token.to_ascii_lowercase();

        // Girdle references: "girdle", bare "g"/"G", "G1"/"g2"-style tokens.
        let girdleish = lower.contains("girdle")
            || (lower.as_bytes().first() == Some(&b'g')
                && lower.as_bytes()[1..].iter().all(u8::is_ascii_digit));
        if girdleish && let Some(g) = self.girdle_tier {
            return TokenResolution::Tiers(vec![g]);
        }
        if lower.contains("culet") {
            // A flat culet tier when the design has one; otherwise the token names
            // the pavilion's closing point -- a point reference, like "PCP".
            return self.culet_tier.map_or(TokenResolution::Ignorable, |c| {
                TokenResolution::Tiers(vec![c])
            });
        }
        if lower == "table"
            && let Some(t) = self.table_tier
        {
            return TokenResolution::Tiers(vec![t]);
        }
        if IGNORABLE_MEET_WORDS.contains(&lower.as_str()) {
            return TokenResolution::Ignorable;
        }
        // Side-prefix stripping: "P1" -> the pavilion tier named "1" (likewise "C").
        let mut chars = token.chars();
        if let Some(first) = chars.next() {
            let want = match first {
                'P' | 'p' => Some(Block::Pavilion),
                'C' | 'c' => Some(Block::Crown),
                _ => None,
            };
            let rest = chars.as_str();
            if let Some(want) = want
                && !rest.is_empty()
                && let Some(i) = self.name_match(rest)
                && self.blocks[i] == want
            {
                return TokenResolution::Tiers(vec![i]);
            }
        }
        // Plural stripping: "Stars" -> a tier named "Star".
        if token.len() > 2
            && let Some(stripped) = token.strip_suffix(['s', 'S'])
            && let Some(i) = self.name_match(stripped)
        {
            return TokenResolution::Tiers(vec![i]);
        }
        TokenResolution::Unresolved
    }

    /// Resolves one token, including compound `"1-2-G1"`-style vertex specs (split
    /// on `-` only after the whole token fails to resolve as a plain name).
    #[must_use]
    pub fn resolve_token(&self, token: &str) -> TokenResolution {
        match self.resolve_simple(token) {
            TokenResolution::Unresolved => {}
            resolved => return resolved,
        }
        if token.contains('-') {
            let parts: Vec<&str> = token.split('-').filter(|p| !p.is_empty()).collect();
            if parts.len() >= 2 {
                let mut refs = Vec::new();
                let mut all_resolved = true;
                for part in parts {
                    match self.resolve_simple(part) {
                        TokenResolution::Tiers(v) | TokenResolution::Partial(v) => refs.extend(v),
                        TokenResolution::Ignorable => {}
                        TokenResolution::Unresolved => all_resolved = false,
                    }
                }
                if !refs.is_empty() {
                    return if all_resolved {
                        TokenResolution::Tiers(refs)
                    } else {
                        TokenResolution::Partial(refs)
                    };
                }
            }
        }
        TokenResolution::Unresolved
    }

    /// Resolves a whole stated name list (one tier's `"Meet <names>"` tokens) into
    /// the distinct referenced tier indices, plus whether the list fully resolved
    /// (see [`ResolvedNames::fully`]).
    #[must_use]
    pub fn resolve_names(&self, names: &[String]) -> ResolvedNames {
        let mut refs: Vec<usize> = Vec::new();
        let mut fully = true;
        let mut any = false;
        for name in names {
            match self.resolve_token(name) {
                TokenResolution::Tiers(v) => {
                    refs.extend(v);
                    any = true;
                }
                TokenResolution::Partial(v) => {
                    refs.extend(v);
                    any = true;
                    fully = false;
                }
                TokenResolution::Ignorable => {}
                TokenResolution::Unresolved => fully = false,
            }
        }
        refs.sort_unstable();
        refs.dedup();
        ResolvedNames {
            refs,
            fully: fully && any,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: a `MeetExisting` tier with the given angle/indices/names.
    fn tier(angle_deg: f64, indices: &[f64], names: &[&str]) -> MeetTierInput {
        MeetTierInput {
            angle_deg,
            indices: indices.to_vec(),
            constraint: MeetConstraint::MeetExisting,
            names: names.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// A small letters-and-girdle design shaped like the corpus's common case:
    /// pavilion tiers named "1"/"2", an unnamed girdle wall, crown tiers "A"/"B",
    /// and a flat table tier named "T".
    fn resolver_fixture() -> Vec<MeetTierInput> {
        vec![
            tier(-42.0, &[8.0, 24.0], &["1"]),
            tier(-90.0, &[8.0, 24.0], &[]),
            tier(-50.0, &[16.0], &["2"]),
            tier(41.0, &[8.0, 24.0], &["A"]),
            tier(30.0, &[16.0], &["B"]),
            tier(0.0, &[], &["T"]),
        ]
    }

    #[test]
    fn resolver_exact_match_always_wins_over_fallbacks() {
        // A design whose girdle tier is *named* "g": the bare-"g" girdle fallback
        // must never shadow the real name (here they coincide, but the exact match
        // must also win for a tier named "g" that is NOT the girdle).
        let mut tiers = resolver_fixture();
        tiers[4].names = vec!["g".to_string()]; // crown tier named "g"
        let r = MeetNameResolver::new(&tiers);
        assert_eq!(r.resolve_token("g"), TokenResolution::Tiers(vec![4]));
    }

    #[test]
    fn resolver_case_insensitive_and_girdle_fallbacks() {
        let tiers = resolver_fixture();
        let r = MeetNameResolver::new(&tiers);
        // Case-insensitive: "a" -> the tier named "A".
        assert_eq!(r.resolve_token("a"), TokenResolution::Tiers(vec![3]));
        // Unnamed girdle wall, referenced three common ways.
        assert_eq!(r.resolve_token("girdle"), TokenResolution::Tiers(vec![1]));
        assert_eq!(r.resolve_token("g"), TokenResolution::Tiers(vec![1]));
        assert_eq!(r.resolve_token("G1"), TokenResolution::Tiers(vec![1]));
    }

    #[test]
    fn resolver_splits_compound_vertex_specs() {
        let tiers = resolver_fixture();
        let r = MeetNameResolver::new(&tiers);
        // "1-2-G1": pavilion tiers "1" and "2" plus the unnamed girdle.
        assert_eq!(
            r.resolve_token("1-2-G1"),
            TokenResolution::Tiers(vec![0, 2, 1])
        );
        // A component that resolves nowhere makes the token partial, keeping the
        // resolved subset.
        assert_eq!(r.resolve_token("1-Z9"), TokenResolution::Partial(vec![0]));
    }

    #[test]
    fn resolver_ignores_point_references_and_prose() {
        let tiers = resolver_fixture();
        let r = MeetNameResolver::new(&tiers);
        for word in ["PCP", "TCP", "at", "corner", "index", "Level"] {
            assert_eq!(
                r.resolve_token(word),
                TokenResolution::Ignorable,
                "{word:?} should be ignorable"
            );
        }
        // "culet" on a pointed pavilion (no flat culet tier) names the closing
        // point, not a facet.
        assert_eq!(r.resolve_token("culet"), TokenResolution::Ignorable);
        // "Meet at corner of 1-2-G1" resolves fully: prose ignorable, spec split.
        let names: Vec<String> = ["at", "corner", "of", "1-2-G1"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let resolved = r.resolve_names(&names);
        assert!(resolved.fully);
        assert_eq!(resolved.refs, vec![0, 1, 2]);
        // Prose alone resolves nothing, and must not count as "fully resolved".
        let prose: Vec<String> = ["at", "corner"].iter().map(|s| (*s).to_string()).collect();
        let resolved = r.resolve_names(&prose);
        assert!(!resolved.fully);
        assert_eq!(resolved.refs, Vec::<usize>::new());
    }

    #[test]
    fn resolver_strips_side_prefix_and_plural() {
        let mut tiers = resolver_fixture();
        tiers[4].names = vec!["Star".to_string()];
        let r = MeetNameResolver::new(&tiers);
        // "P1" stated where the pavilion tiers are named bare "1".
        assert_eq!(r.resolve_token("P1"), TokenResolution::Tiers(vec![0]));
        // The prefix must respect the block: "C1" must NOT match pavilion "1"
        // (no crown tier is named "1" here, and there is nothing else to find).
        assert_eq!(r.resolve_token("C1"), TokenResolution::Unresolved);
        // "Stars" -> the tier named "Star".
        assert_eq!(r.resolve_token("Stars"), TokenResolution::Tiers(vec![4]));
    }

    #[test]
    fn resolver_finds_flat_table_when_referenced_by_word() {
        let tiers = resolver_fixture();
        let r = MeetNameResolver::new(&tiers);
        // The flat crown tier is named "T", but prose says "table".
        assert_eq!(r.resolve_token("table"), TokenResolution::Tiers(vec![5]));
    }
}
