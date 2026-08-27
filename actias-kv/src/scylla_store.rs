//! The scylla backend: every query addresses a single partition, so
//! nothing needs ALLOW FILTERING and deleting a namespace is one
//! partition tombstone.

use std::{
    io::{self, Write},
    ops::ControlFlow,
    str::FromStr,
};

use base64::{engine::general_purpose, read, write};
use scylla::{
    client::{session::Session, session_builder::SessionBuilder},
    errors::ExecutionError,
    response::PagingState,
    statement::prepared::PreparedStatement,
};
use uuid::Uuid;

use crate::proto_kv_service::{
    ListNamespacesResponse, ListPairsResponse, Namespace, Pair, PairRequest, ValueType,
};
use crate::store::{DatabaseError, KvStore};

impl From<ExecutionError> for DatabaseError {
    fn from(error: ExecutionError) -> Self {
        DatabaseError::Backend(error.to_string())
    }
}

/// Collapses the driver's per-stage result errors into [`DatabaseError::Rows`];
/// they all mean the same thing to a caller, a result of the wrong shape.
fn rows_error<E: std::fmt::Display>(error: E) -> DatabaseError {
    DatabaseError::Rows(error.to_string())
}

/// The column tuple every pair query selects, in select-list order.
type PairRow = (Option<i32>, Uuid, String, String, String, String);

