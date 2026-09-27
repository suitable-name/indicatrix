//! The coordinator's registry of joined worker connections: id assignment,
//! each connection's reported [`RenderCapability`], liveness, check-out/check-in for
//! request execution, a capacity snapshot, and change notifications. Deliberately free
//! of request-execution logic -- that is C-coord's job, built on [`Registry::checkout`].
//!
//! # One entry per connection
//!
//! A worker started with `join --slots K` opens K connections; each registers
//! separately and gets its own `worker_id`. Every connection serves one request stream
//! at a time, so an entry is either idle (in the registry), checked out (owned by a
//! [`WorkerHandle`]), or being pinged (owned by the liveness thread).
//!
//! # Liveness (see [`LivenessConfig`])
//!
//! Every idle connection that has seen no traffic for [`LivenessConfig::ping_interval`]
//! (default 10 s) gets a `PING`; it must answer `PONG` before
//! [`LivenessConfig::dead_after`] (default 30 s) has passed since its last traffic, or
//! it is dropped (connection closed, entry removed, subscribers notified). Traffic means
//! a `PONG`, a registration, or a check-in (a returned [`WorkerHandle`] counts: the job
//! that held it saw the stream's own events). A checked-out connection is never pinged;
//! the job holding it is responsible for noticing a dead stream and calling
//! [`WorkerHandle::discard`].

use super::liveness;
use crate::serve::ConnectionSlot;
use indicatrix_net::messages::{Backend, PayloadEncoding, RenderCapability};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// A joined worker's connection as the registry stores it.
///
/// A blocking byte stream the coordinator writes `ClientMessage`s to and reads
/// `StreamEvent`s from, with socket timeouts it can set. Implemented for the worker
/// port's TLS stream and, for tests, a plain `TcpStream`.
pub trait WorkerConn: Read + Write + Send {
    /// Sets the underlying socket's read and write timeouts (`None` = blocking).
    ///
    /// # Errors
    ///
    /// Whatever the socket's own timeout setters return.
    fn set_timeouts(
        &mut self,
        read: Option<Duration>,
        write: Option<Duration>,
    ) -> std::io::Result<()>;
}

impl WorkerConn for TcpStream {
    fn set_timeouts(
        &mut self,
        read: Option<Duration>,
        write: Option<Duration>,
    ) -> std::io::Result<()> {
        self.set_read_timeout(read)?;
        self.set_write_timeout(write)
    }
}

impl WorkerConn for crate::serve::TlsStream {
    fn set_timeouts(
        &mut self,
        read: Option<Duration>,
        write: Option<Duration>,
    ) -> std::io::Result<()> {
        self.sock.set_read_timeout(read)?;
        self.sock.set_write_timeout(write)
    }
}

/// When the liveness thread pings idle connections and when it gives up on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivenessConfig {
    /// Ping an idle connection after this long without traffic (default 10 s).
    pub ping_interval: Duration,
    /// Drop a connection after this long without traffic (default 30 s).
    pub dead_after: Duration,
    /// How often the liveness thread looks for due pings (default 1 s).
    pub tick: Duration,
}

impl Default for LivenessConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(10),
            dead_after: Duration::from_secs(30),
            tick: Duration::from_secs(1),
        }
    }
}

/// What the registry knows about one joined worker connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerInfo {
    /// The coordinator-assigned id (unique per coordinator process; also sent to the
    /// worker in `WELCOME.registration`).
    pub worker_id: u32,
    /// What the worker reported in its `HELLO`.
    pub capability: RenderCapability,
    /// Where the connection came from.
    pub peer: Option<SocketAddr>,
    /// The worker certificate's label (`<label>` of its `worker:<label>` Common Name,
    /// see `crate::pki::role`): the one identity that survives reconnects, shared by
    /// every `join --slots K` connection of that worker. `None` without TLS.
    pub label: Option<String>,
    /// The payload encoding negotiated for this connection: the worker encodes its
    /// `FRAME`s with it (each header still names its own encoding).
    pub payload_encoding: PayloadEncoding,
}

/// A capacity snapshot over every registered connection (idle, busy or being pinged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capacity {
    /// Registered worker connections.
    pub workers: u32,
    /// CPU tracer threads across them (`Backend::Cpu`, plus nested coordinators' counts).
    pub threads: u32,
    /// GPU adapters across them (`Backend::Gpu`, plus nested coordinators' counts).
    pub gpus: u32,
    /// The largest `max_pixels` any of them accepts (0 with no workers).
    pub max_pixels: u32,
    /// How many of them render HDR environments (`RenderCapability::hdr`: their asset
    /// cache opened) -- only these take part in HDR jobs.
    pub hdr_workers: u32,
}

