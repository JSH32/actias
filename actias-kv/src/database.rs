use std::{
    io::{self, Write},
    str::FromStr,
};

use base64::{engine::general_purpose, read, write};
use scylla::{
    Bytes, Session, SessionBuilder,
    cql_to_rust::FromRowError,
    prepared_statement::PreparedStatement,
    transport::{errors::QueryError, query_result::FirstRowTypedError},
};
use thiserror::Error;
use uuid::Uuid;

use crate::proto_kv_service::{
    ListNamespacesResponse, ListPairsResponse, Namespace, Pair, PairRequest, ValueType,
};

#[derive(Error, Debug)]
pub enum DatabaseError {
    #[error("{0}")]
    Query(#[from] QueryError),
    #[error("{0}")]
    FirstRowTyped(#[from] FirstRowTypedError),
    #[error("{0}")]
    FromRow(#[from] FromRowError),
    #[error("Invalid data provided: {0}")]
    Invalid(String),
}

/// Data access for the pairs and namespaces tables.
///
/// Every query here addresses a single partition: pairs partition on
/// (project_id, namespace) and the namespace registry partitions on
/// project_id, so nothing needs ALLOW FILTERING and deleting a namespace is
/// one partition tombstone.
pub struct Database {
    session: Session,

    get_statement: PreparedStatement,
    set_statement: PreparedStatement,
    delete_statement: PreparedStatement,
    list_statement: PreparedStatement,
    count_statement: PreparedStatement,

    register_namespace_statement: PreparedStatement,
    list_namespaces_statement: PreparedStatement,
    delete_namespace_pairs_statement: PreparedStatement,
    unregister_namespace_statement: PreparedStatement,
    unregister_all_namespaces_statement: PreparedStatement,
}

impl Database {
    pub async fn new(scylla_nodes: Vec<String>) -> Self {
        let session = SessionBuilder::new()
            .known_nodes(scylla_nodes)
            .use_keyspace("kv_service", true)
            .build()
            .await
            .expect("scylla session could not be established from SCYLLA_NODES");

        let prepare = |cql: &'static str| {
            let session = &session;
            async move {
                session
                    .prepare(cql)
                    .await
                    .unwrap_or_else(|e| panic!("failed to prepare {cql:?}: {e}"))
            }
        };

        let get_statement = prepare(
            "SELECT TTL(value), project_id, namespace, key, value, type \
             FROM pairs WHERE project_id = ? AND namespace = ? AND key = ?",
        )
        .await;

        let set_statement = prepare(
            "UPDATE pairs USING TTL ? SET value = ?, type = ? \
             WHERE project_id = ? AND namespace = ? AND key = ?",
        )
        .await;

        let delete_statement =
            prepare("DELETE FROM pairs WHERE project_id = ? AND namespace = ? AND key = ?").await;

        let list_statement = prepare(
            "SELECT TTL(value), project_id, namespace, key, value, type \
             FROM pairs WHERE project_id = ? AND namespace = ?",
        )
        .await;

        let count_statement =
            prepare("SELECT COUNT(*) FROM pairs WHERE project_id = ? AND namespace = ?").await;

        let register_namespace_statement =
            prepare("INSERT INTO namespaces (project_id, name) VALUES (?, ?)").await;

        let list_namespaces_statement =
            prepare("SELECT name FROM namespaces WHERE project_id = ?").await;

        let delete_namespace_pairs_statement =
            prepare("DELETE FROM pairs WHERE project_id = ? AND namespace = ?").await;

        let unregister_namespace_statement =
            prepare("DELETE FROM namespaces WHERE project_id = ? AND name = ?").await;

        let unregister_all_namespaces_statement =
            prepare("DELETE FROM namespaces WHERE project_id = ?").await;

        Self {
            session,
            get_statement,
            set_statement,
            delete_statement,
            list_statement,
            count_statement,
            register_namespace_statement,
            list_namespaces_statement,
            delete_namespace_pairs_statement,
            unregister_namespace_statement,
            unregister_all_namespaces_statement,
        }
    }

    fn project_uuid(project_id: &str) -> Result<Uuid, DatabaseError> {
        Uuid::from_str(project_id).map_err(|e| DatabaseError::Invalid(e.to_string()))
    }

    /// Gets a pair from the database.
    ///
    /// # Arguments
    /// * `project_id` - Project ID.
    /// * `namespace` - Namespace.
    /// * `key` - Key to get.
    ///
    /// # Errors
    /// Returns [`DatabaseError::Invalid`] when the stored type metadata does
    /// not name a [`ValueType`].
    pub async fn get(
        &self,
        project_id: &str,
        namespace: &str,
        key: &str,
    ) -> Result<Option<Pair>, DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        match self
            .session
            .execute(&self.get_statement, (project_id, namespace, key))
            .await?
            .first_row()
        {
            Ok(row) => Ok(Some(Self::row_into_pair(row)?)),
            Err(_) => Ok(None),
        }
    }

    /// Updates/creates pairs. This overwrites and uses a last-write-wins
    /// strategy, and registers each touched namespace so listing can answer
    /// from the registry.
    ///
    /// # Arguments
    /// * `pairs` - List of pairs.
    pub async fn set(&self, pairs: Vec<Pair>) -> Result<(), DatabaseError> {
        for pair in pairs {
            let value_type: String = pair.r#type().into();
            let project_id = Self::project_uuid(&pair.project_id)?;

            self.session
                .execute(
                    &self.set_statement,
                    (
                        pair.ttl.unwrap_or(0),
                        pair.value,
                        value_type,
                        project_id,
                        pair.namespace.clone(),
                        pair.key,
                    ),
                )
                .await?;

            // Plain INSERT is an upsert in CQL, so registering on every write
            // costs one extra statement and needs no read or LWT.
            self.session
                .execute(
                    &self.register_namespace_statement,
                    (project_id, pair.namespace),
                )
                .await?;
        }

        Ok(())
    }

    /// Deletes pairs from the database.
    ///
    /// # Arguments
    /// * `pairs` - Pairs to delete, addressed by (project, namespace, key).
    pub async fn delete(&self, pairs: Vec<PairRequest>) -> Result<(), DatabaseError> {
        for pair in pairs {
            let project_id = Self::project_uuid(&pair.project_id)?;

            self.session
                .execute(
                    &self.delete_statement,
                    (project_id, pair.namespace, pair.key),
                )
                .await?;
        }

        Ok(())
    }

    /// Registers a namespace with no data in it.
    ///
    /// Writes also register implicitly; this exists so a namespace can be
    /// created ahead of any data.
    pub async fn create_namespace(
        &self,
        project_id: &str,
        namespace: &str,
    ) -> Result<(), DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        self.session
            .execute(&self.register_namespace_statement, (project_id, namespace))
            .await?;

        Ok(())
    }

