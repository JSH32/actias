use std::io::{self, Write};

use base64::{engine::general_purpose, read, write};
use scylla::{
    batch::{Batch, BatchType},
    cql_to_rust::FromRowError,
    frame::types::read_uuid,
    prepared_statement::PreparedStatement,
    transport::{errors::QueryError, query_result::FirstRowTypedError},
    Bytes, Session, SessionBuilder,
};
use thiserror::Error;
use uuid::Uuid;

use crate::proto_kv_service::{
    ListNamespacesResponse, ListPairsResponse, Namespace, Pair, ValueType,
};

#[derive(Error, Debug)]
pub enum DatabaseError {
    #[error("{0}")]
    QueryError(#[from] QueryError),
    #[error("{0}")]
    FirstRowTypedError(#[from] FirstRowTypedError),
    #[error("{0}")]
    FromRowError(#[from] FromRowError),
    #[error("Invalid data provided: {0}")]
    InvalidError(String),
}

pub struct Database {
    session: Session,

    delete_statement: PreparedStatement,
    update_statement: PreparedStatement,
    get_statement: PreparedStatement,
    get_namespace_statement: PreparedStatement,
    get_namespaces_statement: PreparedStatement,
}

impl Database {
    pub async fn new(scylla_nodes: Vec<String>) -> Self {
        let session = SessionBuilder::new()
            .known_nodes(scylla_nodes)
            .use_keyspace("kv_service", true)
            .build()
            .await
            .unwrap();

        let delete_statement = session
            .prepare(
                "DELETE FROM kv_service.pairs WHERE project_id = ? AND namespace = ? AND key = ?",
            )
            .await
            .unwrap();

        let update_statement = session
            .prepare(
                r#"
            UPDATE kv_service.pairs 
                USING TTL ? 
                SET value = ?, 
                    type = ?
                WHERE project_id = ?
                    AND namespace = ?
                    AND key = ?"#,
            )
            .await
            .unwrap();

        let get_statement = session
            .prepare(
                r#"
            SELECT TTL(value), 
                project_id, 
                namespace, 
                key, 
                value, 
                type
            FROM kv_service.pairs
            WHERE project_id = ? 
                AND namespace = ?
                AND key = ?"#,
            )
            .await
            .unwrap();

        let get_namespace_statement = session
            .prepare(
                r#"
            SELECT TTL(value), 
                project_id, 
                namespace, 
                key,
                value,
                type
            FROM kv_service.pairs
            WHERE project_id = ?
                AND namespace = ?
            LIMIT ?
            ALLOW FILTERING"#,
            )
            .await
            .unwrap();

        let get_namespaces_statement = session
            .prepare("SELECT project_id, namespace, key FROM kv_service.pairs WHERE project_id = ? ALLOW FILTERING")
            .await
            .unwrap();

        Self {
            session,
            delete_statement,
            update_statement,
            get_statement,
            get_namespace_statement,
            get_namespaces_statement,
        }
    }

    /// Get the key value pair based on parameters
    pub async fn get(
        &self,
        project_id: &str,
        namespace: &str,
        key: &str,
    ) -> Result<Option<Pair>, DatabaseError> {
        let mut values = Vec::new();

        let project_id = read_uuid(&mut project_id.as_bytes())
            .map_err(|e| DatabaseError::InvalidError(e.to_string()))?;

        values.push((project_id, namespace.clone(), key.clone()));
        // TTL(value),
        //         project_id,
        //         namespace,
        //         key,
        //         value,
        //         type

        Ok(
            match self
                .session
                .execute(&self.get_statement, (project_id, namespace, key))
                .await
                .map_err(DatabaseError::from)?
                .first_row()
            {
                Ok(row) => {
                    let typed =
                        row.into_typed::<(Option<i32>, Uuid, String, String, String, String)>()?;

                    let value_type: ValueType = typed
                        .5
                        .try_into()
                        .map_err(|e| DatabaseError::InvalidError(e))?;

                    Some(Pair {
                        project_id: typed.1.to_string(),
                        namespace: typed.2,
                        r#type: value_type.into(),
                        ttl: typed.0,
                        key: typed.3,
                        value: typed.4,
                    })
                }
                Err(_) => None,
            },
        )
    }

    pub async fn delete(
        &self,
        project_id: &str,
        namespace: &str,
        keys: Vec<String>,
    ) -> Result<(), DatabaseError> {
        let mut batch = Batch::new(BatchType::Logged);

        let mut batch_params = Vec::new();
        for key in keys {
            batch.append_statement(self.delete_statement.clone());
            batch_params.push((project_id.clone(), namespace.clone(), key));
        }

        self.session.batch(&batch, batch_params).await?;
        Ok(())
    }

    pub async fn set(&self, pairs: Vec<Pair>) -> Result<(), DatabaseError> {
        let mut batch = Batch::new(BatchType::Logged);

        let mut batch_params = Vec::new();
        for value in pairs {
            let value_type: String = value.r#type().into();

            let mut project_id = value.project_id.as_bytes();
            let project_id = read_uuid(&mut project_id)
                .map_err(|e| DatabaseError::InvalidError(e.to_string()))?;

            batch.append_statement(self.update_statement.clone());
            batch_params.push((
                value.ttl.unwrap_or(0),
                value.value,
                value_type,
                project_id,
                value.namespace.clone(),
                value.key,
            ));
        }

        self.session.batch(&batch, batch_params).await?;
        Ok(())
    }

    // TODO: Do this another way instead of runtime deduplication.
    // Since this is eventual consistency we should spin up an async task to check if any keys remain and delete from namespace table.
    pub async fn get_namespaces(
        &self,
        project_id: &str,
    ) -> Result<ListNamespacesResponse, DatabaseError> {
        let mut values = Vec::new();

        let mut project_id: &[u8] = project_id.as_bytes();
        values.push(
            read_uuid(&mut project_id).map_err(|e| DatabaseError::InvalidError(e.to_string()))?,
        );

        let mut found_names = vec![];
        let mut namespaces = vec![];

        for row in self
            .session
            .execute(&self.get_namespaces_statement, values)
            .await
            .unwrap()
            .rows_or_empty()
        {
            let (project_id, namespace) = row.into_typed::<(String, String)>()?;

            if !found_names.contains(&namespace) {
                found_names.push(namespace.clone());
                namespaces.push(Namespace {
                    project_id,
                    name: namespace,
                })
            }
        }

        Ok(ListNamespacesResponse { namespaces })
    }

    pub async fn list(
        &self,
        project_id: &str,
        namespace: &str,
        page_size: i32,
        token: Option<String>,
    ) -> Result<ListPairsResponse, DatabaseError> {
        let bytes_token = match token.clone() {
            None => None,
            Some(v) => {
                let mut output = Vec::new();

                let mut decoder =
                    read::DecoderReader::new(v.as_bytes(), &general_purpose::STANDARD_NO_PAD);

                io::copy(&mut decoder, &mut output).unwrap();

                Some(Bytes::from(output))
            }
        };

        let mut project_id = project_id.as_bytes();
        let project_id =
            read_uuid(&mut project_id).map_err(|e| DatabaseError::InvalidError(e.to_string()))?;

        let page = self
            .session
            .execute_paged(
                &self.get_namespace_statement,
                (project_id, namespace, page_size),
                bytes_token,
            )
            .await
            .map_err(DatabaseError::from)?;

        let token = {
            let mut output = String::new();

            {
                let mut encoder = write::EncoderStringWriter::from_consumer(
                    &mut output,
                    &general_purpose::STANDARD_NO_PAD,
                );

                encoder
                    .write_all(&page.paging_state.clone().unwrap())
                    .unwrap();
            }

            output
        };

        let mut pairs = vec![];

        for row in page
            .rows()
            .map_err(|e| DatabaseError::InvalidError(e.to_string()))?
            .into_iter()
        {
            let typed =
                row.into_typed::<(Option<i32>, String, String, String, String, String, i64)>()?;

            let value_type: ValueType = typed
                .5
                .try_into()
                .map_err(|e| DatabaseError::InvalidError(e))?;

            pairs.push(Pair {
                project_id: typed.1,
                namespace: typed.2,
                r#type: value_type.into(),
                ttl: typed.0,
                key: typed.3,
                value: typed.4,
            })
        }

        Ok(ListPairsResponse {
            page_size: page_size,
            token,
            pairs,
        })
    }
}

impl Into<String> for ValueType {
    fn into(self) -> String {
        match self {
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
            _ => return Err("Invalid metadata existed for 'type".to_owned()),
        })
    }
}
