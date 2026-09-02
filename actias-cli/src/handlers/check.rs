//! `actias check`: the declaration pass and the type check, the same two
//! refusals a publish would make, without publishing anything.

use std::path::Path;

use crate::{
    analyze, capabilities,
    errors::{Error, Result},
    script::ScriptConfig,
    ui,
};

/// Runs `actias check`: the declaration pass and the type check over
/// one project.
///
/// # Errors
/// Returns the declaration pass's or the type check's refusal, naming
/// what it refused.
pub fn handle(directory: &str) -> Result<()> {
    let config = ScriptConfig::from_path(Path::new(directory)).map_err(Error::Script)?;

    // The same declaration pass publish runs, so a project that checks
    // cleanly also publishes cleanly.
    let declared = capabilities::extract(&config).map_err(Error::Script)?;

    ui::done("Validated", &config.entry_point);
    if !declared.kv.is_empty() {
        ui::done("Declares", format!("kv {}", declared.kv.join(", ")));
    }
    if !declared.events.is_empty() {
        ui::done("Handles", declared.events.join(", "));
    }
    if !declared.secrets.is_empty() {
        ui::done(
            "Declares",
            format!("secrets {}", declared.secrets.join(", ")),
        );
    }
    if !declared.objects.is_empty() {
        ui::done(
            "Declares",
            format!("objects {}", declared.objects.join(", ")),
        );
    }
    if !declared.databases.is_empty() {
        let names = declared
            .databases
            .iter()
            .map(|entry| entry.split('=').next().unwrap_or(entry))
            .collect::<Vec<_>>()
            .join(", ");
        ui::done("Declares", format!("databases {names}"));
    }
    if !declared.queues.is_empty() {
        ui::done("Declares", format!("queues {}", declared.queues.join(", ")));
    }
    if !declared.connections.is_empty() {
        ui::done(
            "Declares",
            format!("connections {}", declared.connections.join(", ")),
        );
    }
    if !declared.workflows.is_empty() {
        ui::done(
            "Declares",
            format!("workflows {}", declared.workflows.join(", ")),
        );
    }
    if !declared.lifecycle.is_empty() {
        ui::done(
            "Declares",
            format!("lifecycle {}", declared.lifecycle.join(", ")),
        );
    }
    if !declared.publishes.is_empty() {
        ui::done("Publishes", declared.publishes.join(", "));
    }
    if !declared.receives.is_empty() {
        ui::done("Receives", declared.receives.join(", "));
    }
    let migrations: Vec<String> = declared
        .databases
        .iter()
        .chain(declared.objects.iter())
        .filter(|entry| entry.contains('='))
        .cloned()
        .collect();
    if !migrations.is_empty() {
        let listed = migrations
            .iter()
            .map(|entry| entry.replacen('=', " from ", 1))
            .collect::<Vec<_>>()
            .join(", ");
        ui::done("Migrations", listed);
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
            ui::error("check", error);
        }
        return Err(Error::Script(format!(
            "{} contract error(s) in follows, receives, publishes, queues or migrations.",
            flow_errors.len()
        )));
    }

    // Strict types over the whole bundle, with the platform surface shadowed
    // in; diagnostics point at the user's own lines.
    analyze::analyze(&config).map_err(Error::Script)?;
    ui::done(
        "Checked",
        // Which checker ran is worth saying: they do not see the same
        // things, and the linked one is the editor's.
        if crate::service::locate().is_some() {
            "types (language service)"
        } else {
            "types (luau-analyze, cross-module types are any)"
        },
    );

    Ok(())
}
