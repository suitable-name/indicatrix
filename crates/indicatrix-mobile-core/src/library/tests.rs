//! Tests of the library over temporary databases created through the crate.

use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use indicatrix_vault::{
    db::sqlite::SEARCH_RESULT_CAP,
    local,
    model::{detail::FacetingDiagramDetail, entry::FacetingDiagramEntry},
};

use super::*;
use crate::{
    app_state::AppState,
    document::{asc_bytes, new_from_template, save_bytes},
    platform::{AppDirs, PickedFile, Platform},
    platform_mock::{MockPlatform, unique_temp_dir},
    settings::LibrarySort,
};

/// A library in a temporary folder, removed when the fixture drops.
pub(super) struct Fixture {
    root: PathBuf,
    state: LibraryState,
    handle: Option<LibraryHandle>,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let root = unique_temp_dir("library");
        let dirs = AppDirs {
            files: root.join("files"),
            cache: root.join("cache"),
        };
        let mut state = LibraryState::default();
        let handle = state.handle(&dirs).expect("the library opens");
        Self {
            root,
            state,
            handle: Some(handle),
        }
    }

    pub(super) fn h(&self) -> &LibraryHandle {
        self.handle.as_ref().expect("the handle is open")
    }

    /// The cached thumbnail files.
    fn thumb_files(&self) -> Vec<PathBuf> {
        fs::read_dir(self.root.join("cache").join("thumbs")).map_or_else(
            |_| Vec::new(),
            |entries| entries.flatten().map(|entry| entry.path()).collect(),
        )
    }

    pub(super) fn all(&self) -> Vec<LibraryCard> {
        self.h()
            .search(&LibraryQuery::default(), 0, 5_000)
            .expect("searches")
            .0
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.handle = None;
        self.state.close();
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub(super) fn picked(name: &str, bytes: &[u8]) -> PickedFile {
    PickedFile {
        display_name: name.to_owned(),
        bytes: bytes.to_vec(),
    }
}

/// The native design file and the `.asc` of template `index`, as the app saves them.
fn template_files(index: usize) -> (PickedFile, PickedFile) {
    let mock = Arc::new(MockPlatform::default());
    let platform: Arc<dyn Platform> = mock;
    let mut state = AppState::new(platform);
    new_from_template(&mut state, index).expect("the template exists");
    let native = save_bytes(&mut state).expect("saves").file;
    let asc = asc_bytes(&state).expect("exports").file;
    (
        picked(&native.suggested_name, &native.bytes),
        picked(&asc.suggested_name, &asc.bytes),
    )
}

/// One synthetic row.
struct Row<'a> {
    title: &'a str,
    shape: &'a str,
    gear: u32,
    ri: f64,
    facets: &'a str,
    designer: &'a str,
    remote: bool,
}

impl<'a> Row<'a> {
    const fn new(title: &'a str, shape: &'a str, gear: u32, ri: f64) -> Self {
        Self {
            title,
            shape,
            gear,
            ri,
            facets: "57",
            designer: "",
            remote: false,
        }
    }
}

fn insert(handle: &LibraryHandle, row: &Row<'_>) -> EntryId {
    let database = handle.lock();
    let entry = FacetingDiagramEntry {
        title: row.title.to_owned(),
        url: if row.remote {
            format!("https://example.org/{}", row.title)
        } else {
            format!("local://{}.asc", row.title)
        },
        design_id: String::new(),
    };
    let detail = FacetingDiagramDetail {
        shape: Some(row.shape.to_owned()),
        index_gear: Some(row.gear.to_string()),
        refractive_index: Some(row.ri.to_string()),
        facets_count: Some(row.facets.to_owned()),
        designer_info: (!row.designer.is_empty()).then(|| row.designer.to_owned()),
        ..FacetingDiagramDetail::default()
    };
    let source = if row.remote {
        "facetdiagrams.org"
    } else {
        local::LOCAL_SOURCE_ID
    };
    database
        .save_design(&entry, &detail, source)
        .expect("the row saves")
}

