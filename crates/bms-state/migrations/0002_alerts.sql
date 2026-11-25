-- Monitoring alerts (lab 4): a hive inside an EW zone, two hives too close.
CREATE TYPE alert_kind AS ENUM ('zone_entered', 'proximity');

-- An alert is open while resolved_at is NULL.
CREATE TABLE alerts (
    alert_id      BIGSERIAL PRIMARY KEY,
    kind          alert_kind NOT NULL,
    hive_id       TEXT NOT NULL REFERENCES hives (hive_id),
    -- zone_entered: the zone the hive is inside.
    zone_id       UUID REFERENCES ew_zones (zone_id),
    -- proximity: the other hive and the distance between them.
    other_hive_id TEXT REFERENCES hives (hive_id),
    distance_m    DOUBLE PRECISION,
    raised_at     TIMESTAMPTZ NOT NULL,
    resolved_at   TIMESTAMPTZ,
    CHECK (
        (kind = 'zone_entered' AND zone_id IS NOT NULL AND other_hive_id IS NULL)
        OR (kind = 'proximity' AND other_hive_id IS NOT NULL AND zone_id IS NULL)
    ),
    CHECK (resolved_at IS NULL OR resolved_at >= raised_at)
);

-- At most one open alert per hive and zone.
CREATE UNIQUE INDEX alerts_open_zone ON alerts (hive_id, zone_id)
    WHERE kind = 'zone_entered' AND resolved_at IS NULL;

-- Speeds up "list open alerts".
CREATE INDEX alerts_open ON alerts (raised_at) WHERE resolved_at IS NULL;
