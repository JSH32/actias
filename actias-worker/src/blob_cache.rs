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

/// One configured client for the platform's object storage; shared by the
/// hash-keyed blob cache and the object snapshot shipper.
/// Makes sure `bucket` exists. Creating an existing bucket is a
/// conflict, not a failure; anything else is a connectivity or
/// credential problem, and this runs at boot, where dying loudly is
/// the right outcome.
///
/// # Panics
/// When the bucket can neither be found nor created.
pub async fn ensure_bucket(client: &aws_sdk_s3::Client, bucket: &str) {
    if let Err(error) = client.create_bucket().bucket(bucket).send().await {
        let service_error = error.into_service_error();
        if !service_error.is_bucket_already_owned_by_you()
            && !service_error.is_bucket_already_exists()
        {
            panic!("object bucket '{bucket}' could not be ensured: {service_error}");
        }
    }
}

pub fn s3_client(endpoint: &str, access_key: &str, secret_key: &str) -> aws_sdk_s3::Client {
    let credentials =
        aws_credential_types::Credentials::new(access_key, secret_key, None, None, "worker");

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

impl BlobCache {
    /// Builds the client without touching the network; the bucket belongs
    /// to script-service, which ensures it exists.
    pub fn new(config: BlobCacheConfig) -> Self {
        Self {
            client: s3_client(&config.endpoint, &config.access_key, &config.secret_key),
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
    ///
    /// # Errors
    /// Returns the store's message, tagged with the hash that was asked for.
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
