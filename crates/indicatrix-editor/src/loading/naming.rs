//! Auto-names for brand-new tiers (the desktop's Save Tier, the mouse-driven slice
//! tool) and the names generated tiers (Duplicate, Mirror, Generate steps) get.
//!
//! A `MeetNamed` constraint binds to the FIRST tier bearing a name, so two tiers that
//! share a name (even one token of a `/`-joined multi-name) silently redirect every
//! reference to the later one. Every generator here therefore checks candidates with
//! [`name_collides`].

use indicatrix_formats::asc::asc_safe_tier_name;

/// The girdle band of `meet_solver::blocks::classify_blocks`: a tier is a girdle when
/// `|cos(theta)|` is at most this (about 5.7e-5 degrees either side of 90).
const GIRDLE_COS_TOLERANCE: f64 = 1e-6;

/// The suffix a mirrored tier's name gets when the caller supplies none (or only
/// whitespace).
const DEFAULT_MIRROR_SUFFIX: &str = "'";

/// The `/`-separated name tokens of `name` -- the tokens `ConstraintTier::names` yields --
/// with empty ones dropped.
fn name_tokens(name: &str) -> impl Iterator<Item = &str> {
    name.split('/').filter(|token| !token.is_empty())
}

/// `text` as one whitespace-free name token: whitespace runs become `_` (the form a
/// plain `.asc` export writes) and so does the `/` that would split it into several
/// names.
fn name_token(text: &str) -> String {
    asc_safe_tier_name(text).replace('/', "_")
}

/// Whether any `/`-separated token of `candidate` collides with a token of `existing_names`.
///
/// Tokens compare equal ignoring ASCII case. This is the test `MeetNameResolver::name_match`
/// (`indicatrix::geometry::meet_solver::names`) effectively applies, since it binds a
/// reference to the first tier bearing ANY matching token.
#[must_use]
pub fn name_collides(candidate: &str, existing_names: &[String]) -> bool {
    name_tokens(candidate).any(|token| {
        existing_names
            .iter()
            .flat_map(|existing| name_tokens(existing))
            .any(|other| other.eq_ignore_ascii_case(token))
    })
}

/// The name for a copy of a tier mirrored to the other block (angle negated).
///
/// Each of the source's `/`-joined names gets `suffix` appended (`"P1"` becomes `"P1'"`;
/// a blank suffix means `'`), whitespace-free so it survives a plain `.asc` export, and
/// the candidate counts up (`"P1'"`, `"P1'2"`, `"P1'3"`, ...) until [`name_collides`]
/// finds no clash with `existing_names`. An unnamed source falls back to
/// [`next_free_block_name`] for the mirrored tier's own angle
/// (`mirrored_angle_deg`), since a bare suffix is no name.
#[must_use]
pub fn unique_mirror_name(
    source_name: &str,
    suffix: &str,
    mirrored_angle_deg: f64,
    existing_names: &[String],
) -> String {
    let tokens: Vec<String> = name_tokens(source_name)
        .map(name_token)
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return next_free_block_name(mirrored_angle_deg, existing_names);
    }
    let suffix = name_token(suffix);
    let suffix = if suffix.is_empty() {
        DEFAULT_MIRROR_SUFFIX.to_string()
    } else {
        suffix
    };
    let mut n: u32 = 1;
    loop {
        let counter = if n == 1 { String::new() } else { n.to_string() };
        let candidate = tokens
            .iter()
            .map(|token| format!("{token}{suffix}{counter}"))
            .collect::<Vec<_>>()
            .join("/");
        if !name_collides(&candidate, existing_names) {
            return candidate;
        }
        n += 1;
    }
}

/// The names for a generated ladder of tiers at `angles_deg`, one per angle, appended to
/// a design whose tiers are named `existing_names`.
///
/// With a `prefix`, the names are `"<prefix><n>"` counting up from 1 and skipping every
/// number whose name [`name_collides`] with an existing tier (or an earlier name of this
/// ladder), so a second ladder with the same prefix continues the first instead of
/// repeating its names. A blank prefix names each tier [`next_free_block_name`] for its
/// own angle. The prefix is made whitespace-free and `/`-free first.
#[must_use]
pub fn series_names(prefix: &str, angles_deg: &[f64], existing_names: &[String]) -> Vec<String> {
    let prefix = name_token(prefix);
    let mut taken: Vec<String> = existing_names.to_vec();
    let mut next: u32 = 1;
    let mut names = Vec::with_capacity(angles_deg.len());
    for &angle_deg in angles_deg {
        let name = if prefix.is_empty() {
            next_free_block_name(angle_deg, &taken)
        } else {
            loop {
                let candidate = format!("{prefix}{next}");
                next += 1;
                if !name_collides(&candidate, &taken) {
                    break candidate;
                }
            }
        };
        taken.push(name.clone());
        names.push(name);
    }
    names
}

