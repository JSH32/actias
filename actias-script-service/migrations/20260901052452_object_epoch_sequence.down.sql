CREATE TABLE object_epochs
(
    object_id CHAR(64) NOT NULL,
    epoch     BIGINT   NOT NULL DEFAULT 1,

    PRIMARY KEY (object_id)
);

-- The epochs the leases carry are what the ledger held; anything whose
-- residency ended is not recoverable, which is the asymmetry that
-- motivated the sequence.
INSERT INTO object_epochs (object_id, epoch)
SELECT object_id, epoch FROM leases;

ALTER TABLE leases DROP COLUMN epoch;

DROP SEQUENCE object_epoch_seq;
