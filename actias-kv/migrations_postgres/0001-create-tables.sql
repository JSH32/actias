-- Key/value pairs, one btree keyed (project_id, namespace, key): point
-- gets and key-ordered listing ride the index, and a namespace or
-- project delete is a plain range delete. expires_at NULL means the
-- pair never expires; reads filter expired rows and the service's
-- sweeper reclaims them.
CREATE TABLE IF NOT EXISTS pairs (
    project_id   uuid        NOT NULL,
    namespace    text        NOT NULL,
    key          text        NOT NULL,
    -- Stored as text as it is just metadata describing how value parses.
    type         text        NOT NULL,
    value        text        NOT NULL,
    expires_at   timestamptz,

    PRIMARY KEY (project_id, namespace, key)
);

-- The sweeper's path to expired rows without scanning live ones.
CREATE INDEX IF NOT EXISTS pairs_expiry
    ON pairs (expires_at) WHERE expires_at IS NOT NULL;

-- Registry of the namespaces in a project. Maintained on writes and
-- namespace deletion; without it, listing namespaces would mean
-- scanning every pair the project owns.
CREATE TABLE IF NOT EXISTS namespaces (
    project_id   uuid  NOT NULL,
    name         text  NOT NULL,

    PRIMARY KEY (project_id, name)
);
