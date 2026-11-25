# bms-state data model

Requirement BMS-3: the current state of HiveMind objects is stored in a database;
the data model is designed and documented in the project.

- **Database:** PostgreSQL 18, database `state`, owned by user `bms_state`.
  No other service can connect to it (database-per-service, see `infra/bms/postgres/init.sql`).
- **Schema:** `crates/bms-state/migrations/`, applied automatically by `bms-state` on startup
  (sqlx migrations; applied versions are recorded in `_sqlx_migrations`).
- **Writer:** only `bms-state`, from events received through RabbitMQ (queue `bms.state`).
- **Readers:** the `bms-state` HTTP API (`/api/v1/...`).

![ER diagram](diagrams/png/er-state.png)

## Types

| Type             | Values                                           | Meaning                                       |
|------------------|--------------------------------------------------|-----------------------------------------------|
| `hive_status`    | `moving`, `stopped`, `inactive`, `disconnected` | Hive status as decided by BMS (BMS-2)         |
| `reported_state` | `stop`, `move`, `error`, `unknown`               | State the hive reports about itself           |

PostgreSQL `ENUM` types: stored in 4 bytes, read as words, and the database rejects
any other value.

## Tables

### `hives` — current state of each hive (one row per hive)

| Column              | Type             | Null | Meaning |
|---------------------|------------------|------|---------|
| `hive_id`           | `TEXT` PK        | no   | Hive identifier from Communication Control |
| `status`            | `hive_status`    | no   | `moving` / `stopped` from position changes; `inactive` after silence; `disconnected` by Communication Control |
| `latitude`          | `DOUBLE PRECISION` | yes | Latest position; NULL until the first telemetry after (re)connecting |
| `longitude`         | `DOUBLE PRECISION` | yes | |
| `speed`             | `DOUBLE PRECISION` | yes | Latest reported speed |
| `height`            | `DOUBLE PRECISION` | yes | Latest reported height |
| `reported_state`    | `reported_state` | yes  | State reported by the hive; for reference only (an idle hive may report `move`) |
| `last_telemetry_at` | `TIMESTAMPTZ`    | yes  | Timestamp of the telemetry the row reflects; older telemetry is ignored |
| `last_seen_at`      | `TIMESTAMPTZ`    | no   | When BMS last received any event about the hive |
| `connected_at`      | `TIMESTAMPTZ`    | no   | When the hive (re)connected |

### `ew_zones` — history of EW zones (one row per zone)

| Column        | Type               | Null | Meaning |
|---------------|--------------------|------|---------|
| `zone_id`     | `UUID` PK          | no   | Zone identifier from Communication Control |
| `latitude`    | `DOUBLE PRECISION` | no   | Center of the zone |
| `longitude`   | `DOUBLE PRECISION` | no   | |
| `radius_km`   | `DOUBLE PRECISION` | no   | Radius, `> 0` |
| `active_from` | `TIMESTAMPTZ`      | no   | Start of activity: creation time in Communication Control |
| `active_to`   | `TIMESTAMPTZ`      | yes  | End of activity; **NULL while the zone is active** |

BMS-4 asks for "activity status (active / inactive) and time bounds (from / to)":
the bounds are `active_from` / `active_to`, the status is derived (`active_to IS NULL`)
rather than stored, so the two cannot contradict each other.

Constraints and indexes:
- `CHECK (active_to IS NULL OR active_to >= active_from)`;
- partial index `ew_zones_active` on rows with `active_to IS NULL`, for listing active zones.

### `processed_events` — events already applied

| Column         | Type          | Null | Meaning |
|----------------|---------------|------|---------|
| `event_id`     | `UUID` PK     | no   | `event_id` of an applied event |
| `processed_at` | `TIMESTAMPTZ` | no   | When it was applied (default `now()`) |

RabbitMQ delivers at least once, so an event may arrive twice. Each event is applied in
one transaction together with inserting its `event_id` here; if the id already exists,
the transaction changes nothing.

## How events change the state

All changes of one event happen in one transaction with the `processed_events` insert.

| Event (routing key) | Effect |
|---------------------|--------|
| `hive.connected`, `hive.reconnected` | Upsert hive: `status = stopped`, position and telemetry fields reset to NULL, `connected_at` and `last_seen_at` set |
| `hive.telemetry` | Upsert hive: position, speed, height, reported state; `status = moving` if the position changed by more than 1e-7° since the previous telemetry, otherwise `stopped`. Skipped if older than `last_telemetry_at`. An unknown hive is created |
| `hive.disconnected` | `status = disconnected` |
| `hive.moved`, `hive.stopped` | No change (movement is seen in telemetry) |
| `zone.activated` | Insert zone with `active_from`; an already known zone is left unchanged |
| `zone.deactivated` | `active_to` = event time, only if the zone is still active |
| `zone.snapshot` | Insert missing zones; close active zones absent from the snapshot |

Besides events, a background task in `bms-state` marks hives `inactive` every 5 s when
nothing was heard from them for `HIVE_INACTIVE_AFTER_SECS` (default 30 s, as in CC-7).
Their next telemetry makes them `moving` or `stopped` again.

![Hive status](diagrams/png/hive-status.png)

## Guarantees

- **No duplicates:** a redelivered event is skipped (`processed_events`).
- **No partial updates:** an event is applied completely or not at all (transaction).
- **No going back in time:** telemetry older than the stored one does not overwrite it.
- **Self-healing:** a hive missed while BMS was down appears with its first telemetry;
  zones changed while `bms-ingest` was down are reconciled by the startup snapshot.

## Limitations

- **Clock skew:** `last_seen_at` and zone end times come from the `bms-ingest` clock,
  `active_from` from the Communication Control clock, the inactivity check uses the
  database clock. Small differences are harmless with a 30 s timeout; zone end time
  uses `GREATEST(active_from, …)` so it never precedes the start.
- **Zone end time precision:** up to the polling interval (2 s); for zones deleted while
  `bms-ingest` was down, the time of the reconciling snapshot.
- **`processed_events` grows** with every event; in production old rows would be deleted
  periodically (duplicates arrive within minutes, not months).