/// Whether `angle_deg` (either sign) is a girdle angle, by the same
/// `|cos(theta)| <= 1e-6` test `classify_blocks` uses. Not a call into that function:
/// it needs `MeetTierInput`s and the previous-tier side rule.
fn is_girdle_angle(angle_deg: f64) -> bool {
    angle_deg.abs().to_radians().cos().abs() <= GIRDLE_COS_TOLERANCE
}

/// The auto-name for a brand-new tier saved with a blank Name field.
///
/// An empty name can never become a `MeetNamed` target (`ConstraintTier::names()`
/// returns nothing for it), so leaving a fresh `AddTier` unnamed silently makes it
/// un-meetable until the cutter notices.
///
/// The block letter is `G` for a girdle tier (`|angle|` at 90 degrees, either sign),
/// `P` for a pavilion tier (negative angle) and `C` for a crown tier (non-negative
/// angle). The girdle test is the one `meet_solver::blocks::classify_blocks` applies
/// (`|cos(theta)| <= 1e-6`), so a tier this names `G1` is also classified as a girdle
/// by the table. The crown/pavilion split is simplified: this only has to pick a
/// reasonable DEFAULT name the cutter can always retype, so it does not reproduce that
/// module's unsigned-zero "inherits the previous tier's side" rule just to name a
/// single new tier. Collision against `existing_names` ([`name_collides`]: ASCII
/// case-insensitive, per `/`-joined name) counts up past it (`G1`, `G2`, ...; `C1`,
/// `C2`, ...).
#[must_use]
pub fn next_free_block_name(angle_deg: f64, existing_names: &[String]) -> String {
    let letter = if is_girdle_angle(angle_deg) {
        'G'
    } else if angle_deg.is_sign_negative() {
        'P'
    } else {
        'C'
    };
    let mut n: u32 = 1;
    loop {
        let candidate = format!("{letter}{n}");
        if !name_collides(&candidate, existing_names) {
            return candidate;
        }
        n += 1;
    }
}

/// The tier list's Duplicate name: never collides with `existing_names`.
///
/// Counts up a `" (N)"` suffix (`"P1 (2)"`,
/// `"P1 (3)"`, ...) rather than appending an apostrophe: the apostrophe scheme produced
/// an unreadable "P1''''" pile on a second or third duplicate of the same tier and,
/// worse, silently created a duplicate name that `MeetNameResolver::name_match`
/// (`indicatrix::geometry::meet_solver::names`) resolves by binding to whichever tier
/// holds it FIRST -- so a duplicate's stale copy of a popular name could silently steal
/// every future `MeetNamed` reference meant for the original.
///
/// [`split_duplicate_suffix`] first removes a trailing `" (N)"` a PREVIOUS call to this
/// same function already appended, so duplicating "P1 (2)" produces "P1 (3)" rather than
/// nesting into "P1 (2) (2)". An empty source name (an unnamed tier) falls back to the
/// base "Tier" rather than producing a bare "(2)" -- giving the duplicate a real name is
/// also what lets it become a `MeetNamed` target, since `ConstraintTier::names` never
/// resolves a name from an empty string.
///
/// Moved here from the desktop's `tier_actions/tier_generation.rs`, unchanged, so the web
/// app's Duplicate names copies identically.
#[must_use]
pub fn unique_duplicate_name(source_name: &str, existing_names: &[String]) -> String {
    let (base, source_number) = split_duplicate_suffix(source_name.trim());
    let base = if base.is_empty() { "Tier" } else { base };
    // One past the source's OWN number when it already carries one, so this
    // holds to its documented contract on its own terms rather than relying on
    // the caller's list happening to contain the source tier: duplicating
    // "P1 (2)" gives "P1 (3)" even against an empty list. `checked_add` falls
    // back to 2 for the (unreachable by duplicating, but typeable by hand)
    // number that cannot be counted past.
    let mut n: u32 = source_number
        .and_then(|number| number.checked_add(1))
        .unwrap_or(2);
    loop {
        let candidate = format!("{base} ({n})");
        if !existing_names
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
        n += 1;
    }
}

