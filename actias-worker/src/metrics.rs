//! Worker metrics in prometheus text form, hand-rolled on purpose: three
//! series and a gauge do not earn a metrics framework, and the text format
//! is a stable contract. Served at /_metrics, inside the underscore
//! namespace no script identifier can occupy.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// Per-script counters; snapshots are cheap because scrapes are rare.
#[derive(Default)]
pub struct Metrics {
    scripts: Mutex<HashMap<String, ScriptStats>>,
}

#[derive(Default, Clone)]
struct ScriptStats {
    requests: u64,
    errors: u64,
    duration_ms_total: u64,
}

impl Metrics {
    /// Notes one finished request against its script label.
    pub fn record(&self, script: &str, elapsed: Duration, ok: bool) {
        let mut scripts = self.scripts.lock().expect("no poisoned lock");
        let stats = scripts.entry(script.to_owned()).or_default();
        stats.requests += 1;
        if !ok {
            stats.errors += 1;
        }
        stats.duration_ms_total += elapsed.as_millis() as u64;
    }

    /// The whole exposition: per-script counters plus whatever gauges the
    /// caller measured at scrape time.
    pub fn render(&self, objects_resident: usize) -> String {
        let scripts = self.scripts.lock().expect("no poisoned lock").clone();

        let mut out = String::new();
        out.push_str("# TYPE actias_requests_total counter\n");
        for (script, stats) in &scripts {
            out.push_str(&format!(
                "actias_requests_total{{script=\"{script}\"}} {}\n",
                stats.requests
            ));
        }
        out.push_str("# TYPE actias_request_errors_total counter\n");
        for (script, stats) in &scripts {
            out.push_str(&format!(
                "actias_request_errors_total{{script=\"{script}\"}} {}\n",
                stats.errors
            ));
        }
        out.push_str("# TYPE actias_request_duration_ms_total counter\n");
        for (script, stats) in &scripts {
            out.push_str(&format!(
                "actias_request_duration_ms_total{{script=\"{script}\"}} {}\n",
                stats.duration_ms_total
            ));
        }
        out.push_str("# TYPE actias_objects_resident gauge\n");
        out.push_str(&format!("actias_objects_resident {objects_resident}\n"));

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_requests_appear_in_the_exposition() {
        let metrics = Metrics::default();
        metrics.record("my-script", Duration::from_millis(12), true);
        metrics.record("my-script", Duration::from_millis(8), false);

        let text = metrics.render(3);

        assert!(text.contains("actias_requests_total{script=\"my-script\"} 2"));
        assert!(text.contains("actias_request_errors_total{script=\"my-script\"} 1"));
        assert!(text.contains("actias_request_duration_ms_total{script=\"my-script\"} 20"));
        assert!(text.contains("actias_objects_resident 3"));
    }
}
