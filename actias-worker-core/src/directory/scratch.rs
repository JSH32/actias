//! Deriving a row from a throwaway copy of an object's settled state.
//!
//! Two callers need a row for an object that is not resident and must
//! not be woken: the backfill, which walks rows a newer publish left
//! behind, and the verified read's tail, where the last derivation
//! failed and the stored row is only the last good one. Both want the
//! row the object would produce, without a lease, without residency,
//! and without disturbing the object at all.
//!
//! Restore settled state to a scratch file, apply the class's migration
//! ladder to that copy, evaluate read-only, take the row, discard
//! everything. Applying the ladder to scratch is legitimate for the
//! same reason restore-by-replay is: migrations are a deterministic
//! append-only ladder over data that is not changing, so the scratch
//! copy's post-ladder state is exactly what first-touch would produce.
//! Nothing durable advances, and no publish ever wakes an object.
//!
//! **This is deliberately not the steady path.** Deriving needs guest
//! code running against object state, and both are only cheap where the
//! write path already has them: a warm vm and an open file. Here each
//! evaluation pays a restore, so it belongs to work that runs once per
//! object per schema change, never once per write.
//!
//! # The hazard this module exists to contain
//!
//! [`crate::objects::spawn_object_task`] loads the file's pending alarm
//! and arms it. Against a restored copy that would fire a real handler
//! on a throwaway, so this never spawns a task. It builds a home
//! directly with no pending alarm, no alarm mirror, no after-write gate,
//! no claim refresh and no destroy sequence: none of them exist to be
//! triggered, rather than existing and being suppressed.

use std::sync::Arc;

use crate::objects::ObjectHome;
use crate::runtime::{ActiasRuntime, PreparedRevision};
use crate::storage::SqliteStorage;

/// What one scratch evaluation produced.
#[derive(Debug)]
pub enum Scratched {
    /// The row the object's settled state derives to now.
    Row(super::evaluate::Row),
    /// The class declares no directory, so there is nothing to derive.
    NoDirectory,
}

