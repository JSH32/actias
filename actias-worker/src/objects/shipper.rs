//! Coalesced snapshot shipping and the output gate that rides it: a
//! write marks the object dirty and takes a ticket; one background
//! flight per object ships the latest state, re-shipping while writes
//! keep landing; the ticket resolves when a flight carrying that
//! write's frames has written its manifest. A call is answered only
//! once its ticket resolves, so "the caller heard success" and "this
//! commit survives the machine" are the same event.
//!
//! Coverage is decided by flight ordering, never by frame offsets:
//!
//! ```text
//!   flight N     |- reads file ------------|  manifest
//!   write W                * commit
//!   ticket                 `- target = N+1     (begun read as N)
//!   flight N+1                       |- reads file --|  manifest
//!                                    ^
//!                     begins after W committed, so it sees W's frames
//! ```
//!
//! A flight takes its number before it reads the file, so a mark taken
//! while [`Shipper::begun`] reads N is covered by flight N+1. The cost
//! is waiting at most one flight longer than strictly necessary, and
//! never one fewer.
//!
//! A completed flight promises that everything committed when it read
//! the file is durable: it either shipped those frames and wrote the
//! manifest naming them, or found them already covered by an earlier
//! manifest and did nothing. The manifest is the release point either
//! way, because restore reads only the segments a manifest names, so an
//! uploaded segment whose manifest never landed reconstructs nothing
//! ([`crate::objects::store`]). A graceful drain flushes every dirty
//! object before the process leaves, and what a hard crash takes back is
//! work nobody was promised: calls still waiting on their ticket, whose
//! callers see a transport failure.
//!
//! The release point is not the end state. Answering on a manifest write
//! is what makes a gated write cost a round trip to the object store;
//! replicating the WAL tail to peers under the same lease and answering
//! on their acks replaces it, and takes the flight-ordering argument
//! above with it. Keep changes here small, and do not build anything new
//! on `begun`/`landed`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use tokio::sync::watch;

/// The actual ship, boxed so tests can count instead of upload. It is
/// handed a [`Release`] it may fire before it completes, once the
/// flight's frames are durable somewhere the gate accepts (a replica
/// quorum); a flight that never fires it releases when it completes.
pub type ShipFn =
    Arc<dyn Fn(Release) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// A flight's early release: firing it resolves every ticket the flight
/// covers before the store has been written. Dropping it unfired means
/// the flight's completion is the release, as before.
pub struct Release(Option<tokio::sync::oneshot::Sender<()>>);

impl Release {
    /// Releases the flight's tickets now.
    pub fn now(mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

/// Consecutive failed flights before the loop stops retrying and waits
/// for the next write to re-arm it. Tickets are bounded by their own
/// budget, so this only decides how long a hopeless object keeps
/// talking to the store.
const MAX_CONSECUTIVE_FAILURES: u32 = 6;

/// Longest a retry backoff grows between failed flights.
const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

/// How many flights a node may have in the air at once. Unbounded, a
/// node with N dirty objects opens N connections to the store.
///
/// The reserve is for flights a caller is blocked on. Background
/// shipping (alarm writes, a hibernating object's last flush) takes only
/// general permits, so it starves first when the node is saturated.
pub struct ShipLimits {
    /// The general lane, split fairly by scope.
    general: Arc<actias_worker_core::shares::Pool>,
    reserved: tokio::sync::Semaphore,
}

/// A flight's permission to talk to the store, from whichever lane.
enum ShipPermit<'a> {
    Reserved {
        _held: tokio::sync::SemaphorePermit<'a>,
    },
    General {
        _held: actias_worker_core::shares::Permit,
    },
}

impl ShipLimits {
    /// An unbounded `general` pool (the drills' posture) reserves
    /// nothing, since nothing ever queues behind it.
    pub fn new(general: Arc<actias_worker_core::shares::Pool>, reserved: usize) -> Arc<Self> {
        let reserved = if general.total() == 0 {
            0
        } else {
            reserved.min(general.total())
        };
        Arc::new(Self {
            general,
            reserved: tokio::sync::Semaphore::new(reserved),
        })
    }