/// Four rows: ids in insertion order.
fn seed(handle: &LibraryHandle) -> [EntryId; 4] {
    let mut first = Row::new("Torbj\u{f6}rn Sphene", "Oval", 96, 1.9);
    first.facets = "55+6";
    first.designer = "Capps, Jerry; Lapidary Journal";
    let mut second = Row::new("Alpha Round", "Round", 64, 1.54);
    second.designer = "Smith, Ann";
    let third = Row::new("beta Round", "Round", 96, 1.76);
    let mut fourth = Row::new("Gamma Remote", "Round", 96, 1.54);
    fourth.remote = true;
    [
        insert(handle, &first),
        insert(handle, &second),
        insert(handle, &third),
        insert(handle, &fourth),
    ]
}

fn ids(cards: &[LibraryCard]) -> Vec<EntryId> {
    cards.iter().map(|card| card.id).collect()
}

fn query() -> LibraryQuery {
    LibraryQuery::default()
}

fn found(fx: &Fixture, query: &LibraryQuery) -> (Vec<LibraryCard>, usize) {
    fx.h().search(query, 0, 100).expect("searches")
}

#[test]
fn the_row_cap_matches_the_vault() {
    assert_eq!(i64::try_from(QUERY_ROW_CAP), Ok(SEARCH_RESULT_CAP));
}

#[test]
fn opening_creates_an_empty_library() {
    let fx = Fixture::new();
    let (cards, total) = found(&fx, &query());
    assert_eq!(cards.len(), 0);
    assert_eq!(total, 0);
    assert_eq!(
        fx.h().filter_options().expect("options"),
        FilterOptions::default()
    );
    assert_eq!(fx.state.revision(), 0);
    assert!(fx.root.join("files").join("library").is_dir());
}

#[test]
fn an_opener_makes_a_handle_the_state_can_adopt() {
    let root = unique_temp_dir("library-opener");
    let dirs = AppDirs {
        files: root.join("files"),
        cache: root.join("cache"),
    };
    let mut state = LibraryState::default();
    let opener = state.opener(&dirs);
    let handle = std::thread::spawn(move || opener.open())
        .join()
        .expect("the thread ends")
        .expect("opens");
    assert!(state.current().is_none());
    state.adopt(handle.clone());
    assert!(state.current().is_some());
    seed(&handle);
    assert_eq!(
        state
            .handle(&dirs)
            .expect("same handle")
            .search(&query(), 0, 10)
            .expect("searches")
            .1,
        4
    );
    handle.set_ignored(&[1], true).expect("marks");
    assert_eq!(state.revision(), 1);
    drop(handle);
    state.close();
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn designs_import_once_and_duplicates_are_counted() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let (_, asc) = template_files(2);
    let report = fx.h().import(vec![
        native.clone(),
        asc.clone(),
        picked("notes.txt", b"hello"),
    ]);
    assert_eq!(report.added, 2, "{report:?}");
    assert_eq!(report.duplicates, 0);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, "notes.txt");
    assert_eq!(fx.state.revision(), 1);

    let again = fx.h().import(vec![native, asc]);
    assert_eq!((again.added, again.duplicates), (0, 2), "{again:?}");
    assert_eq!(again.failed.len(), 0);
    assert_eq!(
        fx.state.revision(),
        1,
        "an import that wrote nothing is not a change"
    );

    let cards = fx.all();
    assert_eq!(cards.len(), 2);
    assert!(cards.iter().all(|card| card.mine && card.facets > 0));
    assert!(
        cards
            .iter()
            .any(|card| card.title == "Standard Round Brilliant")
    );
}

#[test]
fn the_same_file_twice_in_one_import_is_one_design() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let report = fx.h().import(vec![native.clone(), native]);
    assert_eq!((report.added, report.duplicates), (1, 1), "{report:?}");
    assert_eq!(fx.all().len(), 1);
}

