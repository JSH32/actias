use colored::*;
use std::path::Path;

use crate::{
    analyze, capabilities,
    errors::{Error, Result},
    script::ScriptConfig,
};

/// Handle the Check command
pub fn handle(directory: &str) -> Result<()> {
    let config = ScriptConfig::from_path(Path::new(directory)).map_err(Error::Script)?;

    // The same declaration pass publish runs, so a project that checks
    // cleanly also publishes cleanly.
    let declared = capabilities::extract(&config).map_err(Error::Script)?;

    println!("{}", "📜 Project validated!".green());
    if !declared.kv.is_empty() {
        println!("📦 Declares kv: {}", declared.kv.join(", ").purple());
    }
    if !declared.events.is_empty() {
        println!("⚡ Handles events: {}", declared.events.join(", ").purple());
    }
    if !declared.secrets.is_empty() {
        println!(
            "🔐 Declares secrets: {}",
            declared.secrets.join(", ").purple()
        );
    }
    if !declared.objects.is_empty() {
        println!(
            "🧩 Declares objects: {}",
            declared.objects.join(", ").purple()
        );
    }
    if !declared.databases.is_empty() {
        println!(
            "🗄️ Declares databases: {}",
            declared
                .databases
                .iter()
                .map(|entry| entry.split('=').next().unwrap_or(entry))
                .collect::<Vec<_>>()
                .join(", ")
                .purple()
        );
    }
    if !declared.queues.is_empty() {
        println!(
            "📬 Declares queues: {}",
            declared.queues.join(", ").purple()
        );
    }
    if !declared.workflows.is_empty() {
        println!(
            "🧭 Declares workflows: {}",
            declared.workflows.join(", ").purple()
        );
    }
    if !declared.publishes.is_empty() {
        println!("📡 Publishes: {}", declared.publishes.join(", ").purple());
    }
    if !declared.receives.is_empty() {
        println!("📥 Receives: {}", declared.receives.join(", ").purple());
    }
    let migrations: Vec<String> = declared
        .databases
        .iter()
        .chain(declared.objects.iter())
        .filter(|entry| entry.contains('='))
        .cloned()
        .collect();
    if !migrations.is_empty() {
        println!(
            "🗂️ Migrations: {}",
            migrations
                .iter()
                .map(|entry| entry.replacen('=', " from ", 1))
                .collect::<Vec<_>>()
                .join(", ")
                .purple()
        );
    }

    // Flow cross-references (the sixteenth revision's checkable half):
    // a follow without a handler is dead delivery, and an in-project
    // publisher must actually publish what a receives entry consumes.
    let mut flow_errors: Vec<String> = Vec::new();
    for entry in &declared.receives {
        let Some((consumer, stream)) = entry.split_once("<-") else {
            continue;
        };
        let Some((source, _topic)) = stream.split_once(':') else {
            continue;
        };
        let published = declared
            .publishes
            .iter()
            .any(|p| p.split('=').next() == Some(stream));
        if declared.objects.iter().any(|class| class == source) && !published {
            flow_errors.push(format!(
                "'{consumer}' receives '{stream}', but '{source}' never publishes it."
            ));
        }
    }
    for site in &declared.follow_sites {
        let handled = declared.receives.iter().any(|entry| {
            entry
                .split_once("<-")
                .is_some_and(|(_, stream)| stream == site)
        });
        if !handled {
            flow_errors.push(format!(
                "state:follow targets '{site}', but no class declares receives[\"{site}\"]: \
                 deliveries would be discarded."
            ));
        }
    }
    let bundle_dirs: std::collections::HashSet<String> = config
        .to_bundle()
        .map(|bundle| {
            bundle
                .files
                .iter()
                .filter(|file| file.file_path.ends_with(".sql"))
                .filter_map(|file| {
                    file.file_path
                        .rsplit_once('/')
                        .map(|(dir, _)| dir.to_owned())
                })
                .collect()
        })
        .unwrap_or_default();

    // A queue and its handler only work as a pair: a declared queue
    // with no `on "queue:name"` accepts sends and never delivers them,
    // and a handler for an undeclared queue can never be sent to.
    for queue in &declared.queues {
        let handler = format!("queue:{queue}");
        if !declared.events.contains(&handler) {
            flow_errors.push(format!(
                "queue \"{queue}\" is declared, but nothing handles it; \
                 sends would sit undelivered. Add on \"{handler}\"."
            ));
        }
    }
    for event in &declared.events {
        if let Some(queue) = event.strip_prefix("queue:")
            && !declared.queues.iter().any(|declared| declared == queue)
        {
            flow_errors.push(format!(
                "on \"{event}\" handles a queue this project never declares; \
                 nothing can send to it. Add queue \"{queue}\"."
            ));
        }
    }

    for entry in &migrations {
        let Some((name, dir)) = entry.split_once('=') else {
            continue;
        };
        let dir = dir.trim_end_matches('/');
        if !bundle_dirs.contains(dir) {
            flow_errors.push(format!(
                "'{name}' declares migrations in '{dir}', which holds no .sql files."
            ));
        }
    }
    for dir in &bundle_dirs {
        let declared_here = migrations
            .iter()
            .filter_map(|entry| entry.split_once('='))
            .any(|(_, declared)| declared.trim_end_matches('/') == dir);
        if !declared_here {
            flow_errors.push(format!(
                "'{dir}' holds migrations nothing declares; add \
                 migrations = \"{dir}\" to the database or class that owns it."
            ));
        }
    }

    if !flow_errors.is_empty() {
        for error in &flow_errors {
            println!("❌ {}", error.red());
        }
        return Err(Error::Script(format!(
            "{} contract error(s) in follows, receives, publishes, queues or migrations.",
            flow_errors.len()
        )));
    }

    // Strict types over the whole bundle, with the platform surface shadowed
    // in; diagnostics point at the user's own lines.
    analyze::analyze(&config).map_err(Error::Script)?;
    println!("{}", "🔎 Types check out (luau-analyze).".green());

    Ok(())
}