    /// Waits for permission to talk to the store. A gated flight may
    /// take either pool, trying the reserve first; an ungated one may
    /// only take a general permit, which is the scope's share.
    async fn acquire(&self, gated: bool, scope: &str) -> ShipPermit<'_> {
        if gated && let Ok(held) = self.reserved.try_acquire() {
            return ShipPermit::Reserved { _held: held };
        }
        ShipPermit::General {
            _held: self.general.acquire(scope).await,
        }
    }
}

/// Node-wide shipping counters for `/_metrics`, shared by every
/// object's shipper: the pressure that matters is the node's total.
#[derive(Default)]
pub struct ShipGauges {
    pub in_flight: AtomicI64,
    /// Flights waiting for a permit; persistently above zero means the
    /// bound is the bottleneck.
    pub queued: AtomicI64,
    /// Objects with committed writes the store has not taken yet. The
    /// backlog, and with the output gate also what predicts ack latency.
    pub dirty: AtomicI64,
    pub ships: AtomicU64,
    pub failures: AtomicU64,
    pub ship_ms_total: AtomicU64,
    /// Answers held by the output gate and how long they waited; their
    /// mean is what durability costs a caller.
    pub gate_waits: AtomicU64,
    pub gate_wait_ms_total: AtomicU64,
    /// Gates that ran out of budget: committed, unconfirmed, and the
    /// caller told the outcome is unknown.
    pub gates_expired: AtomicU64,
}

pub struct Shipper {
    ship: ShipFn,
    dirty: AtomicBool,
    running: AtomicBool,
    label: String,
    /// The scope the general lane charges this object's flights to.
    scope: String,
    /// Flights begun; the number a flight takes as it starts.
    begun: AtomicU64,
    /// Highest flight number whose manifest landed. Tickets wait on it.
    landed: watch::Sender<u64>,
    gauges: Arc<ShipGauges>,
    limits: Arc<ShipLimits>,
    /// Outstanding tickets; non-zero means a caller is blocked on the
    /// next flight, which earns it the reserved lane.
    waiting: AtomicU64,
}

impl Shipper {
    pub fn new(
        label: String,
        scope: String,
        ship: ShipFn,
        gauges: Arc<ShipGauges>,
        limits: Arc<ShipLimits>,
    ) -> Arc<Self> {
        Arc::new(Self {
            ship,
            dirty: AtomicBool::new(false),
            running: AtomicBool::new(false),
            label,
            scope,
            begun: AtomicU64::new(0),
            landed: watch::Sender::new(0),
            gauges,
            limits,
            waiting: AtomicU64::new(0),
        })
    }

    /// Notes a write and makes sure a flight is (or will be) in the air.
    /// Coalescing lives here: N marks during one flight cost one more
    /// flight, not N.
    pub fn mark_dirty(self: &Arc<Self>) {
        // On the transition, so the gauge counts objects rather than
        // writes.
        if !self.dirty.swap(true, Ordering::SeqCst) {
            self.gauges.dirty.fetch_add(1, Ordering::Relaxed);
        }
        self.ensure_flight();
    }

    /// Notes a write and hands back the ticket that resolves once it is
    /// durable. The mark happens here, not when the ticket is awaited,
    /// so the flight is already in the air while the caller finishes.
    pub fn mark_and_ticket(self: &Arc<Self>) -> Ticket {
        // Reading the counter before the mark is what makes the ticket
        // conservative: any flight numbered past this began after the
        // write it is waiting for.
        let target = self.begun.load(Ordering::SeqCst) + 1;
        // Before the mark, so the flight it arms sees the waiting caller
        // and takes the reserved lane.
        self.waiting.fetch_add(1, Ordering::SeqCst);
        self.mark_dirty();
        Ticket {
            target,
            landed: self.landed.subscribe(),
            gauges: self.gauges.clone(),
            shipper: self.clone(),
        }
    }