impl Capacity {
    fn add(&mut self, capability: &RenderCapability) {
        self.workers += 1;
        self.hdr_workers += u32::from(capability.hdr);
        match &capability.backend {
            Backend::Cpu { threads } => self.threads += threads,
            Backend::Gpu { .. } => self.gpus += 1,
            Backend::Coordinator { threads, gpus, .. } => {
                self.threads += threads;
                self.gpus += gpus;
            }
        }
        self.max_pixels = self.max_pixels.max(capability.max_pixels);
    }
}

/// Where one entry's connection currently is.
enum Slot {
    Idle(Box<dyn WorkerConn>),
    CheckedOut,
    Pinging,
}

struct Entry {
    info: WorkerInfo,
    slot: Slot,
    last_traffic: Instant,
    /// The worker port's `--max-connections` slot, released when the entry goes.
    _connection_slot: Option<ConnectionSlot>,
}

/// The joined-worker registry. Shared as `Arc<Registry>` by the worker listener (which
/// registers), the liveness thread, the viewer connections (capacity for `WELCOME`) and
/// request execution (check-out).
pub struct Registry {
    entries: Mutex<BTreeMap<u32, Entry>>,
    subscribers: Mutex<Vec<mpsc::Sender<Capacity>>>,
    next_id: AtomicU32,
    next_nonce: AtomicU64,
    liveness: LivenessConfig,
    /// `ASSET` transfers (HDR maps) sent to joined workers so far (diagnostics).
    assets_forwarded: AtomicU64,
}

