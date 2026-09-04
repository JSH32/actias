-- The control plane's regions: where a data plane can be reached and
-- which bucket holds its bytes. A single-region deployment keeps this
-- table empty; a region nobody registered routes to the project's home.
CREATE TABLE regions (
    -- A region token: one to sixteen of a-z, 0-9 and '-', not starting
    -- with '-'. The same spelling every worker and placement store
    -- carries.
    name TEXT PRIMARY KEY,
    -- The region's data-plane ingress, host:port, what another region
    -- forwards a call to.
    data_plane_addr TEXT NOT NULL,
    -- The region's object bucket: what its workers ship to and a move
    -- copies between.
    bucket TEXT NOT NULL,
    -- The region's placement service as the control plane reaches it;
    -- a move lists the project's objects there.
    placement_addr TEXT NOT NULL DEFAULT '',
    -- The region's own object storage, when it is not the control
    -- plane's; empty means the control plane's S3 settings reach the
    -- bucket. A move copies with these.
    s3_endpoint TEXT NOT NULL DEFAULT '',
    s3_access_key TEXT NOT NULL DEFAULT '',
    s3_secret_key TEXT NOT NULL DEFAULT '',
    registered_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A project's latest move between homes, for the console to follow:
-- the step it is at, the objects copied, the error if it failed.
CREATE TABLE project_moves (
    project_id UUID PRIMARY KEY,
    from_region TEXT NOT NULL,
    to_region TEXT NOT NULL,
    step TEXT NOT NULL,
    objects_total BIGINT NOT NULL DEFAULT 0,
    objects_copied BIGINT NOT NULL DEFAULT 0,
    error TEXT NOT NULL DEFAULT '',
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);

-- A project's home region and whether it is between homes. The policy
-- row is the one place the workers read a project's runtime facts
-- from, so the region rides it; a project with no row is at the
-- control plane's default region and not moving.
ALTER TABLE project_policies
    ADD COLUMN region TEXT NOT NULL DEFAULT 'local',
    ADD COLUMN moving BOOLEAN NOT NULL DEFAULT false;
