//! A scripted, in-memory [`LibraryTransport`] and the design/summary/database
//! fixtures the mirror-sync tests build against -- no socket, no TLS, no real
//! `indicatrix-worker`.

use crate::bridge::library::{client as library_client, mirror::options::LibraryTransport};
use indicatrix_net::library::{DesignRecord, DesignSummary, LibraryRequest, LibraryResponse};
use indicatrix_vault::db::sqlite::Database;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

/// A scripted, in-memory [`LibraryTransport`] -- no socket, no TLS, no real
/// `indicatrix-worker`. Counts how many times each request kind is made so tests can
/// pin the "no-op second sync costs nothing but one `SearchPage` per page" claim
/// precisely, and pages `search_result` at [`Self::page_size`] rows per
/// `SearchPage` reply -- a scripted stand-in for `apps/indicatrix-worker`'s own
/// `SEARCH_RESULT_CAP`-per-page behavior, small enough in tests to exercise a real
/// multi-page walk without seeding a thousand-plus rows.
pub(super) struct FakeTransport {
    search_result: Vec<DesignSummary>,
    /// How many `search_result` rows one `SearchPage` reply hands back. Defaults
    /// (via [`Self::new`]) to `usize::MAX` -- one page always covers the whole
    /// `search_result` list -- so every pre-existing single-page test keeps making
    /// exactly one `SearchPage` call, unchanged; [`Self::with_page_size`] shrinks it
    /// for a test that specifically wants to exercise pagination.
    page_size: usize,
    designs: HashMap<i64, DesignRecord>,
    attachments: HashMap<i64, (String, Vec<u8>)>,
    pub(super) search_calls: AtomicUsize,
    pub(super) fetch_design_calls: AtomicUsize,
    pub(super) fetch_attachment_calls: AtomicUsize,
    /// Set of entry ids [`LibraryRequest::FetchDesign`] was actually called for --
    /// lets a test assert WHICH designs were (not) re-fetched, not just a count.
    pub(super) fetched_entry_ids: RefCell<Vec<i64>>,
    /// When `Some(id)`, [`LibraryRequest::FetchDesign`] for that entry id returns a
    /// transport-level [`library_client::LibraryClientError::Client`] error instead
    /// of a normal reply -- standing in for a `LibrarySession` whose held connection
    /// dropped mid-sync AND whose own reconnect attempt then also failed (see
    /// `library_client::request_with_reconnect`'s "propagates the error when
    /// reconnecting also fails" case): a genuine transport failure reaching
    /// `super::pass::run_mirror_sync`, not a logical [`LibraryResponse::NotFound`].
    /// Set via [`Self::with_fetch_design_failure`].
    fail_fetch_design_for: Option<i64>,
}

impl FakeTransport {
    pub(super) fn new(summaries: Vec<DesignSummary>, designs: Vec<DesignRecord>) -> Self {
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

    pub(super) fn with_attachment(mut self, id: i64, name: &str, content: Vec<u8>) -> Self {
        self.attachments.insert(id, (name.to_string(), content));
        self
    }

    /// Shrinks how many rows one `SearchPage` reply hands back, forcing
    /// `super::super::catalogue::enumerate_remote_catalogue` into more than one round
    /// trip for a `search_result` longer than `page_size`. See [`Self::page_size`]'s
    /// own doc comment.
    pub(super) fn with_page_size(mut self, page_size: usize) -> Self {
        self.page_size = page_size;
        self
    }

    /// Makes [`LibraryRequest::FetchDesign`] for `entry_id` fail with a
    /// transport-level error -- see [`Self::fail_fetch_design_for`]'s own doc
    /// comment for what this simulates.
    pub(super) fn with_fetch_design_failure(mut self, entry_id: i64) -> Self {
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
/// PRE-POPULATED database and fail spuriously against stale data, with the
/// assertions themselves innocent.
///
/// Deleting any pre-existing file first removes the failure mode entirely, rather
/// than merely making a collision less likely.
pub(super) fn temp_db() -> (Arc<Mutex<Database>>, std::path::PathBuf) {
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

pub(super) fn design_record(
    entry_id: i64,
    title: &str,
    url: &str,
    extra_field: &str,
) -> DesignRecord {
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
        angle_settings: vec![indicatrix_net::library::AngleSettingWire {
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

pub(super) fn design_summary(
    entry_id: i64,
    title: &str,
    url: &str,
    extra_field: &str,
) -> DesignSummary {
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
