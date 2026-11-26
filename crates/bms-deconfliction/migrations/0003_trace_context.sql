-- Trace context (W3C traceparent) of the alert that started the saga.
-- The engine sends commands later, in its own tick; with this context those
-- steps stay in the same distributed trace as the alert.
ALTER TABLE sagas ADD COLUMN trace_context JSONB NOT NULL DEFAULT '{}';
