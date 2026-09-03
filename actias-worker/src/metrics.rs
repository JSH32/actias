//! Worker metrics in prometheus text form, hand-rolled on purpose: three
//! series and a gauge do not earn a metrics framework, and the text format
//! is a stable contract. Served at /_metrics, inside the underscore
//! namespace no script identifier can occupy.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// Per-script counters; snapshots are cheap because scrapes are rare.
/// Keyed (project, script) so dashboards can narrow to one tenant.
#[derive(Default)]
pub struct Metrics {
    scripts: Mutex<HashMap<(String, String), ScriptStats>>,
    /// Reads served from a restored snapshot replica instead of the
    /// owner's mailbox; the multi-node read story in one number.
    pub replica_reads: std::sync::atomic::AtomicU64,
}

#[derive(Default, Clone)]
struct ScriptStats {
    requests: u64,
    errors: u64,
    duration_ms_total: u64,
}

impl Metrics {
    /// Notes one finished request against its project and script labels.
    pub fn record(&self, project: &str, script: &str, elapsed: Duration, ok: bool) {
        let mut scripts = self.scripts.lock().expect("no poisoned lock");
        let stats = scripts
            .entry((project.to_owned(), script.to_owned()))
            .or_default();
        stats.requests += 1;
        if !ok {
            stats.errors += 1;
        }
        stats.duration_ms_total += elapsed.as_millis() as u64;
    }

