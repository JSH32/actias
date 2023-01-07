-- Script table, stores script information.
CREATE TABLE scripts 
(
    id                UUID        NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    public_identifier VARCHAR(24) NOT NULL UNIQUE,
    last_updated      TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY(id)
);

-- Script revisions.
CREATE TABLE revisions
(
    id        UUID        NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    created   TIMESTAMPTZ NOT NULL DEFAULT now(),
    script_id UUID        NOT NULL,
    bundle    JSONB       NOT NULL,

    PRIMARY KEY (id),
    FOREIGN KEY (script_id) REFERENCES scripts (id) ON DELETE CASCADE
);
