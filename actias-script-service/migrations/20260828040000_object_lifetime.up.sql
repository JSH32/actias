-- Lifetime lands on the directory (docs/OBJECT-LIFECYCLE.md): expiry
-- stamped at claim from the class's declared lifespan, tombstones as
-- the deletion commit point, the creating object recorded for the
-- deferred cascade, and the identity hash so the alarm and lease
-- tables (which key by hash) join against directory rows. The hash
-- backfills organically: every claim carries it, and rows untouched
-- since before this migration also predate expiry, so a NULL hash
-- never hides a due row.
ALTER TABLE object_instances ADD COLUMN object_id CHAR(64) NULL;
ALTER TABLE object_instances ADD COLUMN expire_at TIMESTAMPTZ NULL;
ALTER TABLE object_instances ADD COLUMN deleted_at TIMESTAMPTZ NULL;
ALTER TABLE object_instances ADD COLUMN created_by TEXT NULL;

-- The expiry sweep's index: only rows that can expire live in it.
CREATE INDEX object_instances_expire_at
    ON object_instances (expire_at)
    WHERE expire_at IS NOT NULL;

-- The joins from hash-keyed tables walk in through this.
CREATE INDEX object_instances_object_id
    ON object_instances (object_id)
    WHERE object_id IS NOT NULL;
