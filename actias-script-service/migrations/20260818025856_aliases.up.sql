-- Named environment aliases: a mutable pointer from (script, name) to one
-- revision. Rollback is moving the pointer. Deleting a revision drops any
-- alias still aimed at it, so an alias can never dangle.
CREATE TABLE aliases
(
    script_id    UUID        NOT NULL,
    name         VARCHAR(64) NOT NULL,
    revision_id  UUID        NOT NULL,
    last_updated TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (script_id, name),
    FOREIGN KEY (script_id) REFERENCES scripts (id) ON DELETE CASCADE,
    FOREIGN KEY (revision_id) REFERENCES revisions (id) ON DELETE CASCADE
);
