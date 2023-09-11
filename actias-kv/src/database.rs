use std::{
    io::{self, Write},
    str::FromStr,
};

use base64::{engine::general_purpose, read, write};
use scylla::{
    batch::{Batch, BatchType},
    cql_to_rust::FromRowError,
    prepared_statement::PreparedStatement,
    transport::{errors::QueryError, query_result::FirstRowTypedError},
    Bytes, Session, SessionBuilder,
};
use thiserror::Error;
use uuid::Uuid;

use crate::proto_kv_service::{
    ListNamespacesResponse, ListPairsResponse, Namespace, Pair, PairRequest, ValueType,
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
    get_project_statement: PreparedStatement,
    get_project_namespace_statement: PreparedStatement,
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
            ALLOW FILTERING"#,
            )
            .await
            .unwrap();

        let get_project_statement = session
            .prepare(
                r#"
            SELECT 
                namespace, 
                key
            FROM kv_service.pairs
            WHERE project_id = ?
            ALLOW FILTERING"#,
            )
            .await
            .unwrap();

        let get_project_namespace_statement = session
            .prepare(
                r#"
            SELECT key
            FROM kv_service.pairs
            WHERE project_id = ?
                AND namespace = ?
            ALLOW FILTERING"#,
            )
            .await
            .unwrap();

        let get_namespaces_statement = session
            .prepare("SELECT namespace FROM kv_service.pairs WHERE project_id = ? ALLOW FILTERING")
            .await
            .unwrap();

        Self {
            session,
            delete_statement,
            get_project_namespace_statement,
            get_project_statement,
            update_statement,
            get_statement,
            get_namespace_statement,
            get_namespaces_statement,
        }
    }

    /// Gets a pair from the database
    ///
    /// # Arguments
    /// * project_id - Project ID
    /// * namespace - Namespace
    /// * key - Key to get
    pub async fn get(
        &self,
        project_id: &str,
        namespace: &str,
        key: &str,
    ) -> Result<Option<Pair>, DatabaseError> {
        let mut values = Vec::new();

        let project_id =
            Uuid::from_str(&project_id).map_err(|e| DatabaseError::InvalidError(e.to_string()))?;

        values.push((project_id, namespace.clone(), key.clone()));

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

    /// Deletes an entire namespace from a project.
    ///
    /// # Arguments
    /// * project_id - Project ID
    /// * namespace - Namespace to delete
    pub async fn delete_namespace(
        &self,
        project_id: &str,
        namespace: &str,
    ) -> Result<(), DatabaseError> {
        let mut pairs = vec![];

        let project_id_uuid =
            Uuid::from_str(&project_id).map_err(|e| DatabaseError::InvalidError(e.to_string()))?;

        for row in self
            .session
            .execute(
                &self.get_project_namespace_statement,
                (project_id_uuid, namespace.to_owned()),
            )
            .await
            .map_err(|e| DatabaseError::InvalidError(e.to_string()))?
            .rows_or_empty()
        {
            let (key,) = row.into_typed::<(String,)>()?;
            pairs.push(PairRequest {
                project_id: project_id.to_owned(),
                namespace: namespace.to_owned(),
                key,
            });
        }

        self.delete(pairs).await?;

        Ok(())
    }

    /// Deletes an entire project from the database
    ///
    /// # Arguments
    /// * project_id - Project ID
    pub async fn delete_project(&self, project_id: &str) -> Result<(), DatabaseError> {
        let mut pairs = vec![];

        let project_id_uuid =
            Uuid::from_str(&project_id).map_err(|e| DatabaseError::InvalidError(e.to_string()))?;

        for row in self
            .session
            .execute(&self.get_project_statement, vec![project_id_uuid])
            .await
            .map_err(|e| DatabaseError::InvalidError(e.to_string()))?
            .rows_or_empty()
        {
            let (namespace, key) = row.into_typed::<(String, String)>()?;

            pairs.push(PairRequest {
                project_id: project_id.to_owned(),
                namespace,
                key,
            });
        }

        self.delete(pairs).await?;

        Ok(())
    }

    /// Deletes a pair from the database
    ///
    /// # Arguments
    /// * project_id - Project ID
    /// * keys - Keys to delete (namespace, key)
    pub async fn delete(&self, pairs: Vec<PairRequest>) -> Result<(), DatabaseError> {
        let mut batch = Batch::new(BatchType::Logged);

        let mut batch_params = Vec::new();
        for pair in pairs {
            batch.append_statement(self.delete_statement.clone());
            batch_params.push((
                Uuid::from_str(&pair.project_id)
                    .map_err(|e| DatabaseError::InvalidError(e.to_string()))?,
                pair.namespace,
                pair.key,
            ));
        }

        self.session.batch(&batch, batch_params).await?;
        Ok(())
    }

    pub async fn set(&self, pairs: Vec<Pair>) -> Result<(), DatabaseError> {
        let mut batch = Batch::new(BatchType::Logged);

        let mut batch_params = Vec::new();
        for value in pairs {
            let value_type: String = value.r#type().into();

            let project_id = Uuid::from_str(&value.project_id)
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

    /// Gets namespaces from the database
    ///
    /// # Arguments
    /// * project_id - Project ID
    ///
    /// TODO: Do this another way instead of runtime deduplication.
    /// Since this is eventual consistency we should spin up an async task to check if any keys remain and delete from namespace table.
    pub async fn get_namespaces(
        &self,
        project_id: &str,
    ) -> Result<ListNamespacesResponse, DatabaseError> {
        let values =
            vec![Uuid::from_str(&project_id)
                .map_err(|e| DatabaseError::InvalidError(e.to_string()))?];

        let mut found_names: Vec<(String, u32)> = vec![];

        for row in self
            .session
            .execute(&self.get_namespaces_statement, values)
            .await
            .map_err(|e| DatabaseError::InvalidError(e.to_string()))?
            .rows_or_empty()
        {
            let namespace = row.into_typed::<(String,)>()?;

            if !found_names.iter().any(|(ns, _)| ns == &namespace.0) {
                found_names.push((namespace.0.clone(), 1));
            } else {
                for (ns, count) in &mut found_names {
                    if ns == &namespace.0 {
                        *count += 1
                    }
                }
            }
        }

        let mut namespaces = vec![];
        for name in found_names {
            namespaces.push(Namespace {
                project_id: project_id.to_string(),
                name: name.0,
                count: name.1 as i32,
            })
        }

        Ok(ListNamespacesResponse { namespaces })
    }

    /// Lists pairs from the database
    ///
    /// # Arguments
    /// * project_id - Project ID
    /// * namespace - Namespace
    /// * page_size - Page size
    /// * token - Optional paging token
    pub async fn list(
        &self,
        project_id: &str,
        namespace: &str,
        page_size: i32,
        token: Option<String>,
    ) -> Result<ListPairsResponse, DatabaseError> {
        let project_id =
            Uuid::from_str(&project_id).map_err(|e| DatabaseError::InvalidError(e.to_string()))?;

        let bytes_token = match token.clone() {
            None => None,
            Some(v) => {
                let mut output = Vec::new();

                let mut decoder =
                    read::DecoderReader::new(v.as_bytes(), &general_purpose::STANDARD_NO_PAD);

                io::copy(&mut decoder, &mut output).map_err(|_| {
                    DatabaseError::InvalidError("Invalid token provided".to_string())
                })?;

                Some(Bytes::from(output))
            }
        };

        let mut statement = self.get_namespace_statement.clone();
        statement.set_page_size(page_size);

        let page = self
            .session
            .execute_paged(&statement, (project_id, namespace), bytes_token)
            .await
            .map_err(DatabaseError::from)?;

        let token = match page.paging_state.clone() {
            Some(v) => {
                let mut output = String::new();

                write::EncoderStringWriter::from_consumer(
                    &mut output,
                    &general_purpose::STANDARD_NO_PAD,
                )
                .write_all(&v)
                .map_err(|e| DatabaseError::InvalidError(e.to_string()))?;

                Some(output)
            }
            None => None,
        };

        let mut pairs = vec![];

        for row in page
            .rows()
            .map_err(|e| DatabaseError::InvalidError(e.to_string()))?
            .into_iter()
        {
            let typed = row.into_typed::<(Option<i32>, Uuid, String, String, String, String)>()?;

            let value_type: ValueType = typed
                .5
                .try_into()
                .map_err(|e| DatabaseError::InvalidError(e))?;

            pairs.push(Pair {
                project_id: typed.1.to_string(),
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
