-- The record a node leaves behind when it exits the registry: whether
-- it drained (graceful deregistration flushes shippers and syncers to
-- zero) and which objects it held at that moment. Leases cascade away
-- with the node row, so this capture is the only durable answer to
-- "what did the dead node hold", which is exactly the crash-scoped
-- directory sweep's input. The sweep consumes and deletes rows;
-- drained departures are recorded for the same observability but
-- carry no repair obligation.
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
