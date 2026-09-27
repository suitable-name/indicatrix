//! Tests for [`super::super::MeetInstruction`] parsing (via the crate-private
//! `parse_meet_instruction`) and [`super::super::AscTier::names`].

use super::super::{MeetInstruction, meet_instruction::parse_meet_instruction, parse_asc};

#[test]
fn parses_meet_instruction_with_comma_separated_names() {
    let schedule = parse_asc(
        "GemCad 5.0\ng 96 0.0\ny 1 n\nI 1.72\nH Test\n\
         a -90.000000 0.58736554 69 n G2 27 G Meet P1, P2, G1\n",
    )
    .expect("must parse");
    let instr = schedule.tiers[0]
        .meet_instruction()
        .expect("must have a G note");
    assert_eq!(
        instr,
        MeetInstruction::Meet(vec!["P1".to_string(), "P2".to_string(), "G1".to_string()])
    );
}

#[test]
fn parses_meet_instruction_with_whitespace_separated_names() {
    let instr = parse_meet_instruction("Meet P2 P3 P5").expect("must parse");
    assert_eq!(
        instr,
        MeetInstruction::Meet(vec!["P2".to_string(), "P3".to_string(), "P5".to_string()])
    );
}

/// Real corpus prose drops connector words that were never facet names -- see
/// `extract_meet_names`'s doc comment.
#[test]
fn parses_meet_instruction_drops_connector_words() {
    assert_eq!(
        parse_meet_instruction("Meet 2 and the culet"),
        Some(MeetInstruction::Meet(vec![
            "2".to_string(),
            "culet".to_string()
        ]))
    );
    assert_eq!(
        parse_meet_instruction("Meet P1 or P2"),
        Some(MeetInstruction::Meet(vec![
            "P1".to_string(),
            "P2".to_string()
        ]))
    );
    // Lowercase single-letter facet names are common in this corpus and must
    // never be dropped, even though "a"/"an" are also articles.
    assert_eq!(
        parse_meet_instruction("Meet a and b"),
        Some(MeetInstruction::Meet(vec![
            "a".to_string(),
            "b".to_string()
        ]))
    );
}

#[test]
fn parses_meet_instruction_with_four_named_facets() {
    let instr = parse_meet_instruction("Meet G1, G2, C1, C2").expect("must parse");
    assert_eq!(
        instr,
        MeetInstruction::Meet(vec![
            "G1".to_string(),
            "G2".to_string(),
            "C1".to_string(),
            "C2".to_string()
        ])
    );
}

#[test]
fn classifies_real_corpus_notes_text() {
    // Every one of these is verbatim (or near-verbatim) text seen in the real
    // `.asc` corpus fixtures above.
    assert_eq!(
        parse_meet_instruction("Cut to mast depth X."),
        Some(MeetInstruction::Other("Cut to mast depth X.".to_string()))
    );
    assert_eq!(
        parse_meet_instruction("Set stone size."),
        Some(MeetInstruction::ScaleReference)
    );
    assert_eq!(
        parse_meet_instruction("Set girdle width."),
        Some(MeetInstruction::ScaleReference)
    );
    assert_eq!(
        parse_meet_instruction("Establish girdle thickness"),
        Some(MeetInstruction::ScaleReference)
    );
    assert_eq!(
        parse_meet_instruction("TCP"),
        Some(MeetInstruction::CutToCenterpoint)
    );
    assert_eq!(
        parse_meet_instruction("Cut to centerpoint."),
        Some(MeetInstruction::CutToCenterpoint)
    );
    assert_eq!(
        parse_meet_instruction("Level girdle."),
        Some(MeetInstruction::LevelGirdle)
    );
    assert_eq!(
        parse_meet_instruction("GMP"),
        Some(MeetInstruction::GirdleMeetPoint)
    );
    assert_eq!(
        parse_meet_instruction("Meet girdle"),
        Some(MeetInstruction::Meet(vec!["girdle".to_string()]))
    );
    assert_eq!(
        parse_meet_instruction("Or continuous girdle"),
        Some(MeetInstruction::Other("Or continuous girdle".to_string()))
    );
    assert_eq!(parse_meet_instruction(""), None);
}

#[test]
fn tier_names_splits_joined_names() {
    let schedule = parse_asc(
        "GemCad 5.0\ng 96 0.0\ny 1 n\nI 1.72\nH Test\n\
         a -40.00000 0.49897 92 n c 84 76 94 n d 90 86 G Meet girdle\n",
    )
    .expect("must parse");
    assert_eq!(schedule.tiers[0].names(), vec!["c", "d"]);
}
