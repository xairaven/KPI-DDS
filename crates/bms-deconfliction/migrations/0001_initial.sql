-- Copies of other services' data the saga needs, built from events
-- (database-per-service: this service never reads the database of bms-state).

-- Latest position of every hive (from hive.telemetry).
CREATE TABLE hive_positions (
    hive_id     TEXT PRIMARY KEY,
    latitude    DOUBLE PRECISION NOT NULL,
    longitude   DOUBLE PRECISION NOT NULL,
    reported_at TIMESTAMPTZ NOT NULL
);

-- EW zones active now (from zone.*); a deactivated zone is deleted.
CREATE TABLE ew_zones (
    zone_id   UUID PRIMARY KEY,
    latitude  DOUBLE PRECISION NOT NULL,
    longitude DOUBLE PRECISION NOT NULL,
    radius_km DOUBLE PRECISION NOT NULL CHECK (radius_km > 0)
);

-- Deconfliction saga: separates two hives that came too close.
--   started       saga created, safe point being planned
--   command_sent  move command sent to Communication Control, waiting for delivery
--   moving        command delivered (hive.moved), waiting until the hives are apart
--   compensating  attempt failed, the moved hive is being sent back
--   completed     hives are apart
--   failed        both attempts failed, the operator must act
CREATE TYPE saga_status AS ENUM (
    'started', 'command_sent', 'moving', 'compensating', 'completed', 'failed'
);

CREATE TABLE sagas (
    saga_id          BIGSERIAL PRIMARY KEY,
    -- The proximity alert of bms-state that started the saga: one saga per alert.
    alert_id         BIGINT NOT NULL UNIQUE,
    hive_id          TEXT NOT NULL,
    other_hive_id    TEXT NOT NULL,
    status           saga_status NOT NULL,
    -- 1: move one hive; 2 (plan B): move the other one.
    attempt          SMALLINT NOT NULL DEFAULT 1 CHECK (attempt IN (1, 2)),
    -- Hive commanded in the current attempt, its position before the command
    -- (where compensation sends it back) and the safe point it is sent to.
    moving_hive_id   TEXT,
    origin_latitude  DOUBLE PRECISION,
    origin_longitude DOUBLE PRECISION,
    target_latitude  DOUBLE PRECISION,
    target_longitude DOUBLE PRECISION,
    -- The current step must finish by then, otherwise the saga compensates.
    deadline         TIMESTAMPTZ,
    started_at       TIMESTAMPTZ NOT NULL,
    finished_at      TIMESTAMPTZ,
    CHECK ((status IN ('completed', 'failed')) = (finished_at IS NOT NULL))
);

-- Unfinished sagas, looked up by deadline and by hive.
CREATE INDEX sagas_unfinished ON sagas (deadline) WHERE finished_at IS NULL;

-- Journal of every step of every saga.
CREATE TABLE saga_steps (
    step_id BIGSERIAL PRIMARY KEY,
    saga_id BIGINT NOT NULL REFERENCES sagas (saga_id),
    status  saga_status NOT NULL,
    note    TEXT NOT NULL,
    at      TIMESTAMPTZ NOT NULL
);

-- Events already applied (see bms_common::db::begin_event).
CREATE TABLE processed_events (
    event_id     UUID PRIMARY KEY,
    processed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Transactional outbox for saga.* events (see bms-state, migration 0003).
CREATE TABLE outbox (
    id           BIGSERIAL PRIMARY KEY,
    routing_key  TEXT NOT NULL,
    envelope     JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_at TIMESTAMPTZ
);

CREATE INDEX outbox_unpublished ON outbox (id) WHERE published_at IS NULL;