/// Data access for the pairs and namespaces tables.
pub struct ScyllaStore {
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

/// Connects a session pointed at the kv keyspace.
///
/// Session construction lives apart from [`Database::new`] so environments
/// that need connection options the service does not (address translation in
/// tests, for one) can build their own.
///
/// # Panics
/// Panics when no node accepts a session; this runs at startup, where dying
/// loudly is the right outcome.
pub async fn connect(scylla_nodes: Vec<String>) -> Session {
    SessionBuilder::new()
        .known_nodes(scylla_nodes)
        .use_keyspace("kv_service", true)
        .build()
        .await
        .expect("scylla session could not be established from SCYLLA_NODES")
}

impl ScyllaStore {
    pub async fn new(session: Session) -> Self {
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

    /// Reads a project's registered namespace names into owned strings, so
    /// callers can keep querying while iterating them.
    async fn namespace_names(&self, project_uuid: Uuid) -> Result<Vec<String>, DatabaseError> {
        let result = self
            .session
            .execute_unpaged(&self.list_namespaces_statement, (project_uuid,))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;

        let names = result
            .rows::<(String,)>()
            .map_err(rows_error)?
            .map(|row| row.map(|(name,)| name))
            .collect::<Result<Vec<_>, _>>()
            .map_err(rows_error)?;

        Ok(names)
    }

    /// Converts a row in the shape every pair query selects into a [`Pair`].
    fn row_into_pair(typed: PairRow) -> Result<Pair, DatabaseError> {
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

#[async_trait::async_trait]
impl KvStore for ScyllaStore {
    /// Gets a pair from the database.
    ///
    /// # Errors
    /// Returns [`DatabaseError::Invalid`] when the stored type metadata does
    /// not name a [`ValueType`].
    async fn get(
        &self,
        project_id: &str,
        namespace: &str,
        key: &str,
    ) -> Result<Option<Pair>, DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        let result = self
            .session
            .execute_unpaged(&self.get_statement, (project_id, namespace, key))
            .await?
            .into_rows_result()
            .map_err(rows_error)?;

        match result.maybe_first_row::<PairRow>().map_err(rows_error)? {
            Some(row) => Ok(Some(Self::row_into_pair(row)?)),
            None => Ok(None),
        }
    }

    /// Updates/creates pairs. This overwrites and uses a last-write-wins
    /// strategy, and registers each touched namespace so listing can answer
    /// from the registry.
    ///
    /// # Arguments
    /// * `pairs` - List of pairs.
    async fn set(&self, pairs: Vec<Pair>) -> Result<(), DatabaseError> {
        for pair in pairs {
            let value_type: String = pair.r#type().into();
            let project_id = Self::project_uuid(&pair.project_id)?;

            self.session
                .execute_unpaged(
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
                .execute_unpaged(
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
    async fn delete(&self, pairs: Vec<PairRequest>) -> Result<(), DatabaseError> {
        for pair in pairs {
            let project_id = Self::project_uuid(&pair.project_id)?;

            self.session
                .execute_unpaged(
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
    async fn create_namespace(
        &self,
        project_id: &str,
        namespace: &str,
    ) -> Result<(), DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        self.session
            .execute_unpaged(&self.register_namespace_statement, (project_id, namespace))
            .await?;

        Ok(())
    }

    /// Deletes an entire namespace: one partition tombstone for the pairs,
    /// one registry row.
    ///
    /// # Arguments
    /// * `project_id` - Project ID.
    /// * `namespace` - Namespace to delete.
    async fn delete_namespace(
        &self,
        project_id: &str,
        namespace: &str,
    ) -> Result<(), DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        self.session
            .execute_unpaged(
                &self.delete_namespace_pairs_statement,
                (project_id, namespace),
            )
            .await?;

        self.session
            .execute_unpaged(
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
    async fn delete_project(&self, project_id: &str) -> Result<(), DatabaseError> {
        let project_uuid = Self::project_uuid(project_id)?;

        let namespaces = self.namespace_names(project_uuid).await?;

        for name in namespaces {
            self.session
                .execute_unpaged(&self.delete_namespace_pairs_statement, (project_uuid, name))
                .await?;
        }

        self.session
            .execute_unpaged(&self.unregister_all_namespaces_statement, (project_uuid,))
            .await?;

        Ok(())
    }

    /// Gets a project's namespaces from the registry, with a per-partition
    /// pair count for each.
    ///
    /// # Arguments
    /// * `project_id` - Project ID.
    async fn get_namespaces(
        &self,
        project_id: &str,
    ) -> Result<ListNamespacesResponse, DatabaseError> {
        let project_uuid = Self::project_uuid(project_id)?;

        let mut namespaces = vec![];

        for name in self.namespace_names(project_uuid).await? {
            // COUNT over one partition, not the cluster; acceptable at the
            // sizes a dashboard lists. Expired rows fall out automatically.
            let (count,) = self
                .session
                .execute_unpaged(&self.count_statement, (project_uuid, name.as_str()))
                .await?
                .into_rows_result()
                .map_err(rows_error)?
                .first_row::<(i64,)>()
                .map_err(rows_error)?;

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
    async fn list(
        &self,
        project_id: &str,
        namespace: &str,
        page_size: i32,
        token: Option<String>,
    ) -> Result<ListPairsResponse, DatabaseError> {
        let project_id = Self::project_uuid(project_id)?;

        // The driver panics on a nonpositive page size, and this one comes
        // off the wire.
        if page_size <= 0 {
            return Err(DatabaseError::Invalid(
                "Page size must be positive".to_string(),
            ));
        }

        let paging_state = match token {
            None => PagingState::start(),
            Some(v) => {
                let mut output = Vec::new();

                let mut decoder =
                    read::DecoderReader::new(v.as_bytes(), &general_purpose::STANDARD_NO_PAD);

                io::copy(&mut decoder, &mut output)
                    .map_err(|_| DatabaseError::Invalid("Invalid token provided".to_string()))?;

                PagingState::new_from_raw_bytes(output)
            }
        };

        let mut statement = self.list_statement.clone();
        statement.set_page_size(page_size);

        let (page, paging_response) = self
            .session
            .execute_single_page(&statement, (project_id, namespace), paging_state)
            .await?;

        let token = match paging_response.into_paging_control_flow() {
            ControlFlow::Break(()) => None,
            ControlFlow::Continue(state) => {
                // The server can hand back a state even when the partition is
                // exhausted; probing with a count from that position keeps the
                // last page from advertising a next page. Partition-local, so
                // the probe is cheap.
                let (count_page, _) = self
                    .session
                    .execute_single_page(
                        &self.count_statement,
                        (project_id, namespace),
                        state.clone(),
                    )
                    .await?;

                let (remaining,) = count_page
                    .into_rows_result()
                    .map_err(rows_error)?
                    .first_row::<(i64,)>()
                    .map_err(rows_error)?;

                match state.as_bytes_slice() {
                    Some(bytes) if remaining > 0 => {
                        let mut output = String::new();

                        write::EncoderStringWriter::from_consumer(
                            &mut output,
                            &general_purpose::STANDARD_NO_PAD,
                        )
                        .write_all(bytes)
                        .map_err(|e| DatabaseError::Invalid(e.to_string()))?;

                        Some(output)
                    }
                    _ => None,
                }
            }
        };

        let rows_result = page.into_rows_result().map_err(rows_error)?;

        let mut pairs = vec![];
        for row in rows_result.rows::<PairRow>().map_err(rows_error)? {
            pairs.push(Self::row_into_pair(row.map_err(rows_error)?)?);
        }

        Ok(ListPairsResponse {
            page_size,
            token,
            pairs,
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

/// Container-backed tests: the shared conformance suite against a real
/// scylla, plus the container plumbing only this backend needs.
///
/// These live here rather than in `tests/` because this crate is a binary and
/// has no library target for an integration test to import. One container per
/// test; scylla in developer mode starts in seconds.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::conformance;
    use scylla::errors::TranslationError;
    use scylla::policies::address_translator::{AddressTranslator, UntranslatedPeer};
    use serial_test::serial;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{ContainerAsync, GenericImage, ImageExt, core::WaitFor};

    /// Routes every discovered peer to one address.
    ///
    /// The container advertises its bridge ip, but the driver pairs that ip
    /// with the mapped host port, an address nothing listens on. With a
    /// single node, everything the driver discovers is the contact point.
    struct EverythingIsHere(SocketAddr);

    #[async_trait::async_trait]
    impl AddressTranslator for EverythingIsHere {
        async fn translate_address(
            &self,
            _peer: &UntranslatedPeer,
        ) -> Result<SocketAddr, TranslationError> {
            Ok(self.0)
        }
    }

    /// Starts scylla, applies the real migrations, and connects.
    ///
    /// The container rides along because dropping it stops the database.
    async fn store() -> (ContainerAsync<GenericImage>, ScyllaStore) {
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
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();

        let builder = || {
            SessionBuilder::new()
                .known_node(addr.to_string())
                .address_translator(Arc::new(EverythingIsHere(addr)))
        };

        // The driver can win the race against scylla's CQL listener, and a
        // freshly built session can still have a broken pool while shards
        // come up, so readiness is a served query, not a connection.
        let mut session = None;
        for _ in 0..60 {
            if let Ok(s) = builder().build().await
                && s.query_unpaged("SELECT release_version FROM system.local", ())
                    .await
                    .is_ok()
            {
                session = Some(s);
                break;
            }

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        let session = session.expect("scylla accepts connections");

        // The real migrator, twice: the second run must find everything
        // recorded and change nothing, or a restarting migration container
        // would corrupt a live deployment.
        crate::migrate::apply(&session)
            .await
            .expect("migrations apply");
        crate::migrate::apply(&session)
            .await
            .expect("migrations are re-runnable");

        let data_session = builder()
            .use_keyspace("kv_service", true)
            .build()
            .await
            .expect("scylla accepts a data session");

        (container, ScyllaStore::new(data_session).await)
    }

    #[tokio::test]
    #[serial(containers)]
    async fn pairs_round_trip_and_writes_register_their_namespace() {
        let (_container, db) = store().await;
        conformance::pairs_round_trip_and_writes_register_their_namespace(&db).await;
    }

    #[tokio::test]
    #[serial(containers)]
    async fn a_ttl_write_reports_its_remaining_life() {
        let (_container, db) = store().await;
        conformance::a_ttl_write_reports_its_remaining_life(&db).await;
    }

    #[tokio::test]
    #[serial(containers)]
    async fn listing_pages_one_namespace_and_stops_at_its_end() {
        let (_container, db) = store().await;
        conformance::listing_pages_one_namespace_and_stops_at_its_end(&db).await;
    }

    #[tokio::test]
    #[serial(containers)]
    async fn namespace_and_project_deletion_remove_data_and_registry() {
        let (_container, db) = store().await;
        conformance::namespace_and_project_deletion_remove_data_and_registry(&db).await;
    }
}
