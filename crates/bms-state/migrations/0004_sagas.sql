-- Read copy of deconfliction sagas, built from saga.* events published by
-- bms-deconfliction (this service never reads that service's database).
CREATE TYPE saga_status AS ENUM (
    'started', 'command_sent', 'moving', 'compensating', 'completed', 'failed'
);

CREATE TABLE sagas (
    saga_id        BIGINT PRIMARY KEY,
    -- The proximity alert (in table alerts) that started the saga.
    alert_id       BIGINT NOT NULL,
    hive_id        TEXT NOT NULL,
    other_hive_id  TEXT NOT NULL,
    status         saga_status NOT NULL,
    moving_hive_id TEXT,
    -- Reason of the latest change.
    note           TEXT NOT NULL,
    updated_at     TIMESTAMPTZ NOT NULL
);