    /// A ticket for the flight that will carry everything unshipped so
    /// far, without marking a new write: what a reply that wrote nothing
    /// waits on when the object still has writes in the air. [`None`]
    /// when nothing is dirty or flying, which costs one load each.
    pub fn ticket_if_unsettled(self: &Arc<Self>) -> Option<Ticket> {
        let dirty = self.dirty.load(Ordering::SeqCst);
        let running = self.running.load(Ordering::SeqCst);
        if !dirty && !running {
            return None;
        }
        // Dirty means a flight after the current one carries the rest;
        // flying alone means the current flight is the last.
        let begun = self.begun.load(Ordering::SeqCst);
        let target = if dirty { begun + 1 } else { begun };
        if *self.landed.borrow() >= target {
            return None;
        }
        self.waiting.fetch_add(1, Ordering::SeqCst);
        Some(Ticket {
            target,
            landed: self.landed.subscribe(),
            gauges: self.gauges.clone(),
            shipper: self.clone(),
        })
    }

    fn ensure_flight(self: &Arc<Self>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            let mut failures = 0;
            while this.dirty.swap(false, Ordering::SeqCst) {
                this.gauges.dirty.fetch_sub(1, Ordering::Relaxed);
                let number = this.begun.fetch_add(1, Ordering::SeqCst) + 1;
                // The permit is taken around the store call only: the
                // bookkeeping above and below is local and instant, and
                // holding a permit across it would shrink the bound for
                // no reason.
                let gated = this.waiting.load(Ordering::SeqCst) > 0;
                this.gauges.queued.fetch_add(1, Ordering::Relaxed);
                let permit = this.limits.acquire(gated, &this.scope).await;
                this.gauges.queued.fetch_sub(1, Ordering::Relaxed);
                this.gauges.in_flight.fetch_add(1, Ordering::Relaxed);
                let started = std::time::Instant::now();
                let (early, released) = tokio::sync::oneshot::channel();
                let flight = (this.ship)(Release(Some(early)));
                tokio::pin!(flight);
                let mut released_early = false;
                let outcome = tokio::select! {
                    outcome = &mut flight => outcome,
                    fired = released => {
                        // Fired: the frames are durable on a quorum, so the
                        // callers are answered now and the store catches
                        // up. Dropped unfired: the flight's completion is
                        // the release, as before.
                        if fired.is_ok() {
                            released_early = true;
                            this.landed.send_replace(number);
                        }
                        flight.await
                    }
                };
                drop(permit);
                this.gauges.in_flight.fetch_sub(1, Ordering::Relaxed);
                this.gauges.ships.fetch_add(1, Ordering::Relaxed);
                this.gauges
                    .ship_ms_total
                    .fetch_add(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                match outcome {
                    Ok(()) => {
                        failures = 0;
                        if !released_early {
                            this.landed.send_replace(number);
                        }
                    }
                    Err(error) => {
                        // After an early release the callers were already
                        // answered on the quorum; what remains unshipped is
                        // the store's copy, which the retry below carries.
                        actias_common::tracing::warn!(
                            %error,
                            object = this.label,
                            released_early,
                            "object snapshot did not ship"
                        );
                        this.gauges.failures.fetch_add(1, Ordering::Relaxed);
                        failures += 1;
                        if failures >= MAX_CONSECUTIVE_FAILURES {
                            // Leaving the object dirty keeps it unsettled,
                            // so the drain still waits for it and the next
                            // write re-arms the loop.
                            if !this.dirty.swap(true, Ordering::SeqCst) {
                                this.gauges.dirty.fetch_add(1, Ordering::Relaxed);
                            }
                            break;
                        }
                        // The write is still unshipped, and a ticket may be
                        // waiting on it: retry rather than idling until the
                        // next write happens along.
                        if !this.dirty.swap(true, Ordering::SeqCst) {
                            this.gauges.dirty.fetch_add(1, Ordering::Relaxed);
                        }
                        tokio::time::sleep(backoff(failures)).await;
                    }
                }
            }
            this.running.store(false, Ordering::SeqCst);
            // A mark that landed between the last check and the flag
            // going down would otherwise wait for the next write. After
            // a run of failures the re-arm waits the full backoff first.
            if this.dirty.load(Ordering::SeqCst) {
                if failures >= MAX_CONSECUTIVE_FAILURES {
                    tokio::time::sleep(MAX_BACKOFF).await;
                }
                this.ensure_flight();
            }
        });
    }

    /// True once nothing is dirty and nothing is flying.
    pub fn settled(&self) -> bool {
        !self.dirty.load(Ordering::SeqCst) && !self.running.load(Ordering::SeqCst)
    }
}

