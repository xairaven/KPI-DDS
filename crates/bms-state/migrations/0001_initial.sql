-- moving / stopped: decided by BMS from position changes, not from the hive's own report.
-- inactive: no telemetry for too long. disconnected: Communication Control dropped the hive.
CREATE TYPE hive_status AS ENUM ('moving', 'stopped', 'inactive', 'disconnected');

-- State as reported by the hive itself (not fully reliable).
CREATE TYPE reported_state AS ENUM ('stop', 'move', 'error', 'unknown');

-- Current state of every hive known to BMS (one row per hive).
CREATE TABLE hives (
    hive_id           TEXT PRIMARY KEY,
    status            hive_status NOT NULL,
    -- Position and motion from the latest telemetry; NULL until the first telemetry arrives.
    latitude          DOUBLE PRECISION,
    longitude         DOUBLE PRECISION,
    speed             DOUBLE PRECISION,
    height            DOUBLE PRECISION,
    -- Kept for reference only; status is what BMS relies on.
    reported_state    reported_state,
    -- Timestamp of the telemetry the row reflects; older telemetry is ignored.
    last_telemetry_at TIMESTAMPTZ,
    -- When BMS last heard anything about the hive.
    last_seen_at      TIMESTAMPTZ NOT NULL,
    connected_at      TIMESTAMPTZ NOT NULL
);

-- History of EW zones: a zone is active while active_to is NULL.
CREATE TABLE ew_zones (
    zone_id     UUID PRIMARY KEY,
    latitude    DOUBLE PRECISION NOT NULL,
    longitude   DOUBLE PRECISION NOT NULL,
    radius_km   DOUBLE PRECISION NOT NULL CHECK (radius_km > 0),
    active_from TIMESTAMPTZ NOT NULL,
    active_to   TIMESTAMPTZ,
    CHECK (active_to IS NULL OR active_to >= active_from)
);

-- Speeds up "list active zones".
CREATE INDEX ew_zones_active ON ew_zones (active_to) WHERE active_to IS NULL;

-- Events already applied, so a redelivered event is not applied twice.
CREATE TABLE processed_events (
    event_id     UUID PRIMARY KEY,
    processed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
