-- Versioned, envelope-encrypted project secrets. Versions are immutable
-- rows: set/rotate always insert the next version, delete tombstones the
-- head, and pinned versions keep resolving so a workflow run finishes with
-- the credentials it started with.
CREATE TABLE secret_versions (
    project_id  TEXT   NOT NULL,
    name        TEXT   NOT NULL,
    -- Monotonic per (project_id, name), from 1; tombstoned rows still
    -- count, so a name set again after deletion continues the sequence.
    version     BIGINT NOT NULL,
    -- Label of the master key that wrapped this row's data key; lets two
    -- masters coexist during a rotation.
    kek_id      TEXT   NOT NULL,
    -- KEK-wrapped per-version data key, wrap nonce prefixed.
    dek_wrapped BYTEA  NOT NULL,
    -- Nonce for the value ciphertext; the data key encrypts exactly one
    -- value, so it is never reused.
    nonce       BYTEA  NOT NULL,
    ciphertext  BYTEA  NOT NULL,
    created_ms  BIGINT NOT NULL,
    -- User id that performed the write; audit metadata, never identity.
    created_by  TEXT,
    -- Set on the head by DeleteSecret; hides the name from listings and
    -- head resolution while leaving every version resolvable by pin.
    deleted_ms  BIGINT,
    PRIMARY KEY (project_id, name, version)
);
