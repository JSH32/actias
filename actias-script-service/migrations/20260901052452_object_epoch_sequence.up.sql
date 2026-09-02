-- Epochs come from one monotonic sequence instead of a row per object
-- name. The fence they exist for is "a claim outranks anything the
-- identity held before", and a global sequence gives that by
-- construction: every value it hands out is above every value it has
-- ever handed out, so a recreated name cannot land under its own
-- tombstone without any per-name memory at all.
--
-- What goes away is a table that grew with lifetime churn and could
-- never be collected, because collecting a name's epoch is exactly
-- what would let its next life replay an old one.
CREATE SEQUENCE object_epoch_seq AS BIGINT START WITH 1;

-- The sequence has to start ABOVE every epoch already in flight, or it
-- hands out values that lose to epochs objects already shipped under.
-- A ledger counted per name, so a busy object could sit at 93 while the
-- sequence would have started at 1: its next claim would be fenced out
-- of its own storage, its writes would commit locally and never ship,
-- and callers would see "the write was not confirmed durable" until the
-- sequence happened to catch up.
--
-- Seeded from the ledger this replaces, which is the only place that
-- knows the answer. `false` makes the next `nextval` return exactly
-- this value.
SELECT setval(
    'object_epoch_seq',
    GREATEST((SELECT COALESCE(MAX(epoch), 0) FROM object_epochs) + 1, 1),
    false
);

-- The live epoch belongs to the residency, so it rides the lease: a
-- claim mints one, a re-claim by the holder reads its own, and the row
-- dies with the residency it describes.
ALTER TABLE leases ADD COLUMN epoch BIGINT NOT NULL DEFAULT 1;

DROP TABLE object_epochs;