    /// Deletes an entire namespace: one partition tombstone for the pairs,
    /// one registry row.
    ///
    /// # Arguments
    /// * `project_id` - Project ID.
    /// * `namespace` - Namespace to delete.
    pub async fn delete_namespace(
        &self,
        project_id: &str,
        namespace: &str,
    ) -> Result<(), DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        self.session
            .execute(
                &self.delete_namespace_pairs_statement,
                (project_id, namespace),
            )
            .await?;

        self.session
            .execute(
                &self.unregister_namespace_statement,
                (project_id, namespace),
            )
            .await?;

        Ok(())
    }

    /// Deletes every namespace a project has.
    ///
    /// # Arguments
    /// * `project_id` - Project ID.
    pub async fn delete_project(&self, project_id: &str) -> Result<(), DatabaseError> {
        let project_uuid = Self::project_uuid(project_id)?;

        let namespaces = self
            .session
            .execute(&self.list_namespaces_statement, (project_uuid,))
            .await?
            .rows_or_empty();

        for row in namespaces {
            let (name,) = row.into_typed::<(String,)>()?;

            self.session
                .execute(&self.delete_namespace_pairs_statement, (project_uuid, name))
                .await?;
        }

        self.session
            .execute(&self.unregister_all_namespaces_statement, (project_uuid,))
            .await?;

        Ok(())
    }

    /// Gets a project's namespaces from the registry, with a per-partition
    /// pair count for each.
    ///
    /// # Arguments
    /// * `project_id` - Project ID.
    pub async fn get_namespaces(
        &self,
        project_id: &str,
    ) -> Result<ListNamespacesResponse, DatabaseError> {
        let project_uuid = Self::project_uuid(project_id)?;

        let mut namespaces = vec![];

        for row in self
            .session
            .execute(&self.list_namespaces_statement, (project_uuid,))
            .await?
            .rows_or_empty()
        {
            let (name,) = row.into_typed::<(String,)>()?;

            // COUNT over one partition, not the cluster; acceptable at the
            // sizes a dashboard lists. Expired rows fall out automatically.
            let (count,) = self
                .session
                .execute(&self.count_statement, (project_uuid, name.as_str()))
                .await?
                .first_row_typed::<(i64,)>()?;

            namespaces.push(Namespace {
                project_id: project_id.to_string(),
                name,
                count: count as i32,
            });
        }

        Ok(ListNamespacesResponse { namespaces })
    }

    /// Lists pairs from a namespace, one page at a time.
    ///
    /// # Arguments
    /// * `project_id` - Project ID.
    /// * `namespace` - Namespace.
    /// * `page_size` - Page size.
    /// * `token` - Optional paging token from the previous page.
    ///
    /// # Errors
    /// Returns [`DatabaseError::Invalid`] when the token is not one this
    /// service handed out.
    pub async fn list(
        &self,
        project_id: &str,
        namespace: &str,
        page_size: i32,
        token: Option<String>,
    ) -> Result<ListPairsResponse, DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        let paging_state = match token {
            None => None,
            Some(v) => {
                let mut output = Vec::new();

                let mut decoder =
                    read::DecoderReader::new(v.as_bytes(), &general_purpose::STANDARD_NO_PAD);

                io::copy(&mut decoder, &mut output)
                    .map_err(|_| DatabaseError::Invalid("Invalid token provided".to_string()))?;

                Some(Bytes::from(output))
            }
        };

        let mut statement = self.list_statement.clone();
        statement.set_page_size(page_size);

        let page = self
            .session
            .execute_paged(&statement, (project_id, namespace), paging_state)
            .await?;

        let token = match page.paging_state.clone() {
            Some(state) => {
                // The driver can hand back a state even when the partition is
                // exhausted; probing with a count from that position keeps the
                // last page from advertising a next page. Partition-local, so
                // the probe is cheap.
                let (remaining,) = self
                    .session
                    .execute_paged(
                        &self.count_statement,
                        (project_id, namespace),
                        Some(state.clone()),
                    )
                    .await?
                    .first_row_typed::<(i64,)>()?;

                if remaining > 0 {
                    let mut output = String::new();

                    write::EncoderStringWriter::from_consumer(
                        &mut output,
                        &general_purpose::STANDARD_NO_PAD,
                    )
                    .write_all(&state)
                    .map_err(|e| DatabaseError::Invalid(e.to_string()))?;

                    Some(output)
                } else {
                    None
                }
            }
            None => None,
        };

        let mut pairs = vec![];
        for row in page
            .rows()
            .map_err(|e| DatabaseError::Invalid(e.to_string()))?
        {
            pairs.push(Self::row_into_pair(row)?);
        }

        Ok(ListPairsResponse {
            page_size,
            token,
            pairs,
        })
    }

    /// Converts a row in the shape every pair query selects into a [`Pair`].
    fn row_into_pair(row: scylla::frame::response::result::Row) -> Result<Pair, DatabaseError> {
        let typed = row.into_typed::<(Option<i32>, Uuid, String, String, String, String)>()?;

        let value_type: ValueType = typed.5.try_into().map_err(DatabaseError::Invalid)?;

        Ok(Pair {
            ttl: typed.0,
            project_id: typed.1.to_string(),
            namespace: typed.2,
            key: typed.3,
            value: typed.4,
            r#type: value_type.into(),
        })
    }
}