/// Derives `class`/`name`'s row from an already-restored scratch file.
///
/// The caller owns the restore and the cleanup: this takes a runtime
/// whose script has run (so the class is registered) and a storage
/// handle on the scratch copy, and does the rest. Splitting it there
/// keeps s3 out of the kernel.
///
/// # Errors
/// The derive's own failure, verbatim: a throw, a blown budget, a shape
/// the kernel refuses, or a value that does not conform to the class's
/// declaration. Every one of them is the caller's to contain; a
/// backfill marks the object and moves on rather than failing the pass.
pub fn evaluate_scratch(
    runtime: &ActiasRuntime,
    revision: Arc<PreparedRevision>,
    storage: SqliteStorage,
    class: &str,
    name: &str,
    budget_ms: u64,
) -> Result<Scratched, String> {
    // No pending alarm and no alarm mirror: an armed alarm on a
    // throwaway copy would fire a handler for an object nobody called.
    let home = Arc::new(ObjectHome::for_scratch(storage, Some(revision.clone())));

    // The ladder first, exactly as first-touch would: a row derived
    // from a pre-migration copy would be a row the live object will
    // never produce.
    if let Some(dir) = crate::extensions::objects::class_migrations(runtime, class) {
        crate::platform::database::Database::apply_declared_migrations(&home, &dir)?;
    }

    // The registry reads the class table off the runtime, and the
    // derive reaches storage through app data, so the home has to be
    // installed before the call and taken back after: a scratch home
    // outliving this call would let ordinary dispatch write to a file
    // that is about to be deleted.
    let previous = runtime.remove_app_data::<Arc<ObjectHome>>();
    runtime.set_app_data::<Arc<ObjectHome>>(home.clone());

    runtime.begin_short_budget(budget_ms);
    let derived = crate::extensions::objects::derive_directory(runtime, class, name);
    runtime.end_call_budget();

    runtime.remove_app_data::<Arc<ObjectHome>>();
    if let Some(previous) = previous {
        runtime.set_app_data::<Arc<ObjectHome>>(previous);
    }

    let Some(derived) = derived else {
        return Ok(Scratched::NoDirectory);
    };
    let row = derived?;

    // The same conformance the write path applies, for the same reason:
    // a value of a kind the declaration did not name would land in a
    // column bound to a different type.
    if let Some(spec) = revision.directory_spec(class) {
        super::evaluate::conform(&row, &spec)?;
    }
    Ok(Scratched::Row(row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::shape::Value;

    /// A runtime over one source, with the class registered but no task
    /// spawned: exactly the shape scratch evaluation runs in.
    async fn runtime(source: &str) -> (ActiasRuntime, Arc<PreparedRevision>) {
        let runtime = crate::objects::testing::runtime_with(source).await;
        let revision = runtime
            .app_data_ref::<Arc<PreparedRevision>>()
            .expect("the runtime carries its revision")
            .clone();
        (runtime, revision)
    }

    fn storage(dir: &std::path::Path, name: &str) -> SqliteStorage {
        SqliteStorage::open(&dir.join(name)).expect("opens")
    }

    const AUCTION: &str = r#"
        local Auction = object "Auction" {
            init = function(state)
                state.sql:exec("CREATE TABLE lot (state TEXT)")
                state.sql:exec("INSERT INTO lot VALUES ('open')")
            end,
            directory = {
                from = function(state)
                    return {
                        state = state.sql:query_one("SELECT state FROM lot").state,
                        bids = state.store:get("bids") or 0,
                    }
                end,
                fields = { state = f.string, bids = f.integer },
            },
        }
        on "fetch" (function() return { body = "ok" } end)
    "#;

    /// Restore, evaluate, discard: the row comes back without the
    /// object ever being resident.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_restored_copy_derives_its_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Stand in for a restore: a file carrying settled state.
        {
            let mut settled = storage(dir.path(), "settled.db");
            settled
                .platform()
                .execute_batch(
                    "CREATE TABLE lot (state TEXT);
                     INSERT INTO lot VALUES ('sold');",
                )
                .expect("seeds");
        }

        let (runtime, revision) = runtime(AUCTION).await;
        let scratched = evaluate_scratch(
            &runtime,
            revision,
            storage(dir.path(), "settled.db"),
            "Auction",
            "lot-a",
            crate::directory::DEFAULT_EVAL_BUDGET_MS,
        )
        .expect("evaluates");

        let Scratched::Row(row) = scratched else {
            panic!("the class declares a directory");
        };
        assert!(row.contains(&("state".to_owned(), Value::Text("sold".to_owned()))));
    }

    /// The hazard this module exists for: spawning a task would load
    /// the file's alarm and arm it, firing a handler against a copy
    /// that is about to be deleted. A scratch home has no alarm at all.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_due_alarm_on_the_copy_fires_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut settled = storage(dir.path(), "armed.db");
            settled
                .platform()
                .execute_batch(
                    "CREATE TABLE lot (state TEXT);
                     INSERT INTO lot VALUES ('open');",
                )
                .expect("seeds");
            // Long past due, so anything that armed it would fire at once.
            settled
                .save_alarm(1, "Auction", "lot-a", "own")
                .expect("arms");
        }

        let (runtime, revision) = runtime(AUCTION).await;
        let scratched = evaluate_scratch(
            &runtime,
            revision,
            storage(dir.path(), "armed.db"),
            "Auction",
            "lot-a",
            crate::directory::DEFAULT_EVAL_BUDGET_MS,
        )
        .expect("evaluates");
        assert!(matches!(scratched, Scratched::Row(_)));

        // Still armed and untouched: nothing consumed or cleared it,
        // because no alarm was ever loaded.
        let mut after = storage(dir.path(), "armed.db");
        assert!(
            after.load_alarm().expect("reads").is_some(),
            "a scratch evaluation must leave the copy's alarm alone"
        );
    }

    /// A class with no directory is not a failure; there is simply
    /// nothing to derive, which a backfill skips.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_class_without_a_directory_derives_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (runtime, revision) = runtime(
            r#"
            local Plain = object "Plain" {
                init = function(state) state.sql:exec("CREATE TABLE t (n INTEGER)") end,
            }
            on "fetch" (function() return { body = "ok" } end)
            "#,
        )
        .await;

        let scratched = evaluate_scratch(
            &runtime,
            revision,
            storage(dir.path(), "plain.db"),
            "Plain",
            "one",
            crate::directory::DEFAULT_EVAL_BUDGET_MS,
        )
        .expect("evaluates");
        assert!(matches!(scratched, Scratched::NoDirectory));
    }
}
