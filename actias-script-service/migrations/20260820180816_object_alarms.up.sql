-- The alarm registry: one row per object holding an armed alarm, mirrored
-- asynchronously by whichever node hosts the object. Unlike leases these
-- rows are NOT tied to a node: an alarm must outlive its holder dying,
-- which is the whole point. The sweep is one indexed query over due_ms.
CREATE TABLE object_alarms (
    -- blake3 of the object identity, hex; same key the leases use.
    object_id TEXT PRIMARY KEY,
    -- The object's own key (scope/class/name), what a wake needs.
    own_key TEXT NOT NULL,
    -- Unix milliseconds the alarm is due at.
    due_ms BIGINT NOT NULL
);

CREATE INDEX object_alarms_due ON object_alarms (due_ms);
