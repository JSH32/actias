use std::io::{self, Write};

use base64::{engine::general_purpose, read, write};
use scylla::{
    batch::{Batch, BatchType},
    cql_to_rust::FromRowError,
    prepared_statement::PreparedStatement,
    transport::{errors::QueryError, query_result::FirstRowTypedError},
    Bytes, Session, SessionBuilder,
};
use thiserror::Error;

use crate::proto_kv_service::{ListPairsResponse, Pair, ValueType};

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
                    type = ?, 
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
                type,
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
                type,
            FROM kv_service.pairs
            WHERE project_id = ?
                AND namespace = ?
            LIMIT ?"#,
            )
            .await
            .unwrap();

        Self {
            session,
            delete_statement,
            update_statement,
            get_statement,
            get_namespace_statement,
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
        values.push(project_id.clone());
        values.push(namespace.clone());
        values.push(key.clone());

        Ok(
            match self
                .session
                .execute(&self.get_statement, (project_id, namespace, key))
                .await
                .map_err(DatabaseError::from)?
                .first_row()
            {
                Ok(row) => {
                    let typed = row
                        .into_typed::<(Option<i32>, String, String, String, String, String, i64)>(
                        )?;

                    let value_type: ValueType = typed
                        .5
                        .try_into()
                        .map_err(|e| DatabaseError::InvalidError(e))?;

                    Some(Pair {
                        project_id: typed.1,
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
            let value_type: String = value
                .r#type()
                .try_into()
                .map_err(|e| DatabaseError::InvalidError(e))?;

            batch.append_statement(self.update_statement.clone());
            batch_params.push((
                value.ttl.unwrap_or(0),
                value.value,
                value_type,
                value.project_id.clone(),
                value.namespace.clone(),
                value.key,
            ));
        }

        self.session.batch(&batch, batch_params).await?;
        Ok(())
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

        let page = self
            .session
            .execute_paged(
                &self.get_namespace_statement,
                (project_id, namespace),
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

        let pairs: Result<Vec<Pair>, DatabaseError> = page
            .rows()
            .map_err(|e| DatabaseError::InvalidError(e.to_string()))?
            .into_iter()
            .map(|row| {
                let typed =
                    row.into_typed::<(Option<i32>, String, String, String, String, String, i64)>()?;

                let value_type: ValueType = typed
                    .5
                    .try_into()
                    .map_err(|e| DatabaseError::InvalidError(e))?;

                Ok(Pair {
                    project_id: typed.1,
                    namespace: typed.2,
                    r#type: value_type.into(),
                    ttl: typed.0,
                    key: typed.3,
                    value: typed.4,
                })
            })
            .collect();

        Ok(ListPairsResponse {
            page_size: page_size,
            token,
            pairs: pairs?,
        })
    }
}

impl TryInto<String> for ValueType {
    type Error = String;

    fn try_into(self) -> Result<String, Self::Error> {
        Ok(match self {
            ValueType::String => "string",
            ValueType::Object => "object",
            ValueType::Integer => "integer",
            _ => return Err("Invalid metadata provided for 'type'".to_owned()),
        }
        .to_string())
    }
}

impl TryFrom<String> for ValueType {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(match value.as_ref() {
            "string" => ValueType::String,
            "object" => ValueType::Object,
            "integer" => ValueType::Integer,
            _ => return Err("Invalid metadata existed for 'type".to_owned()),
        })
    }
}
