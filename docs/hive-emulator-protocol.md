# Hive Emulator protocol (as implemented)

What the lecturer's system actually sends and accepts, reconstructed from its source code
(`infra/hiveemulator/upstream`, commit `bd78215`) and observed traffic.
The course PDF describes an earlier design; where they differ, this document follows the code.

```
                (A) HTTP                         (C) Redis pub/sub
 HiveMind  ◄──────────────►  Communication  ─────────────────────►  BMS
 (swarm)                     Control           channel "HiveChannel"
                                ▲
                                │ (B) HTTP, client API
                         map / operator / BMS
```

The BMS uses **(C)** as its input and **(B)** to read EW zones and send commands.
It never talks to swarms directly.

## Common conventions

- JSON field names are **PascalCase** (`HiveID`, `Latitude`).
- `Location` = `{ "Latitude": float, "Longitude": float }`.
- Timestamps are ISO 8601 (`2026-11-18T17:27:27.8901024+00:00`).
- `State` is a **number** (C# enum `HiveMindState`):

  | Value | Name               |
  |-------|--------------------|
  | 0     | None               |
  | 1     | Stop               |
  | 2     | Move               |
  | 3     | Error              |
  | 4     | SetInterference    |
  | 5     | DeleteInterference |

## (C) Message bus — Redis Pub/Sub, channel `HiveChannel`

Fire-and-forget: a subscriber receives only messages published while it is connected.
Messages have **no type field**; the type is identified by which fields are present.
Every message has `HiveID`.

| Type         | Identified by                | Published when                                           |
|--------------|------------------------------|----------------------------------------------------------|
| Connected    | `IsSuccessfullyConnected`    | A hive unknown to CC connects                            |
| Reconnected  | `IsSuccessfullyReconnected`  | A hive already known to CC connects again (e.g. restart) |
| Disconnected | `IsSuccessfullyDisconnected` | **Only** on `DELETE /api/v1/client/hive/{id}`            |
| Telemetry    | `Telemetry`                  | CC accepts a telemetry report (~every 5 s per hive)      |
| Move         | `Destination`                | A move command was delivered to the hive                 |
| Stop         | `IsImmediateStop`            | A stop command was delivered to the hive                 |

### Connected / Reconnected

```json
{
  "IsSuccessfullyConnected": true,
  "Hive": {
    "HiveID": "2", "HiveIP": "hive-2", "HivePort": 5149, "HiveSchema": "http",
    "Telemetry": null
  },
  "InitialOperationalArea": {
    "RadiusKM": 5,
    "InitialLocation": { "Latitude": 48.69719, "Longitude": 38.066246 },
    "InitialHeight": 1, "Speed": 5, "TelemetryIntervalMs": 30000, "PingIntervalMs": 15000
  },
  "HiveID": "2"
}
```

Reconnected is identical except `IsSuccessfullyReconnected` replaces `IsSuccessfullyConnected`.

### Disconnected

```json
{ "IsSuccessfullyDisconnected": true, "HiveID": "2" }
```

### Telemetry

```json
{
  "IsSuccessfullySent": true,
  "Telemetry": {
    "HiveID": "1",
    "Location": { "Latitude": 48.68248, "Longitude": 38.04585 },
    "Speed": 15, "Height": 5, "State": 2,
    "Timestamp": "2026-11-18T17:27:27.9135011+00:00"
  },
  "HiveID": "1"
}
```

### Move

```json
{ "IsSuccessfullySent": true, "Destination": { "Latitude": 48.7, "Longitude": 38.07 }, "HiveID": "1" }
```

### Stop

```json
{ "IsImmediateStop": true, "StopTimestamp": "0001-01-01T00:00:00", "HiveID": "1" }
```

### Not published on the bus

- EW zone (interference) added or deleted — must be polled via `GET /api/v1/client/interferences`.
- A hive that crashes or stops — no ping mechanism exists, so no Disconnected event is sent;
  its telemetry simply stops.
- Commands that failed to reach the hive.

## (B) Client API — Communication Control, `http://<cc>:8080/api/v1/client`

| Method | Path                  | Body / result                                                               |
|--------|-----------------------|-----------------------------------------------------------------------------|
| GET    | `/area`               | Operational area config                                                     |
| GET    | `/hive`               | All connected hives (`HiveModel[]`, with last `Telemetry`)                  |
| GET    | `/hive/{hiveId}`      | One hive, or 404                                                            |
| DELETE | `/hive/{hiveId}`      | Disconnect hive → publishes Disconnected                                    |
| PATCH  | `/hive`               | `{"Hives":["1"],"Destination":{Location}}` → 202, async; publishes Move     |
| POST   | `/hive/stop`          | `{"Hives":["1"]}` → 202, async; publishes Stop                              |
| GET    | `/interferences`      | `[{"Id":uuid,"RadiusKM":float,"Location":{...},"CreatedAt":time}]`          |
| POST   | `/interference`       | `{"RadiusKM":float,"Location":{...}}` → `{"InterferenceId":uuid,...}`       |
| DELETE | `/interference/{id}`  | → 204 always, even for unknown ids                                          |

`/area` response:

```json
{
  "Latitude": 48.69719, "Longitude": 38.066246, "Radius_KM": 5,
  "InitialHeight_KM": 1, "InitialSpeed_KM": 5,
  "TelemetryInterval_MS": 30000, "PingInterval_MS": 15000
}
```

## (A) Hive ↔ Communication Control (context only, not used by BMS)

| From → To | Request                                     | Response                                              |
|-----------|---------------------------------------------|-------------------------------------------------------|
| Hive → CC | `POST /api/v1/hive/connect` `{HiveSchema, HiveIP, HivePort, HiveID}` | `{ConnectResult, OperationalArea, Interferences}` |
| Hive → CC | `POST /api/v1/hive/telemetry` `{HiveID, Location, Speed, Height, State}` | `{Timestamp}`, or 404 if hive unknown |
| CC → Hive | `POST /api/v1/command` with `commandType` = `Move` / `Stop` / `SetInterference` / `DeleteInterference` | — |
| —         | `GET /api/v1/ping` exists on HiveMind but is never called | — |

## Deviations from the course PDF

| PDF says                                         | Implementation does                                        |
|--------------------------------------------------|------------------------------------------------------------|
| `POST /api/v1/connect`, `/api/v1/telemetry`      | `/api/v1/hive/connect`, `/api/v1/hive/telemetry`           |
| Command types Stop / Move / GetTelemetry         | Move / Stop / SetInterference / DeleteInterference         |
| Ping both ways, drop hive after 10 missed pings (CC-8/9, HM-8/9) | Not implemented                            |
| HiveMind retries connecting (HM-1)               | Exits on first failure                                     |
| State values Stop / Move / Error                 | Numeric enum; idle hives may report 2 (Move)               |
| Telemetry interval from config (30 s)            | Hives report ~every 5 s                                    |
| Events: joined, disconnected, telemetry, status changed | Connected, Reconnected, Disconnected, Telemetry, Move, Stop |
