//! The `secret_versions` table as a SeaORM entity: one immutable row per
//! secret version, composite-keyed. The schema itself lives in the sqlx
//! migration files the migration container applies; this is the typed view
//! the rpcs query through.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "secret_versions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub project_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub name: String,
    /// Monotonic per (project_id, name), from 1; tombstoned rows still
    /// count, so a revived name continues the sequence.
    #[sea_orm(primary_key, auto_increment = false)]
    pub version: i64,
    /// Label of the master key that wrapped this row's data key.
    pub kek_id: String,
    /// KEK-wrapped per-version data key, wrap nonce prefixed.
    pub dek_wrapped: Vec<u8>,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub created_ms: i64,
    /// User id that performed the write; audit metadata, never identity.
    pub created_by: Option<String>,
    /// Set on the head by DeleteSecret; hides the name from listings and
    /// head resolution while leaving every version resolvable by pin.
    pub deleted_ms: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
