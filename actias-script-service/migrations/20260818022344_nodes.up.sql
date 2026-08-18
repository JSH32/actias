-- Membership half of the placement store: one row per worker node, alive
-- while it keeps heartbeating. Object leases will join this store later.
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

-- Liveness reads filter on the heartbeat cutoff.
CREATE INDEX nodes_last_heartbeat_idx ON nodes (last_heartbeat);
