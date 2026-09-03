//! Warm vms, built ahead of the request or the object that will use
//! them. Building a vm means creating a Luau state, registering the
//! platform's extensions and running the revision's entry point, which
//! is the largest fixed cost on a request and on an object's first call.
//! The pool pays it in the background: a take hands over a vm that has
//! run its entry point and served nothing, and starts a refill so the
//! next take finds one too.
//!
//! What a pooled vm is: fresh. It has executed the top level of the
//! entry point once, the way every vm does, and nothing else; the
//! request or object that takes it is its first and only use, so the
//! one-environment-per-request rule holds exactly. What it is not: a
//! vm that ran the top level at the instant of the take. A top-level
//! `os.time()` or a `secret` declaration read their value when the vm
//! was built, at most [`VmPool::TTL`] earlier; a warm vm older than that
//! is discarded rather than handed out.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use actias_worker_core::runtime::ActiasRuntime;

/// How a vm is built when the pool has none; the same closure refills.
pub type VmBuild = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<ActiasRuntime, String>> + Send>> + Send + Sync,
>;

/// Which construction a pooled vm had. A request vm carries the wall
/// backstop; an object vm has none, since its budget is per call.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Flavor {
    Request,
    Object,
}

/// The pool's key: one revision, one construction.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct VmKey {
    pub revision_id: String,
    pub flavor: Flavor,
}

struct Warm {
    vm: ActiasRuntime,
    built_at: std::time::Instant,
}

#[derive(Default)]
struct Shelf {
    warm: VecDeque<Warm>,
    /// A refill in progress; a second take does not start another.
    refilling: bool,
    /// When the key was last taken; a cold key stops refilling.
    last_take: Option<std::time::Instant>,
}

/// Node-wide counters for `/_metrics`.
#[derive(Default)]
pub struct VmPoolGauges {
    /// Takes served from a warm vm, and takes that had to build inline.
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub builds: AtomicU64,
    pub build_ms_total: AtomicU64,
    /// Warm vms thrown away past their ttl.
    pub expired: AtomicU64,
}

pub struct VmPool {
    shelves: Mutex<HashMap<VmKey, Shelf>>,
    /// Warm vms kept per key; 0 disables the pool.
    target: usize,
    pub gauges: Arc<VmPoolGauges>,
}

impl VmPool {
    /// Longest a warm vm waits to be taken. Bounds how stale a
    /// top-level value can be, and how much memory idle revisions hold.
    pub const TTL: std::time::Duration = std::time::Duration::from_secs(30);

    pub fn new(target: usize) -> Arc<Self> {
        let pool = Arc::new(Self {
            shelves: Mutex::new(HashMap::new()),
            target,
            gauges: Arc::default(),
        });
        // Warm vms past their ttl and shelves nobody has taken from
        // leave on their own; a take is not the only way out.
        let sweeper = pool.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Self::TTL / 2).await;
                sweeper.sweep();
            }
        });
        pool
    }

    /// Drops warm vms past the ttl and shelves that have been cold for
    /// a ttl, so an idle or replaced revision holds nothing.
    pub fn sweep(&self) {
        let mut shelves = self.shelves.lock().expect("no poisoned lock");
        let mut expired = 0;
        shelves.retain(|_, shelf| {
            let before = shelf.warm.len();
            shelf
                .warm
                .retain(|warm| warm.built_at.elapsed() < Self::TTL);
            expired += before - shelf.warm.len();
            let cold = shelf.last_take.is_none_or(|at| at.elapsed() >= Self::TTL);
            shelf.refilling || !(cold && shelf.warm.is_empty())
        });
        self.gauges
            .expired
            .fetch_add(expired as u64, Ordering::Relaxed);
    }

    /// A vm for `key`: warm when one is on the shelf, built inline
    /// otherwise. Either way a refill starts so the shelf holds
    /// `target` again.
    ///
    /// # Errors
    /// Returns the build's failure when the vm had to be built inline.
    pub async fn take(
        self: &Arc<Self>,
        key: VmKey,
        build: VmBuild,
    ) -> Result<ActiasRuntime, String> {
        // A live session's revision has no id; pooling under an empty key
        // would hand one tenant's vm to another. Such vms are built for
        // the take, every time.
        if self.target == 0 || key.revision_id.is_empty() {
            return self.build_counted(&build).await;
        }
        let warm = {
            let mut shelves = self.shelves.lock().expect("no poisoned lock");
            let shelf = shelves.entry(key.clone()).or_default();
            shelf.last_take = Some(std::time::Instant::now());
            loop {
                match shelf.warm.pop_front() {
                    Some(warm) if warm.built_at.elapsed() < Self::TTL => break Some(warm.vm),
                    Some(_) => {
                        self.gauges.expired.fetch_add(1, Ordering::Relaxed);
                    }
                    None => break None,
                }
            }
        };
        self.refill(key, build.clone());
        match warm {
            Some(vm) => {
                self.gauges.hits.fetch_add(1, Ordering::Relaxed);
                Ok(vm)
            }
            None => {
                self.gauges.misses.fetch_add(1, Ordering::Relaxed);
                self.build_counted(&build).await
            }
        }
    }

    async fn build_counted(&self, build: &VmBuild) -> Result<ActiasRuntime, String> {
        let started = std::time::Instant::now();
        let vm = build().await;
        self.gauges.builds.fetch_add(1, Ordering::Relaxed);
        self.gauges
            .build_ms_total
            .fetch_add(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        vm
    }

    /// Builds vms for `key` until the shelf holds `target`, one at a
    /// time, off the caller's path. A key nobody has taken for a ttl
    /// stops refilling, so an idle revision does not hold vms forever.
    fn refill(self: &Arc<Self>, key: VmKey, build: VmBuild) {
        {
            let mut shelves = self.shelves.lock().expect("no poisoned lock");
            let shelf = shelves.entry(key.clone()).or_default();
            if shelf.refilling || shelf.warm.len() >= self.target {
                return;
            }
            shelf.refilling = true;
        }
        let pool = self.clone();
        tokio::spawn(async move {
            loop {
                let wanted = {
                    let shelves = pool.shelves.lock().expect("no poisoned lock");
                    let shelf = shelves.get(&key);
                    let hot = shelf
                        .and_then(|s| s.last_take)
                        .is_some_and(|at| at.elapsed() < Self::TTL);
                    let live = shelf.map_or(0, |s| {
                        s.warm
                            .iter()
                            .filter(|w| w.built_at.elapsed() < Self::TTL)
                            .count()
                    });
                    hot && live < pool.target
                };
                if !wanted {
                    break;
                }
                match pool.build_counted(&build).await {
                    Ok(vm) => {
                        let mut shelves = pool.shelves.lock().expect("no poisoned lock");
                        shelves
                            .entry(key.clone())
                            .or_default()
                            .warm
                            .push_back(Warm {
                                vm,
                                built_at: std::time::Instant::now(),
                            });
                    }
                    Err(error) => {
                        // A revision that fails to build fails every take
                        // inline too, where the caller sees the error; the
                        // shelf just stays empty.
                        actias_common::tracing::debug!(%error, "vm warm-up failed");
                        break;
                    }
                }
            }
            let mut shelves = pool.shelves.lock().expect("no poisoned lock");
            if let Some(shelf) = shelves.get_mut(&key) {
                shelf.refilling = false;
            }
        });
    }

    /// Warm vms on every shelf, for the gauge.
    pub fn warm_count(&self) -> usize {
        self.shelves
            .lock()
            .expect("no poisoned lock")
            .values()
            .map(|s| s.warm.len())
            .sum()
    }
}