impl Registry {
    /// An empty registry with the given liveness schedule. Call
    /// [`Self::spawn_liveness`] to start pinging.
    #[must_use]
    pub fn new(liveness: LivenessConfig) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(BTreeMap::new()),
            subscribers: Mutex::new(Vec::new()),
            next_id: AtomicU32::new(1),
            next_nonce: AtomicU64::new(1),
            liveness,
            assets_forwarded: AtomicU64::new(0),
        })
    }

    /// How many `ASSET` transfers (HDR maps answering a worker's `NEED_ASSET`) the
    /// coordinator has sent to joined workers so far.
    #[must_use]
    pub fn assets_forwarded(&self) -> u64 {
        self.assets_forwarded.load(Ordering::Relaxed)
    }

    /// Counts `count` more `ASSET` transfers to joined workers.
    pub(super) fn note_assets_forwarded(&self, count: u32) {
        self.assets_forwarded
            .fetch_add(u64::from(count), Ordering::Relaxed);
    }

    /// Starts the liveness thread (see the module doc comment). It holds only a `Weak`
    /// reference and exits once the registry is dropped.
    pub fn spawn_liveness(this: &Arc<Self>) {
        liveness::spawn(Arc::downgrade(this));
    }

    /// The liveness schedule this registry was built with.
    #[must_use]
    pub const fn liveness(&self) -> LivenessConfig {
        self.liveness
    }

    /// Reserves the next `worker_id` -- before `WELCOME` is written, so the id can go
    /// out in `WELCOME.registration`; [`Self::insert`] then registers the connection.
    pub fn allocate_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// A fresh `PING` nonce.
    pub(super) fn next_nonce(&self) -> u64 {
        self.next_nonce.fetch_add(1, Ordering::Relaxed)
    }

    /// Registers an idle, already-welcomed connection under `info.worker_id` and
    /// notifies subscribers. `connection_slot` (the worker port's cap) is released when
    /// the entry is removed.
    pub fn insert(
        &self,
        info: WorkerInfo,
        conn: Box<dyn WorkerConn>,
        connection_slot: Option<ConnectionSlot>,
    ) {
        let id = info.worker_id;
        self.lock_entries().insert(
            id,
            Entry {
                info,
                slot: Slot::Idle(conn),
                last_traffic: Instant::now(),
                _connection_slot: connection_slot,
            },
        );
        self.notify();
    }

    /// The current capacity snapshot (every registered connection counts, busy or not).
    #[must_use]
    pub fn capacity(&self) -> Capacity {
        let mut capacity = Capacity::default();
        for entry in self.lock_entries().values() {
            capacity.add(&entry.info.capability);
        }
        capacity
    }

    /// Every registered connection's info, with whether it is idle right now.
    #[must_use]
    pub fn workers(&self) -> Vec<(WorkerInfo, bool)> {
        self.lock_entries()
            .values()
            .map(|e| (e.info.clone(), matches!(e.slot, Slot::Idle(_))))
            .collect()
    }

    /// A channel that receives the new [`Capacity`] after every registration or removal
    /// (for the viewer side's `CapabilityChanged`). Dropping the receiver unsubscribes.
    #[must_use]
    pub fn subscribe(&self) -> mpsc::Receiver<Capacity> {
        let (tx, rx) = mpsc::channel();
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    /// Borrows the lowest-id IDLE connection whose info satisfies `filter`, or `None`.
    ///
    /// The returned [`WorkerHandle`] owns the connection exclusively (one request stream
    /// at a time) and gives it back on drop; call [`WorkerHandle::discard`] instead when
    /// the stream broke. A connection being pinged at that instant is skipped, like a
    /// busy one -- retry shortly.
    pub fn checkout(
        this: &Arc<Self>,
        filter: impl Fn(&WorkerInfo) -> bool,
    ) -> Option<WorkerHandle> {
        let (info, conn) = this.lock_entries().values_mut().find_map(|entry| {
            if !filter(&entry.info) {
                return None;
            }
            match std::mem::replace(&mut entry.slot, Slot::CheckedOut) {
                Slot::Idle(conn) => Some((entry.info.clone(), conn)),
                busy => {
                    entry.slot = busy;
                    None
                }
            }
        })?;
        Some(WorkerHandle {
            registry: Arc::clone(this),
            info,
            conn: Some(conn),
        })
    }

    /// Gives a checked-out or pinged connection back: idle again, traffic seen now.
    pub(super) fn check_in(&self, worker_id: u32, conn: Box<dyn WorkerConn>) {
        if let Some(entry) = self.lock_entries().get_mut(&worker_id) {
            entry.slot = Slot::Idle(conn);
            entry.last_traffic = Instant::now();
        }
    }

    /// Unregisters `worker_id` (its connection, if held here, is closed by dropping it)
    /// and notifies subscribers.
    pub fn remove(&self, worker_id: u32, why: &str) {
        let removed = self.lock_entries().remove(&worker_id);
        if let Some(entry) = removed {
            tracing::warn!(
                "coordinator: worker #{worker_id} ({:?}) dropped: {why}",
                entry.info.peer
            );
            drop(entry);
            self.notify();
        }
    }

    /// Takes every idle connection whose last traffic is at least `ping_interval` old,
    /// marking it as being pinged. Returns `(worker_id, connection, drop deadline)`.
    pub(super) fn take_due_pings(&self, now: Instant) -> Vec<(u32, Box<dyn WorkerConn>, Instant)> {
        let mut due = Vec::new();
        for (id, entry) in self.lock_entries().iter_mut() {
            if !matches!(entry.slot, Slot::Idle(_))
                || now.duration_since(entry.last_traffic) < self.liveness.ping_interval
            {
                continue;
            }
            if let Slot::Idle(conn) = std::mem::replace(&mut entry.slot, Slot::Pinging) {
                due.push((*id, conn, entry.last_traffic + self.liveness.dead_after));
            }
        }
        due
    }

    fn lock_entries(&self) -> MutexGuard<'_, BTreeMap<u32, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn notify(&self) {
        let capacity = self.capacity();
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|tx| tx.send(capacity).is_ok());
    }
}

/// Exclusive use of one joined worker connection for one request stream, from
/// [`Registry::checkout`]. Dropping it checks the connection back in (idle again);
/// [`Self::discard`] unregisters it instead.
pub struct WorkerHandle {
    registry: Arc<Registry>,
    info: WorkerInfo,
    conn: Option<Box<dyn WorkerConn>>,
}

impl WorkerHandle {
    /// The worker's registry info (id, capability, peer, negotiated encoding).
    #[must_use]
    pub const fn info(&self) -> &WorkerInfo {
        &self.info
    }

    /// The connection: write `ClientMessage`s, read `StreamEvent`s.
    ///
    /// Leave it with no partially read or written frame before giving it back.
    ///
    /// # Panics
    ///
    /// Never in practice: a handle holds its connection until dropped or discarded.
    pub fn stream(&mut self) -> &mut dyn WorkerConn {
        self.conn
            .as_deref_mut()
            .expect("a WorkerHandle holds its connection until dropped or discarded")
    }

    /// The stream broke (I/O error, protocol violation, deadline): close the connection
    /// and unregister the worker -- `join` reconnects on its own.
    pub fn discard(mut self, why: &str) {
        drop(self.conn.take());
        self.registry.remove(self.info.worker_id, why);
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.registry.check_in(self.info.worker_id, conn);
        }
    }
}
