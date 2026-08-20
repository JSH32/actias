-- The instance directory: every object identity ever claimed, so the
-- platform can answer "what object data exists" even after the revision
-- that declared it is gone. Written by AcquireLease; identity fields are
-- the preimage of the lease's object_id hash: (scope, class, name), where
-- the scope is the project for resource classes and the script for
-- __cron. The owner script is metadata ("declared by"), never identity.
CREATE TABLE object_instances (
    scope_id UUID NOT NULL,
    class TEXT NOT NULL,
    name TEXT NOT NULL,
    script_id UUID NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (scope_id, class, name)
);
