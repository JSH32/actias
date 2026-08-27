//! Snapshot shipping: an object's durable truth leaves the node. On every
//! call that wrote, the whole file goes to the blob store with a manifest
//! carrying the lease epoch; restore-on-spawn brings it back wherever the
//! object is next resident, which makes failover and rehoming the same
//! code path. Frame batches slot in between snapshots later; the manifest
//! and fence are already the final shape.

use serde::{Deserialize, Serialize};

/// What the store remembers about one object's shipped state.
#[derive(Serialize, Deserialize)]
pub struct Manifest {
    /// The shipper's lease epoch; a zombie ex-owner's uploads lose to any
    /// newer epoch here.
    pub epoch: u64,
    /// Unix ms of the ship, informational.
    pub shipped_at: i64,
}

pub struct ObjectStore {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl ObjectStore {
    pub fn new(client: aws_sdk_s3::Client, bucket: String) -> Self {
        Self { client, bucket }
    }

    fn snapshot_key(object_id: &str) -> String {
        format!("objects/{object_id}/snapshot.db")
    }

    fn manifest_key(object_id: &str) -> String {
        format!("objects/{object_id}/manifest.json")
    }

    /// Ships the file as this epoch's snapshot, fenced: an existing
    /// manifest with a newer epoch means someone else owns the object now,
    /// and this upload is refused.
    pub async fn ship(
        &self,
        object_id: &str,
        epoch: u64,
        file: &std::path::Path,
    ) -> Result<(), String> {
        if let Some(manifest) = self.manifest(object_id).await?
            && manifest.epoch > epoch
        {
            return Err(format!(
                "fenced: epoch {epoch} lost to {}; this node no longer owns the object",
                manifest.epoch
            ));
        }

        let bytes = tokio::fs::read(file).await.map_err(|e| e.to_string())?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(Self::snapshot_key(object_id))
            .body(bytes.into())
            .send()
            .await
            .map_err(|e| e.into_service_error().to_string())?;

        let manifest = serde_json::to_vec(&Manifest {
            epoch,
            shipped_at: actias_worker_core::extensions::objects::unix_now_ms(),
        })
        .map_err(|e| e.to_string())?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(Self::manifest_key(object_id))
            .body(manifest.into())
            .send()
            .await
            .map_err(|e| e.into_service_error().to_string())?;

        Ok(())
    }

    /// Restores the last shipped snapshot into `file`; false when nothing
    /// was ever shipped (a genuinely new object).
    pub async fn restore(&self, object_id: &str, file: &std::path::Path) -> Result<bool, String> {
        if self.manifest(object_id).await?.is_none() {
            return Ok(false);
        }

        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::snapshot_key(object_id))
            .send()
            .await
            .map_err(|e| e.into_service_error().to_string())?;
        let bytes = object
            .body
            .collect()
            .await
            .map_err(|e| e.to_string())?
            .into_bytes();

        tokio::fs::write(file, bytes)
            .await
            .map_err(|e| e.to_string())?;
        Ok(true)
    }

    pub async fn manifest(&self, object_id: &str) -> Result<Option<Manifest>, String> {
        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::manifest_key(object_id))
            .send()
            .await;

        match result {
            Ok(object) => {
                let bytes = object
                    .body
                    .collect()
                    .await
                    .map_err(|e| e.to_string())?
                    .into_bytes();
                serde_json::from_slice(&bytes)
                    .map(Some)
                    .map_err(|e| e.to_string())
            }
            Err(error) => {
                let service = error.into_service_error();
                if service.is_no_such_key() {
                    Ok(None)
                } else {
                    Err(service.to_string())
                }
            }
        }
    }
}