/// Exponential, capped; the first retry is nearly immediate because most
/// shipping failures are a blip rather than an outage.
fn backoff(failures: u32) -> std::time::Duration {
    MAX_BACKOFF.min(std::time::Duration::from_millis(50 << failures.min(6)))
}

/// One write's claim on durability: resolves when a flight carrying that
/// write's frames has written its manifest.
pub struct Ticket {
    target: u64,
    landed: watch::Receiver<u64>,
    gauges: Arc<ShipGauges>,
    /// Held so the shipper knows a caller is still blocked on it, and
    /// so the count comes back down however the wait ends.
    shipper: Arc<Shipper>,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.shipper.waiting.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Ticket {
    /// Waits for the covering flight, giving up after `budget`.
    ///
    /// # Errors
    /// Returns the reason the write could not be confirmed durable, for
    /// a caller that must now treat its call's outcome as unknown. The
    /// frames keep being retried in the background either way.
    pub async fn wait(mut self, budget: std::time::Duration) -> Result<(), String> {
        let started = std::time::Instant::now();
        let confirmed = tokio::time::timeout(budget, async {
            while *self.landed.borrow_and_update() < self.target {
                if self.landed.changed().await.is_err() {
                    // The shipper is gone, so nothing will ever confirm
                    // this write; the object's residency ended under it.
                    return false;
                }
            }
            true
        })
        .await;

        self.gauges.gate_waits.fetch_add(1, Ordering::Relaxed);
        self.gauges
            .gate_wait_ms_total
            .fetch_add(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        if !matches!(confirmed, Ok(true)) {
            self.gauges.gates_expired.fetch_add(1, Ordering::Relaxed);
        }

        match confirmed {
            Ok(true) => Ok(()),
            Ok(false) => {
                Err("the object's shipper ended before the write was confirmed".to_owned())
            }
            Err(_) => Err(format!(
                "the write was not confirmed durable within {}ms",
                budget.as_millis()
            )),
        }
    }
}

/// Every live shipper on this node, for the drain flush.
pub type Shippers = Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<Shipper>>>>;

/// Waits for every shipper to settle, bounded; the drain calls this so
/// a deploy never leaves dirty state behind.
/// Longest the drain waits per dirty object still to be shipped. The
/// bound on flights means a backlog drains in waves rather than all at
/// once, so a fixed budget would quietly start losing the deploys it
/// exists to protect as soon as a node holds enough objects.
const DRAIN_PER_OBJECT: std::time::Duration = std::time::Duration::from_millis(50);

pub async fn flush_all(shippers: &Shippers, budget: std::time::Duration) {
    let outstanding = {
        let map = shippers.lock().expect("no poisoned lock");
        map.values().filter(|shipper| !shipper.settled()).count()
    };
    let budget = budget.max(DRAIN_PER_OBJECT * outstanding as u32);
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let pending: Vec<String> = {
            let map = shippers.lock().expect("no poisoned lock");
            map.iter()
                .filter(|(_, shipper)| !shipper.settled())
                .map(|(id, _)| id.clone())
                .collect()
        };
        if pending.is_empty() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            actias_common::tracing::warn!(
                left = pending.len(),
                "drain flush ran out of time; the epoch fence covers the rest"
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test(flavor = "multi_thread")]
    async fn many_marks_coalesce_into_few_ships_and_flush_settles() {
        let ships = Arc::new(AtomicUsize::new(0));
        let ship: ShipFn = {
            let ships = ships.clone();
            Arc::new(move |_release| {
                let ships = ships.clone();
                Box::pin(async move {
                    ships.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    Ok(())
                })
            })
        };
        let shipper = Shipper::new(
            "t".into(),
            "scope".into(),
            ship,
            Arc::default(),
            ShipLimits::new(actias_worker_core::shares::Pool::unbounded("ships"), 0),
        );

        for _ in 0..50 {
            shipper.mark_dirty();
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

        let shippers: Shippers = Arc::new(std::sync::Mutex::new(
            [("t".to_owned(), shipper.clone())].into_iter().collect(),
        ));
        flush_all(&shippers, std::time::Duration::from_secs(5)).await;

        let count = ships.load(Ordering::SeqCst);
        assert!(shipper.settled());
        assert!(count >= 1, "at least one ship");
        assert!(count < 50, "50 writes must not mean 50 ships: {count}");
    }

    /// A ticket taken while a flight is already in the air waits for the
    /// next one: the flight that was already reading the file cannot be
    /// proven to carry the write that had not yet happened when it began.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_ticket_waits_for_the_flight_that_began_after_it() {
        let (allow, allowed) = watch::channel(0u64);
        let flights = Arc::new(AtomicU64::new(0));
        let counted = flights.clone();
        let ship: ShipFn = Arc::new(move |_release| {
            // Each flight parks until the test lets that many finish, so
            // "which flight confirmed the ticket" is decidable.
            let number = counted.fetch_add(1, Ordering::SeqCst) + 1;
            let mut allowed = allowed.clone();
            Box::pin(async move {
                while *allowed.borrow_and_update() < number {
                    if allowed.changed().await.is_err() {
                        break;
                    }
                }
                Ok(())
            })
        });
        let shipper = Shipper::new(
            "t".into(),
            "scope".into(),
            ship,
            Arc::default(),
            ShipLimits::new(actias_worker_core::shares::Pool::unbounded("ships"), 0),
        );

        // Flight 1 is in the air and parked.
        shipper.mark_dirty();
        while flights.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        // This write commits after flight 1 started reading, so only
        // flight 2 can be its covering flight.
        let ticket = shipper.mark_and_ticket();
        let waiting = tokio::spawn(ticket.wait(std::time::Duration::from_secs(5)));

        allow.send_replace(1);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !waiting.is_finished(),
            "the flight already in the air cannot confirm a later write"
        );

        allow.send_replace(2);
        waiting
            .await
            .expect("the waiting task")
            .expect("the next flight confirms it");
    }

    /// Group commit: writes that arrive while one flight is in the air
    /// share the flight after it, so a burst costs one round trip rather
    /// than one each. This is what keeps the output gate affordable.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_burst_of_writes_shares_one_flight() {
        let ships = Arc::new(AtomicUsize::new(0));
        let counted = ships.clone();
        let ship: ShipFn = Arc::new(move |_release| {
            let ships = counted.clone();
            Box::pin(async move {
                ships.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                Ok(())
            })
        });
        let shipper = Shipper::new(
            "t".into(),
            "scope".into(),
            ship,
            Arc::default(),
            ShipLimits::new(actias_worker_core::shares::Pool::unbounded("ships"), 0),
        );

        // The first write puts a flight in the air; the rest land while
        // it flies and are confirmed together by the one after it.
        let tickets: Vec<_> = (0..20).map(|_| shipper.mark_and_ticket()).collect();
        for ticket in tickets {
            ticket
                .wait(std::time::Duration::from_secs(5))
                .await
                .expect("confirmed");
        }

        let count = ships.load(Ordering::SeqCst);
        assert!(
            count <= 3,
            "20 gated writes must not mean 20 flights: {count}"
        );
    }

