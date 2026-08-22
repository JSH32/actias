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
            declared.databases.join(", ").purple()
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

    // Strict types over the whole bundle, with the platform surface shadowed
    // in; diagnostics point at the user's own lines.
    analyze::analyze(&config).map_err(Error::Script)?;
    println!("{}", "🔎 Types check out (luau-analyze).".green());

    Ok(())
}
