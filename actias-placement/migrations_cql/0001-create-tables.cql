-- Every operation reads one partition. Time columns are unix
-- milliseconds, the unit the service speaks.

-- Membership: one partition per region, one row per node. Liveness is
-- decided by the reader against the cutoff; the reaper deletes what is
-- past it.
CREATE TABLE IF NOT EXISTS placement.nodes (
    region          TEXT,
    node_id         UUID,
    address         TEXT,
    capabilities    LIST<TEXT>,
    load            INT,
    registered_ms   BIGINT,
    last_heartbeat_ms BIGINT,
    PRIMARY KEY ((region), node_id)
);

-- Leases: one row per object, claimed with IF NOT EXISTS so exactly
-- one node wins. The epoch is the residency's fence.
CREATE TABLE IF NOT EXISTS placement.leases (
    object_id   TEXT,
    node_id     UUID,
    epoch       BIGINT,
    acquired_ms BIGINT,
    PRIMARY KEY ((object_id))
);

-- The same leases by holder, for the departure record a dead node
-- leaves and the cascade that frees its objects.
CREATE TABLE IF NOT EXISTS placement.leases_by_node (
    node_id     UUID,
    object_id   TEXT,
    PRIMARY KEY ((node_id), object_id)
);

-- The instance directory: one partition per (scope, class), names in
-- clustering order so a listing and a prefix range are one partition
-- read. last_epoch is the identity's memory: every claim, tombstone
-- and marker lands above it.
CREATE TABLE IF NOT EXISTS placement.instances (
    scope_id    UUID,
    class       TEXT,
    name        TEXT,
    object_id   TEXT,
    script_id   UUID,
    created_ms  BIGINT,
    created_by  TEXT,
    expire_at_ms BIGINT,
    deleted_at_ms BIGINT,
    last_epoch  BIGINT,
    PRIMARY KEY ((scope_id, class), name)
);

-- The classes a scope has ever held, so the counts know which
-- partitions to fold.
CREATE TABLE IF NOT EXISTS placement.classes (
    scope_id    UUID,
    class       TEXT,
    PRIMARY KEY ((scope_id), class)
);

-- Hash to identity, for the tables that key by hash (leases, alarms,
-- departures) to find the directory row.
CREATE TABLE IF NOT EXISTS placement.instance_ids (
    object_id   TEXT,
    scope_id    UUID,
    class       TEXT,
    name        TEXT,
    PRIMARY KEY ((object_id))
);

-- Alarms by due minute, and the object's own row so a set replaces.
CREATE TABLE IF NOT EXISTS placement.alarms (
    region      TEXT,
    due_bucket  BIGINT,
    due_ms      BIGINT,
    object_id   TEXT,
    own_key     TEXT,
    PRIMARY KEY ((region, due_bucket), due_ms, object_id)
);
CREATE TABLE IF NOT EXISTS placement.alarms_by_object (
    object_id   TEXT,
    due_ms      BIGINT,
    own_key     TEXT,
    PRIMARY KEY ((object_id))
);
-- Which minutes hold alarms, so a sweep reads only those.
CREATE TABLE IF NOT EXISTS placement.alarm_buckets (
    region      TEXT,
    due_bucket  BIGINT,
    PRIMARY KEY ((region), due_bucket)
);

-- Expiries by due minute, maintained at claim and cleared at tombstone.
CREATE TABLE IF NOT EXISTS placement.expiries (
    region      TEXT,
    due_bucket  BIGINT,
    expire_at_ms BIGINT,
    scope_id    UUID,
    class       TEXT,
    name        TEXT,
    PRIMARY KEY ((region, due_bucket), expire_at_ms, scope_id, class, name)
);
CREATE TABLE IF NOT EXISTS placement.expiry_buckets (
    region      TEXT,
    due_bucket  BIGINT,
    PRIMARY KEY ((region), due_bucket)
);

-- Tombstoned identities whose purge has not run, by tombstone time.
CREATE TABLE IF NOT EXISTS placement.tombstones (
    region      TEXT,
    deleted_at_ms BIGINT,
    scope_id    UUID,
    class       TEXT,
    name        TEXT,
    PRIMARY KEY ((region), deleted_at_ms, scope_id, class, name)
);

-- The record a node leaves when it exits. One partition per region;
-- a departure is taken with IF EXISTS, so it is handed out once.
CREATE TABLE IF NOT EXISTS placement.departures (
    region      TEXT,
    node_id     UUID,
    drained     BOOLEAN,
    departed_ms BIGINT,
    object_ids  LIST<TEXT>,
    PRIMARY KEY ((region), node_id)
);

-- Forwarding rows: one per object the platform moved away from this
-- region, its birth region; a claim here answers with the region
-- instead of a lease. Deleted when the object comes home.
CREATE TABLE IF NOT EXISTS placement.moves (
    object_id   TEXT,
    region      TEXT,
    moved_ms    BIGINT,
    PRIMARY KEY ((object_id))
);