/// Splits `name` into its base portion and a trailing `" (N)"` counter.
///
/// The counter is a whole, non-negative number in parentheses, preceded by exactly one
/// space -- see [`unique_duplicate_name`]'s own doc comment for why. A name with no such suffix comes
/// back whole with `None`, and so does one whose digits do not fit a `u32`, for the same
/// reason `"P1 (a)"` does: a suffix this cannot read is part of the name the cutter
/// typed, not a counter to continue.
#[must_use]
pub fn split_duplicate_suffix(name: &str) -> (&str, Option<u32>) {
    let Some((base, rest)) = name.rsplit_once(" (") else {
        return (name, None);
    };
    let Some(digits) = rest.strip_suffix(')') else {
        return (name, None);
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return (name, None);
    }
    digits
        .parse()
        .map_or((name, None), |number| (base, Some(number)))
}

#[cfg(test)]
mod tests {
    use super::{
        name_collides, next_free_block_name, series_names, split_duplicate_suffix,
        unique_duplicate_name, unique_mirror_name,
    };

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn name_collides_checks_every_slash_token_ignoring_case() {
        let existing = names(&["P1/P2", "Star"]);
        assert!(name_collides("p2", &existing));
        assert!(name_collides("X/STAR", &existing));
        assert!(!name_collides("P3", &existing));
        assert!(!name_collides("", &existing));
        assert!(!name_collides("P1", &[]));
    }

    #[test]
    fn next_free_block_name_skips_a_name_inside_a_multi_name() {
        assert_eq!(next_free_block_name(45.0, &names(&["C1/C2"])), "C3");
    }

    #[test]
    fn unique_mirror_name_suffixes_each_name_and_stays_asc_safe() {
        assert_eq!(unique_mirror_name("P1", "'", 41.0, &[]), "P1'");
        assert_eq!(unique_mirror_name("P1", "", 41.0, &[]), "P1'");
        assert_eq!(unique_mirror_name("P1", "a b", 41.0, &[]), "P1a_b");
        assert_eq!(
            unique_mirror_name("Crown Main", "'", -34.5, &[]),
            "Crown_Main'"
        );
        assert_eq!(unique_mirror_name("P2/P3", "'", 41.0, &[]), "P2'/P3'");
    }

    #[test]
    fn unique_mirror_name_counts_up_past_a_clash() {
        assert_eq!(
            unique_mirror_name("P1", "'", 41.0, &names(&["P1'"])),
            "P1'2"
        );
        assert_eq!(
            unique_mirror_name("P1", "'", 41.0, &names(&["P1'", "p1'2"])),
            "P1'3"
        );
        assert_eq!(
            unique_mirror_name("P1", "'", 41.0, &names(&["P1'/Z"])),
            "P1'2",
            "a clash with one name of a multi-name tier still counts"
        );
    }

    #[test]
    fn an_unnamed_source_mirrors_to_a_block_name() {
        assert_eq!(unique_mirror_name("", "'", 41.0, &[]), "C1");
        assert_eq!(unique_mirror_name("", "'", -41.0, &names(&["P1"])), "P2");
    }

    #[test]
    fn series_names_count_on_past_existing_numbers() {
        assert_eq!(series_names("Step", &[30.0, 32.0], &[]), ["Step1", "Step2"]);
        let existing = names(&["step1", "Step2/X"]);
        assert_eq!(
            series_names("Step", &[30.0, 32.0], &existing),
            ["Step3", "Step4"]
        );
    }

    #[test]
    fn series_names_with_a_blank_prefix_use_block_names() {
        assert_eq!(
            series_names("  ", &[30.0, 32.0, -20.0], &names(&["C1"])),
            ["C2", "C3", "P1"]
        );
    }

    #[test]
    fn a_series_prefix_is_made_whitespace_and_slash_free() {
        assert_eq!(series_names("Up per/x", &[30.0], &[]), ["Up_per_x1"]);
    }

