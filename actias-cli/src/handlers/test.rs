//! `actias test`: the project's tests, on the platform's own runtime.

use colored::*;
use std::path::Path;

use crate::{
    errors::{Error, Result},
    script::ScriptConfig,
    testing,
    util::get_dir,
};

pub async fn handle(script_dir: &str) -> Result<()> {
    let script_path = get_dir(script_dir, false, false).map_err(Error::Io)?;
    let config = ScriptConfig::from_path(Path::new(&script_path)).map_err(Error::Script)?;

    let summary = testing::run_tests(&config).await.map_err(Error::Script)?;

    println!(
        "\n{} passed, {} failed",
        summary.passed.to_string().green(),
        summary.failed.to_string().red(),
    );

    if summary.failed > 0 {
        return Err(Error::Command(format!("{} test(s) failed", summary.failed)));
    }

    Ok(())
}
