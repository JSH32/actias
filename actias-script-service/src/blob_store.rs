//! Content-addressed blob storage: every bundle file's raw bytes live in
//! object storage under their blake3 hash, immutable and deduped across all
//! revisions and projects. Postgres keeps only manifests.

use actias_common::thiserror;
use aws_sdk_s3::primitives::ByteStream;

/// Errors leaving the blob store; the payload never includes bucket names or
/// endpoints, which the wire mapper would otherwise leak.
#[derive(thiserror::Error, Debug)]
pub enum BlobStoreError {
    #[error("blob store request failed: {0}")]
    Storage(String),
    #[error("blob '{0}' is not stored")]
    Missing(String),
}

impl From<BlobStoreError> for tonic::Status {
    fn from(err: BlobStoreError) -> Self {
        match err {
            BlobStoreError::Missing(hash) => {
                tonic::Status::failed_precondition(format!("Blob '{hash}' is not stored."))
            }
            BlobStoreError::Storage(detail) => {
                actias_common::tracing::error!(error = %detail, "blob store failure");
                tonic::Status::internal("Blob storage error.")
            }
        }
    }
}

/// Object storage scoped to one bucket of hash-keyed blobs. Only platform
/// services reach it; client bytes arrive through the api, which is what
/// keeps every stored hash server-computed.
#[derive(Clone)]
pub struct BlobStore {
    client: aws_sdk_s3::Client,
    bucket: String,
}

/// Connection settings, read from the environment by `Config`.
pub struct BlobStoreConfig {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket: String,
}

impl BlobStore {
    /// Connects and makes sure the bucket exists.
    ///
    /// # Panics
    /// Panics when the bucket can neither be found nor created; this runs at
    /// startup, where dying loudly is the right outcome.
    pub async fn new(config: BlobStoreConfig) -> Self {
        let client = Self::client_for(&config, &config.endpoint);

        // Creating an existing bucket is a conflict, not a failure; anything
        // else is a real connectivity or credential problem.
        if let Err(error) = client.create_bucket().bucket(&config.bucket).send().await {
            let service_error = error.into_service_error();
            if !service_error.is_bucket_already_owned_by_you()
                && !service_error.is_bucket_already_exists()
            {
                panic!("blob store bucket could not be ensured: {service_error}");
            }
        }

        Self {
            client,
            bucket: config.bucket,
        }
    }

    fn client_for(config: &BlobStoreConfig, endpoint: &str) -> aws_sdk_s3::Client {
        let credentials = aws_credential_types::Credentials::new(
            config.access_key.clone(),
            config.secret_key.clone(),
            None,
            None,
            "blob-store",
        );

        let s3_config = aws_sdk_s3::config::Builder::new()
            .endpoint_url(endpoint)
            .credentials_provider(credentials)
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            // Bucket-in-path addressing, which minio serves without
            // wildcard dns.
            .force_path_style(true)
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .build();

        aws_sdk_s3::Client::from_conf(s3_config)
    }