#[test]
fn files_that_are_no_design_fail_with_a_sentence() {
    let fx = Fixture::new();
    let report = fx.h().import(vec![
        picked("map.hdr", b"#?RADIANCE"),
        picked("broken.indicatrix", b"not a design"),
        picked("broken.asc", b"not an asc"),
    ]);
    assert_eq!(report.added, 0);
    assert_eq!(report.failed.len(), 3, "{report:?}");
    assert!(
        report.failed.iter().all(|(_, why)| !why.is_empty()),
        "{report:?}"
    );
    // The older `.indicatrix.toml` sidecar is no longer a file type: it is refused with a
    // reason, with or without an `.asc` beside it.
    let report = fx.h().import(vec![picked(
        "lonely.indicatrix.toml",
        b"format_version = 1",
    )]);
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(fx.all().len(), 0);
}

#[test]
fn an_import_measures_the_proportions_and_the_shape() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    assert_eq!(fx.h().import(vec![native]).added, 1);
    let id = fx.all()[0].id;
    let record = fx
        .h()
        .lock()
        .get_diagram_full(id)
        .expect("reads")
        .expect("exists");
    assert!(
        record.lw_ratio.is_some() && record.volume.is_some(),
        "{record:?}"
    );
    assert_eq!(record.shape.as_deref(), Some("Round"));
    assert_eq!(fx.all()[0].shape, "Round");
}

#[test]
fn search_folds_case_and_matches_designers() {
    let fx = Fixture::new();
    let [first, ..] = seed(fx.h());
    let (cards, total) = found(
        &fx,
        &LibraryQuery {
            text: "TORBJ\u{d6}RN".to_owned(),
            ..query()
        },
    );
    assert_eq!((ids(&cards), total), (vec![first], 1));
    let card = &cards[0];
    assert_eq!(card.title, "Torbj\u{f6}rn Sphene");
    assert_eq!(card.designer, "Capps, Jerry");
    assert_eq!(
        (card.shape.as_str(), card.gear, card.facets),
        ("Oval", 96, 61)
    );
    assert_eq!(card.ri, Some(1.9));
    assert!(card.mine && !card.ignored && card.updated_unix > 0);
    let (by_designer, _) = found(
        &fx,
        &LibraryQuery {
            text: "capps".to_owned(),
            ..query()
        },
    );
    assert_eq!(ids(&by_designer), vec![first]);
}

#[test]
fn search_filters_by_shape_gear_index_and_ownership() {
    let fx = Fixture::new();
    let [first, second, third, fourth] = seed(fx.h());
    let round = LibraryQuery {
        shape: Some("Round".to_owned()),
        ..query()
    };
    assert_eq!(ids(&found(&fx, &round).0), vec![second, third, fourth]);
    let mine = LibraryQuery {
        mine_only: true,
        ..round.clone()
    };
    assert_eq!(ids(&found(&fx, &mine).0), vec![second, third]);
    let gear = LibraryQuery {
        gear: Some(96),
        ..query()
    };
    assert_eq!(ids(&found(&fx, &gear).0), vec![first, third, fourth]);
    let both = LibraryQuery {
        gear: Some(96),
        ..round
    };
    assert_eq!(ids(&found(&fx, &both).0), vec![third, fourth]);
    let high = LibraryQuery {
        ri_min: Some(1.7),
        ..query()
    };
    assert_eq!(ids(&found(&fx, &high).0), vec![first, third]);
    let low = LibraryQuery {
        ri_max: Some(1.6),
        ..query()
    };
    assert_eq!(ids(&found(&fx, &low).0), vec![second, fourth]);
    assert_eq!(
        found(
            &fx,
            &LibraryQuery {
                text: "nothing like this".to_owned(),
                ..query()
            }
        )
        .0
        .len(),
        0
    );
}

