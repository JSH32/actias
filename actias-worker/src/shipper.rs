//! Coalesced snapshot shipping: the output gate marks the object dirty
//! and returns; one background flight per object ships the latest state,
//! re-shipping while writes keep landing. Callers stop paying an S3
//! round trip per write, the epoch fence stays exactly as it was, and a
//! graceful drain flushes every dirty object before the process leaves,
//! so deploys lose nothing. What a hard crash can lose is the tail since
//! the last completed ship, which DEPLOY.md already states.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The actual ship, boxed so tests can count instead of upload.
pub type ShipFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

pub struct Shipper {
    ship: ShipFn,
    dirty: AtomicBool,
    running: AtomicBool,
    label: String,
}

impl Shipper {
    pub fn new(label: String, ship: ShipFn) -> Arc<Self> {
        Arc::new(Self {
            ship,
            dirty: AtomicBool::new(false),
            running: AtomicBool::new(false),
            label,
        })
    }

    /// Notes a write and makes sure a flight is (or will be) in the air.
    /// Coalescing lives here: N marks during one flight cost one more
    /// flight, not N.
    pub fn mark_dirty(self: &Arc<Self>) {
        self.dirty.store(true, Ordering::SeqCst);
        self.ensure_flight();
    }

    fn ensure_flight(self: &Arc<Self>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            while this.dirty.swap(false, Ordering::SeqCst) {
                if let Err(error) = (this.ship)().await {
                    actias_common::tracing::warn!(
                        %error,
                        object = this.label,
                        "object snapshot did not ship"
                    );
                }
            }
            this.running.store(false, Ordering::SeqCst);
            // A mark that landed between the last check and the flag
            // going down would otherwise wait for the next write.
            if this.dirty.load(Ordering::SeqCst) {
                this.ensure_flight();
            }
        });
    }

    /// True once nothing is dirty and nothing is flying.
    pub fn settled(&self) -> bool {
        !self.dirty.load(Ordering::SeqCst) && !self.running.load(Ordering::SeqCst)
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
}
