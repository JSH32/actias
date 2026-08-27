//! The storage contract: what any kv backend must answer, spoken in the
//! service's own proto types. The gRPC layer holds a `dyn KvStore` and
//! never learns which backend is behind it; the conformance suite below
//! is the contract's test form, and every backend runs all of it.

use crate::proto_kv_service::{ListNamespacesResponse, ListPairsResponse, Pair, PairRequest};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DatabaseError {
    /// A backend failure a caller cannot act on; logged, never shown.
    #[error("{0}")]
    Backend(String),
    /// A response did not carry rows in the shape its query promises.
    #[error("{0}")]
    Rows(String),
    #[error("Invalid data provided: {0}")]
    Invalid(String),
}

/// One kv backend. Semantics every implementation must keep:
///
/// - `set` overwrites last-write-wins and registers each touched
///   namespace; a `ttl` of zero or [`None`] means the pair never
///   expires, and a positive `ttl` means the pair stops existing for
///   every other method after that many seconds.
/// - `list` pages in ascending key order; its token is opaque, only
///   meaningful to the backend that minted it, and absent on the last
///   page.
/// - Deleting a namespace removes its pairs and its registry entry;
///   deleting a project removes everything the project owns.
#[async_trait::async_trait]
pub trait KvStore: Send + Sync {
    async fn get(
        &self,
        project_id: &str,
        namespace: &str,
        key: &str,
    ) -> Result<Option<Pair>, DatabaseError>;

    async fn set(&self, pairs: Vec<Pair>) -> Result<(), DatabaseError>;

    async fn delete(&self, pairs: Vec<PairRequest>) -> Result<(), DatabaseError>;

    async fn create_namespace(
        &self,
        project_id: &str,
        namespace: &str,
    ) -> Result<(), DatabaseError>;

    async fn delete_namespace(
        &self,
        project_id: &str,
        namespace: &str,
    ) -> Result<(), DatabaseError>;

    async fn delete_project(&self, project_id: &str) -> Result<(), DatabaseError>;

    async fn get_namespaces(
        &self,
        project_id: &str,
    ) -> Result<ListNamespacesResponse, DatabaseError>;

    async fn list(
        &self,
        project_id: &str,
        namespace: &str,
        page_size: i32,
        token: Option<String>,
    ) -> Result<ListPairsResponse, DatabaseError>;
}

/// The contract as tests: every backend's test module calls each of
/// these against a fresh store, so a semantic drift between backends
/// is a red suite, not a deployment surprise.
#[cfg(test)]
pub(crate) mod conformance {
    use super::KvStore;
    use crate::proto_kv_service::{Pair, ValueType};
    use uuid::Uuid;

    pub fn pair(project: Uuid, namespace: &str, key: &str, value: &str) -> Pair {
        Pair {
            project_id: project.to_string(),
            namespace: namespace.to_owned(),
            r#type: ValueType::String.into(),
            ttl: None,
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }

    pub async fn pairs_round_trip_and_writes_register_their_namespace(store: &dyn KvStore) {
        let project = Uuid::new_v4();

        store
            .set(vec![pair(project, "cache", "greeting", "hello")])
            .await
            .expect("set succeeds");

        let stored = store
            .get(&project.to_string(), "cache", "greeting")
            .await
            .expect("get runs")
            .expect("pair exists");
        assert_eq!(stored.value, "hello");

        // The first write into a namespace is what makes it listable.
        let namespaces = store
            .get_namespaces(&project.to_string())
            .await
            .expect("namespaces list");
        assert_eq!(namespaces.namespaces.len(), 1);
        assert_eq!(namespaces.namespaces[0].name, "cache");
        assert_eq!(namespaces.namespaces[0].count, 1);
    }

    pub async fn a_ttl_write_reports_its_remaining_life(store: &dyn KvStore) {
        let project = Uuid::new_v4();
        let mut expiring = pair(project, "cache", "flash", "gone soon");
        expiring.ttl = Some(600);

        store.set(vec![expiring]).await.expect("set succeeds");

        let stored = store
            .get(&project.to_string(), "cache", "flash")
            .await
            .expect("get runs")
            .expect("pair exists before its ttl");
        let ttl = stored.ttl.expect("a ttl write reports a ttl");
        assert!(
            (1..=600).contains(&ttl),
            "remaining ttl {ttl} outside (0, 600]"
        );

        let durable = store
            .get(&project.to_string(), "cache", "flash")
            .await
            .expect("get runs")
            .expect("still alive");
        assert!(durable.ttl.is_some());
    }

    pub async fn listing_pages_one_namespace_and_stops_at_its_end(store: &dyn KvStore) {
        let project = Uuid::new_v4();
        let project_id = project.to_string();

        store
            .set(vec![
                pair(project, "a", "k1", "v1"),
                pair(project, "a", "k2", "v2"),
                pair(project, "a", "k3", "v3"),
                pair(project, "b", "other", "elsewhere"),
            ])
            .await
            .expect("set succeeds");

        let first = store
            .list(&project_id, "a", 2, None)
            .await
            .expect("page one");
        assert_eq!(first.pairs.len(), 2);
        let token = first.token.expect("a further page is advertised");

        let second = store
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

    pub async fn namespace_and_project_deletion_remove_data_and_registry(store: &dyn KvStore) {
        let project = Uuid::new_v4();
        let project_id = project.to_string();

        // An explicitly created namespace is listable while still empty.
        store
            .create_namespace(&project_id, "empty")
            .await
            .expect("create succeeds");
        store
            .set(vec![
                pair(project, "doomed", "k", "v"),
                pair(project, "kept", "k", "v"),
            ])
            .await
            .expect("set succeeds");

        store
            .delete_namespace(&project_id, "doomed")
            .await
            .expect("namespace delete succeeds");

        let names: Vec<_> = store
            .get_namespaces(&project_id)
            .await
            .expect("list runs")
            .namespaces
            .into_iter()
            .map(|n| (n.name, n.count))
            .collect();
        assert_eq!(names, vec![("empty".to_owned(), 0), ("kept".to_owned(), 1)]);
        assert!(
            store
                .get(&project_id, "doomed", "k")
                .await
                .expect("get runs")
                .is_none(),
            "pair survived its namespace"
        );

        store
            .delete_project(&project_id)
            .await
            .expect("project delete succeeds");

        assert!(
            store
                .get_namespaces(&project_id)
                .await
                .expect("list runs")
                .namespaces
                .is_empty(),
            "registry survived the project delete"
        );
        assert!(
            store
                .get(&project_id, "kept", "k")
                .await
                .expect("get runs")
                .is_none(),
            "pair survived the project delete"
        );
    }
}
