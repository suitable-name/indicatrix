//! Round-trip and wire-stability tests for the library-sync protocol: filter/record
//! types, [`super::LibraryRequest`], and [`super::LibraryResponse`].

use super::*;
use crate::messages::{read_message, write_message};

fn sample_summary() -> DesignSummary {
    DesignSummary {
        entry_id: 42,
        title: "Round Brilliant".to_string(),
        url: "https://example.test/diagram/42".to_string(),
        design_id: Some("RB-1".to_string()),
        shape: Some("Round".to_string()),
        index_gear: Some("96".to_string()),
        facets_count: Some("57".to_string()),
        designer_info: Some("Capps, Jerry".to_string()),
        lw_ratio: Some("1.00".to_string()),
        refractive_index: Some("2.417".to_string()),
        volume: Some("0.65".to_string()),
        competition_diagram: None,
        ignored: false,
        version: [7u8; 32],
    }
}

#[test]
fn library_request_variants_round_trip() {
    for req in [
        LibraryRequest::Search {
            query: "round".to_string(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            order: SortOrderWire::default(),
            tag_filter: None,
        },
        LibraryRequest::Search {
            query: "round".to_string(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            order: SortOrderWire::Title,
            tag_filter: Some("Favorites".to_string()),
        },
        LibraryRequest::FilterOptions,
        LibraryRequest::FetchDesign { entry_id: 42 },
        LibraryRequest::FetchAttachment { attachment_id: 9 },
        LibraryRequest::SearchPage {
            query: "round".to_string(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            cursor: None,
        },
        LibraryRequest::SearchPage {
            query: String::new(),
            shape_filter: "Round".to_string(),
            gear_filter: "96".to_string(),
            range: RangeFilterWire::default(),
            cursor: Some(1000),
        },
        LibraryRequest::FetchDesignSource { entry_id: 42 },
    ] {
        let mut buf = Vec::new();
        write_message(&mut buf, &req).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: LibraryRequest = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, req);
    }
}

#[test]
fn library_response_variants_round_trip() {
    let design = DesignRecord {
        entry_id: 42,
        title: "Round Brilliant".to_string(),
        url: "https://example.test/diagram/42".to_string(),
        design_id: Some("RB-1".to_string()),
        page_url: "https://example.test/diagram/42".to_string(),
        diagram_image_name: Some("rb.svg".to_string()),
        diagram_image_data: Some(vec![1, 2, 3]),
        competition_diagram: None,
        lw_ratio: Some("1.00".to_string()),
        refractive_index: Some("2.417".to_string()),
        index_gear: Some("96".to_string()),
        volume: Some("0.65".to_string()),
        facets_count: Some("57".to_string()),
        shape: Some("Round".to_string()),
        designer_info: Some("Capps, Jerry".to_string()),
        // Non-default values throughout so a dropped field fails the round-trip.
        preview_material: Some("Diamond".to_string()),
        hw_ratio: Some("1.00".to_string()),
        tw_ratio: Some("0.53".to_string()),
        uw_ratio: Some("0.16".to_string()),
        pw_ratio: Some("0.43".to_string()),
        cw_ratio: Some("0.14".to_string()),
        symmetry_order: Some("8".to_string()),
        mirror_symmetry: Some(true),
        designer: Some("Capps, Jerry".to_string()),
        angle_settings: vec![AngleSettingWire {
            order_index: 0,
            facet: "P1".to_string(),
            angle: "41.0".to_string(),
            index: "96".to_string(),
            notes: String::new(),
        }],
        attachments: vec![AttachedFileMeta {
            id: 1,
            name: "schedule.pdf".to_string(),
            url: "https://example.test/schedule.pdf".to_string(),
            size: 12_345,
        }],
        version: [1u8; 32],
    };

    for resp in [
        LibraryResponse::SearchResults {
            items: vec![sample_summary()],
            excluded_for_missing_curves: 0,
        },
        LibraryResponse::FilterOptions {
            shapes: vec!["Round".to_string()],
            gears: vec!["96".to_string()],
            ranges: AttributeRangesWire {
                ri: (1.4, 2.5),
                lw_ratio: (0.8, 1.5),
                volume: (0.1, 1.0),
                facets: (20, 200),
            },
        },
        LibraryResponse::Design(Box::new(design)),
        LibraryResponse::Attachment {
            name: "schedule.pdf".to_string(),
            content: vec![0xDE, 0xAD, 0xBE, 0xEF],
        },
        LibraryResponse::NotFound,
        LibraryResponse::Error(ErrorMsg {
            code: 1,
            message: "bad filter".to_string(),
        }),
        LibraryResponse::SearchResultsPage {
            results: vec![sample_summary()],
            next_cursor: Some(42),
            excluded_for_missing_curves: 0,
        },
        LibraryResponse::SearchResultsPage {
            results: Vec::new(),
            next_cursor: None,
            excluded_for_missing_curves: 0,
        },
        LibraryResponse::DesignSource {
            entry_id: 42,
            file_name: "round-brilliant.asc".to_string(),
            asc_text: "GemCad 5.0\n...\n".to_string(),
        },
        LibraryResponse::DesignSourceNotAvailable,
    ] {
        let mut buf = Vec::new();
        write_message(&mut buf, &resp).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: LibraryResponse = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, resp);
    }
}

#[test]
fn client_message_library_round_trips() {
    use crate::messages::ClientMessage;
    let msg = ClientMessage::Library(Box::new(LibraryRequest::FilterOptions));
    let mut buf = Vec::new();
    write_message(&mut buf, &msg).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: ClientMessage = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, msg);
}

