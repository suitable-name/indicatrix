//! Frosting in `.gcs`: what the reader states as frosted ([`gcs_frosting`]) and what the writer
//! ([`to_gcs_string_with_frosting`]) keeps. `.asc` has no notation for it, so the schedule is
//! the same either way and the frosting travels beside it.

use super::octagon_schedule;
use crate::gcs::{
    GcsTierFrosting, gcs_frosting, gcs_to_asc_schedule, parse_gcs, to_gcs_string,
    to_gcs_string_with_frosting,
};

#[test]
fn a_file_without_frosting_states_none() {
    let text = to_gcs_string(&octagon_schedule()).expect("the octagon writes");
    let design = parse_gcs(&text).expect("the written file parses");
    assert_eq!(gcs_frosting(&design).len(), 0);
}

#[test]
fn an_empty_frosting_list_writes_exactly_the_unfrosted_text() {
    let schedule = octagon_schedule();
    assert_eq!(
        to_gcs_string_with_frosting(&schedule, &[]).expect("writes"),
        to_gcs_string(&schedule).expect("writes"),
    );
}

#[test]
fn tier_and_facet_frosting_survive_the_writer_and_the_reader() {
    let schedule = octagon_schedule();
    // Tier 0 whole, tier 2 only the facets in slots 1 and 5.
    let frosting = vec![
        (0, GcsTierFrosting::Whole),
        (2, GcsTierFrosting::Facets(vec![1, 5])),
    ];
    let text = to_gcs_string_with_frosting(&schedule, &frosting).expect("writes");
    assert!(
        text.contains("frosting=\"0.5\""),
        "the file states the frosting"
    );
    let design = parse_gcs(&text).expect("the written file parses");
    assert_eq!(gcs_frosting(&design), frosting);
    // The schedule has no frosting notation: it is what the unfrosted file gives.
    let plain = parse_gcs(&to_gcs_string(&schedule).expect("writes")).expect("parses");
    assert_eq!(gcs_to_asc_schedule(&design), gcs_to_asc_schedule(&plain));
}

#[test]
fn a_frosting_position_past_the_schedule_names_nothing() {
    let schedule = octagon_schedule();
    let frosting = vec![
        (99, GcsTierFrosting::Whole),
        (2, GcsTierFrosting::Facets(vec![40])),
    ];
    let text = to_gcs_string_with_frosting(&schedule, &frosting).expect("writes");
    let design = parse_gcs(&text).expect("parses");
    assert_eq!(gcs_frosting(&design).len(), 0);
}

#[test]
fn a_tier_with_every_facet_frosted_reads_as_whole() {
    let text = r#"<GemCutStudio version="1000"><index gear="96"/>
        <tier angle="30" depth="0.5">
          <facet nx="0" ny="-0.5" nz="0.8" index_angle="0" frosting="0.5"/>
          <facet nx="0" ny="-0.5" nz="0.8" index_angle="90" frosting="0.5"/>
        </tier>
        <tier angle="40" depth="0.6">
          <facet nx="0" ny="-0.6" nz="0.7" index_angle="0" frosting="0.5"/>
          <facet nx="0" ny="-0.6" nz="0.7" index_angle="90"/>
        </tier>
        <tier angle="50" depth="0.7" frosting="0.5">
          <facet nx="0" ny="-0.7" nz="0.6" index_angle="0"/>
        </tier></GemCutStudio>"#;
    let design = parse_gcs(text).expect("parses");
    assert_eq!(
        gcs_frosting(&design),
        vec![
            (0, GcsTierFrosting::Whole),
            (1, GcsTierFrosting::Facets(vec![0])),
            (2, GcsTierFrosting::Whole),
        ]
    );
}
