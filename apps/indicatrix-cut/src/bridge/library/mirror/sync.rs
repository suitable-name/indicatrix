//! The mirror algorithm itself: enumerating the remote catalogue
//! ([`enumerate_remote_catalogue`]), the additive/update-only per-design sync loop
//! ([`run_mirror_sync`]/[`sync_one_design`]), and the worker-thread wrapper
//! ([`spawn_mirror_sync`]) the UI actually calls. See this group's own `mod.rs` doc
//! comment for the full design.

use super::options::{
    LibraryTransport, MirrorCounts, MirrorHandle, MirrorOptions, MirrorOutcome, MirrorProgress,
    mirror_source_id,
};
use crate::{bridge::library::client::LibrarySession, settings::WorkerSettings};
use indicatrix_net::library::{AngleSettingWire, LibraryRequest, LibraryResponse, RangeFilterWire};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{
        angle::AngleSetting, detail::FacetDiagramDetail, entry::FacetDiagramEntry,
        file::AttachedFile, mirror::MirrorState,
    },
};
use slint::{ComponentHandle, Weak};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

/// Spawns the mirror-sync worker thread against `worker`'s design library, writing into
/// `db`. `on_progress` is invoked on the UI event loop after each design examined;
/// `on_done` is invoked once, exactly once, when the sync completes, is cancelled, or
/// fails outright. Follows `bridge::export_thread::spawn_export`'s exact pattern (a
/// `thread::spawn` worker, an `Arc<AtomicBool>` cancel flag, results marshalled back via
/// `Weak::upgrade_in_event_loop`).
///
/// `db` is locked only for the duration of each individual database call inside the
/// sync loop (see [`run_mirror_sync`]), never for the whole sync -- so the local library
/// UI (search, detail, import) stays responsive on the SAME database while a
/// multi-minute sync runs in the background.
///
/// Drives the sync against one [`LibrarySession`] -- built here, from `worker`, and held
/// for the whole sync -- rather than reconnecting per request, the whole point of this
/// module's held-connection task; see [`LibrarySession`]'s own doc comment for what that
/// buys (one handshake instead of thousands for a real catalogue) and how it behaves when
/// that held connection drops mid-sync.
pub fn spawn_mirror_sync<T, P, D>(
    ui_weak: Weak<T>,
    db: Arc<Mutex<Database>>,
    worker: WorkerSettings,
    options: MirrorOptions,
    on_progress: P,
    on_done: D,
) -> MirrorHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, MirrorProgress) + Send + 'static + Clone,
    D: Fn(&T, MirrorOutcome) + Send + 'static,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
    let ui_weak_done = ui_weak.clone();

    thread::spawn(move || {
        let source_id = mirror_source_id(&worker);
        let session = LibrarySession::new(worker);
        let progress_ui_weak = ui_weak;
        let outcome = run_mirror_sync(
            &db,
            &session,
            &source_id,
            options,
            &cancel_worker,
            move |progress| {
                let on_progress = on_progress.clone();
                let _ =
                    progress_ui_weak.upgrade_in_event_loop(move |ui| on_progress(&ui, progress));
            },
        );
        let _ = ui_weak_done.upgrade_in_event_loop(move |ui| on_done(&ui, outcome));
    });

    MirrorHandle { cancel }
}

/// Walks [`LibraryRequest::SearchPage`] to exhaustion, concatenating every page's
/// [`DesignSummary`](indicatrix_net::library::DesignSummary) list into one `Vec` -- the
/// WHOLE remote catalogue matching the (currently always empty/unfiltered) query, not
/// just its first page. See the module doc comment's "Pagination" section for the
/// keyset-cursor scheme this implements (`cursor: None`, then `cursor:
/// Some(next_cursor)` from each reply, until a reply's `next_cursor` is `None`) and
/// why it never needs to skip or re-see a row even if designs are added to the remote
/// catalogue while this runs.
///
/// Returns `Err(MirrorOutcome::Failed(..))` -- ready to propagate straight out of
/// [`run_mirror_sync`] -- on the first request that fails outright or replies with
/// anything other than [`LibraryResponse::SearchResultsPage`]; nothing is written
/// locally by this function (it only ever reads from `transport`), so a failure here
/// leaves the local database exactly as it was, matching this module's existing
/// "nothing written until enumeration succeeds" behavior.
fn enumerate_remote_catalogue(
    transport: &impl LibraryTransport,
) -> Result<Vec<indicatrix_net::library::DesignSummary>, MirrorOutcome> {
    let mut summaries = Vec::new();
    let mut cursor = None;
    loop {
        let request = LibraryRequest::SearchPage {
            query: String::new(),
            shape_filter: "All".to_string(),
            gear_filter: "All".to_string(),
            range: RangeFilterWire::default(),
            cursor,
        };
        match transport.request(&request) {
            Ok(LibraryResponse::SearchResultsPage {
                results,
                next_cursor,
                // This exhaustive, unfiltered walk never sets `range.performance`, so
                // there is nothing this count could ever report here -- see
                // `apps/indicatrix-worker::serve::library::search_page`'s own doc comment
                // on why `SearchResultsPage` always sends `0` for exactly that reason.
                excluded_for_missing_curves: _,
            }) => {
                summaries.extend(results);
                match next_cursor {
                    Some(c) => cursor = Some(c),
                    None => return Ok(summaries),
                }
            }
            Ok(other) => {
                return Err(MirrorOutcome::Failed(format!(
                    "remote worker replied to SearchPage with an unexpected message: {other:?}"
                )));
            }
            Err(e) => {
                return Err(MirrorOutcome::Failed(format!(
                    "could not list the remote library: {e}"
                )));
            }
        }
    }
}

