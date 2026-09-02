//! `actias objects`: a project's durable object instances, with their
//! residency and lifetime as the placement store reports them.

use colored::*;
use prettytable::{Table, row};

use crate::{
    client::Client,
    commands::ObjectOperations,
    errors::{Result, progenitor_error},
};

/// "in 3d" / "now" from a unix-ms deadline; a dash means never.
fn due(ms: f64) -> String {
    if ms <= 0.0 {
        return "-".to_owned();
    }
    let left = ms as i64
        - std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
    if left <= 0 {
        return "now".to_owned();
    }
    let (value, unit) = match left {
        ms if ms < 3_600_000 => (ms / 60_000, "m"),
        ms if ms < 86_400_000 => (ms / 3_600_000, "h"),
        ms => (ms / 86_400_000, "d"),
    };
    format!("in {}{unit}", value.max(1))
}

pub async fn handle(client: &Client, project: &str, operation: ObjectOperations) -> Result<()> {
    match operation {
        ObjectOperations::List { class, page } => {
            let mut request = client.list_objects().project(project);
            if let Some(class) = &class {
                request = request.class(class);
            }
            request = request.page(page.unwrap_or(0) as f64).page_size(100.0);
            let response = request.send().await.map_err(progenitor_error)?.into_inner();

            let mut table = Table::new();
            table.add_row(row![
                "Class",
                "Name",
                "Status",
                "Expires",
                "Alarm",
                "Declared By"
            ]);
            for item in &response.items {
                let status = if item.deleted_at_ms > 0.0 {
                    "deleting"
                } else if !item.node_id.is_empty() {
                    "resident"
                } else {
                    "cold"
                };
                table.add_row(row![
                    item.class,
                    item.name,
                    status,
                    due(item.expire_at_ms),
                    due(item.alarm_due_ms),
                    item.declared_by
                ]);
            }
            table.printstd();
            println!(
                "{} of {} instance(s)",
                response.items.len().to_string().yellow(),
                (response.total as u64).to_string().yellow()
            );
        }
        ObjectOperations::Delete { class, name } => {
            let outcome = client
                .delete_object()
                .project(project)
                .class(&class)
                .name(&name)
                .send()
                .await
                .map_err(progenitor_error)?
                .into_inner();
            if outcome.deleting > 0.0 {
                println!(
                    "Deleting {class} \"{name}\"; the name may be recreated and starts fresh."
                );
            } else {
                println!("Nothing to delete: no live {class} \"{name}\".");
            }
        }
        ObjectOperations::Rebuild { class } => {
            let rebuilt = client
                .object_directory_rebuild()
                .project(project)
                .class(&class)
                .send()
                .await
                .map_err(progenitor_error)?
                .into_inner();
            if !rebuilt.held {
                println!("Another node is rebuilding {class} right now; its work is this work.");
                return Ok(());
            }
            println!(
                "Rebuilt {class}: {} row(s) from {} live instance(s), {} retired.",
                (rebuilt.rows as u64).to_string().yellow(),
                (rebuilt.live as u64).to_string().yellow(),
                (rebuilt.tombstones as u64).to_string().yellow()
            );
            if rebuilt.without_row > 0.0 {
                // Not a failure: nothing has settled for those objects
                // yet, so there is no row to copy. A large count here
                // means a backfill is the thing actually needed.
                println!(
                    "{} live instance(s) had no row to recover yet.",
                    (rebuilt.without_row as u64).to_string().yellow()
                );
            }
        }
        ObjectOperations::DeleteClass { class } => {
            let outcome = client
                .delete_class()
                .project(project)
                .class(&class)
                .send()
                .await
                .map_err(progenitor_error)?
                .into_inner();
            println!(
                "Deleting {} instance(s) of {class}.",
                (outcome.deleting as u64).to_string().yellow()
            );
        }
    }
    Ok(())
}