#[test]
fn search_sorts_and_pages() {
    let fx = Fixture::new();
    let [first, second, third, fourth] = seed(fx.h());
    let sorted = |sort| ids(&found(&fx, &LibraryQuery { sort, ..query() }).0);
    assert_eq!(
        sorted(LibrarySort::CatalogueOrder),
        vec![first, second, third, fourth]
    );
    assert_eq!(
        sorted(LibrarySort::Title),
        vec![second, third, fourth, first]
    );
    assert_eq!(
        sorted(LibrarySort::Newest),
        vec![fourth, third, second, first]
    );
    assert_eq!(
        sorted(LibrarySort::RecentlyEdited),
        vec![fourth, third, second, first]
    );
    let (page, total) = fx.h().search(&query(), 1, 2).expect("searches");
    assert_eq!((ids(&page), total), (vec![second, third], 4));
    assert_eq!(fx.h().search(&query(), 10, 2).expect("searches").0.len(), 0);
    assert_eq!(fx.h().search(&query(), 0, 0).expect("searches").0.len(), 0);
}

#[test]
fn ignored_entries_hide_until_asked_for() {
    let fx = Fixture::new();
    let [first, second, third, fourth] = seed(fx.h());
    fx.h().set_ignored(&[second], true).expect("marks");
    let (cards, total) = found(&fx, &query());
    assert_eq!((ids(&cards), total), (vec![first, third, fourth], 3));
    let (shown, total) = found(
        &fx,
        &LibraryQuery {
            show_ignored: true,
            ..query()
        },
    );
    assert_eq!(total, 4);
    assert!(
        shown
            .iter()
            .find(|card| card.id == second)
            .expect("listed")
            .ignored
    );
    fx.h().set_ignored(&[second], false).expect("restores");
    assert_eq!(found(&fx, &query()).1, 4);
    assert_eq!(fx.state.revision(), 2);
}

#[test]
fn tags_set_add_filter_and_list() {
    let fx = Fixture::new();
    let [first, second, third, _] = seed(fx.h());
    let tags = |names: &[&str]| {
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    };
    fx.h()
        .set_tags(
            &[first, second],
            &tags(&["Favorite", "  Cut Soon ", "favorite", ""]),
        )
        .expect("tags");
    let card_tags = |id| {
        found(&fx, &query())
            .0
            .into_iter()
            .find(|card| card.id == id)
            .expect("listed")
            .tags
    };
    assert_eq!(card_tags(first), tags(&["Cut Soon", "Favorite"]));
    let favorite = LibraryQuery {
        tag: Some("favorite".to_owned()),
        ..query()
    };
    assert_eq!(ids(&found(&fx, &favorite).0), vec![first, second]);
    assert_eq!(
        found(
            &fx,
            &LibraryQuery {
                tag: Some("nobody has it".to_owned()),
                ..query()
            }
        )
        .0
        .len(),
        0
    );

    fx.h()
        .set_tags(&[first], &tags(&["Favorite"]))
        .expect("replaces");
    assert_eq!(card_tags(first), tags(&["Favorite"]));
    assert_eq!(card_tags(second), tags(&["Cut Soon", "Favorite"]));
    fx.h()
        .add_tags(&[first, third], &tags(&["Zed"]))
        .expect("adds");
    assert_eq!(card_tags(first), tags(&["Favorite", "Zed"]));
    assert_eq!(card_tags(third), tags(&["Zed"]));
    fx.h().set_tags(&[third], &[]).expect("clears");
    assert_eq!(card_tags(third).len(), 0);
    assert_eq!(
        fx.h().filter_options().expect("options").tags,
        tags(&["Cut Soon", "Favorite", "Zed"])
    );
}

#[test]
fn filter_options_list_what_the_library_has() {
    let fx = Fixture::new();
    seed(fx.h());
    let options = fx.h().filter_options().expect("options");
    assert_eq!(options.shapes, vec!["Oval".to_owned(), "Round".to_owned()]);
    assert_eq!(options.gears, vec![64, 96]);
    assert_eq!(options.tags.len(), 0);
}

