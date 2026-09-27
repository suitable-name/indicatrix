//! Fractional index positions, distinct names sharing one tier, and the
//! girdle-tier-literally-named-`"G"` corpus quirk.

use super::{
    super::parse_asc,
    fixtures::{ASC_GIRDLE_NAMED_G, ASC_GIRDLE_NAMED_G_NO_NOTES},
};

#[test]
fn parses_fractional_index_positions() {
    let content = "GemCad 4.41\n\
                    g 96 48.0\n\
                    y 1 y\n\
                    I 1.54\n\
                    H Triolette Replica\n\
                    a -90.00 1.02050 88.8 7.2\n";
    let schedule = parse_asc(content).expect("must parse fractional indices");
    assert_eq!(schedule.tiers[0].indices, vec![88.8, 7.2]);
}

#[test]
fn distinct_names_on_one_tier_are_merged_not_dropped() {
    // Real pattern: two different facet names sharing one angle/mast row, each
    // with its own index group. Geometrically this is still one tier (same
    // angle+depth), so both index groups must survive.
    let content = "GemCad 5.0\n\
                    g 96 0.0\n\
                    y 1 n\n\
                    I 1.72\n\
                    H Test\n\
                    a -40.00000 0.49897 92 n c 84 76 94 n d 90 86 G Meet girdle\n";
    let schedule = parse_asc(content).expect("must parse distinct names on one tier");
    assert_eq!(schedule.tiers.len(), 1);
    assert_eq!(schedule.tiers[0].name, "c/d");
    assert_eq!(
        schedule.tiers[0].indices,
        vec![92.0, 84.0, 76.0, 94.0, 90.0, 86.0]
    );
}

/// Real corpus tier line (girdle tier literally named "G"): `parse_tier`'s token
/// loop must honor a pending `expect_name` from the preceding "n" marker before
/// checking for the "G" notes marker, or the name token "G" is misread
/// as the start of the notes tail and every index after it is swallowed into
/// `notes` instead. 2,296 of 5,759 corpus files hit this because "G" is a very
/// common girdle-tier name.
#[test]
fn tier_named_g_is_not_swallowed_by_notes_marker() {
    let schedule = parse_asc(ASC_GIRDLE_NAMED_G).expect("must parse a tier named \"G\"");
    assert_eq!(schedule.tiers.len(), 1);
    let tier = &schedule.tiers[0];
    assert_eq!(tier.name, "G");
    assert_eq!(tier.indices, vec![36.0, 12.0, 84.0, 60.0]);
    assert_eq!(tier.notes, "Set stone size.");
}

/// Same "G"-named-tier corpus quirk, but with no "G <notes>" tail at all -- every
/// index after the name token must still parse as an index, not get swallowed as
/// (empty) notes text.
#[test]
fn tier_named_g_with_no_notes_keeps_all_indices() {
    let schedule = parse_asc(ASC_GIRDLE_NAMED_G_NO_NOTES)
        .expect("must parse a tier named \"G\" with no notes tail");
    assert_eq!(schedule.tiers.len(), 1);
    let tier = &schedule.tiers[0];
    assert_eq!(tier.name, "G");
    assert_eq!(tier.indices.len(), 12);
    assert_eq!(tier.notes, "");
}