/// A filter with `ri_tolerance`/`include_ignored`/`performance` all set round-trips
/// through a `LibraryRequest::Search`.
#[test]
fn range_filter_wire_with_every_new_field_set_round_trips() {
    let range = RangeFilterWire {
        ri_min: Some(1.4),
        ri_max: Some(2.5),
        lw_min: None,
        lw_max: None,
        volume_min: None,
        volume_max: None,
        facets_min: Some(40),
        facets_max: Some(90),
        ri_tolerance: Some((1.76, 0.02)),
        include_ignored: true,
        performance: vec![
            PerformanceFilterWire {
                metric: PerformanceMetricWire::Windowing,
                bound: PerformanceBoundWire::AtMost(20.0),
                tilt_radius_deg: 37.25,
                aggregate: PerformanceAggregateWire::Worst,
            },
            PerformanceFilterWire {
                metric: PerformanceMetricWire::Brilliance,
                bound: PerformanceBoundWire::AtLeast(55.5),
                tilt_radius_deg: 90.0,
                aggregate: PerformanceAggregateWire::Mean,
            },
        ],
    };

    let req = LibraryRequest::Search {
        query: "round".to_string(),
        shape_filter: "All".to_string(),
        gear_filter: "All".to_string(),
        range,
        order: SortOrderWire::Newest,
        tag_filter: Some("Heirloom".to_string()),
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &req).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: LibraryRequest = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, req);
}

/// Every [`SortOrderWire`] variant round-trips, and the type's default matches
/// [`SortOrderWire::CatalogueOrder`] -- pins the wire encoding for the sort order a
/// [`LibraryRequest::Search`] carries.
#[test]
fn sort_order_wire_every_variant_round_trips_and_defaults_to_catalogue_order() {
    assert_eq!(SortOrderWire::default(), SortOrderWire::CatalogueOrder);
    for order in [
        SortOrderWire::CatalogueOrder,
        SortOrderWire::Title,
        SortOrderWire::Newest,
        SortOrderWire::RecentlyEdited,
    ] {
        let mut buf = Vec::new();
        write_message(&mut buf, &order).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: SortOrderWire = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, order);
    }
}

/// [`LibraryRequest::Search`] carries a real `order`/`tag_filter` pair (not the
/// defaults) and round-trips both untouched -- a dedicated test (on top of this
/// variant's coverage in [`library_request_variants_round_trip`]) so this contract
/// is pinned by name.
#[test]
fn search_request_order_and_tag_filter_round_trip() {
    let req = LibraryRequest::Search {
        query: String::new(),
        shape_filter: "All".to_string(),
        gear_filter: "All".to_string(),
        range: RangeFilterWire::default(),
        order: SortOrderWire::RecentlyEdited,
        tag_filter: Some("Competition".to_string()),
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &req).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: LibraryRequest = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, req);
    let LibraryRequest::Search {
        order, tag_filter, ..
    } = decoded
    else {
        panic!("expected Search, got {decoded:?}");
    };
    assert_eq!(order, SortOrderWire::RecentlyEdited);
    assert_eq!(tag_filter.as_deref(), Some("Competition"));
}

/// [`DesignSummary::ignored`] round-trips both settings rather than silently
/// resetting to `false` on decode.
#[test]
fn design_summary_ignored_flag_round_trips_both_settings() {
    for ignored in [false, true] {
        let summary = DesignSummary {
            ignored,
            ..sample_summary()
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &summary).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: DesignSummary = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, summary);
        assert_eq!(decoded.ignored, ignored);
    }
}

/// A nonzero [`LibraryResponse::SearchResults::excluded_for_missing_curves`]
/// survives the wire (the round-trip test above only exercises `0`, which
/// wouldn't catch a decode that leaves the field at its zero default).
#[test]
fn search_results_exclusion_count_survives_the_wire() {
    let resp = LibraryResponse::SearchResults {
        items: vec![sample_summary()],
        excluded_for_missing_curves: 17,
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &resp).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: LibraryResponse = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, resp);
    let LibraryResponse::SearchResults {
        excluded_for_missing_curves,
        ..
    } = decoded
    else {
        panic!("expected SearchResults, got {decoded:?}");
    };
    assert_eq!(excluded_for_missing_curves, 17);
}

