use scylla::{Session, SessionBuilder};

pub struct Database {
    session: Session,
}

impl Database {
    pub async fn new(scylla_nodes: Vec<String>) -> Self {
        let session = SessionBuilder::new()
            .known_nodes(scylla_nodes)
            .use_keyspace("kv_service", true)
            .build()
            .await
            .unwrap();

        Self { session }
    }
}
