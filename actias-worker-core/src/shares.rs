//! Fair share of a node's bounds by scope.
//!
//! Every node-wide bound (requests in flight, blocking work, ship
//! flights, open connections, resident objects, directory queries) is a
//! [`Pool`] of permits shared among the scopes using it under one rule:
//! a scope may hold `max(floor, total / active_scopes)`, where the active
//! scopes are those holding or asking for a permit within the last
//! second. A scope alone on a node gets the node; contending scopes
//! split it; the floor keeps a small scope from being starved to zero
//! by a large one. Nothing is reserved for a scope that is not there,
//! which is what makes a single-tenant node and a shared one the same
//! code with the same numbers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a scope stays active after its last permit or refusal.
const ACTIVE_WINDOW: Duration = Duration::from_secs(1);

/// What a refused caller is told to wait; the share is recomputed every
/// window, so a second is when the answer can change.
const RETRY_AFTER: Duration = Duration::from_secs(1);

/// The bounds a node splits by scope. Zero means unbounded for that
/// pool, the drills' posture.
#[derive(Clone, Copy, Debug)]
pub struct ScopeLimits {
    pub requests: usize,
    pub blocking: usize,
    pub ships: usize,
    pub connections: usize,
    pub residents: usize,
    pub directory_queries: usize,
    /// The least share any scope gets, as a fraction of each pool.
    pub floor: f64,
}

/// One node's pools, one per bound; every consumer keys by scope.
pub struct ScopeShares {
    pub requests: Arc<Pool>,
    pub blocking: Arc<Pool>,
    pub ships: Arc<Pool>,
    pub connections: Arc<Pool>,
    pub residents: Arc<Pool>,
    pub directory_queries: Arc<Pool>,
}

impl ScopeShares {
    pub fn new(limits: ScopeLimits) -> Arc<Self> {
        Arc::new(Self {
            requests: Pool::new("requests", limits.requests, limits.floor),
            blocking: Pool::new("blocking", limits.blocking, limits.floor),
            ships: Pool::new("ships", limits.ships, limits.floor),
            connections: Pool::new("connections", limits.connections, limits.floor),
            residents: Pool::new("residents", limits.residents, limits.floor),
            directory_queries: Pool::new(
                "directory_queries",
                limits.directory_queries,
                limits.floor,
            ),
        })
    }

    /// Every pool unbounded; tests and embedded runs.
    pub fn unbounded() -> Arc<Self> {
        Self::new(ScopeLimits {
            requests: 0,
            blocking: 0,
            ships: 0,
            connections: 0,
            residents: 0,
            directory_queries: 0,
            floor: 0.0,
        })
    }

    /// The pools in exposition order.
    pub fn pools(&self) -> [&Arc<Pool>; 6] {
        [
            &self.requests,
            &self.blocking,
            &self.ships,
            &self.connections,
            &self.residents,
            &self.directory_queries,
        ]
    }
}

/// Counters a pool keeps for `/_metrics`.
#[derive(Default)]
pub struct PoolGauges {
    pub granted: AtomicU64,
    /// Answered "over your share" at once, by [`Pool::try_acquire`].
    pub refused: AtomicU64,
    /// Permits that had to wait, and for how long, by [`Pool::acquire`].
    pub waited: AtomicU64,
    pub wait_ms_total: AtomicU64,
}

/// One bound, split by scope under the fair-share rule.
pub struct Pool {
    name: &'static str,
    /// Permits in all; 0 is unbounded.
    total: usize,
    /// The least any active scope may hold.
    floor: usize,
    inner: Mutex<Inner>,
    notify: tokio::sync::Notify,
    pub gauges: PoolGauges,
}

#[derive(Default)]
struct Inner {
    in_use: usize,
    scopes: HashMap<String, ScopeUse>,
}

struct ScopeUse {
    in_use: usize,
    last_seen: Instant,
}

/// What a scope holds while it holds it; dropping it releases the
/// permit and wakes whoever waits.
pub struct Permit {
    pool: Arc<Pool>,
    scope: String,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.pool.release(&self.scope);
    }
}

impl std::fmt::Debug for Permit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Permit")
            .field("pool", &self.pool.name)
            .field("scope", &self.scope)
            .finish()
    }
}

/// A scope over its share, answered rather than queued.
#[derive(Debug)]
pub struct Refused {
    pub pool: &'static str,
    pub retry_after: Duration,
}

/// One scrape of a pool.
pub struct PoolSnapshot {
    pub name: &'static str,
    pub total: usize,
    pub in_use: usize,
    pub active_scopes: usize,
    pub share: usize,
}

