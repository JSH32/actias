//! Moving a project between homes: the job of FLEET.md 6.3. The home is
//! a leased cache and the bucket is the truth, so a move is a copy and a
//! column flip, every step re-runnable: mark the project moving (claims
//! refuse everywhere), wait for the old region's residencies to end,
//! copy the scope's prefixes between the regions' buckets with the
//! manifests last, flip the home, clear the mark. Progress is one row
//! per project for the console; a failure records itself, keeps the old
//! home and clears the mark, so the project keeps serving where it was
//! and the move may be started again.

use sqlx::{Pool, Postgres};
use tonic::Status;
use uuid::Uuid;

use crate::blob_store::BlobStore;
use crate::proto_node_registry::node_registry_service_client::NodeRegistryServiceClient;
use crate::proto_node_registry::{
    CountInstancesRequest, ListInstancesRequest, MoveRef, SetMoveRequest,
};
use crate::proto_script_service::ProjectMove;

/// Rows per page when listing a class's instances at the old region.
const PAGE: u32 = 500;

/// The one row of a project's latest move, as the console reads it:
/// regions, step, counts, error, the two times as unix milliseconds.
type MoveRow = (String, String, String, i64, i64, String, i64, i64);

/// Reads the project's latest move; [`None`] when it never moved.
pub(crate) async fn read_move(
    database: &Pool<Postgres>,
    project_id: Uuid,
) -> Result<Option<ProjectMove>, Status> {
    let row: Option<MoveRow> = sqlx::query_as(
        "SELECT from_region, to_region, step, objects_total, objects_copied, error,
                (EXTRACT(EPOCH FROM started_at) * 1000)::BIGINT,
                COALESCE((EXTRACT(EPOCH FROM finished_at) * 1000)::BIGINT, 0)
         FROM project_moves WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(database)
    .await
    .map_err(|e| Status::internal(e.to_string()))?;
    Ok(row.map(
        |(from_region, to_region, step, total, copied, error, started_ms, finished_ms)| {
            ProjectMove {
                project_id: project_id.to_string(),
                from_region,
                to_region,
                step,
                objects_total: u64::try_from(total).unwrap_or(0),
                objects_copied: u64::try_from(copied).unwrap_or(0),
                error,
                started_ms,
                finished_ms,
            }
        },
    ))
}

/// What a move needs of the service, cloned into its task.
pub(crate) struct Mover {
    pub database: Pool<Postgres>,
    pub blobs: BlobStore,
    pub default_region: String,
    pub placement_uri: String,
    pub drain: std::time::Duration,
}

/// A registered region's addresses.
struct RegionRow {
    bucket: String,
    placement_addr: String,
    s3_endpoint: String,
    s3_access_key: String,
    s3_secret_key: String,
}

impl Mover {
    /// Records the move and marks the project moving; the answer is the
    /// row the console follows.
    ///
    /// # Errors
    /// Both regions must be registered with a bucket, and the old one
    /// with a placement address unless it is the control plane's own;
    /// otherwise the move cannot copy.
    pub(crate) async fn start(
        &self,
        project_id: Uuid,
        from: &str,
        to: &str,
    ) -> Result<ProjectMove, Status> {
        let old = self.region(from).await?;
        let new = self.region(to).await?;
        if old.placement_addr.is_empty() && from != self.default_region {
            return Err(Status::failed_precondition(format!(
                "'{from}' has no placement address registered; a move lists the project's objects there"
            )));
        }
        if old.bucket.is_empty() || new.bucket.is_empty() {
            return Err(Status::failed_precondition(
                "Both regions name a bucket; a move copies between them.",
            ));
        }
        sqlx::query(
            "INSERT INTO project_moves (project_id, from_region, to_region, step)
             VALUES ($1, $2, $3, 'marking')
             ON CONFLICT (project_id) DO UPDATE
             SET from_region = EXCLUDED.from_region, to_region = EXCLUDED.to_region,
                 step = 'marking', objects_total = 0, objects_copied = 0, error = '',
                 started_at = now(), finished_at = NULL",
        )
        .bind(project_id)
        .bind(from)
        .bind(to)
        .execute(&self.database)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO project_policies (project_id, moving) VALUES ($1, true)
             ON CONFLICT (project_id) DO UPDATE SET moving = true, updated_at = now()",
        )
        .bind(project_id)
        .execute(&self.database)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;
        read_move(&self.database, project_id)
            .await?
            .ok_or_else(|| Status::internal("the move row vanished"))
    }

    /// The job, after [`Mover::start`]: drain, copy, flip, clear. Never
    /// panics; a failure lands in the row.
    pub(crate) async fn run(self, project_id: Uuid, from: String, to: String) {
        match self.steps(project_id, &from, &to).await {
            Ok(copied) => {
                actias_common::tracing::info!(%project_id, from, to, copied, "project moved");
            }
            Err(error) => {
                actias_common::tracing::warn!(
                    %project_id, from, to, %error,
                    "project move failed; the old home keeps serving"
                );
                let _ = sqlx::query(
                    "UPDATE project_moves SET step = 'failed', error = $2, finished_at = now()
                     WHERE project_id = $1",
                )
                .bind(project_id)
                .bind(error)
                .execute(&self.database)
                .await;
                let _ = sqlx::query(
                    "UPDATE project_policies SET moving = false, updated_at = now()
                     WHERE project_id = $1",
                )
                .bind(project_id)
                .execute(&self.database)
                .await;
            }
        }
    }

    async fn steps(&self, project_id: Uuid, from: &str, to: &str) -> Result<u64, String> {
        let old = self
            .region(from)
            .await
            .map_err(|s| s.message().to_owned())?;
        let new = self.region(to).await.map_err(|s| s.message().to_owned())?;

        // Drain: every worker's cached policy turns moving within a
        // pointer ttl, and its sweep ends the scope's residencies.
        self.step(project_id, "draining", None, None).await?;
        tokio::time::sleep(self.drain).await;

        // The scope's objects, from the old region's placement store:
        // its classes, then each class's rows.
        let placement_addr = if from == self.default_region && old.placement_addr.is_empty() {
            self.placement_uri.clone()
        } else {
            old.placement_addr.clone()
        };
        let mut placement = NodeRegistryServiceClient::new(
            tonic::transport::Endpoint::from_shared(placement_addr.clone())
                .map_err(|e| format!("'{placement_addr}' is not a placement address: {e}"))?
                .connect_lazy(),
        );
        let classes = placement
            .count_instances(CountInstancesRequest {
                project_ids: vec![project_id.to_string()],
            })
            .await
            .map_err(|s| {
                format!(
                    "the old region's placement store did not answer: {}",
                    s.message()
                )
            })?
            .into_inner()
            .counts;
        let mut object_ids = Vec::new();
        for class in classes {
            let mut page = 0u32;
            loop {
                let listed = placement
                    .list_instances(ListInstancesRequest {
                        project_ids: vec![project_id.to_string()],
                        class: class.class.clone(),
                        name_prefix: String::new(),
                        page_size: PAGE,
                        page,
                    })
                    .await
                    .map_err(|s| format!("listing '{}' failed: {}", class.class, s.message()))?
                    .into_inner();
                let count = listed.instances.len();
                object_ids.extend(
                    listed
                        .instances
                        .into_iter()
                        .map(|row| row.object_id)
                        .filter(|id| !id.is_empty()),
                );
                if count < PAGE as usize {
                    break;
                }
                page += 1;
            }
        }
        let total = u64::try_from(object_ids.len()).unwrap_or(u64::MAX);
        self.step(project_id, "copying", Some(total), Some(0))
            .await?;

        // The copy: each object's prefix, manifests last inside it, then
        // the project's directory, from the old region's storage to the
        // new region's (server side on one endpoint, streamed across
        // two). Content addressed, so re-running overwrites nothing
        // that differs. A residency still settling its last flight past
        // the drain window writes after the first pass; passes repeat
        // from the newest time seen until one copies nothing, and only
        // then is the home flipped, so nothing written before the flip
        // is left behind. No new residency can start while the project
        // is moving, so the passes converge.
        let from_store =
            self.blobs
                .region_client(&old.s3_endpoint, &old.s3_access_key, &old.s3_secret_key);
        let to_store =
            self.blobs
                .region_client(&new.s3_endpoint, &new.s3_access_key, &new.s3_secret_key);
        let same_endpoint = old.s3_endpoint == new.s3_endpoint;
        let mut prefixes: Vec<String> = object_ids
            .iter()
            .map(|id| format!("objects/{id}/"))
            .collect();
        prefixes.push(format!("directory/{project_id}/"));
        let mut copied_keys = 0u64;
        let mut since_ms = 0i64;
        for pass in 0.. {
            let mut newest = since_ms;
            let mut copied_this_pass = 0u64;
            for (done, prefix) in prefixes.iter().enumerate() {
                let (copied, seen) = BlobStore::copy_prefix_between(
                    &from_store,
                    &old.bucket,
                    &to_store,
                    &new.bucket,
                    same_endpoint,
                    prefix,
                    since_ms,
                )
                .await
                .map_err(|e| format!("copying {prefix}: {e}"))?;
                copied_this_pass += copied;
                newest = newest.max(seen);
                if pass == 0 && (done + 1).is_multiple_of(50) {
                    self.step(
                        project_id,
                        "copying",
                        Some(total),
                        Some(u64::try_from(done + 1).unwrap_or(u64::MAX)),
                    )
                    .await?;
                }
            }
            copied_keys += copied_this_pass;
            if pass > 0 && copied_this_pass == 0 {
                break;
            }
            if pass >= 20 {
                return Err(
                    "the old region kept writing through twenty copy passes; the move stops"
                        .to_owned(),
                );
            }
            since_ms = newest;
        }
        self.step(project_id, "copying", Some(total), Some(total))
            .await?;

        // Forwarding rows travel with the project: an object the
        // platform had moved elsewhere is still elsewhere after the
        // home flips, and the new home must say so (FLEET.md 4.2). One
        // moved to the region that is now its home is home again.
        let new_placement_addr = if new.placement_addr.is_empty() {
            self.placement_uri.clone()
        } else {
            new.placement_addr.clone()
        };
        let mut new_placement = NodeRegistryServiceClient::new(
            tonic::transport::Endpoint::from_shared(new_placement_addr.clone())
                .map_err(|e| format!("'{new_placement_addr}' is not a placement address: {e}"))?
                .connect_lazy(),
        );
        for object_id in &object_ids {
            let moved = placement
                .get_move(MoveRef {
                    object_id: object_id.clone(),
                })
                .await
                .map_err(|s| format!("reading {object_id}'s forwarding row: {}", s.message()))?
                .into_inner()
                .region;
            if moved.is_empty() {
                continue;
            }
            if moved != to {
                new_placement
                    .set_move(SetMoveRequest {
                        object_id: object_id.clone(),
                        region: moved,
                    })
                    .await
                    .map_err(|s| {
                        format!("carrying {object_id}'s forwarding row: {}", s.message())
                    })?;
            }
            let _ = placement
                .clear_move(MoveRef {
                    object_id: object_id.clone(),
                })
                .await;
        }

        // The flip, then the clear: caches see the new home within a
        // pointer ttl; a call in the window is refused as moving and
        // retried.
        self.step(project_id, "flipping", None, None).await?;
        sqlx::query(
            "UPDATE project_policies SET region = $2, moving = false, updated_at = now()
             WHERE project_id = $1",
        )
        .bind(project_id)
        .bind(to)
        .execute(&self.database)
        .await
        .map_err(|e| e.to_string())?;
        sqlx::query(
            "UPDATE project_moves SET step = 'done', finished_at = now() WHERE project_id = $1",
        )
        .bind(project_id)
        .execute(&self.database)
        .await
        .map_err(|e| e.to_string())?;
        Ok(copied_keys)
    }

    async fn step(
        &self,
        project_id: Uuid,
        step: &str,
        total: Option<u64>,
        copied: Option<u64>,
    ) -> Result<(), String> {
        sqlx::query(
            "UPDATE project_moves
             SET step = $2,
                 objects_total = COALESCE($3, objects_total),
                 objects_copied = COALESCE($4, objects_copied)
             WHERE project_id = $1",
        )
        .bind(project_id)
        .bind(step)
        .bind(total.map(|n| i64::try_from(n).unwrap_or(i64::MAX)))
        .bind(copied.map(|n| i64::try_from(n).unwrap_or(i64::MAX)))
        .execute(&self.database)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    async fn region(&self, name: &str) -> Result<RegionRow, Status> {
        let row: Option<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT bucket, placement_addr, s3_endpoint, s3_access_key, s3_secret_key
             FROM regions WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(&self.database)
        .await
        .map_err(|e| Status::internal(e.to_string()))?;
        row.map(
            |(bucket, placement_addr, s3_endpoint, s3_access_key, s3_secret_key)| RegionRow {
                bucket,
                placement_addr,
                s3_endpoint,
                s3_access_key,
                s3_secret_key,
            },
        )
        .ok_or_else(|| {
            Status::failed_precondition(format!(
                "'{name}' is not a registered region; a move needs both regions' buckets"
            ))
        })
    }
}
