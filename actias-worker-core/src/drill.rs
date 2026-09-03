//! Crash points for the durability drills. With the `drill` feature,
//! `ACTIAS_DRILL_FAULT` names a point; when the running code reaches it
//! the process exits at once, the way a machine dying there would end
//! it. Without the feature every point compiles to nothing, so a
//! production binary has no fault to set, whatever its environment says.
//! The compose image builds with the feature for `scripts/crash-drill.sh`.

/// Exits the process when `point` is the configured fault.
#[cfg(feature = "drill")]
pub fn fault(point: &str) {
    use std::sync::OnceLock;
    static FAULT: OnceLock<Option<String>> = OnceLock::new();
    let configured = FAULT.get_or_init(|| {
        std::env::var("ACTIAS_DRILL_FAULT")
            .ok()
            .filter(|s| !s.is_empty())
    });
    if configured.as_deref() == Some(point) {
        eprintln!("drill fault: exiting at {point}");
        std::process::exit(137);
    }
}

/// Nothing: the binary was built without crash points.
#[cfg(not(feature = "drill"))]
pub fn fault(_point: &str) {}