impl Pool {
    /// A pool of `total` permits (0 is unbounded) whose floor is
    /// `floor_fraction` of the total, never below one permit.
    pub fn new(name: &'static str, total: usize, floor_fraction: f64) -> Arc<Self> {
        let floor = if total == 0 {
            0
        } else {
            ((total as f64 * floor_fraction).ceil() as usize).max(1)
        };
        Arc::new(Self {
            name,
            total,
            floor,
            inner: Mutex::default(),
            notify: tokio::sync::Notify::new(),
            gauges: PoolGauges::default(),
        })
    }

    pub fn unbounded(name: &'static str) -> Arc<Self> {
        Self::new(name, 0, 0.0)
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Permits in all; 0 is unbounded.
    pub fn total(&self) -> usize {
        self.total
    }

    /// A permit now, or a refusal the caller answers with: the shape for
    /// a front door, where queueing a saturating scope would cost a slot.
    ///
    /// # Errors
    /// Returns [`Refused`] when the scope is at its share or the pool is
    /// full; the scope counts as active either way.
    pub fn try_acquire(self: &Arc<Self>, scope: &str) -> Result<Permit, Refused> {
        let admitted = self.admit(scope);
        if admitted {
            self.gauges.granted.fetch_add(1, Ordering::Relaxed);
            Ok(Permit {
                pool: self.clone(),
                scope: scope.to_owned(),
            })
        } else {
            self.gauges.refused.fetch_add(1, Ordering::Relaxed);
            Err(Refused {
                pool: self.name,
                retry_after: RETRY_AFTER,
            })
        }
    }

    /// A permit, waiting for one: the shape for work already admitted,
    /// where the caller is better served late than refused.
    pub async fn acquire(self: &Arc<Self>, scope: &str) -> Permit {
        let mut waited: Option<Instant> = None;
        loop {
            // Registered before the check, so a release between the
            // check and the wait cannot be missed.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.admit(scope) {
                self.gauges.granted.fetch_add(1, Ordering::Relaxed);
                if let Some(since) = waited {
                    self.gauges.waited.fetch_add(1, Ordering::Relaxed);
                    self.gauges
                        .wait_ms_total
                        .fetch_add(since.elapsed().as_millis() as u64, Ordering::Relaxed);
                }
                return Permit {
                    pool: self.clone(),
                    scope: scope.to_owned(),
                };
            }
            waited.get_or_insert_with(Instant::now);
            notified.await;
        }
    }

    /// The pool as it stands, for a scrape.
    pub fn snapshot(&self) -> PoolSnapshot {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune(&mut inner, Instant::now());
        PoolSnapshot {
            name: self.name,
            total: self.total,
            in_use: inner.in_use,
            active_scopes: inner.scopes.len(),
            share: self.share(&inner),
        }
    }

    /// Grants a permit to `scope` when the pool allows, refreshing the
    /// scope's activity either way. Work-conserving: free capacity is
    /// anyone's, and the share only bites when the node is short. A
    /// scope under its share is admitted while the pool has room; a
    /// scope over its share is admitted while the pool keeps a floor's
    /// worth free, so a scope that arrives late always finds its floor.
    fn admit(&self, scope: &str) -> bool {
        let now = Instant::now();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        prune(&mut inner, now);
        let scope_use = inner.scopes.entry(scope.to_owned()).or_insert(ScopeUse {
            in_use: 0,
            last_seen: now,
        });
        scope_use.last_seen = now;
        let held = scope_use.in_use;
        let share = self.share(&inner);
        if self.total != 0 {
            let room = if held < share {
                self.total
            } else {
                self.total.saturating_sub(self.floor)
            };
            if inner.in_use >= room {
                return false;
            }
        }
        inner.in_use += 1;
        if let Some(scope_use) = inner.scopes.get_mut(scope) {
            scope_use.in_use += 1;
        }
        true
    }

    fn release(&self, scope: &str) {
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.in_use = inner.in_use.saturating_sub(1);
            if let Some(scope_use) = inner.scopes.get_mut(scope) {
                scope_use.in_use = scope_use.in_use.saturating_sub(1);
                scope_use.last_seen = Instant::now();
            }
        }
        self.notify.notify_waiters();
    }

    /// Whether `scope` holds at least its share: the one to give way
    /// when the pool is full.
    pub fn over_share(&self, scope: &str) -> bool {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let share = self.share(&inner);
        inner.scopes.get(scope).is_some_and(|s| s.in_use >= share)
    }

    /// The scope holding the most at or beyond its share, when any
    /// does: the one whose idlest resident gives way to a scope under
    /// its share on a full node. At its share counts, so a pool whose
    /// floors add up to more than it holds still turns over rather
    /// than refusing the newcomer for good.
    pub fn most_over_share(&self) -> Option<String> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let share = self.share(&inner);
        inner
            .scopes
            .iter()
            .filter(|(_, s)| s.in_use > 0 && s.in_use >= share)
            .max_by_key(|(_, s)| s.in_use)
            .map(|(scope, _)| scope.clone())
    }

    /// `max(floor, total / active_scopes)`; unbounded pools have no share.
    fn share(&self, inner: &Inner) -> usize {
        if self.total == 0 {
            return usize::MAX;
        }
        let active = inner.scopes.len().max(1);
        (self.total / active).max(self.floor)
    }
}