    /// The gauges are the only view an operator has of this, and the
    /// dirty one is the easiest to get wrong: it counts objects waiting
    /// on the store, so it must move on the transition and come back to
    /// zero once the store has taken everything, including after the
    /// retries a failure causes.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_gauges_track_the_backlog_and_the_gate() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counted = attempts.clone();
        let ship: ShipFn = Arc::new(move |_release| {
            // Fails once, so the re-mark path is exercised rather than
            // only the happy one.
            let n = counted.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if n == 0 {
                    Err("the store blinked".to_owned())
                } else {
                    Ok(())
                }
            })
        });
        let gauges: Arc<ShipGauges> = Arc::default();
        let shipper = Shipper::new(
            "t".into(),
            "scope".into(),
            ship,
            gauges.clone(),
            ShipLimits::new(actias_worker_core::shares::Pool::unbounded("ships"), 0),
        );

        shipper
            .mark_and_ticket()
            .wait(std::time::Duration::from_secs(5))
            .await
            .expect("the retry confirms it");

        assert_eq!(gauges.dirty.load(Ordering::SeqCst), 0, "backlog drained");
        assert_eq!(gauges.in_flight.load(Ordering::SeqCst), 0, "nothing flying");
        assert_eq!(gauges.failures.load(Ordering::SeqCst), 1, "the blink");
        assert!(gauges.ships.load(Ordering::SeqCst) >= 2, "failed then flew");
        assert_eq!(gauges.gate_waits.load(Ordering::SeqCst), 1);
        assert_eq!(
            gauges.gates_expired.load(Ordering::SeqCst),
            0,
            "it was confirmed, not abandoned"
        );
    }

    /// The bound is the whole point: however many objects want the
    /// store at once, only so many talk to it, and the rest wait their
    /// turn instead of opening a connection each.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_node_never_exceeds_its_flight_bound() {
        const OBJECTS: usize = 40;
        const BOUND: usize = 4;

        let peak = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicUsize::new(0));
        let gauges: Arc<ShipGauges> = Arc::default();
        let limits = ShipLimits::new(
            actias_worker_core::shares::Pool::new("ships", BOUND, 0.0),
            0,
        );

        let shippers: Vec<_> = (0..OBJECTS)
            .map(|n| {
                let (peak, live) = (peak.clone(), live.clone());
                let ship: ShipFn = Arc::new(move |_release| {
                    let (peak, live) = (peak.clone(), live.clone());
                    Box::pin(async move {
                        let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        live.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
                });
                Shipper::new(
                    format!("obj-{n}"),
                    "scope".into(),
                    ship,
                    gauges.clone(),
                    limits.clone(),
                )
            })
            .collect();

        let tickets: Vec<_> = shippers.iter().map(|s| s.mark_and_ticket()).collect();
        for ticket in tickets {
            ticket
                .wait(std::time::Duration::from_secs(10))
                .await
                .expect("every write confirms");
        }

        let observed = peak.load(Ordering::SeqCst);
        assert!(
            observed <= BOUND,
            "{OBJECTS} objects put {observed} flights in the air against a bound of {BOUND}"
        );
        assert!(observed > 1, "the bound serialized everything: {observed}");
        assert_eq!(gauges.queued.load(Ordering::SeqCst), 0, "queue drained");
    }

    /// A flight that fires its release answers the caller before it has
    /// finished with the store, and a store failure after that does not
    /// take the answer back.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_early_release_answers_before_the_flight_completes() {
        let (finish, finished) = watch::channel(false);
        let watched = finished.clone();
        let ship: ShipFn = Arc::new(move |release: Release| {
            let mut finished = watched.clone();
            Box::pin(async move {
                release.now();
                while !*finished.borrow_and_update() {
                    if finished.changed().await.is_err() {
                        break;
                    }
                }
                Err("the store blinked after the quorum".to_owned())
            })
        });
        let shipper = Shipper::new(
            "t".into(),
            "scope".into(),
            ship,
            Arc::default(),
            ShipLimits::new(actias_worker_core::shares::Pool::unbounded("ships"), 0),
        );

        let ticket = shipper.mark_and_ticket();
        ticket
            .wait(std::time::Duration::from_secs(2))
            .await
            .expect("released on the quorum, before the store");
        assert!(!shipper.settled(), "the store's copy is still owed");
        finish.send_replace(true);
        drop(finished);
    }

    /// A store that never answers costs the write its acknowledgment
    /// rather than hanging the caller forever.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_unconfirmable_write_gives_up_on_its_budget() {
        let ship: ShipFn =
            Arc::new(move |_release| Box::pin(async move { Err("no store".to_owned()) }));
        let shipper = Shipper::new(
            "t".into(),
            "scope".into(),
            ship,
            Arc::default(),
            ShipLimits::new(actias_worker_core::shares::Pool::unbounded("ships"), 0),
        );

        let error = shipper
            .mark_and_ticket()
            .wait(std::time::Duration::from_millis(200))
            .await
            .expect_err("an unshipped write is never confirmed");
        assert!(error.contains("200ms"), "{error}");
    }
}
