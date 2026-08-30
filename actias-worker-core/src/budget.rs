//! Per-scope budgets for guest code, counted in work rather than
//! seconds: waiting on io costs nothing, computing costs.
//!
//! The Luau adapter charges one tick per VM interrupt. Measured, a loop
//! iteration is one tick and a 200ms await is eight, which is what makes
//! the count a work meter and what the ceilings below are set from. A
//! wasm host would charge fuel instead; nothing here names an engine.
//!
//! The clock remains as a backstop for what ticks cannot see, code
//! blocked inside a host call that never re-enters the VM.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Work one scope may spend. About a second of pure computation: a
/// tight loop turns roughly 7 million ticks a second (the `tick_rate`
/// probe in objects.rs), and real handlers spend thousands.
pub const DEFAULT_WORK_LIMIT: u64 = 5_000_000;

/// Top-level evaluation only declares, which costs thousands at most.
pub const DECLARATION_WORK_LIMIT: u64 = 500_000;

/// Ticks between clock reads; the clock costs more than a tick and the
/// backstop is measured in seconds.
const WALL_SAMPLE: u64 = 256;

/// Process-wide origin, so a deadline is one atomic rather than a lock
/// around an [`Instant`].
fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    origin().elapsed().as_millis() as u64
}

/// What one scope may spend.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// Work units; [`None`] does not bound work.
    pub work: Option<u64>,
    /// The backstop; [`None`] does not bound time.
    pub wall: Option<Duration>,
}

impl Budget {
    /// Bounded by both, which is every scope the platform arms.
    pub fn new(work: u64, wall: Duration) -> Self {
        Self {
            work: Some(work),
            wall: Some(wall),
        }
    }
}

/// Which ceiling a scope hit, so the message names the real cause.
#[derive(Debug, Clone, Copy)]
pub enum Exhausted {
    Work(u64),
    Wall(Duration),
}

impl std::fmt::Display for Exhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Exhausted::Work(limit) => write!(
                f,
                "This code did too much work in one call; the limit is {limit} units. \
                 A loop that never ends is the usual cause."
            ),
            Exhausted::Wall(limit) => write!(
                f,
                "This call took longer than {} seconds and was stopped.",
                limit.as_secs()
            ),
        }
    }
}

/// What a scope spent, for metering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Consumed {
    pub work: u64,
}

/// The meter one vm charges against, armed per scope: a request, an
/// object call, a connection wake, a workflow attempt. Never per vm
/// lifetime, since a pinned vm lives indefinitely.
#[derive(Default)]
pub struct Meter {
    /// Work in the armed scope.
    work: AtomicU64,
    /// Ceiling for the armed scope; 0 is unarmed or unbounded.
    limit: AtomicU64,
    /// Backstop as a millisecond stamp against [`origin`]; 0 is none.
    deadline: AtomicU64,
    /// Lifetime work, which scopes never reset; what metering reads.
    total: AtomicU64,
}

impl Meter {
    /// Opens a scope, banking any unclosed one so the total stays
    /// correct however scopes nest or fail.
    pub fn arm(&self, budget: Budget) {
        self.bank();
        self.limit
            .store(budget.work.unwrap_or(0), Ordering::Relaxed);
        self.deadline.store(
            budget
                .wall
                .map(|wall| now_ms() + wall.as_millis() as u64)
                .unwrap_or(0),
            Ordering::Relaxed,
        );
    }

    /// Closes the scope; the vm idles unmetered until the next arm.
    pub fn disarm(&self) {
        self.bank();
        self.limit.store(0, Ordering::Relaxed);
        self.deadline.store(0, Ordering::Relaxed);
    }

    /// Charges one unit and says whether the scope may continue. Called
    /// by the engine adapter on every interrupt, so it stays one relaxed
    /// add plus a sampled clock read.
    ///
    /// # Errors
    /// Returns which ceiling was reached.
    pub fn tick(&self) -> Result<(), Exhausted> {
        let used = self.work.fetch_add(1, Ordering::Relaxed) + 1;

        let limit = self.limit.load(Ordering::Relaxed);
        if limit != 0 && used > limit {
            return Err(Exhausted::Work(limit));
        }

        if used.is_multiple_of(WALL_SAMPLE) {
            let deadline = self.deadline.load(Ordering::Relaxed);
            if deadline != 0 && now_ms() > deadline {
                return Err(Exhausted::Wall(Duration::from_millis(deadline)));
            }
        }

        Ok(())
    }

    /// Lifetime work, armed or not.
    pub fn consumed(&self) -> Consumed {
        Consumed {
            work: self.total.load(Ordering::Relaxed) + self.work.load(Ordering::Relaxed),
        }
    }

    /// Moves the open scope's work into the lifetime total.
    fn bank(&self) {
        let used = self.work.swap(0, Ordering::Relaxed);
        if used != 0 {
            self.total.fetch_add(used, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_runs_out_before_the_clock_does() {
        let meter = Meter::default();
        meter.arm(Budget::new(10, Duration::from_secs(3600)));

        for _ in 0..10 {
            meter.tick().expect("inside the budget");
        }
        let over = meter.tick().expect_err("the eleventh is over");
        assert!(matches!(over, Exhausted::Work(10)), "{over:?}");
    }

    #[test]
    fn the_total_survives_scopes_and_their_failures() {
        let meter = Meter::default();

        meter.arm(Budget::new(100, Duration::from_secs(1)));
        for _ in 0..5 {
            meter.tick().expect("fine");
        }
        meter.disarm();

        // A scope that blows its budget still banks what it spent.
        meter.arm(Budget::new(2, Duration::from_secs(1)));
        meter.tick().expect("first");
        meter.tick().expect("second");
        let _ = meter.tick();
        meter.disarm();

        assert_eq!(
            meter.consumed().work,
            8,
            "5 + 3, nothing lost or double counted"
        );
    }

    #[test]
    fn an_unarmed_meter_still_counts_but_never_stops() {
        let meter = Meter::default();
        for _ in 0..1000 {
            meter.tick().expect("unarmed work is unbounded");
        }
        assert_eq!(meter.consumed().work, 1000);
    }
}
