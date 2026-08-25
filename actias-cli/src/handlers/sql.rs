//! Database tooling. Migrations are bundle files the declaration points
//! at, applied in file order by the platform at the database's first
//! touch. The scaffold writes to `migrations/<database>/` and its only
//! job is the next number; the declaration is what makes them apply.

use colored::*;
use std::path::Path;

use crate::{
    commands::SqlOperations,
    errors::{Error, Result},
};

pub fn handle(database: &str, operation: &SqlOperations) -> Result<()> {
    match operation {
        SqlOperations::Create { name, directory } => create(database, name, directory),
    }
}

fn create(database: &str, name: &str, directory: &str) -> Result<()> {
    let dir = Path::new(directory).join("migrations").join(database);
    std::fs::create_dir_all(&dir).map_err(|e| Error::Io(e.to_string()))?;

    // The platform applies migrations sorted by path, so the number is
    // the ordering.
    let next = std::fs::read_dir(&dir)
        .map_err(|e| Error::Io(e.to_string()))?
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name();
            let file = file.to_str()?;
            file.split('_').next()?.parse::<u32>().ok()
        })
        .max()
        .unwrap_or(0)
        + 1;

    let file = dir.join(format!("{next:04}_{name}.sql"));
    std::fs::write(
        &file,
        "-- Applied by the platform at the database's first touch, inside\n\
         -- the touching call's transaction. Statements end with semicolons.\n",
    )
    .map_err(|e| Error::Io(e.to_string()))?;

    println!("📝 {}", file.display().to_string().purple());
    println!(
        "   Applied once declared: {}",
        format!(
            "database \"{database}\" {{ migrations = \"migrations/{database}\" }}"
        )
        .dimmed()
    );
    Ok(())
}