/// The mirror algorithm itself -- generic over [`LibraryTransport`] so it's testable
/// without a socket (see that trait's doc comment). See the module doc comment for the
/// full design: additive/update-only, `url`-keyed identity, two-tier content-hash
/// skipping, eager (capped) attachment fetching, and per-design cancellation safety.
#[must_use]
pub fn run_mirror_sync(
    db: &Arc<Mutex<Database>>,
    transport: &impl LibraryTransport,
    source_id: &str,
    options: MirrorOptions,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(MirrorProgress),
) -> MirrorOutcome {
    let summaries = match enumerate_remote_catalogue(transport) {
        Ok(list) => list,
        Err(outcome) => return outcome,
    };

    let mut counts = MirrorCounts {
        total_found: summaries.len(),
        ..MirrorCounts::default()
    };

    for (processed, summary) in summaries.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return MirrorOutcome::Cancelled(counts);
        }

        let existing_state = {
            let db = db.lock().unwrap_or_else(PoisonError::into_inner);
            db.get_mirror_state(&summary.url).ok().flatten()
        };

        let unchanged = existing_state
            .as_ref()
            .is_some_and(|state| state.summary_version == summary.version);
        if unchanged {
            counts.skipped_unchanged += 1;
            on_progress(MirrorProgress {
                processed: processed + 1,
                counts,
                current_title: summary.title.clone(),
            });
            continue;
        }

        let is_new = existing_state.is_none();
        match sync_one_design(db, transport, source_id, options, &summary, &mut counts) {
            Ok(()) => {
                if is_new {
                    counts.new_count += 1;
                } else {
                    counts.updated_count += 1;
                }
            }
            Err(()) => counts.failed += 1,
        }

        on_progress(MirrorProgress {
            processed: processed + 1,
            counts,
            current_title: summary.title.clone(),
        });
    }

    MirrorOutcome::Completed(counts)
}

/// Fetches and saves exactly one design: `FetchDesign`, then every non-oversized
/// attachment's bytes, then one local `save_diagram_entry` + `save_diagram_detail` +
/// `upsert_mirror_state` -- all network I/O happens before the first local write, so a
/// failure partway through never touches the database (see the module doc comment's
/// "Cancellation" section, which this same all-network-then-all-local ordering also
/// backs).
///
/// `Err(())` on any failure (fetch, or local save) -- the caller only needs to know
/// pass/fail to update [`MirrorCounts::failed`]; the specific reason isn't surfaced
/// per-design (this sync covers up to a thousand designs at once -- see the module doc
/// comment's protocol-limitation note -- so a per-design error UI would be noise; a
/// failed design is simply retried on the next sync, same as one skipped for looking
/// unchanged is not).
fn sync_one_design(
    db: &Arc<Mutex<Database>>,
    transport: &impl LibraryTransport,
    source_id: &str,
    options: MirrorOptions,
    summary: &indicatrix_net::library::DesignSummary,
    counts: &mut MirrorCounts,
) -> Result<(), ()> {
    let design = match transport.request(&LibraryRequest::FetchDesign {
        entry_id: summary.entry_id,
    }) {
        Ok(LibraryResponse::Design(d)) => *d,
        _ => return Err(()),
    };

    let mut attached_files = Vec::with_capacity(design.attachments.len());
    for meta in &design.attachments {
        if meta.size > options.max_attachment_bytes {
            counts.attachments_skipped_too_large += 1;
            continue;
        }
        match transport.request(&LibraryRequest::FetchAttachment {
            attachment_id: meta.id,
        }) {
            Ok(LibraryResponse::Attachment { name, content }) => {
                counts.attachment_bytes_fetched += content.len() as u64;
                counts.attachments_fetched += 1;
                attached_files.push(AttachedFile {
                    name,
                    url: meta.url.clone(),
                    content,
                });
            }
            Ok(LibraryResponse::NotFound) => {
                // Vanished server-side between FetchDesign and FetchAttachment --
                // save the rest of the design without this one file rather than
                // failing it outright.
            }
            _ => return Err(()),
        }
    }

    let entry = FacetDiagramEntry {
        title: design.title.clone(),
        url: design.url.clone(),
        design_id: design.design_id.clone().unwrap_or_default(),
    };
    let detail = FacetDiagramDetail {
        page_url: design.page_url.clone(),
        diagram_image_name: design.diagram_image_name.clone(),
        diagram_image_data: design.diagram_image_data.clone(),
        angle_settings_table: design.angle_settings.iter().map(to_angle_setting).collect(),
        attached_files,
        competition_diagram: design.competition_diagram.clone(),
        lw_ratio: design.lw_ratio.clone(),
        refractive_index: design.refractive_index.clone(),
        index_gear: design.index_gear.clone(),
        volume: design.volume.clone(),
        facets_count: design.facets_count.clone(),
        shape: design.shape.clone(),
        designer_info: design.designer_info.clone(),
        ..FacetDiagramDetail::default()
    };

    let local_entry_id = {
        let db = db.lock().unwrap_or_else(PoisonError::into_inner);
        let Ok(id) = db.save_diagram_entry(&entry, source_id) else {
            return Err(());
        };
        if db.save_diagram_detail(&detail, id).is_err() {
            return Err(());
        }
        // Only recorded once the local write above actually succeeded -- a design
        // that failed to save is never marked as synced, so the next sync retries
        // it rather than silently treating a failed write as done. See this
        // function's own doc comment.
        let _ = db.upsert_mirror_state(&MirrorState {
            url: design.url.clone(),
            source_id: source_id.to_string(),
            summary_version: summary.version,
            design_version: design.version,
        });
        id
    };
    let _ = local_entry_id;

    Ok(())
}