/// Forgets scopes holding nothing that were last seen outside the window.
fn prune(inner: &mut Inner, now: Instant) {
    inner.scopes.retain(|_, scope_use| {
        scope_use.in_use > 0 || now.duration_since(scope_use.last_seen) < ACTIVE_WINDOW
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_scope_gets_the_whole_pool() {
        let pool = Pool::new("t", 4, 0.05);
        let held: Vec<_> = (0..4)
            .map(|_| pool.try_acquire("a").expect("granted"))
            .collect();
        assert!(pool.try_acquire("a").is_err(), "the pool is full");
        drop(held);
        assert!(pool.try_acquire("a").is_ok());
    }

    /// A scope over its share may use what nobody else is using, up to
    /// a floor's worth kept free; the share bites only when the pool
    /// is short, and then the scope over its share is the one refused.
    #[test]
    fn free_capacity_is_anyones_and_the_share_bites_when_the_pool_is_short() {
        let pool = Pool::new("t", 10, 0.2);
        let _b = pool.try_acquire("b").expect("granted");
        // a takes past its share of five while the pool has room, and
        // stops when only the floor (two) is left free.
        let a: Vec<_> = (0..7)
            .map(|_| pool.try_acquire("a").expect("granted"))
            .collect();
        assert_eq!(pool.snapshot().share, 5);
        let refused = pool.try_acquire("a").expect_err("only the floor is free");
        assert_eq!(refused.pool, "t");
        assert!(pool.over_share("a"));
        assert_eq!(pool.most_over_share().as_deref(), Some("a"));
        // b, under its share, still gets the floor.
        let _b2 = pool.try_acquire("b").expect("the floor is b's");
        let _b3 = pool.try_acquire("b").expect("the floor is b's");
        assert!(pool.try_acquire("b").is_err(), "the pool is full");
        assert!(!pool.over_share("b"));
        drop(a);
        assert!(pool.try_acquire("a").is_ok(), "room again");
    }

    #[test]
    fn the_floor_holds_for_a_small_scope_on_a_short_pool() {
        let pool = Pool::new("t", 10, 0.2);
        // Six scopes would share one and two thirds each; the floor is
        // two. Five scopes fill eight permits; the sixth still gets
        // two, because the eighth was the last an over-share scope
        // could take.
        let mut held = Vec::new();
        for scope in ["a", "b", "c", "d"] {
            held.push(pool.try_acquire(scope).expect("granted"));
        }
        for _ in 0..4 {
            held.push(pool.try_acquire("e").expect("free capacity"));
        }
        assert!(pool.try_acquire("e").is_err(), "the floor stays free");
        let _f1 = pool.try_acquire("f").expect("the floor");
        let _f2 = pool.try_acquire("f").expect("the floor is two");
        assert!(pool.try_acquire("f").is_err(), "the pool is full");
    }

    #[test]
    fn an_unbounded_pool_never_refuses() {
        let pool = Pool::unbounded("t");
        let held: Vec<_> = (0..1000)
            .map(|_| pool.try_acquire("a").expect("granted"))
            .collect();
        assert_eq!(held.len(), 1000);
        assert_eq!(pool.gauges.refused.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_release_wakes_a_waiter() {
        let pool = Pool::new("t", 1, 0.0);
        let held = pool.try_acquire("a").expect("granted");
        let waiter = {
            let pool = pool.clone();
            tokio::spawn(async move { pool.acquire("a").await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "waits while held");
        drop(held);
        let permit = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("woken")
            .expect("joined");
        assert_eq!(pool.gauges.waited.load(Ordering::Relaxed), 1);
        drop(permit);
        assert_eq!(pool.snapshot().in_use, 0);
    }

    #[test]
    fn an_idle_scope_leaves_the_active_set() {
        let pool = Pool::new("t", 4, 0.0);
        pool.try_acquire("b").expect("granted");
        let _a = pool.try_acquire("a").expect("granted");
        assert_eq!(pool.snapshot().active_scopes, 2);
        // b released at once and has not been seen since; once the
        // window passes it no longer counts against a.
        let mut inner = pool.inner.lock().expect("no poisoned lock");
        inner.scopes.get_mut("b").expect("b was seen").last_seen =
            Instant::now() - ACTIVE_WINDOW * 2;
        drop(inner);
        assert_eq!(pool.snapshot().active_scopes, 1);
        assert_eq!(pool.snapshot().share, 4);
    }
}

/// Per-scope token buckets for the rates a project's policy sets:
/// requests admitted per second and work units spent per second. Each
/// bucket holds one second of its rate as burst; a zero rate is
/// unbounded. Work is charged after the call, so a request that
/// overspends puts the scope in debt, refused until the bucket refills.
#[derive(Default)]
pub struct RateLimits {
    inner: Mutex<HashMap<String, Bucket>>,
}

struct Bucket {
    requests: f64,
    work: f64,
    refilled: Instant,
}

/// Scopes idle this long are forgotten; a fresh bucket starts full.
const BUCKET_IDLE: Duration = Duration::from_secs(60);
/// Above this many scopes a call prunes the idle ones.
const BUCKET_PRUNE_AT: usize = 4096;

impl RateLimits {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// Admits one request for `scope` under its rates, or refuses.
    ///
    /// # Errors
    /// Returns [`Refused`] when the request bucket is empty or the work
    /// bucket is in debt.
    pub fn admit(
        &self,
        scope: &str,
        requests_per_sec: u32,
        work_per_sec: u64,
    ) -> Result<(), Refused> {
        let refused = || Refused {
            pool: "rate",
            retry_after: RETRY_AFTER,
        };
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        if inner.len() > BUCKET_PRUNE_AT {
            inner.retain(|_, bucket| now.duration_since(bucket.refilled) < BUCKET_IDLE);
        }
        let bucket = inner.entry(scope.to_owned()).or_insert(Bucket {
            requests: f64::from(requests_per_sec),
            work: work_per_sec as f64,
            refilled: now,
        });
        bucket.refill(now, requests_per_sec, work_per_sec);
        if requests_per_sec > 0 {
            if bucket.requests < 1.0 {
                return Err(refused());
            }
            bucket.requests -= 1.0;
        }
        if work_per_sec > 0 && bucket.work < 0.0 {
            return Err(refused());
        }
        Ok(())
    }

    /// Charges `work` units against `scope`; the bucket may go negative.
    pub fn charge(&self, scope: &str, work: u64, work_per_sec: u64) {
        if work_per_sec == 0 {
            return;
        }
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        let bucket = inner.entry(scope.to_owned()).or_insert(Bucket {
            requests: 0.0,
            work: work_per_sec as f64,
            refilled: now,
        });
        bucket.refill(now, 0, work_per_sec);
        bucket.work -= work as f64;
    }
}

impl Bucket {
    /// Adds what the elapsed time earned, up to one second of each rate.
    fn refill(&mut self, now: Instant, requests_per_sec: u32, work_per_sec: u64) {
        let elapsed = now.duration_since(self.refilled).as_secs_f64();
        let requests_per_sec = f64::from(requests_per_sec);
        let work_per_sec = work_per_sec as f64;
        self.requests = (self.requests + elapsed * requests_per_sec).min(requests_per_sec);
        self.work = (self.work + elapsed * work_per_sec).min(work_per_sec);
        self.refilled = now;
    }
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    #[test]
    fn a_rate_admits_its_burst_and_refuses_the_next() {
        let rates = RateLimits::new();
        for _ in 0..3 {
            rates.admit("a", 3, 0).expect("within the burst");
        }
        assert!(rates.admit("a", 3, 0).is_err());
        assert!(
            rates.admit("b", 3, 0).is_ok(),
            "another scope is unaffected"
        );
    }

    #[test]
    fn work_debt_refuses_until_it_refills() {
        let rates = RateLimits::new();
        rates.admit("a", 0, 100).expect("a full bucket");
        rates.charge("a", 250, 100);
        assert!(rates.admit("a", 0, 100).is_err(), "in debt");
        // Turn the clock: a bucket refilled two seconds ago has earned
        // its rate back, capped at one second of it.
        rates
            .inner
            .lock()
            .expect("no poisoned lock")
            .get_mut("a")
            .expect("bucket")
            .refilled = Instant::now() - Duration::from_secs(2);
        assert!(rates.admit("a", 0, 100).is_ok(), "refilled");
    }

    #[test]
    fn zero_rates_never_refuse() {
        let rates = RateLimits::new();
        for _ in 0..1000 {
            rates.admit("a", 0, 0).expect("unbounded");
        }
        rates.charge("a", u64::MAX / 2, 0);
        assert!(rates.admit("a", 0, 0).is_ok());
    }
}
