-- Individual KV pairs.
CREATE TABLE IF NOT EXISTS kv_service.pairs (
    project_id   UUID,
    namespace    TEXT,
    last_updated TIMESTAMP,
    -- This is stored as text as it is just metadata.
    type         TEXT,
    key          TEXT,
    value        TEXT,

    PRIMARY KEY ((project_id, namespace, key), last_updated)
) WITH CLUSTERING ORDER BY (last_updated DESC);