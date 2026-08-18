-- Object leases: ownership half of the placement store. A lease is valid
-- exactly while its holder is alive in the node registry; the cascade
-- makes node age-out and lease expiry the same event, so there is no
-- second clock to disagree with the first.
CREATE TABLE leases
(
    -- blake3 of the object identity (script/class/name), hex.
    object_id CHAR(64)    NOT NULL,
    node_id   UUID        NOT NULL,
    acquired  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (object_id),
    FOREIGN KEY (node_id) REFERENCES nodes (id) ON DELETE CASCADE
);