fn to_angle_setting(a: &AngleSettingWire) -> AngleSetting {
    AngleSetting {
        order_index: a.order_index,
        facet: a.facet.clone(),
        angle: a.angle.clone(),
        index: a.index.clone(),
        notes: a.notes.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::library::client as library_client;
    use indicatrix_net::library::{AttachedFileMeta, DesignRecord, DesignSummary};
    use std::{cell::RefCell, collections::HashMap, sync::atomic::AtomicUsize};

    /// A scripted, in-memory [`LibraryTransport`] -- no socket, no TLS, no real
    /// `indicatrix-worker`. Counts how many times each request kind is made so tests can
    /// pin the "no-op second sync costs nothing but one `SearchPage` per page" claim
    /// precisely, and pages `search_result` at [`Self::page_size`] rows per
    /// `SearchPage` reply -- a scripted stand-in for `apps/indicatrix-worker`'s own
    /// `SEARCH_RESULT_CAP`-per-page behavior, small enough in tests to exercise a real
    /// multi-page walk without seeding a thousand-plus rows.
    struct FakeTransport {
        search_result: Vec<DesignSummary>,
        /// How many `search_result` rows one `SearchPage` reply hands back. Defaults
        /// (via [`Self::new`]) to `usize::MAX` -- one page always covers the whole
        /// `search_result` list -- so every pre-existing single-page test keeps making
        /// exactly one `SearchPage` call, unchanged; [`Self::with_page_size`] shrinks it
        /// for a test that specifically wants to exercise pagination.
        page_size: usize,
        designs: HashMap<i64, DesignRecord>,
        attachments: HashMap<i64, (String, Vec<u8>)>,
        search_calls: AtomicUsize,
        fetch_design_calls: AtomicUsize,
        fetch_attachment_calls: AtomicUsize,
        /// Set of entry ids [`LibraryRequest::FetchDesign`] was actually called for --
        /// lets a test assert WHICH designs were (not) re-fetched, not just a count.
        fetched_entry_ids: RefCell<Vec<i64>>,
        /// When `Some(id)`, [`LibraryRequest::FetchDesign`] for that entry id returns a
        /// transport-level [`library_client::LibraryClientError::Client`] error instead
        /// of a normal reply -- standing in for a `LibrarySession` whose held connection
        /// dropped mid-sync AND whose own reconnect attempt then also failed (see
        /// `library_client::request_with_reconnect`'s "propagates the error when
        /// reconnecting also fails" case): a genuine transport failure reaching
        /// [`run_mirror_sync`], not a logical [`LibraryResponse::NotFound`]. Set via
        /// [`Self::with_fetch_design_failure`].
        fail_fetch_design_for: Option<i64>,
    }

    impl FakeTransport {
        fn new(summaries: Vec<DesignSummary>, designs: Vec<DesignRecord>) -> Self {
            Self {
                search_result: summaries,
                page_size: usize::MAX,
                designs: designs.into_iter().map(|d| (d.entry_id, d)).collect(),
                attachments: HashMap::new(),
                search_calls: AtomicUsize::new(0),
                fetch_design_calls: AtomicUsize::new(0),
                fetch_attachment_calls: AtomicUsize::new(0),
                fetched_entry_ids: RefCell::new(Vec::new()),
                fail_fetch_design_for: None,
            }
        }

        fn with_attachment(mut self, id: i64, name: &str, content: Vec<u8>) -> Self {
            self.attachments.insert(id, (name.to_string(), content));
            self
        }

        /// Shrinks how many rows one `SearchPage` reply hands back, forcing
        /// [`enumerate_remote_catalogue`] into more than one round trip for a
        /// `search_result` longer than `page_size`. See [`Self::page_size`]'s own doc
        /// comment.
        fn with_page_size(mut self, page_size: usize) -> Self {
            self.page_size = page_size;
            self
        }

        /// Makes [`LibraryRequest::FetchDesign`] for `entry_id` fail with a
        /// transport-level error -- see [`Self::fail_fetch_design_for`]'s own doc
        /// comment for what this simulates.
        fn with_fetch_design_failure(mut self, entry_id: i64) -> Self {
            self.fail_fetch_design_for = Some(entry_id);
            self
        }
    }

    impl LibraryTransport for FakeTransport {
        fn request(
            &self,
            req: &LibraryRequest,
        ) -> Result<LibraryResponse, library_client::LibraryClientError> {
            match req {
                LibraryRequest::Search { .. } => {
                    unreachable!(
                        "mirror sync always pages via SearchPage now, never sends Search directly"
                    )
                }
                LibraryRequest::SearchPage { cursor, .. } => {
                    self.search_calls.fetch_add(1, Ordering::Relaxed);
                    let start = cursor.map_or(0, |after| {
                        self.search_result
                            .iter()
                            .position(|s| s.entry_id == after)
                            .map_or(self.search_result.len(), |i| i + 1)
                    });
                    let end = start
                        .saturating_add(self.page_size)
                        .min(self.search_result.len());
                    let page = self.search_result[start..end].to_vec();
                    // Mirrors `apps/indicatrix-worker`'s own rule (see
                    // `serve::library::search_page`): a full page means "there may be
                    // more", a short page means "this was the last one".
                    let next_cursor = if page.len() == self.page_size {
                        page.last().map(|s| s.entry_id)
                    } else {
                        None
                    };
                    Ok(LibraryResponse::SearchResultsPage {
                        results: page,
                        next_cursor,
                        // This fake never models a performance filter -- see this
                        // module's own doc comment on why `excluded_for_missing_curves`
                        // is always `0` for a `SearchPage` reply in practice.
                        excluded_for_missing_curves: 0,
                    })
                }
                LibraryRequest::FetchDesign { entry_id } => {
                    self.fetch_design_calls.fetch_add(1, Ordering::Relaxed);
                    self.fetched_entry_ids.borrow_mut().push(*entry_id);
                    if self.fail_fetch_design_for == Some(*entry_id) {
                        return Err(library_client::LibraryClientError::Client(
                            indicatrix_net::client::ClientError::Net(
                                indicatrix_net::messages::NetError::Framing(
                                    indicatrix_net::framing::FramingError::Io(std::io::Error::new(
                                        std::io::ErrorKind::ConnectionReset,
                                        "connection dropped and could not be re-established",
                                    )),
                                ),
                            ),
                        ));
                    }
                    Ok(self
                        .designs
                        .get(entry_id)
                        .map_or(LibraryResponse::NotFound, |d| {
                            LibraryResponse::Design(Box::new(d.clone()))
                        }))
                }
                LibraryRequest::FetchAttachment { attachment_id } => {
                    self.fetch_attachment_calls.fetch_add(1, Ordering::Relaxed);
                    match self.attachments.get(attachment_id) {
                        Some((name, content)) => Ok(LibraryResponse::Attachment {
                            name: name.clone(),
                            content: content.clone(),
                        }),
                        None => Ok(LibraryResponse::NotFound),
                    }
                }
                LibraryRequest::FilterOptions => unreachable!("mirror sync never sends this"),
                LibraryRequest::FetchDesignSource { .. } => {
                    unreachable!(
                        "mirror sync never sends this -- only the editor's remote 'Load Selected' does"
                    )
                }
            }
        }
    }

    /// A fresh, empty temp database for one test.
    ///
    /// The counter plus `process::id()` make the name unique WITHIN a run, but not
    /// ACROSS runs: a run that dies before cleanup (a panicking test, a killed
    /// `cargo test`) leaks its file, and the OS reuses process ids freely. A later run
    /// whose pid and counter both collide with a leaked file would open a
    /// PRE-POPULATED database -- which is exactly what happened here: 685 leaked files
    /// had accumulated in this machine's temp directory, and four tests in this module
    /// (`a_second_sync_of_an_unchanged_catalogue_skips_every_design_and_makes_no_fetch_design_calls`
    /// among them) failed once against a stale one before passing cleanly on a rerun.
    /// Every assertion involved was correct; the fixture was not.
    ///
    /// Deleting any pre-existing file first removes the failure mode entirely, rather
    /// than merely making a collision less likely.
    fn temp_db() -> (Arc<Mutex<Database>>, std::path::PathBuf) {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "indicatrix-cut-library-mirror-test-{}-{n}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        (Arc::new(Mutex::new(db)), path)
    }

    fn design_record(entry_id: i64, title: &str, url: &str, extra_field: &str) -> DesignRecord {
        let mut record = DesignRecord {
            entry_id,
            title: title.to_string(),
            url: url.to_string(),
            design_id: Some(format!("D-{entry_id}")),
            page_url: url.to_string(),
            diagram_image_name: None,
            diagram_image_data: None,
            competition_diagram: None,
            lw_ratio: Some(extra_field.to_string()),
            refractive_index: Some("1.72".to_string()),
            index_gear: Some("96".to_string()),
            volume: Some("0.65".to_string()),
            facets_count: Some("57".to_string()),
            shape: Some("Round".to_string()),
            designer_info: Some("Capps, Jerry".to_string()),
            // A mirror-sync fixture stands in for a design whose previews have never
            // been generated -- the ordinary case for a freshly mirrored catalogue.
            preview_material: None,
            // Likewise stand-ins for a design with no proportion/symmetry metadata on
            // file yet -- these fields (added alongside `PROTOCOL_VERSION` v6) aren't
            // what any test in this module exercises, so `None` keeps this fixture's
            // existing shape rather than inventing values these tests never check.
            hw_ratio: None,
            tw_ratio: None,
            uw_ratio: None,
            pw_ratio: None,
            cw_ratio: None,
            symmetry_order: None,
            mirror_symmetry: None,
            designer: None,
            angle_settings: vec![AngleSettingWire {
                order_index: 0,
                facet: "P1".to_string(),
                angle: "41.0".to_string(),
                index: "96".to_string(),
                notes: String::new(),
            }],
            attachments: Vec::new(),
            version: [0u8; 32],
        };
        record.version = content_hash(&[title.as_bytes(), url.as_bytes(), extra_field.as_bytes()]);
        record
    }

    fn design_summary(entry_id: i64, title: &str, url: &str, extra_field: &str) -> DesignSummary {
        DesignSummary {
            entry_id,
            title: title.to_string(),
            url: url.to_string(),
            design_id: Some(format!("D-{entry_id}")),
            shape: Some("Round".to_string()),
            index_gear: Some("96".to_string()),
            facets_count: Some("57".to_string()),
            designer_info: Some("Capps, Jerry".to_string()),
            lw_ratio: Some(extra_field.to_string()),
            refractive_index: Some("1.72".to_string()),
            volume: Some("0.65".to_string()),
            competition_diagram: None,
            ignored: false,
            version: content_hash(&[title.as_bytes(), url.as_bytes(), extra_field.as_bytes()]),
        }
    }

    /// A trivial, deterministic stand-in for the real server-side SHA-256 hash (see
    /// `apps/indicatrix-worker/src/serve/library/mod.rs::hash_summary`/`hash_record`) --
    /// this module never computes or checks the hash's own algorithm, only compares two
    /// hashes for equality, so any deterministic function of the "content" a test cares
    /// about is sufficient.
    fn content_hash(parts: &[&[u8]]) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for p in parts {
            hasher.update(p);
        }
        hasher.finalize().into()
    }

    #[test]
    fn a_local_only_design_survives_a_mirror_sync() {
        let (db, path) = temp_db();
        // A design that exists ONLY locally (a user's own `.asc` import) -- a synthetic
        // `local://` URL, exactly `indicatrix_vault::local::import_asc`'s convention.
        let local_entry_id = {
            let guard = db.lock().unwrap();
            guard
                .save_diagram_entry(
                    &FacetDiagramEntry {
                        title: "My Own Trichecker".to_string(),
                        url: "local://my_trichecker.asc".to_string(),
                        design_id: String::new(),
                    },
                    indicatrix_vault::local::LOCAL_SOURCE_ID,
                )
                .unwrap()
        };
        {
            let guard = db.lock().unwrap();
            guard
                .save_diagram_detail(
                    &FacetDiagramDetail {
                        shape: Some("Trichecker".to_string()),
                        attached_files: vec![AttachedFile {
                            name: "my_trichecker.asc".to_string(),
                            url: String::new(),
                            content: b"a real user file".to_vec(),
                        }],
                        ..FacetDiagramDetail::default()
                    },
                    local_entry_id,
                )
                .unwrap();
        }

        let remote_summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
        let remote_design = design_record(1, "Round Brilliant", "https://example.test/1", "v1");
        let transport = FakeTransport::new(vec![remote_summary], vec![remote_design]);

        let outcome = run_mirror_sync(
            &db,
            &transport,
            "remote-library:example.test:9443",
            MirrorOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        );
        assert!(matches!(outcome, MirrorOutcome::Completed(c) if c.new_count == 1));

        // The local-only design is completely untouched: still there, same title, same
        // attachment content -- sync never deleted or altered it.
        let guard = db.lock().unwrap();
        let local_full = guard.get_diagram_full(local_entry_id).unwrap().unwrap();
        assert_eq!(local_full.title, "My Own Trichecker");
        assert_eq!(local_full.url, "local://my_trichecker.asc");
        assert_eq!(local_full.attached_files.len(), 1);
        assert_eq!(local_full.attached_files[0].content, b"a real user file");
        // And the remote design was actually added alongside it -- total count is both.
        assert_eq!(guard.get_total_count().unwrap(), 2);
        drop(guard);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_new_design_is_saved_with_its_attachment() {
        let (db, path) = temp_db();
        let mut record = design_record(1, "Round Brilliant", "https://example.test/1", "v1");
        record.attachments = vec![AttachedFileMeta {
            id: 100,
            name: "schedule.pdf".to_string(),
            url: "https://example.test/schedule.pdf".to_string(),
            size: 5,
        }];
        let summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
        let transport = FakeTransport::new(vec![summary], vec![record]).with_attachment(
            100,
            "schedule.pdf",
            vec![1, 2, 3, 4, 5],
        );

        let outcome = run_mirror_sync(
            &db,
            &transport,
            "remote-library:w",
            MirrorOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        );
        let MirrorOutcome::Completed(counts) = outcome else {
            panic!("expected Completed, got {outcome:?}");
        };
        assert_eq!(counts.new_count, 1);
        assert_eq!(counts.attachments_fetched, 1);
        assert_eq!(counts.attachment_bytes_fetched, 5);

        let guard = db.lock().unwrap();
        let full = guard
            .search_diagrams(
                "",
                "All",
                "All",
                &indicatrix_vault::model::filter::RangeFilter::default(),
            )
            .unwrap();
        assert_eq!(full.len(), 1);
        let entry_id = full[0].id;
        let record = guard.get_diagram_full(entry_id).unwrap().unwrap();
        assert_eq!(record.attached_files.len(), 1);
        assert_eq!(record.attached_files[0].content, vec![1, 2, 3, 4, 5]);
        drop(guard);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_second_sync_of_an_unchanged_catalogue_skips_every_design_and_makes_no_fetch_design_calls()
    {
        let (db, path) = temp_db();
        let summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
        let record = design_record(1, "Round Brilliant", "https://example.test/1", "v1");
        let transport = FakeTransport::new(vec![summary], vec![record]);

        let first = run_mirror_sync(
            &db,
            &transport,
            "remote-library:w",
            MirrorOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        );
        assert!(matches!(first, MirrorOutcome::Completed(c) if c.new_count == 1));
        assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 1);

        let second = run_mirror_sync(
            &db,
            &transport,
            "remote-library:w",
            MirrorOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        );
        let MirrorOutcome::Completed(counts) = second else {
            panic!("expected Completed, got {second:?}");
        };
        assert_eq!(counts.skipped_unchanged, 1);
        assert_eq!(counts.new_count, 0);
        assert_eq!(counts.updated_count, 0);
        // The whole point: a no-op resync makes exactly one more Search and ZERO
        // FetchDesign/FetchAttachment calls beyond the first sync's.
        assert_eq!(transport.search_calls.load(Ordering::Relaxed), 2);
        assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 1);
        assert_eq!(transport.fetch_attachment_calls.load(Ordering::Relaxed), 0);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_changed_design_is_refetched_and_updated_in_place() {
        let (db, path) = temp_db();
        let summary_v1 = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
        let record_v1 = design_record(1, "Round Brilliant", "https://example.test/1", "v1");
        let transport_v1 = FakeTransport::new(vec![summary_v1], vec![record_v1]);
        let _ = run_mirror_sync(
            &db,
            &transport_v1,
            "remote-library:w",
            MirrorOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        );

        // The remote design's L/W ratio changed -- both its summary and design hashes
        // move (see `design_summary`/`design_record`'s shared `content_hash` input).
        let summary_v2 =
            design_summary(1, "Round Brilliant", "https://example.test/1", "v2-updated");
        let record_v2 = design_record(1, "Round Brilliant", "https://example.test/1", "v2-updated");
        let transport_v2 = FakeTransport::new(vec![summary_v2], vec![record_v2]);

        let outcome = run_mirror_sync(
            &db,
            &transport_v2,
            "remote-library:w",
            MirrorOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        );
        let MirrorOutcome::Completed(counts) = outcome else {
            panic!("expected Completed, got {outcome:?}");
        };
        assert_eq!(counts.updated_count, 1);
        assert_eq!(counts.new_count, 0);

        let guard = db.lock().unwrap();
        let items = guard
            .search_diagrams(
                "",
                "All",
                "All",
                &indicatrix_vault::model::filter::RangeFilter::default(),
            )
            .unwrap();
        assert_eq!(
            items.len(),
            1,
            "must update the existing row, not add a second one"
        );
        assert_eq!(items[0].lw_ratio.as_deref(), Some("v2-updated"));
        drop(guard);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn cancelling_mid_sync_leaves_earlier_designs_committed_and_the_rest_untouched() {
        let (db, path) = temp_db();
        let summaries = vec![
            design_summary(1, "First", "https://example.test/1", "v1"),
            design_summary(2, "Second", "https://example.test/2", "v1"),
            design_summary(3, "Third", "https://example.test/3", "v1"),
        ];
        let designs = vec![
            design_record(1, "First", "https://example.test/1", "v1"),
            design_record(2, "Second", "https://example.test/2", "v1"),
            design_record(3, "Third", "https://example.test/3", "v1"),
        ];
        let transport = FakeTransport::new(summaries, designs);
        let cancel = AtomicBool::new(false);

        let mut processed_count = 0;
        let outcome = run_mirror_sync(
            &db,
            &transport,
            "remote-library:w",
            MirrorOptions::default(),
            &cancel,
            |progress| {
                processed_count = progress.processed;
                // Cancel right after the first design commits, before the second is
                // ever looked at.
                if progress.processed == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(processed_count, 1);
        let MirrorOutcome::Cancelled(counts) = outcome else {
            panic!("expected Cancelled, got {outcome:?}");
        };
        assert_eq!(counts.new_count, 1);

        let guard = db.lock().unwrap();
        // Exactly one design landed -- the second and third were never even fetched
        // (FetchDesign was called exactly once), so nothing about them exists locally,
        // not even a half-written row.
        assert_eq!(guard.get_total_count().unwrap(), 1);
        assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 1);
        assert_eq!(*transport.fetched_entry_ids.borrow(), vec![1]);
        drop(guard);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_oversized_attachment_is_skipped_but_the_design_is_still_saved() {
        let (db, path) = temp_db();
        let mut record = design_record(1, "Round Brilliant", "https://example.test/1", "v1");
        record.attachments = vec![
            AttachedFileMeta {
                id: 100,
                name: "huge.pdf".to_string(),
                url: String::new(),
                size: 1000,
            },
            AttachedFileMeta {
                id: 101,
                name: "small.pdf".to_string(),
                url: String::new(),
                size: 5,
            },
        ];
        let summary = design_summary(1, "Round Brilliant", "https://example.test/1", "v1");
        let transport = FakeTransport::new(vec![summary], vec![record])
            .with_attachment(100, "huge.pdf", vec![0u8; 1000])
            .with_attachment(101, "small.pdf", vec![9, 9, 9, 9, 9]);

        let options = MirrorOptions {
            max_attachment_bytes: 500,
        };
        let outcome = run_mirror_sync(
            &db,
            &transport,
            "remote-library:w",
            options,
            &AtomicBool::new(false),
            |_| {},
        );
        let MirrorOutcome::Completed(counts) = outcome else {
            panic!("expected Completed, got {outcome:?}");
        };
        assert_eq!(counts.new_count, 1, "the design itself must still be saved");
        assert_eq!(counts.attachments_fetched, 1);
        assert_eq!(counts.attachments_skipped_too_large, 1);
        assert_eq!(
            transport.fetch_attachment_calls.load(Ordering::Relaxed),
            1,
            "the oversized attachment's bytes must never even be requested"
        );

        let guard = db.lock().unwrap();
        let items = guard
            .search_diagrams(
                "",
                "All",
                "All",
                &indicatrix_vault::model::filter::RangeFilter::default(),
            )
            .unwrap();
        let full = guard.get_diagram_full(items[0].id).unwrap().unwrap();
        assert_eq!(full.attached_files.len(), 1);
        assert_eq!(full.attached_files[0].name, "small.pdf");
        drop(guard);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_failed_fetch_is_counted_and_never_marked_synced() {
        let (db, path) = temp_db();
        // The summary claims entry_id 1, but the transport's `designs` map has nothing
        // for it -- simulating a design that vanished between Search and FetchDesign.
        let summary = design_summary(1, "Ghost", "https://example.test/1", "v1");
        let transport = FakeTransport::new(vec![summary], Vec::new());

        let outcome = run_mirror_sync(
            &db,
            &transport,
            "remote-library:w",
            MirrorOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        );
        let MirrorOutcome::Completed(counts) = outcome else {
            panic!("expected Completed, got {outcome:?}");
        };
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.new_count, 0);

        let guard = db.lock().unwrap();
        assert_eq!(guard.get_total_count().unwrap(), 0);
        assert_eq!(
            guard.get_mirror_state("https://example.test/1").unwrap(),
            None
        );
        drop(guard);
        std::fs::remove_file(&path).ok();
    }

    /// The scenario the held-connection task exists to handle: a mid-sync connection
    /// drop. In production, `LibrarySession` (see `bridge::library::client`) hides a
    /// RECOVERABLE drop entirely -- it reconnects and retries the same request, so
    /// `run_mirror_sync` never even sees a failure (see
    /// `library_client::tests::request_with_reconnect_reconnects_once_after_a_dead_connection_and_succeeds`
    /// for that half, proven with no live socket). This test covers the other half: what
    /// `run_mirror_sync` itself does when a design's connection drop could NOT be
    /// recovered (the reconnect attempt also failed) -- exactly what
    /// `FakeTransport::with_fetch_design_failure` scripts. The sync must not abort: it
    /// counts that one design as failed, leaves it exactly as it was locally (so the next
    /// sync retries it, same as `a_failed_fetch_is_counted_and_never_marked_synced`), and
    /// keeps going to fetch and save every OTHER design in the catalogue.
    #[test]
    fn a_design_whose_connection_could_not_be_recovered_is_skipped_but_the_rest_of_the_sync_completes()
     {
        let (db, path) = temp_db();
        let summaries = vec![
            design_summary(1, "First", "https://example.test/1", "v1"),
            design_summary(2, "Second", "https://example.test/2", "v1"),
            design_summary(3, "Third", "https://example.test/3", "v1"),
        ];
        let designs = vec![
            design_record(1, "First", "https://example.test/1", "v1"),
            design_record(2, "Second", "https://example.test/2", "v1"),
            design_record(3, "Third", "https://example.test/3", "v1"),
        ];
        let transport = FakeTransport::new(summaries, designs).with_fetch_design_failure(2);

        let mut processed_count = 0;
        let outcome = run_mirror_sync(
            &db,
            &transport,
            "remote-library:w",
            MirrorOptions::default(),
            &AtomicBool::new(false),
            |progress| processed_count = progress.processed,
        );
        let MirrorOutcome::Completed(counts) = outcome else {
            panic!(
                "an unrecoverable connection drop for ONE design must not abort the whole \
                 sync, got {outcome:?}"
            );
        };
        assert_eq!(
            processed_count, 3,
            "every design was still examined, including after the drop"
        );
        assert_eq!(counts.total_found, 3);
        assert_eq!(
            counts.new_count, 2,
            "designs 1 and 3 still land despite design 2's failure"
        );
        assert_eq!(counts.failed, 1);

        let guard = db.lock().unwrap();
        assert_eq!(guard.get_total_count().unwrap(), 2);
        let items = guard
            .search_diagrams(
                "",
                "All",
                "All",
                &indicatrix_vault::model::filter::RangeFilter::default(),
            )
            .unwrap();
        let titles: std::collections::HashSet<&str> =
            items.iter().map(|i| i.title.as_str()).collect();
        assert!(titles.contains("First"));
        assert!(titles.contains("Third"));
        assert!(
            !titles.contains("Second"),
            "the design whose connection could not be recovered must never be half-saved, \
             titles were: {titles:?}"
        );
        assert_eq!(
            guard.get_mirror_state("https://example.test/2").unwrap(),
            None,
            "never marked synced, so the next sync retries it -- same as any other failed fetch"
        );
        drop(guard);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mirror_source_id_is_distinct_per_worker_address() {
        let a = WorkerSettings {
            address: "10.0.0.5:9443".to_string(),
            ..WorkerSettings::default()
        };
        let b = WorkerSettings {
            address: "10.0.0.6:9443".to_string(),
            ..WorkerSettings::default()
        };
        assert_ne!(mirror_source_id(&a), mirror_source_id(&b));
    }

    #[test]
    fn a_multi_page_mirror_reaches_designs_beyond_the_first_page() {
        let (db, path) = temp_db();
        let entries: Vec<(i64, &str)> = vec![
            (1, "First"),
            (2, "Second"),
            (3, "Third"),
            (4, "Fourth"),
            (5, "Fifth"),
        ];
        let summaries: Vec<DesignSummary> = entries
            .iter()
            .map(|(id, title)| {
                design_summary(*id, title, &format!("https://example.test/{id}"), "v1")
            })
            .collect();
        let designs: Vec<DesignRecord> = entries
            .iter()
            .map(|(id, title)| {
                design_record(*id, title, &format!("https://example.test/{id}"), "v1")
            })
            .collect();
        // 5 designs at 2 rows per SearchPage reply: pages of [1,2], [3,4], [5] -- the
        // last page is short, so exactly 3 round trips, not 5 and not 1.
        let transport = FakeTransport::new(summaries, designs).with_page_size(2);

        let outcome = run_mirror_sync(
            &db,
            &transport,
            "remote-library:w",
            MirrorOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        );
        let MirrorOutcome::Completed(counts) = outcome else {
            panic!("expected Completed, got {outcome:?}");
        };
        assert_eq!(
            counts.total_found, 5,
            "every design across every page must be counted, not just the first page's"
        );
        assert_eq!(counts.new_count, 5);
        assert_eq!(transport.search_calls.load(Ordering::Relaxed), 3);
        assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 5);

        let guard = db.lock().unwrap();
        assert_eq!(guard.get_total_count().unwrap(), 5);
        let items = guard
            .search_diagrams(
                "",
                "All",
                "All",
                &indicatrix_vault::model::filter::RangeFilter::default(),
            )
            .unwrap();
        let titles: std::collections::HashSet<&str> =
            items.iter().map(|i| i.title.as_str()).collect();
        // The whole point: "Fourth" and "Fifth" sat past the first page and must have
        // actually landed locally, not just been counted.
        assert!(titles.contains("Fourth"), "titles were: {titles:?}");
        assert!(titles.contains("Fifth"), "titles were: {titles:?}");
        drop(guard);
        std::fs::remove_file(&path).ok();
    }
}
