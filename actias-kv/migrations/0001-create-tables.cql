-- Key/value pairs. The partition key is (project_id, namespace) so that a
-- namespace is one partition: listing reads a single partition in clustering
-- order, and deleting a namespace is a single partition tombstone. The trade
-- is that one namespace cannot exceed what a partition comfortably holds;
-- bucket-sharding the partition key is the escape hatch if that ever bites.
CREATE TABLE IF NOT EXISTS kv_service.pairs (
    project_id   UUID,
    namespace    TEXT,
    -- Stored as text as it is just metadata describing how `value` parses.
    type         TEXT,
    key          TEXT,
    value        TEXT,

    PRIMARY KEY ((project_id, namespace), key)
);

-- Registry of the namespaces in a project, one partition per project.
-- Maintained on writes and namespace deletion; without it, listing namespaces
-- would mean scanning every pair the project owns.
CREATE TABLE IF NOT EXISTS kv_service.namespaces (
    project_id   UUID,
    name         TEXT,

    PRIMARY KEY ((project_id), name)
);
