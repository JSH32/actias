//! The platform's clocks and time spellings: the unix clock every
//! surface reads (with the test offset `actias test` advances), cron
//! next-occurrence math, and durations written the way scripts write
//! them. Home of everything time-shaped; the objects module re-exports
//! these under its own path because two other crates reach them there.

/// Milliseconds until a cron event's next occurrence. The expression is
/// whatever follows `cron:`; classic five-field expressions gain a seconds
/// column, since the parser wants six.
pub fn cron_delay_ms(event: &str) -> Result<i64, String> {
    use std::str::FromStr;

    let expr = event.strip_prefix("cron:").unwrap_or(event).trim();
    let normalized = if expr.split_whitespace().count() == 5 {
        format!("0 {expr}")
    } else {
        expr.to_owned()
    };

    let schedule = cron::Schedule::from_str(&normalized)
        .map_err(|e| format!("'{expr}' is not a cron expression: {e}"))?;
    let next = schedule
        .upcoming(chrono::Utc)
        .next()
        .ok_or_else(|| format!("'{expr}' never occurs"))?;

    Ok((next.timestamp_millis() - unix_now_ms()).max(1000))
}

/// Milliseconds since the unix epoch, the clock `state.now()` exposes and
/// the alarm loop schedules against.
/// The virtual-clock offset `actias test` advances; zero everywhere
/// else, so production time is wall time.
static TEST_CLOCK_OFFSET_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Fast-forwards every platform clock in this process: test harness
/// machinery, which is why a 24h await times out in a millisecond test.
pub fn advance_clock_for_tests(ms: i64) {
    TEST_CLOCK_OFFSET_MS.fetch_add(ms.max(0), std::sync::atomic::Ordering::Relaxed);
}

pub fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
        + TEST_CLOCK_OFFSET_MS.load(std::sync::atomic::Ordering::Relaxed)
}

// The declarations crate owns the duration spelling: a class's
// `expire` must parse identically at check and at claim.
pub use actias_declarations::duration::parse_duration_ms;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_the_way_scripts_write_them() {
        assert_eq!(parse_duration_ms("500ms").unwrap(), 500);
        assert_eq!(parse_duration_ms("30s").unwrap(), 30_000);
        assert_eq!(parse_duration_ms("10m").unwrap(), 600_000);
        assert_eq!(parse_duration_ms("24h").unwrap(), 86_400_000);
        assert_eq!(parse_duration_ms("7d").unwrap(), 604_800_000);
        assert_eq!(parse_duration_ms("1.5s").unwrap(), 1500);
        // A bare number is seconds.
        assert_eq!(parse_duration_ms("2").unwrap(), 2000);

        assert!(parse_duration_ms("soon").is_err());
        assert!(parse_duration_ms("10 fortnights").is_err());
    }
}