    #[test]
    fn unique_duplicate_name_starts_at_2_when_nothing_collides() {
        assert_eq!(unique_duplicate_name("P1", &[]), "P1 (2)");
    }

    #[test]
    fn unique_duplicate_name_skips_names_already_in_use() {
        let existing = vec!["P1".to_string(), "P1 (2)".to_string(), "P1 (3)".to_string()];
        assert_eq!(unique_duplicate_name("P1", &existing), "P1 (4)");
    }

    #[test]
    fn unique_duplicate_name_collision_check_is_case_insensitive() {
        let existing = vec!["p1 (2)".to_string()];
        assert_eq!(unique_duplicate_name("P1", &existing), "P1 (3)");
    }

    #[test]
    fn unique_duplicate_name_counts_up_instead_of_nesting() {
        // The empty `existing_names` is the point: this has to hold on the
        // function's own terms, not only because the real caller passes a list
        // that contains the source tier (which would make "P1 (2)" collide).
        assert_eq!(unique_duplicate_name("P1 (2)", &[]), "P1 (3)");
    }

    #[test]
    fn unique_duplicate_name_continues_from_a_high_source_number() {
        assert_eq!(unique_duplicate_name("P1 (9)", &[]), "P1 (10)");
    }

    #[test]
    fn unique_duplicate_name_treats_an_unreadable_suffix_as_part_of_the_name() {
        // Not reachable by duplicating, but a cutter can type any name they
        // like -- this must still produce something, never loop or panic.
        let huge = format!("P1 ({})", u128::from(u32::MAX) + 1);
        assert_eq!(unique_duplicate_name(&huge, &[]), format!("{huge} (2)"));
    }

    #[test]
    fn unique_duplicate_name_falls_back_to_tier_for_an_unnamed_source() {
        assert_eq!(unique_duplicate_name("", &[]), "Tier (2)");
    }

    #[test]
    fn split_duplicate_suffix_reads_a_trailing_parenthesized_number() {
        assert_eq!(split_duplicate_suffix("P1 (2)"), ("P1", Some(2)));
        assert_eq!(split_duplicate_suffix("P1 (12)"), ("P1", Some(12)));
    }

    #[test]
    fn split_duplicate_suffix_leaves_unrelated_text_alone() {
        assert_eq!(split_duplicate_suffix("P1"), ("P1", None));
        assert_eq!(split_duplicate_suffix("P1 (a)"), ("P1 (a)", None));
        assert_eq!(split_duplicate_suffix("P1 (2"), ("P1 (2", None));
        assert_eq!(split_duplicate_suffix("P1/P2 (2)"), ("P1/P2", Some(2)));
    }

    #[test]
    fn next_free_block_name_starts_at_1_for_a_crown_angle() {
        assert_eq!(next_free_block_name(45.0, &[]), "C1");
    }

    #[test]
    fn next_free_block_name_uses_p_for_a_negative_angle() {
        assert_eq!(next_free_block_name(-40.0, &[]), "P1");
    }

    #[test]
    fn next_free_block_name_treats_non_negative_zero_as_crown() {
        assert_eq!(next_free_block_name(0.0, &[]), "C1");
    }

    #[test]
    fn next_free_block_name_uses_g_for_a_girdle_angle_of_either_sign() {
        assert_eq!(next_free_block_name(90.0, &[]), "G1");
        assert_eq!(next_free_block_name(-90.0, &[]), "G1");
    }

    #[test]
    fn next_free_block_name_girdle_collision_check_is_case_insensitive() {
        let existing = vec!["g1".to_string()];
        assert_eq!(next_free_block_name(90.0, &existing), "G2");
    }

    #[test]
    fn next_free_block_name_keeps_c_just_below_the_girdle() {
        assert_eq!(next_free_block_name(89.99, &[]), "C1");
        assert_eq!(next_free_block_name(-89.99, &[]), "P1");
    }

    #[test]
    fn next_free_block_name_skips_names_already_in_use() {
        let existing = vec!["C1".to_string(), "C2".to_string()];
        assert_eq!(next_free_block_name(45.0, &existing), "C3");
    }

    #[test]
    fn next_free_block_name_collision_check_is_case_insensitive() {
        let existing = vec!["c1".to_string()];
        assert_eq!(next_free_block_name(45.0, &existing), "C2");
    }
}