#[test]
fn delete_removes_the_entry_and_its_tags() {
    let fx = Fixture::new();
    let [first, second, ..] = seed(fx.h());
    fx.h()
        .add_tags(&[first], &["Gone".to_owned()])
        .expect("tags");
    fx.h().delete(&[first]).expect("deletes");
    assert_eq!(found(&fx, &query()).1, 3);
    assert!(fx.h().load(first).is_err());
    assert!(fx.h().delete(&[first]).is_err(), "deleting twice says so");
    assert_eq!(
        found(
            &fx,
            &LibraryQuery {
                tag: Some("Gone".to_owned()),
                ..query()
            }
        )
        .0
        .len(),
        0
    );
    assert!(found(&fx, &query()).0.iter().any(|card| card.id == second));
}

#[test]
fn a_saved_design_loads_back_byte_for_byte_and_overwrites_in_place() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let (other, _) = template_files(2);
    let id = fx
        .h()
        .save(None, &native.display_name, &native.bytes)
        .expect("inserts");
    let stored = fx.h().load(id).expect("loads");
    assert_eq!(stored.file_name, native.display_name);
    assert_eq!(stored.bytes, native.bytes);
    assert!(fx.all()[0].mine);

    let before = fx.all()[0].title.clone();
    let again = fx
        .h()
        .save(Some(id), &native.display_name, &other.bytes)
        .expect("overwrites");
    assert_eq!(again, id);
    assert_eq!(fx.h().load(id).expect("loads").bytes, other.bytes);
    assert_eq!(fx.all().len(), 1);
    assert_ne!(
        fx.all()[0].title,
        before,
        "the title follows the saved file"
    );

    let third = fx
        .h()
        .save(None, &native.display_name, &native.bytes)
        .expect("inserts");
    assert_ne!(third, id);
    assert!(
        fx.h()
            .load(third)
            .expect("loads")
            .file_name
            .contains(" (2).")
    );
    assert_eq!(fx.all().len(), 2);
    assert!(fx.h().save(None, "x.isdd", b"nope").is_err());
    assert!(
        fx.h().save(Some(9_999), "y.isdd", &native.bytes).is_ok(),
        "a vanished entry is inserted"
    );
}

#[test]
fn an_imported_asc_loads_back_as_it_came() {
    let fx = Fixture::new();
    let (_, asc) = template_files(1);
    assert_eq!(fx.h().import(vec![asc.clone()]).added, 1);
    let id = fx.all()[0].id;
    let stored = fx.h().load(id).expect("loads");
    assert_eq!(
        (stored.file_name, stored.bytes),
        (asc.display_name, asc.bytes)
    );
}

#[test]
fn export_gives_a_native_file_for_every_entry() {
    let fx = Fixture::new();
    let (native, asc) = template_files(1);
    let id = fx
        .h()
        .save(None, &native.display_name, &native.bytes)
        .expect("inserts");
    let shared = fx.h().export_bytes(id).expect("exports");
    assert_eq!(
        (shared.suggested_name.as_str(), shared.bytes.as_slice()),
        (native.display_name.as_str(), native.bytes.as_slice())
    );

    assert_eq!(fx.h().import(vec![asc]).added, 1);
    let asc_id = fx
        .all()
        .into_iter()
        .find(|card| card.id != id)
        .expect("second entry")
        .id;
    let converted = fx.h().export_bytes(asc_id).expect("exports");
    assert!(
        std::path::Path::new(&converted.suggested_name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("isdd"))
    );
    assert!(local::import_native_design(&converted.suggested_name, &converted.bytes).is_ok());
}

