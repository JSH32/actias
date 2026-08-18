-- Monotonic per-object epochs: bumped on every lease acquisition, never
-- reset, surviving holder death (unlike lease rows, which cascade away).
-- The epoch fences WAL shipping: a zombie ex-owner's uploads lose to any
-- newer epoch's manifest.
CREATE TABLE object_epochs
(
    object_id CHAR(64) NOT NULL,
    epoch     BIGINT   NOT NULL DEFAULT 1,

    PRIMARY KEY (object_id)
);
