use colored::*;
use std::path::Path;

use crate::{
    errors::{Error, Result},
    script::ScriptConfig,
};

/// Handle the Check command
pub fn handle(directory: &str) -> Result<()> {
    match ScriptConfig::from_path(Path::new(directory)) {
        Ok(_) => {
            println!("{}", "📜 Project validated!".green());
            Ok(())
        }
        Err(e) => Err(Error::Script(e)),
    }
}
