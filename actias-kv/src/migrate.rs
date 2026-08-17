//! Applies the CQL migrations embedded in this binary.
//!
//! Replaces the JVM-based cqlmigrate: the service image run with `--migrate`
//! is the whole migration story. `bootstrap.cql` (the keyspace) runs every
//! time and is idempotent; the numbered files run once each, recorded in
//! `schema_migrations` after they apply. A crash between applying and
//! recording means the file runs again on the next attempt, which is why
//! migrations here are written to be safe to re-run (IF NOT EXISTS and
//! friends).

use include_dir::{Dir, include_dir};
use scylla::{Session, SessionBuilder};

static MIGRATIONS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

/// Splits a CQL source file into executable statements.
///
/// Comment lines go first, because a `;` inside a comment would otherwise
/// split a statement apart.
fn split_statements(source: &str) -> Vec<String> {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n")
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .map(str::to_owned)
        .collect()
}

async fn run_file(session: &Session, name: &str, source: &str) -> Result<(), String> {
    for statement in split_statements(source) {
        session
            .query(statement.as_str(), ())
            .await
            .map_err(|e| format!("{name}: {e}"))?;
    }

    Ok(())
}

/// Applies every pending migration through an existing session.
///
/// # Errors
/// Returns the failing file and cause as text; the caller decides whether
/// that is fatal.
pub async fn apply(session: &Session) -> Result<(), String> {
    let bootstrap = MIGRATIONS
        .get_file("bootstrap.cql")
        .and_then(|f| f.contents_utf8())
        .ok_or("bootstrap.cql is missing from the embedded migrations")?;

    run_file(session, "bootstrap.cql", bootstrap).await?;

    session
        .query(
            "CREATE TABLE IF NOT EXISTS kv_service.schema_migrations ( \
                name text PRIMARY KEY, applied_at timestamp)",
            (),
        )
        .await
        .map_err(|e| format!("schema_migrations: {e}"))?;

    // Lexical order is application order, which the NNNN- prefix guarantees.
    let mut files: Vec<_> = MIGRATIONS
        .files()
        .filter(|f| {
            f.path().extension().is_some_and(|e| e == "cql")
                && f.path().file_name().is_some_and(|n| n != "bootstrap.cql")
        })
        .collect();
    files.sort_by_key(|f| f.path().to_path_buf());

    for file in files {
        let name = file.path().display().to_string();

        let applied = session
            .query(
                "SELECT name FROM kv_service.schema_migrations WHERE name = ?",
                (name.as_str(),),
            )
            .await
            .map_err(|e| format!("{name}: {e}"))?
            .rows_or_empty();

        if !applied.is_empty() {
            continue;
        }

        let source = file
            .contents_utf8()
            .ok_or_else(|| format!("{name} is not utf-8"))?;
        run_file(session, &name, source).await?;

        session
            .query(
                "INSERT INTO kv_service.schema_migrations (name, applied_at) \
                 VALUES (?, toTimestamp(now()))",
                (name.as_str(),),
            )
            .await
            .map_err(|e| format!("{name}: {e}"))?;
    }

    Ok(())
}

/// Connects to the given nodes and applies every pending migration.
///
/// Retries the connection, because the migrator typically races the database
/// it migrates out of a cold start.
///
/// # Errors
/// Returns text describing what failed; the process exits nonzero on it.
pub async fn run(scylla_nodes: Vec<String>) -> Result<(), String> {
    let mut session = None;
    for _ in 0..60 {
        match SessionBuilder::new()
            .known_nodes(&scylla_nodes)
            .build()
            .await
        {
            Ok(s) => {
                session = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
        }
    }

    let session = session.ok_or("scylla did not accept a connection in time")?;
    apply(&session).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_semicolon_inside_a_comment_does_not_split_a_statement() {
        // The exact failure that motivated comment stripping: a comment ending
        // in ';' turned everything before it into a bogus statement.
        let statements = split_statements(
            "-- this comment mentions a partition; which ends in a semicolon\n\
             CREATE TABLE t (\n\
                 -- an inline comment too;\n\
                 id int PRIMARY KEY\n\
             );",
        );

        assert_eq!(statements.len(), 1);
        assert!(statements[0].starts_with("CREATE TABLE t"));
        assert!(!statements[0].contains("comment"));
    }

    #[test]
    fn empty_fragments_and_trailing_whitespace_produce_no_statements() {
        assert!(split_statements("-- only a comment\n\n  \n").is_empty());
        assert_eq!(split_statements("SELECT 1;\n\n;;\n").len(), 1);
    }
}