    /// The whole exposition: per-script counters plus whatever gauges the
    /// caller measured at scrape time.
    // One gauge set per subsystem; the list is the inventory of what the
    // node exposes, and a struct would only rename it.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        objects_resident: usize,
        connections: &actias_worker_core::connections::actor::ConnectionGauges,
        ships: &crate::objects::shipper::ShipGauges,
        replicas: &crate::objects::replica::ReplicaGauges,
        vms: (&crate::vm_pool::VmPoolGauges, usize),
        directory: &crate::directory::gauges::DirectoryGauges,
        directory_files: (u64, u64),
    ) -> String {
        let scripts = self.scripts.lock().expect("no poisoned lock").clone();

        let mut out = String::new();
        out.push_str("# TYPE actias_requests_total counter\n");
        for ((project, script), stats) in &scripts {
            out.push_str(&format!(
                "actias_requests_total{{project=\"{project}\",script=\"{script}\"}} {}\n",
                stats.requests
            ));
        }
        out.push_str("# TYPE actias_request_errors_total counter\n");
        for ((project, script), stats) in &scripts {
            out.push_str(&format!(
                "actias_request_errors_total{{project=\"{project}\",script=\"{script}\"}} {}\n",
                stats.errors
            ));
        }
        out.push_str("# TYPE actias_request_duration_ms_total counter\n");
        for ((project, script), stats) in &scripts {
            out.push_str(&format!(
                "actias_request_duration_ms_total{{project=\"{project}\",script=\"{script}\"}} {}\n",
                stats.duration_ms_total
            ));
        }
        out.push_str("# TYPE actias_replica_reads_total counter\n");
        out.push_str(&format!(
            "actias_replica_reads_total {}\n",
            self.replica_reads
                .load(std::sync::atomic::Ordering::Relaxed)
        ));
        out.push_str("# TYPE actias_objects_resident gauge\n");
        out.push_str(&format!("actias_objects_resident {objects_resident}\n"));
        let load = |n: &std::sync::atomic::AtomicI64| n.load(std::sync::atomic::Ordering::Relaxed);
        out.push_str("# TYPE actias_connections_warm gauge\n");
        out.push_str(&format!(
            "actias_connections_warm {}\n",
            load(&connections.warm)
        ));
        out.push_str("# TYPE actias_connections_hibernated gauge\n");
        out.push_str(&format!(
            "actias_connections_hibernated {}\n",
            load(&connections.hibernated)
        ));
        out.push_str("# TYPE actias_connection_wakes_total counter\n");
        out.push_str(&format!(
            "actias_connection_wakes_total {}\n",
            connections.wakes.load(std::sync::atomic::Ordering::Relaxed)
        ));
        out.push_str("# TYPE actias_connection_wake_ms_total counter\n");
        out.push_str(&format!(
            "actias_connection_wake_ms_total {}\n",
            connections
                .wake_ms_total
                .load(std::sync::atomic::Ordering::Relaxed)
        ));

        // Shipping, the object plane's back pressure. in_flight against
        // dirty says whether the store is keeping up; the gate pair says
        // what that costs a caller, since a write is not answered until
        // its frames are durable.
        let count = |n: &std::sync::atomic::AtomicU64| n.load(std::sync::atomic::Ordering::Relaxed);
        out.push_str("# TYPE actias_ships_in_flight gauge\n");
        out.push_str(&format!(
            "actias_ships_in_flight {}\n",
            load(&ships.in_flight)
        ));
        out.push_str("# TYPE actias_ships_queued gauge\n");
        out.push_str(&format!("actias_ships_queued {}\n", load(&ships.queued)));
        out.push_str("# TYPE actias_objects_dirty gauge\n");
        out.push_str(&format!("actias_objects_dirty {}\n", load(&ships.dirty)));
        out.push_str("# TYPE actias_ships_total counter\n");
        out.push_str(&format!("actias_ships_total {}\n", count(&ships.ships)));
        out.push_str("# TYPE actias_ship_failures_total counter\n");
        out.push_str(&format!(
            "actias_ship_failures_total {}\n",
            count(&ships.failures)
        ));
        out.push_str("# TYPE actias_ship_duration_ms_total counter\n");
        out.push_str(&format!(
            "actias_ship_duration_ms_total {}\n",
            count(&ships.ship_ms_total)
        ));
        for (name, kind, value) in [
            (
                "actias_replica_appends_total",
                "counter",
                count(&replicas.appends),
            ),
            (
                "actias_replica_append_refusals_total",
                "counter",
                count(&replicas.append_refusals),
            ),
            (
                "actias_replica_append_ms_total",
                "counter",
                count(&replicas.append_ms_total),
            ),
            (
                "actias_replica_fetches_total",
                "counter",
                count(&replicas.fetches),
            ),
            (
                "actias_replica_reads_confirmed_total",
                "counter",
                count(&replicas.reads_confirmed),
            ),
            (
                "actias_replica_reads_waited_total",
                "counter",
                count(&replicas.reads_waited),
            ),
            (
                "actias_replica_reads_forwarded_total",
                "counter",
                count(&replicas.reads_forwarded),
            ),
            (
                "actias_replica_fanout_appends_total",
                "counter",
                count(&replicas.fanout_appends),
            ),
            (
                "actias_replica_fanout_ack_ms_total",
                "counter",
                count(&replicas.fanout_ack_ms_total),
            ),
            (
                "actias_replica_fanout_failures_total",
                "counter",
                count(&replicas.fanout_failures),
            ),
            (
                "actias_replica_quorum_releases_total",
                "counter",
                count(&replicas.quorum_releases),
            ),
            (
                "actias_replica_store_releases_total",
                "counter",
                count(&replicas.store_releases),
            ),
            (
                "actias_replica_takeovers_total",
                "counter",
                count(&replicas.takeovers),
            ),
            (
                "actias_replica_takeover_incidents_total",
                "counter",
                count(&replicas.takeover_incidents),
            ),
        ] {
            out.push_str(&format!("# TYPE {name} {kind}\n{name} {value}\n"));
        }
        out.push_str(&format!(
            "# TYPE actias_replica_bytes_held gauge\nactias_replica_bytes_held {}\n# TYPE actias_replica_objects_held gauge\nactias_replica_objects_held {}\n",
            replicas.bytes_held.load(std::sync::atomic::Ordering::Relaxed),
            replicas.objects_held.load(std::sync::atomic::Ordering::Relaxed),
        ));
        for (name, value) in [
            ("actias_vm_pool_hits_total", count(&vms.0.hits)),
            ("actias_vm_pool_misses_total", count(&vms.0.misses)),
            ("actias_vm_pool_builds_total", count(&vms.0.builds)),
            (
                "actias_vm_pool_build_ms_total",
                count(&vms.0.build_ms_total),
            ),
            ("actias_vm_pool_expired_total", count(&vms.0.expired)),
        ] {
            out.push_str(&format!("# TYPE {name} counter\n{name} {value}\n"));
        }
        out.push_str(&format!(
            "# TYPE actias_vm_pool_warm gauge\nactias_vm_pool_warm {}\n",
            vms.1
        ));
        out.push_str("# TYPE actias_ack_gate_waits_total counter\n");
        out.push_str(&format!(
            "actias_ack_gate_waits_total {}\n",
            count(&ships.gate_waits)
        ));
        out.push_str("# TYPE actias_ack_gate_wait_ms_total counter\n");
        out.push_str(&format!(
            "actias_ack_gate_wait_ms_total {}\n",
            count(&ships.gate_wait_ms_total)
        ));
        out.push_str("# TYPE actias_ack_gate_expired_total counter\n");
        out.push_str(&format!(
            "actias_ack_gate_expired_total {}\n",
            count(&ships.gates_expired)
        ));
        // The directory's loops. Each pair reads as work done against
        // work failed, so a dashboard sees the flush keeping up, the
        // compactor folding, the invariant gate staying shut on a
        // healthy cluster, and the backfill draining.
        let counters: [(&str, &std::sync::atomic::AtomicU64); 25] = [
            ("actias_directory_flushes_total", &directory.flushes),
            (
                "actias_directory_flushed_rows_total",
                &directory.flushed_rows,
            ),
            (
                "actias_directory_flush_failures_total",
                &directory.flush_failures,
            ),
            ("actias_directory_folds_total", &directory.folds),
            (
                "actias_directory_fold_failures_total",
                &directory.fold_failures,
            ),
            ("actias_directory_passes_total", &directory.passes),
            ("actias_directory_gate_checks_total", &directory.gate_checks),
            ("actias_directory_gate_opened_total", &directory.gate_opened),
            ("actias_directory_rebuilds_total", &directory.rebuilds),
            (
                "actias_directory_rebuilt_rows_total",
                &directory.rebuilt_rows,
            ),
            (
                "actias_directory_placeholder_rows_total",
                &directory.placeholder_rows,
            ),
            (
                "actias_directory_rebuild_failures_total",
                &directory.rebuild_failures,
            ),
            ("actias_directory_sweeps_total", &directory.sweeps),
            ("actias_directory_swept_rows_total", &directory.swept_rows),
            (
                "actias_directory_backfilled_rows_total",
                &directory.backfilled_rows,
            ),
            (
                "actias_directory_backfill_skipped_total",
                &directory.backfill_skipped,
            ),
            (
                "actias_directory_visit_verified_total",
                &directory.visit_verified,
            ),
            (
                "actias_directory_visit_flagged_total",
                &directory.visit_flagged,
            ),
            (
                "actias_directory_visit_recomputed_total",
                &directory.visit_recomputed,
            ),
            (
                "actias_directory_visit_dropped_total",
                &directory.visit_dropped,
            ),
            (
                "actias_directory_overlay_builds_total",
                &directory.overlay_builds,
            ),
            (
                "actias_directory_overlay_build_ms_total",
                &directory.overlay_build_ms_total,
            ),
            (
                "actias_directory_overlay_applies_total",
                &directory.overlay_applies,
            ),
            ("actias_directory_forwarded_total", &directory.forwarded),
            (
                "actias_directory_served_for_peer_total",
                &directory.served_for_peer,
            ),
        ];
        for (name, value) in counters {
            out.push_str(&format!("# TYPE {name} counter\n{name} {}\n", count(value)));
        }
        out.push_str("# TYPE actias_directory_backfill_remaining gauge\n");
        out.push_str(&format!(
            "actias_directory_backfill_remaining {}\n",
            load(&directory.backfill_remaining)
        ));
        out.push_str("# TYPE actias_directory_refusals_total counter\n");
        for (class, refused) in directory.refusals() {
            out.push_str(&format!(
                "actias_directory_refusals_total{{class=\"{class}\"}} {refused}\n"
            ));
        }
        // The content-addressed file cache: reads against the fetches
        // that reached the store.
        out.push_str("# TYPE actias_directory_file_reads_total counter\n");
        out.push_str(&format!(
            "actias_directory_file_reads_total {}\n",
            directory_files.0
        ));
        out.push_str("# TYPE actias_directory_file_fetches_total counter\n");
        out.push_str(&format!(
            "actias_directory_file_fetches_total {}\n",
            directory_files.1
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_requests_appear_in_the_exposition() {
        let metrics = Metrics::default();
        metrics.record("proj-1", "my-script", Duration::from_millis(12), true);
        metrics.record("proj-1", "my-script", Duration::from_millis(8), false);

        let directory = crate::directory::gauges::DirectoryGauges::default();
        directory.count(&directory.folds);
        directory.refused("Lot");
        let text = metrics.render(
            3,
            &Default::default(),
            &Default::default(),
            &crate::objects::replica::ReplicaGauges::default(),
            (&crate::vm_pool::VmPoolGauges::default(), 0),
            &directory,
            (7, 2),
        );

        assert!(text.contains("actias_requests_total{project=\"proj-1\",script=\"my-script\"} 2"));
        assert!(
            text.contains("actias_request_errors_total{project=\"proj-1\",script=\"my-script\"} 1")
        );
        assert!(text.contains(
            "actias_request_duration_ms_total{project=\"proj-1\",script=\"my-script\"} 20"
        ));
        assert!(text.contains("actias_objects_resident 3"));
        assert!(text.contains("actias_directory_folds_total 1\n"));
        assert!(text.contains("actias_directory_refusals_total{class=\"Lot\"} 1\n"));
        assert!(text.contains("actias_directory_file_reads_total 7\n"));
        assert!(text.contains("actias_directory_file_fetches_total 2\n"));
    }
}