#[test]
fn a_replaced_file_keeps_hand_entered_metadata() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let (other, _) = template_files(2);
    assert_eq!(fx.h().import(vec![native.clone()]).added, 1);
    let id = fx.all()[0].id;
    {
        let database = fx.h().lock();
        let mut record = database
            .get_diagram_full(id)
            .expect("reads")
            .expect("exists");
        record.designer_info = Some("Hand, Typed".to_owned());
        database
            .update_diagram_metadata(
                id,
                &indicatrix_vault::model::metadata_update::MetadataUpdate {
                    designer_info: record.designer_info.clone(),
                    shape: record.shape.clone(),
                    refractive_index: record.refractive_index.clone(),
                    index_gear: record.index_gear.clone(),
                    facets_count: record.facets_count,
                    ..Default::default()
                },
            )
            .expect("updates");
    }
    // Same name, different bytes: the entry is replaced.
    let replacing = picked(&native.display_name, &other.bytes);
    assert_eq!(fx.h().import(vec![replacing]).added, 1);
    assert_eq!(fx.all().len(), 1);
    assert_eq!(fx.all()[0].designer, "Hand, Typed");
}

#[test]
fn thumbnails_draw_cache_and_recover_from_a_bad_cache_file() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    assert_eq!(fx.h().import(vec![native]).added, 1);
    let id = fx.all()[0].id;

    let thumbnail = fx.h().thumbnail_rgba(id, 96).expect("draws");
    assert_eq!(thumbnail.width.max(thumbnail.height), 96);
    assert_eq!(
        thumbnail.rgba.len(),
        thumbnail.width as usize * thumbnail.height as usize * 4
    );
    assert!(
        thumbnail
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] > 0),
        "some pixel is visible"
    );
    let corner = thumbnail.rgba[..4].to_vec();
    assert!(
        thumbnail
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel != corner.as_slice()),
        "the picture is not one flat color"
    );

    let files = fx.thumb_files();
    assert_eq!(files.len(), 1, "{files:?}");
    // A valid file in the cache is what the next call returns.
    let marker = Thumbnail {
        width: 2,
        height: 1,
        rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
    };
    fs::write(&files[0], thumbs::encode(&marker)).expect("writes");
    assert_eq!(
        fx.h().thumbnail_rgba(id, 96).expect("hits the cache"),
        marker
    );
    // A damaged one is drawn again.
    fs::write(&files[0], b"garbage").expect("writes");
    assert_eq!(
        fx.h().thumbnail_rgba(id, 96).expect("draws again"),
        thumbnail
    );
    assert_eq!(fx.thumb_files().len(), 1);

    fx.h().delete(&[id]).expect("deletes");
    assert!(
        fx.thumb_files().is_empty(),
        "a deleted design leaves no thumbnail"
    );
    assert!(fx.h().thumbnail_rgba(id, 96).is_err());
}

#[test]
fn a_thousand_entries_search_quickly() {
    let fx = Fixture::new();
    let shapes = ["Round", "Oval", "Square", "Pear"];
    let gears = [64, 80, 88, 96];
    let started = Instant::now();
    for index in 0..1_000_usize {
        let title = format!("Design {index:04}");
        let mut row = Row::new(
            &title,
            shapes[index % 4],
            gears[index % 4],
            1.5 + (index % 10) as f64 / 20.0,
        );
        row.designer = "Synthetic, Sam; Test Journal";
        insert(fx.h(), &row);
    }
    let inserted = started.elapsed();

    let started = Instant::now();
    let text = LibraryQuery {
        text: "design 07".to_owned(),
        sort: LibrarySort::Title,
        ..query()
    };
    let (page, total) = fx.h().search(&text, 0, 60).expect("searches");
    let everything = fx.h().search(&query(), 0, 60).expect("searches");
    let filtered = fx
        .h()
        .search(
            &LibraryQuery {
                shape: Some("Pear".to_owned()),
                gear: Some(96),
                ..query()
            },
            0,
            60,
        )
        .expect("searches");
    let options = fx.h().filter_options().expect("options");
    let searched = started.elapsed();
    eprintln!(
        "1,000 rows inserted in {inserted:?}; four searches and the filter options in {searched:?}"
    );

    assert_eq!((page.len(), total), (60, 100));
    assert_eq!(everything.1, 1_000);
    assert_eq!(filtered.1, 250);
    assert_eq!(options.shapes.len(), 4);
    assert!(searched < Duration::from_secs(2), "{searched:?}");
}
