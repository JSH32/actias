//! Coalesced snapshot shipping and the output gate that rides it: a
//! write marks the object dirty and takes a ticket; one background
//! flight per object ships the latest state, re-shipping while writes
//! keep landing; the ticket resolves when a flight carrying that
//! write's frames has written its manifest. A call is answered only
//! once its ticket resolves, so "the caller heard success" and "this
//! commit survives the machine" are the same event.
//!
//! Coverage is decided by flight ordering rather than frame offsets. A
//! flight takes a number when it begins, and a mark taken while
//! [`Shipper::begun`] reads N is covered by flight N+1: that flight had
//! not started reading the file when the mark was taken, so it sees the
//! committed frames. This can wait one flight longer than strictly
//! necessary and never one fewer.
//!
//! What a completed flight promises is that everything committed when
//! it read the file is durable: it either shipped those frames and
//! wrote the manifest naming them, or found them already covered by an
//! earlier manifest and did nothing. The manifest is the release point
//! either way, because restore reads only the segments a manifest
//! names, so an uploaded segment whose manifest never landed
//! reconstructs nothing (object_store.rs). A graceful
//! drain flushes every dirty object before the process leaves, and what
//! a hard crash takes back is work nobody was promised: calls still
//! waiting on their ticket, whose callers see a transport failure.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::watch;

/// The actual ship, boxed so tests can count instead of upload.
pub type ShipFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// Consecutive failed flights before the loop stops retrying and waits
/// for the next write to re-arm it. Tickets are bounded by their own
/// budget, so this only decides how long a hopeless object keeps
/// talking to the store.
const MAX_CONSECUTIVE_FAILURES: u32 = 6;

/// Longest a retry backoff grows between failed flights.
const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

pub struct Shipper {
    ship: ShipFn,
    dirty: AtomicBool,
    running: AtomicBool,
    label: String,
    /// Flights begun; the number a flight takes as it starts.
    begun: AtomicU64,
    /// Highest flight number whose manifest landed. Tickets wait on it.
    landed: watch::Sender<u64>,
}

impl Shipper {
    pub fn new(label: String, ship: ShipFn) -> Arc<Self> {
        Arc::new(Self {
            ship,
            dirty: AtomicBool::new(false),
            running: AtomicBool::new(false),
            label,
            begun: AtomicU64::new(0),
            landed: watch::Sender::new(0),
        })
    }

    /// Notes a write and makes sure a flight is (or will be) in the air.
    /// Coalescing lives here: N marks during one flight cost one more
    /// flight, not N.
    pub fn mark_dirty(self: &Arc<Self>) {
        self.dirty.store(true, Ordering::SeqCst);
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
        self.mark_dirty();
        Ticket {
            target,
            landed: self.landed.subscribe(),
        }
    }

    fn ensure_flight(self: &Arc<Self>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            let mut failures = 0;
            while this.dirty.swap(false, Ordering::SeqCst) {
                let number = this.begun.fetch_add(1, Ordering::SeqCst) + 1;
                match (this.ship)().await {
                    Ok(()) => {
                        failures = 0;
                        this.landed.send_replace(number);
                    }
                    Err(error) => {
                        actias_common::tracing::warn!(
                            %error,
                            object = this.label,
                            "object snapshot did not ship"
                        );
                        failures += 1;
                        if failures >= MAX_CONSECUTIVE_FAILURES {
                            // Leaving the object dirty keeps it unsettled,
                            // so the drain still waits for it and the next
                            // write re-arms the loop.
                            this.dirty.store(true, Ordering::SeqCst);
                            break;
                        }
                        // The write is still unshipped, and a ticket may be
                        // waiting on it: retry rather than idling until the
                        // next write happens along.
                        this.dirty.store(true, Ordering::SeqCst);
                        tokio::time::sleep(backoff(failures)).await;
                    }
                }
            }
            this.running.store(false, Ordering::SeqCst);
            // A mark that landed between the last check and the flag
            // going down would otherwise wait for the next write.
            if this.dirty.load(Ordering::SeqCst) && failures < MAX_CONSECUTIVE_FAILURES {
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
}

impl Ticket {
    /// Waits for the covering flight, giving up after `budget`.
    ///
    /// # Errors
    /// Returns the reason the write could not be confirmed durable, for
    /// a caller that must now treat its call's outcome as unknown. The
    /// frames keep being retried in the background either way.
    pub async fn wait(mut self, budget: std::time::Duration) -> Result<(), String> {
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
pub async fn flush_all(shippers: &Shippers, budget: std::time::Duration) {
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
            Arc::new(move || {
                let ships = ships.clone();
                Box::pin(async move {
                    ships.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    Ok(())
                })
            })
        };
        let shipper = Shipper::new("t".into(), ship);

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
    /// NEXT one: the flight that was already reading the file cannot be
    /// proven to carry the write that had not yet happened when it began.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_ticket_waits_for_the_flight_that_began_after_it() {
        let (allow, allowed) = watch::channel(0u64);
        let flights = Arc::new(AtomicU64::new(0));
        let counted = flights.clone();
        let ship: ShipFn = Arc::new(move || {
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
        let shipper = Shipper::new("t".into(), ship);

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
        let ship: ShipFn = Arc::new(move || {
            let ships = counted.clone();
            Box::pin(async move {
                ships.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                Ok(())
            })
        });
        let shipper = Shipper::new("t".into(), ship);

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

    /// A store that never answers costs the write its acknowledgment
    /// rather than hanging the caller forever.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_unconfirmable_write_gives_up_on_its_budget() {
        let ship: ShipFn = Arc::new(move || Box::pin(async move { Err("no store".to_owned()) }));
        let shipper = Shipper::new("t".into(), ship);

        let error = shipper
            .mark_and_ticket()
            .wait(std::time::Duration::from_millis(200))
            .await
            .expect_err("an unshipped write is never confirmed");
        assert!(error.contains("200ms"), "{error}");
    }
}
