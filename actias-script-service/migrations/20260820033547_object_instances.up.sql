-- The instance directory: every object identity ever claimed, so the
-- platform can answer "what object data exists" even after the revision
-- that declared it is gone. Written by AcquireLease; identity fields are
-- the preimage of the lease's object_id hash.
CREATE TABLE object_instances (
    script_id UUID NOT NULL,
    class TEXT NOT NULL,
    name TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (script_id, class, name)
);

CREATE INDEX object_instances_script ON object_instances (script_id);
