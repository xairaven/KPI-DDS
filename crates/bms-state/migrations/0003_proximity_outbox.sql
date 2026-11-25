-- A proximity alert is stored once per pair: hive_id < other_hive_id.
ALTER TABLE alerts ADD CONSTRAINT alerts_proximity_ordered_pair
    CHECK (kind <> 'proximity' OR hive_id < other_hive_id);

-- At most one open proximity alert per pair.
CREATE UNIQUE INDEX alerts_open_pair ON alerts (hive_id, other_hive_id)
    WHERE kind = 'proximity' AND resolved_at IS NULL;

-- Transactional outbox: events to publish, written in the same transaction as the
-- state change they describe; a background relay publishes them to RabbitMQ.
CREATE TABLE outbox (
    id           BIGSERIAL PRIMARY KEY,
    routing_key  TEXT NOT NULL,
    envelope     JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_at TIMESTAMPTZ
);

CREATE INDEX outbox_unpublished ON outbox (id) WHERE published_at IS NULL;
