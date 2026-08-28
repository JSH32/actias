//! Making a service's own database exist.
//!
//! A migrator's first act is to reach a database that may not have been
//! created yet: compose seeds them from `docker/init.sql`, but that
//! script only runs on a cluster's FIRST init, so a volume that
//! predates a service leaves it with nowhere to migrate (found
//! 2026-08-26 against a pre-existing volume). Every managed postgres
//! has the same gap in its own way.
//!
//! So the migrator creates what it needs: on 3D000 (the server is
//! there, the database is not) it connects to the maintenance database
//! beside it and issues the CREATE. Any other error is the caller's to
//! report, because "the server refused us" and "the database is
//! missing" want different answers.

use sqlx::{Connection, Executor, PgConnection};

/// The database name at the end of a postgres url, with whatever query
/// string follows removed.
fn database_of(url: &str) -> Option<(String, String)> {
    let (base, tail) = url.rsplit_once('/')?;
    let name = tail.split(['?', '#']).next().unwrap_or(tail);
    if name.is_empty() {
        return None;
    }
    Some((base.to_owned(), name.to_owned()))
}

/// Creates `url`'s database when the server is reachable but the
/// database is not there. Succeeds silently when it already exists,
/// which is every run after the first.
///
/// # Errors
/// Returns the connection's own error text when the server refuses the
/// attempt for any reason other than a missing database, and the
/// CREATE's text when creating fails.
pub async fn ensure_database(url: &str) -> Result<(), String> {
    match PgConnection::connect(url).await {
        Ok(mut connection) => {
            let _ = connection.close().await;
            return Ok(());
        }
        // 3D000, invalid_catalog_name: the server answered, and what it
        // said was that this database does not exist.
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("3D000") => {}
        Err(error) => return Err(error.to_string()),
    }

    let Some((base, name)) = database_of(url) else {
        return Err(format!("no database name in the url: {url}"));
    };
    // The maintenance database every postgres server carries; a server
    // without it is one this platform cannot bootstrap anyway.
    let mut admin = PgConnection::connect(&format!("{base}/postgres"))
        .await
        .map_err(|error| format!("connecting to create '{name}': {error}"))?;
    // The name comes from our own deployment config, never from a
    // request, and postgres has no parameter form for a database name.
    let created = admin
        .execute(format!("CREATE DATABASE \"{}\"", name.replace('"', "\"\"")).as_str())
        .await;
    let _ = admin.close().await;
    match created {
        Ok(_) => Ok(()),
        // Another migrator replica won the race, which is a success.
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("42P04") => Ok(()),
        Err(error) => Err(format!("creating '{name}': {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_yields_its_database_and_server() {
        assert_eq!(
            database_of("postgresql://u:p@host/actias_kv"),
            Some(("postgresql://u:p@host".to_owned(), "actias_kv".to_owned()))
        );
        // Query strings ride along on managed providers; the name stops
        // before them.
        assert_eq!(
            database_of("postgres://u:p@host:5432/actias_api?sslmode=require")
                .map(|(_, name)| name),
            Some("actias_api".to_owned())
        );
        assert_eq!(database_of("postgresql://u:p@host/"), None);
    }
}
