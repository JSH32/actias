-- Script table, stores script information.
CREATE TABLE scripts 
(
    id                UUID        NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    project_id        UUID        NOT NULL,
    public_identifier VARCHAR(64) NOT NULL UNIQUE,
    last_updated      TIMESTAMPTZ NOT NULL DEFAULT now(),
    current_revision  UUID,

    PRIMARY KEY (id)
);

-- Script revisions.
CREATE TABLE revisions
(
    id             UUID             NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    created        TIMESTAMPTZ      NOT NULL DEFAULT now(),
    script_id      UUID             NOT NULL,
    entry_point    VARCHAR(32767)   NOT NULL,
    -- Not used for anything other than storing and retrieving.
    script_config  JSONB            NOT NULL,

    PRIMARY KEY (id),
    FOREIGN KEY (script_id) REFERENCES scripts (id) ON DELETE CASCADE
);

-- Files in revisions. The path is a file's identity within its bundle; the
-- hash is its content identity, computed here so it is authoritative.
CREATE TABLE files
(
    revision_id  UUID           NOT NULL,
    content      bytea          NOT NULL,
    file_path    VARCHAR(32767) NOT NULL,
    hash         CHAR(64)       NOT NULL,
    size         BIGINT         NOT NULL,
    content_type VARCHAR(256)   NOT NULL,
    -- 0 module, 1 asset; mirrors the proto FileKind values.
    kind         SMALLINT       NOT NULL DEFAULT 0,

    FOREIGN KEY (revision_id) REFERENCES revisions (id) ON DELETE CASCADE
);
