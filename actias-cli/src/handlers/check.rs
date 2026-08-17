use colored::*;
use std::path::Path;

use crate::{
    capabilities,
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

    Ok(())
}