    /// Stores one blob under its hash. Rewriting an existing key writes the
    /// same bytes, so the operation is idempotent by construction.
    ///
    /// # Errors
    /// Returns [`BlobStoreError::Storage`] with the store's message.
    pub async fn put(&self, hash: &str, bytes: Vec<u8>) -> Result<(), BlobStoreError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(hash)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| BlobStoreError::Storage(e.to_string()))?;

        Ok(())
    }

    /// Fetches one blob's raw bytes.
    ///
    /// # Errors
    /// Returns [`BlobStoreError::Missing`] when no blob has this hash.
    pub async fn get(&self, hash: &str) -> Result<Vec<u8>, BlobStoreError> {
        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(hash)
            .send()
            .await
            .map_err(|e| {
                let service_error = e.into_service_error();
                if service_error.is_no_such_key() {
                    BlobStoreError::Missing(hash.to_owned())
                } else {
                    BlobStoreError::Storage(service_error.to_string())
                }
            })?;

        object
            .body
            .collect()
            .await
            .map(|data| data.into_bytes().to_vec())
            .map_err(|e| BlobStoreError::Storage(e.to_string()))
    }

    /// A client for a region's own object storage, with its credentials;
    /// this store's own client when the region has none of its own.
    pub fn region_client(
        &self,
        endpoint: &str,
        access_key: &str,
        secret_key: &str,
    ) -> aws_sdk_s3::Client {
        if endpoint.is_empty() {
            return self.client.clone();
        }
        Self::client_for(
            &BlobStoreConfig {
                endpoint: endpoint.to_owned(),
                access_key: access_key.to_owned(),
                secret_key: secret_key.to_owned(),
                bucket: String::new(),
            },
            endpoint,
        )
    }

    /// Copies every object under `prefix` modified after `since_ms`
    /// from one bucket to another, `manifest.json` files last so a
    /// reader of the target never sees a manifest whose chunks are not
    /// there yet. On one endpoint the copy is server side; across two
    /// it streams through this process. Idempotent: an existing target
    /// key is overwritten with the same content-addressed bytes. Returns
    /// the keys copied and the newest modification time seen, so a
    /// caller can copy again from there until a pass copies nothing.
    ///
    /// # Errors
    /// Returns [`BlobStoreError::Storage`] with the store's message.
    pub async fn copy_prefix_between(
        from: &aws_sdk_s3::Client,
        from_bucket: &str,
        to: &aws_sdk_s3::Client,
        to_bucket: &str,
        same_endpoint: bool,
        prefix: &str,
        since_ms: i64,
    ) -> Result<(u64, i64), BlobStoreError> {
        let mut keys = Vec::new();
        let mut newest = since_ms;
        let mut token: Option<String> = None;
        loop {
            let mut request = from.list_objects_v2().bucket(from_bucket).prefix(prefix);
            if let Some(next) = token.take() {
                request = request.continuation_token(next);
            }
            let page = request
                .send()
                .await
                .map_err(|e| BlobStoreError::Storage(e.into_service_error().to_string()))?;
            for object in page.contents() {
                let Some(key) = object.key() else { continue };
                let modified_ms = object
                    .last_modified()
                    .map(|at| at.to_millis().unwrap_or(0))
                    .unwrap_or(0);
                if modified_ms > since_ms {
                    keys.push(key.to_owned());
                    newest = newest.max(modified_ms);
                }
            }
            match page.next_continuation_token() {
                Some(next) if page.is_truncated().unwrap_or(false) => {
                    token = Some(next.to_owned());
                }
                _ => break,
            }
        }
        let (manifests, rest): (Vec<String>, Vec<String>) = keys
            .into_iter()
            .partition(|key| key.ends_with("/manifest.json"));
        let mut copied = 0u64;
        for key in rest.into_iter().chain(manifests) {
            if same_endpoint {
                to.copy_object()
                    .bucket(to_bucket)
                    .key(&key)
                    .copy_source(format!("{from_bucket}/{key}"))
                    .send()
                    .await
                    .map_err(|e| BlobStoreError::Storage(e.into_service_error().to_string()))?;
            } else {
                let object = from
                    .get_object()
                    .bucket(from_bucket)
                    .key(&key)
                    .send()
                    .await
                    .map_err(|e| BlobStoreError::Storage(e.into_service_error().to_string()))?;
                let bytes = object
                    .body
                    .collect()
                    .await
                    .map_err(|e| BlobStoreError::Storage(e.to_string()))?
                    .into_bytes();
                to.put_object()
                    .bucket(to_bucket)
                    .key(&key)
                    .body(ByteStream::from(bytes))
                    .send()
                    .await
                    .map_err(|e| BlobStoreError::Storage(e.into_service_error().to_string()))?;
            }
            copied += 1;
        }
        Ok((copied, newest))
    }

    /// Size in bytes of the blob with this hash, or [`None`] when it is not
    /// stored.
    ///
    /// # Errors
    /// Returns [`BlobStoreError::Storage`] with the store's message; a blob
    /// that is simply absent answers [`None`].
    pub async fn head(&self, hash: &str) -> Result<Option<u64>, BlobStoreError> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(hash)
            .send()
            .await
        {
            Ok(object) => Ok(Some(object.content_length().unwrap_or(0) as u64)),
            Err(error) => {
                let service_error = error.into_service_error();
                if service_error.is_not_found() {
                    Ok(None)
                } else {
                    Err(BlobStoreError::Storage(service_error.to_string()))
                }
            }
        }
    }

    /// The subset of `hashes` not stored yet, preserving request order.
    ///
    /// # Errors
    /// Returns whatever [`BlobStore::head`] returns for the first hash it
    /// cannot ask about.
    pub async fn missing(&self, hashes: &[String]) -> Result<Vec<String>, BlobStoreError> {
        let mut missing = Vec::new();
        for hash in hashes {
            if self.head(hash).await?.is_none() {
                missing.push(hash.clone());
            }
        }
        Ok(missing)
    }
}