impl From<ValueType> for String {
    fn from(value: ValueType) -> Self {
        match value {
            ValueType::String => "string",
            ValueType::Json => "json",
            ValueType::Integer => "integer",
            ValueType::Number => "number",
            ValueType::Boolean => "boolean",
        }
        .to_string()
    }
}

impl TryFrom<String> for ValueType {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(match value.as_ref() {
            "string" => ValueType::String,
            "json" => ValueType::Json,
            "number" => ValueType::Number,
            "integer" => ValueType::Integer,
            "boolean" => ValueType::Boolean,
            _ => return Err(format!("'{value}' is not a stored value type")),
        })
    }
}

/// Container-backed tests running the real schema against a real scylla.
///
/// These live here rather than in `tests/` because this crate is a binary and
/// has no library target for an integration test to import. One container per
/// test; scylla in developer mode starts in seconds.
#[cfg(test)]
mod tests {
    use super::*;
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{ContainerAsync, GenericImage, ImageExt, core::WaitFor};

    /// Starts scylla, applies the real migrations, and connects.
    ///
    /// The container rides along because dropping it stops the database.
    async fn database() -> (ContainerAsync<GenericImage>, Database) {
        let container = GenericImage::new("scylladb/scylla", "6.2")
            .with_wait_for(WaitFor::message_on_stderr("serving"))
            .with_cmd([
                "--smp",
                "1",
                "--developer-mode",
                "1",
                "--overprovisioned",
                "1",
            ])
            .start()
            .await
            .expect("scylla starts");

        let port = container
            .get_host_port_ipv4(9042)
            .await
            .expect("cql port is published");
        let node = format!("127.0.0.1:{port}");

        // The driver can win the race against scylla's CQL listener, so
        // connecting retries briefly.
        let mut session = None;
        for _ in 0..60 {
            match SessionBuilder::new().known_node(&node).build().await {
                Ok(s) => {
                    session = Some(s);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
            }
        }
        let session = session.expect("scylla accepts connections");

        // The same files the migration runner applies, statement by statement.
        // Comment lines go first, because a ';' inside a comment would split
        // the statement apart.
        for source in [
            include_str!("../migrations/bootstrap.cql"),
            include_str!("../migrations/0001-create-tables.cql"),
        ] {
            let without_comments: String = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");

            for statement in without_comments.split(';') {
                let statement = statement.trim();
                if !statement.is_empty() {
                    session
                        .query(statement, ())
                        .await
                        .unwrap_or_else(|e| panic!("migration statement failed: {e}"));
                }
            }
        }

        (container, Database::new(vec![node]).await)
    }

    fn pair(project: Uuid, namespace: &str, key: &str, value: &str) -> Pair {
        Pair {
            project_id: project.to_string(),
            namespace: namespace.to_owned(),
            r#type: ValueType::String.into(),
            ttl: None,
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }

    #[tokio::test]
    async fn pairs_round_trip_and_writes_register_their_namespace() {
        let (_container, db) = database().await;
        let project = Uuid::new_v4();

        db.set(vec![pair(project, "cache", "greeting", "hello")])
            .await
            .expect("set succeeds");

        let stored = db
            .get(&project.to_string(), "cache", "greeting")
            .await
            .expect("get runs")
            .expect("pair exists");
        assert_eq!(stored.value, "hello");

        // The first write into a namespace is what makes it listable.
        let namespaces = db
            .get_namespaces(&project.to_string())
            .await
            .expect("namespaces list");
        assert_eq!(namespaces.namespaces.len(), 1);
        assert_eq!(namespaces.namespaces[0].name, "cache");
        assert_eq!(namespaces.namespaces[0].count, 1);
    }

    #[tokio::test]
    async fn listing_pages_one_namespace_and_stops_at_its_end() {
        let (_container, db) = database().await;
        let project = Uuid::new_v4();
        let project_id = project.to_string();

        db.set(vec![
            pair(project, "a", "k1", "v1"),
            pair(project, "a", "k2", "v2"),
            pair(project, "a", "k3", "v3"),
            pair(project, "b", "other", "elsewhere"),
        ])
        .await
        .expect("set succeeds");

        let first = db.list(&project_id, "a", 2, None).await.expect("page one");
        assert_eq!(first.pairs.len(), 2);
        let token = first.token.expect("a further page is advertised");

        let second = db
            .list(&project_id, "a", 2, Some(token))
            .await
            .expect("page two");
        assert_eq!(second.pairs.len(), 1);
        assert!(second.token.is_none(), "last page must not advertise more");

        // Namespace b's pair stays out of namespace a's listing.
        let mut seen: Vec<_> = first
            .pairs
            .into_iter()
            .chain(second.pairs)
            .map(|p| p.key)
            .collect();
        seen.sort();
        assert_eq!(seen, vec!["k1", "k2", "k3"]);
    }

    #[tokio::test]
    async fn namespace_and_project_deletion_remove_data_and_registry() {
        let (_container, db) = database().await;
        let project = Uuid::new_v4();
        let project_id = project.to_string();

        // An explicitly created namespace is listable while still empty.
        db.create_namespace(&project_id, "empty")
            .await
            .expect("create succeeds");
        db.set(vec![
            pair(project, "doomed", "k", "v"),
            pair(project, "kept", "k", "v"),
        ])
        .await
        .expect("set succeeds");

        db.delete_namespace(&project_id, "doomed")
            .await
            .expect("namespace delete succeeds");

        let names: Vec<_> = db
            .get_namespaces(&project_id)
            .await
            .expect("list runs")
            .namespaces
            .into_iter()
            .map(|n| (n.name, n.count))
            .collect();
        assert_eq!(names, vec![("empty".to_owned(), 0), ("kept".to_owned(), 1)]);
        assert!(
            db.get(&project_id, "doomed", "k")
                .await
                .expect("get runs")
                .is_none(),
            "pair survived its namespace"
        );

        db.delete_project(&project_id)
            .await
            .expect("project delete succeeds");

        assert!(
            db.get_namespaces(&project_id)
                .await
                .expect("list runs")
                .namespaces
                .is_empty(),
            "registry survived the project delete"
        );
        assert!(
            db.get(&project_id, "kept", "k")
                .await
                .expect("get runs")
                .is_none(),
            "pair survived the project delete"
        );
    }
}
