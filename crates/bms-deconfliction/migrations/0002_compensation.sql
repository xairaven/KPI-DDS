-- Whether the compensating command (send the moved hive back to its origin)
-- has been accepted by Communication Control in the current attempt.
ALTER TABLE sagas ADD COLUMN compensation_sent BOOLEAN NOT NULL DEFAULT false;