/// [`LibraryRequest::FetchDesignSource`] round-trips its `entry_id` -- a dedicated
/// test (on top of this field's coverage in
/// [`library_request_variants_round_trip`]) so this new variant's own contract is
/// pinned by name.
#[test]
fn fetch_design_source_request_round_trips() {
    let req = LibraryRequest::FetchDesignSource { entry_id: 7 };
    let mut buf = Vec::new();
    write_message(&mut buf, &req).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: LibraryRequest = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, req);
}

/// [`LibraryResponse::DesignSource`] round-trips every field, including `asc_text`
/// content that itself contains newlines (a real `.asc` file's shape) -- a
/// dedicated test (on top of this variant's coverage in
/// [`library_response_variants_round_trip`]) so this new variant's own contract is
/// pinned by name.
#[test]
fn design_source_response_round_trips() {
    let resp = LibraryResponse::DesignSource {
        entry_id: 7,
        file_name: "test.asc".to_string(),
        asc_text: "GemCad 5.0\nR1  c 41.0 96\n".to_string(),
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &resp).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: LibraryResponse = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, resp);
}

/// [`LibraryResponse::DesignSourceNotAvailable`] round-trips as its own distinct
/// variant, not collapsing onto [`LibraryResponse::NotFound`] -- the two carry
/// different meanings (see [`LibraryResponse::DesignSourceNotAvailable`]'s own doc
/// comment).
#[test]
fn design_source_not_available_round_trips_and_differs_from_not_found() {
    let mut buf = Vec::new();
    write_message(&mut buf, &LibraryResponse::DesignSourceNotAvailable).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: LibraryResponse = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, LibraryResponse::DesignSourceNotAvailable);
    assert_ne!(decoded, LibraryResponse::NotFound);
}

/// Same property as [`search_results_exclusion_count_survives_the_wire`], for
/// [`LibraryResponse::SearchResultsPage`] (whose field is currently always `0` in
/// practice).
#[test]
fn search_results_page_exclusion_count_survives_the_wire() {
    let resp = LibraryResponse::SearchResultsPage {
        results: vec![sample_summary()],
        next_cursor: Some(42),
        excluded_for_missing_curves: 3,
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &resp).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: LibraryResponse = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, resp);
    let LibraryResponse::SearchResultsPage {
        excluded_for_missing_curves,
        ..
    } = decoded
    else {
        panic!("expected SearchResultsPage, got {decoded:?}");
    };
    assert_eq!(excluded_for_missing_curves, 3);
}

/// [`LibraryRequest::Search`] round-trips for every [`SortOrderWire`] variant
/// crossed with `tag_filter` both `Some` and `None` -- the full
/// combination, on top of the individual cases already covered by
/// [`library_request_variants_round_trip`]/[`search_request_order_and_tag_filter_round_trip`]/
/// [`range_filter_wire_with_every_new_field_set_round_trips`], so no single
/// order/tag-filter pairing is silently unexercised.
#[test]
fn search_request_every_sort_order_and_tag_filter_combination_round_trips() {
    for order in [
        SortOrderWire::CatalogueOrder,
        SortOrderWire::Title,
        SortOrderWire::Newest,
        SortOrderWire::RecentlyEdited,
    ] {
        for tag_filter in [None, Some("Favorites".to_string())] {
            let req = LibraryRequest::Search {
                query: "round".to_string(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
                order,
                tag_filter: tag_filter.clone(),
            };
            let mut buf = Vec::new();
            write_message(&mut buf, &req).unwrap();
            let mut cursor = std::io::Cursor::new(buf);
            let decoded: LibraryRequest = read_message(&mut cursor).unwrap();
            assert_eq!(decoded, req, "order={order:?}, tag_filter={tag_filter:?}");
        }
    }
}

/// Pins [`SortOrderWire`]'s four variants at `postcard` discriminants 0-3, in
/// declaration order -- unlike the round-trip tests above, a renamed/reordered
/// variant that still round-trips against itself would not fail those; this reads
/// the raw encoded byte the way
/// [`crate::messages::stream::tests::cancel_and_library_keep_postcard_discriminants_0_and_1`]
/// pins `ClientMessage`'s own variants.
#[test]
fn sort_order_wire_postcard_discriminants_are_stable() {
    for (order, expected) in [
        (SortOrderWire::CatalogueOrder, 0),
        (SortOrderWire::Title, 1),
        (SortOrderWire::Newest, 2),
        (SortOrderWire::RecentlyEdited, 3),
    ] {
        let bytes = postcard::to_allocvec(&order).unwrap();
        assert_eq!(
            bytes,
            [expected],
            "{order:?} must stay postcard discriminant {expected}"
        );
    }
}
