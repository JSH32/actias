-- The placement store, one per region: membership, leases, the
-- instance directory, alarms and the record a dead node leaves behind.
-- What the script service's registry tables were, in one schema.

-- Membership: one row per worker node, alive while it keeps
-- heartbeating. Liveness reads filter on the heartbeat cutoff.
CREATE TABLE nodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Where other platform services reach the node, host:port.
    address TEXT NOT NULL,
    -- Roles the node serves, e.g. 'http'.
    capabilities TEXT[] NOT NULL DEFAULT '{}',
    -- Requests in flight at the last heartbeat.
    load INTEGER NOT NULL DEFAULT 0,
    registered TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_heartbeat TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX nodes_last_heartbeat_idx ON nodes (last_heartbeat);

-- Leases: a lease is valid exactly while its holder is alive in the
-- registry; the cascade makes node age-out and lease expiry the same
-- event. The live epoch belongs to the residency, so it rides the row.
CREATE TABLE leases
(
    -- blake3 of the object identity (scope/class/name), hex.
    object_id CHAR(64)    NOT NULL,
    node_id   UUID        NOT NULL,
    epoch     BIGINT      NOT NULL DEFAULT 1,
    acquired  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (object_id),
    FOREIGN KEY (node_id) REFERENCES nodes (id) ON DELETE CASCADE
);

-- The instance directory: every object identity ever claimed, so the
-- platform can answer "what object data exists" after the revision
-- that declared it is gone. Identity is (scope, class, name), the
-- scope being the project for resource classes and the script for
-- __cron; the owner script is metadata, never identity. Lifetime
-- lands here too: expiry stamped at claim, tombstones as the deletion
-- commit point, the creating object for the deferred cascade, and the
-- identity hash so the hash-keyed tables join against these rows.
CREATE TABLE object_instances (
    scope_id UUID NOT NULL,
    class TEXT NOT NULL,
    name TEXT NOT NULL,
    script_id UUID NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    object_id CHAR(64) NULL,
    expire_at TIMESTAMPTZ NULL,
    deleted_at TIMESTAMPTZ NULL,
    created_by TEXT NULL,
    -- The identity's memory of its fence: every claim, tombstone and
    -- marker lands above it. Epochs are the clock or one past this,
    -- whichever is later, so nothing global hands them out.
    last_epoch BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (scope_id, class, name)
);
-- The expiry sweep's index: only rows that can expire live in it.
CREATE INDEX object_instances_expire_at
    ON object_instances (expire_at)
    WHERE expire_at IS NOT NULL;
-- The joins from hash-keyed tables walk in through this.
CREATE INDEX object_instances_object_id
    ON object_instances (object_id)
    WHERE object_id IS NOT NULL;

-- Alarms: one row per object holding an armed alarm, mirrored by
-- whichever node hosts the object. Not tied to a node: an alarm must
-- outlive its holder dying. The sweep is one indexed query over due_ms.
CREATE TABLE object_alarms (
    object_id TEXT PRIMARY KEY,
    -- The object's own key (scope/class/name), what a wake needs.
    own_key TEXT NOT NULL,
    -- Unix milliseconds the alarm is due at.
    due_ms BIGINT NOT NULL
);
CREATE INDEX object_alarms_due ON object_alarms (due_ms);

-- The record a node leaves when it exits: whether it drained and which
-- objects it held at that moment. Leases cascade away with the node
-- row, so this capture is the only durable answer to "what did the
-- dead node hold", the crash-scoped directory sweep's input.
CREATE TABLE node_departures
(
    node_id     UUID PRIMARY KEY,
    drained     BOOLEAN     NOT NULL,
    departed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    object_ids  TEXT[]      NOT NULL DEFAULT '{}'
);
CREATE INDEX node_departures_undrained
    ON node_departures (departed_at)
    WHERE NOT drained;

-- Forwarding rows: one per object the platform moved away from this
-- region, its birth region. A claim here answers with the region
-- instead of a lease; a caller elsewhere learns it from that answer,
-- once, and forwards. Deleted when the object comes home.
CREATE TABLE moves (
    object_id CHAR(64)    NOT NULL,
    region    TEXT        NOT NULL,
    moved_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (object_id)
);
