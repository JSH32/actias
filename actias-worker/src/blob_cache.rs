//! Read side of the platform blob store: bundle bytes are pulled from
//! object storage by blake3 hash into a bounded local cache, so revisions
//! stop transiting script-service and a publish that changes one file
//! refetches only that file.
//!
//! Blobs are immutable by construction (the key is the content's hash), so
//! entries never expire; only byte pressure evicts them.

use std::sync::Arc;

/// Connection settings, read from the environment by `Config`. The same
/// shape script-service uses, minus any write access being exercised.
pub struct BlobCacheConfig {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket: String,
    pub cache_bytes: u64,
}

/// Hash-keyed local cache over the blob bucket.
#[derive(Clone)]
pub struct BlobCache {
    client: aws_sdk_s3::Client,
    bucket: String,
    cache: moka::future::Cache<String, Arc<Vec<u8>>>,
}

impl BlobCache {
    /// Builds the client without touching the network; the bucket belongs
    /// to script-service, which ensures it exists.
    pub fn new(config: BlobCacheConfig) -> Self {
        let credentials = aws_credential_types::Credentials::new(
            config.access_key.clone(),
            config.secret_key.clone(),
            None,
            None,
            "blob-cache",
        );

        let s3_config = aws_sdk_s3::config::Builder::new()
            .endpoint_url(&config.endpoint)
            .credentials_provider(credentials)
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            // Bucket-in-path addressing, which minio serves without
            // wildcard dns.
            .force_path_style(true)
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .build();

        Self {
            client: aws_sdk_s3::Client::from_conf(s3_config),
            bucket: config.bucket,
            cache: moka::future::Cache::builder()
                .max_capacity(config.cache_bytes)
                .weigher(|_, blob: &Arc<Vec<u8>>| blob.len().clamp(1, u32::MAX as usize) as u32)
                .build(),
        }
    }

    /// Bytes of the blob with this hash, from cache or storage.
    ///
    /// Concurrent misses of one hash collapse into a single fetch; a failed
    /// fetch is not cached, so a flaky store costs retries, not poisoning.
    pub async fn get(&self, hash: &str) -> anyhow::Result<Arc<Vec<u8>>> {
        self.cache
            .try_get_with(hash.to_owned(), {
                let client = self.client.clone();
                let bucket = self.bucket.clone();
                let hash = hash.to_owned();
                async move {
                    let object = client
                        .get_object()
                        .bucket(bucket)
                        .key(&hash)
                        .send()
                        .await
                        .map_err(|e| {
                            anyhow::anyhow!("blob '{hash}': {}", e.into_service_error())
                        })?;

                    let bytes = object
                        .body
                        .collect()
                        .await
                        .map_err(|e| anyhow::anyhow!("blob '{hash}' body: {e}"))?
                        .into_bytes();

                    Ok::<_, anyhow::Error>(Arc::new(bytes.to_vec()))
                }
            })
            .await
            .map_err(|error: Arc<anyhow::Error>| anyhow::anyhow!("{error:#}"))
    }
}
