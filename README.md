# Battlefield Management System

Labs of the course «Проектування розподілених систем» (Design of Distributed Systems), KPI.

**Author:** Kovalov Oleksandr, group ІМ-51мн

> [!NOTE]
> Work in progress.

Battlefield Management System (BMS) observes drone swarms (hives) of the lecturer's
[Hive Emulator](https://gitlab.com/kzotkin/hiveemulator). It keeps the state of every
hive, watches the electronic warfare (EW) zones, raises alerts when a hive enters a zone
or two hives come too close, and separates such hives automatically with a saga.

![Component diagram](docs/diagrams/png/component.png)

## Features

- **State reconstruction** from the Communication Control message bus (Redis Pub/Sub):
  position, speed, `moving` / `stopped` / `inactive` / `disconnected` status, kept in PostgreSQL.
- **EW zones** polled from Communication Control, with history (`active_from` / `active_to`).
- **Monitoring**: a hive inside an EW zone, two hives closer than 500 m (with hysteresis).
- **Deconfliction saga**: commands one hive to a safe point; on failure compensates
  (sends it back) and tries plan B with the other hive.
- **Reliable messaging**: RabbitMQ with durable quorum queues, publisher confirms,
  at-least-once delivery, idempotent consumers, transactional outbox.
- **REST API** with OpenAPI / Swagger UI, protected by bearer tokens.
- **Desktop client** (egui) with an OpenStreetMap map of hives, zones, alerts and sagas.
- **Distributed tracing** (OpenTelemetry → Jaeger): one bus message can be followed
  through every service, including the saga's command to Communication Control.

## Architecture

| Component | Role |
|---|---|
| `bms-ingest` | Subscribes to Redis `HiveChannel`, polls EW zones, translates both into BMS events and publishes them to RabbitMQ. Runs as a **single instance**. |
| `bms-state` | Projects events into its database (`state`), detects zone crossings, proximity and inactive hives, serves the REST API on port 9000. |
| `bms-deconfliction` | Orchestrates the deconfliction saga in its own database (`deconfliction`) and sends move commands to Communication Control. |
| `bms-ui` | Desktop client of the `bms-state` API. |
| `bms-common` | Library shared by the services: events, RabbitMQ, database, outbox, geometry, telemetry. |

Services talk only through the RabbitMQ topic exchange `bms.events`; each has its own
database and database user (database per service).

| Routing key | Producer | Consumers |
|---|---|---|
| `hive.connected`, `hive.reconnected`, `hive.disconnected`, `hive.telemetry`, `hive.moved`, `hive.stopped` | ingest | state; deconfliction (`hive.telemetry`, `hive.moved`) |
| `zone.activated`, `zone.deactivated`, `zone.snapshot` | ingest | state, deconfliction |
| `alert.zone_entered`, `alert.proximity` | state | deconfliction (`alert.proximity`) |
| `saga.<status>` | deconfliction | state |

More documentation:

- [Hive Emulator protocol](docs/hive-emulator-protocol.md): what Communication Control sends and accepts.
- [Data model](docs/data-model.md): both databases, guarantees and known limitations.
- [Diagrams](docs/diagrams): PlantUML sources and rendered PNGs.

## Repository layout

```
crates/            Rust workspace: bms-common, bms-ingest, bms-state, bms-deconfliction, bms-ui
Dockerfile         one multi-stage Dockerfile for the three services (--target <service>)
infra/hiveemulator lecturer's system: Redis, Communication Control, map, HiveMinds (git submodule + compose)
infra/bms          RabbitMQ, PostgreSQL, Jaeger and the BMS services (compose)
docs/              protocol notes, data model, diagrams
```

## Running locally

Requirements: Docker with Compose, Rust 1.100 (installed automatically by `rust-toolchain.toml`).

```bash
git clone --recurse-submodules <this repository>
# or, in an existing clone:
git submodule update --init
```

**1. The lecturer's system** (Redis, Communication Control, map, two hives):

```bash
docker compose -f infra/hiveemulator/docker-compose.yml up -d
```

**2. BMS** — everything in containers:

```bash
docker compose -f infra/bms/docker-compose.yml --profile app up -d --build
```

Without `--profile app` only the infrastructure starts (RabbitMQ, PostgreSQL, Jaeger),
and the services can be run with cargo instead, e.g. for debugging:

```bash
cargo run -p bms-ingest
cargo run -p bms-state
cargo run -p bms-deconfliction
```

**3. The desktop client:**

```bash
BMS_API_TOKEN=local-dev-token cargo run -p bms-ui --release
```

Useful commands:

```bash
docker compose -f infra/bms/docker-compose.yml --profile app logs -f bms-state   # logs of one service
docker compose -f infra/bms/docker-compose.yml --profile app stop bms-ingest bms-state bms-deconfliction
docker compose -f infra/bms/docker-compose.yml down       # stop, data kept
docker compose -f infra/bms/docker-compose.yml down -v    # stop and wipe data (re-runs init.sql)
docker compose -f infra/hiveemulator/docker-compose.yml restart hive-1 hive-2   # hives back to start
```

### Kubernetes (kind)

The same BMS in a local Kubernetes cluster ([`infra/k8s`](infra/k8s): `base` + `overlays/kind`).
It uses the same host ports, so stop the BMS compose first.

```bash
docker compose -f infra/hiveemulator/docker-compose.yml up -d
docker compose -f infra/bms/docker-compose.yml --profile app stop
docker compose -f infra/bms/docker-compose.yml --profile app build      # images bms-*:local

kind create cluster --config infra/k8s/overlays/kind/kind-cluster.yaml
docker network connect hiveemulator_default bms-control-plane         # reach Redis and CC by name
kind load docker-image bms-ingest:local bms-state:local bms-deconfliction:local --name bms
kubectl apply -k infra/k8s/overlays/kind

kubectl -n bms get pods
kubectl -n bms logs deploy/bms-state -f
kind delete cluster --name bms                                         # remove everything
```

After rebuilding an image, load it again and restart the deployment:
`kubectl -n bms rollout restart deploy/bms-state`.

### Cloud: AWS + Azure

```
        AWS eu-north-1 (EC2 t3.small)               Azure denmarkeast (VM B2als_v2)
  ┌────────────────────────────────────┐      ┌────────────────────────────────────┐
  │ Docker Compose:                    │      │ k3s (Kubernetes):                  │
  │  Redis (password) ◄────────────────┼──────┤  bms-ingest                        │
  │  Communication Control ◄───────────┼──────┤  bms-deconfliction                 │
  │  map-client, HiveMind ×2           │      │  bms-state ◄── HTTPS (Traefik,     │
  │                                    │      │  RabbitMQ, PostgreSQL, Jaeger      │
  └────────────────────────────────────┘      └──────────── Let's Encrypt) ────────┘
   firewall: Redis and CC only from Azure       firewall: 80/443 for all; SSH and
   (CC and the map also from the admin)         the Kubernetes API from the admin
```

Infrastructure is code ([`infra/cloud`](infra/cloud), Terraform / OpenTofu), images come from
GitHub Container Registry, built by [GitHub Actions](.github/workflows/images.yml).

Requirements: `aws configure` (an IAM user with EC2 access), `az login`, an SSH key in
`~/.ssh/bms-cloud/bms-cloud`, and once per Azure subscription:
`az provider register --namespace Microsoft.Network` and `--namespace Microsoft.Compute`.

```bash
cd infra/cloud
echo "azure_subscription_id = \"$(az account show --query id -o tsv)\"" > terraform.tfvars
tofu init
tofu apply        # servers, firewalls, addresses; writes infra/k8s/overlays/azure/{cloud,secrets}.env
tofu output       # addresses, URLs, SSH commands

# kubeconfig of the k3s cluster (after cloud-init has installed k3s)
ssh -i ~/.ssh/bms-cloud/bms-cloud azureuser@$(tofu output -raw azure_public_ip) sudo cat /etc/rancher/k3s/k3s.yaml \
  | sed "s/127.0.0.1/$(tofu output -raw bms_url | sed 's|https://||')/" > kubeconfig
cd ../..

export KUBECONFIG=infra/cloud/kubeconfig
kubectl apply -k infra/k8s/overlays/azure
kubectl -n bms get pods

BMS_API_URL=$(tofu -chdir=infra/cloud output -raw bms_url) \
BMS_API_TOKEN=$(tofu -chdir=infra/cloud output -raw api_token) \
  cargo run -p bms-ui --release
kubectl -n bms port-forward svc/jaeger 16686     # Jaeger UI at http://localhost:16686

tofu -chdir=infra/cloud destroy                   # remove everything
```

The firewalls allow SSH and the admin UIs only from the address `tofu apply` was run from;
if it changes, run `tofu apply` again.

### Addresses

| What | Address | Credentials |
|---|---|---|
| BMS API (Swagger UI) | http://localhost:9000/swagger/ | bearer token `local-dev-token` (Authorize button) |
| Jaeger (traces) | http://localhost:16686 | — |
| RabbitMQ management | http://localhost:15672 | `bms` / `bms` |
| PostgreSQL | `localhost:5432`, databases `state`, `deconfliction` | `bms_state` / `bms_state`, `bms_deconfliction` / `bms_deconfliction` |
| Communication Control (Swagger UI) | http://localhost:8080/swagger | — |
| Lecturer's web map | http://localhost:8081 | — |

All credentials are for local development only.

### Configuration

Every service reads environment variables (or a `.env` file); the defaults fit the local setup.

| Variable | Used by | Default |
|---|---|---|
| `RABBITMQ_URL` | ingest, state, deconfliction | `amqp://bms:bms@localhost:5672` |
| `REDIS_URL` | ingest | `redis://localhost:6379` |
| `REDIS_CHANNEL` | ingest | `HiveChannel` |
| `CC_URL` | ingest, deconfliction | `http://localhost:8080` |
| `ZONE_POLL_INTERVAL_MS` | ingest | `2000` |
| `DATABASE_URL` | state, deconfliction | `postgres://bms_state:bms_state@localhost:5432/state`, `postgres://bms_deconfliction:bms_deconfliction@localhost:5432/deconfliction` |
| `API_ADDR` | state | `0.0.0.0:9000` |
| `BMS_API_TOKENS` | state | **required**, comma-separated bearer tokens accepted by the API (`local-dev-token` in compose and kind) |
| `HIVE_INACTIVE_AFTER_SECS` | state | `30` |
| `PROXIMITY_THRESHOLD_M` | state | `500` |
| `SAFE_DISTANCE_M` | deconfliction | `1000` |
| `SEPARATION_M` | deconfliction | `600` |
| `COMMAND_TIMEOUT_SECS` | deconfliction | `15` |
| `SEPARATION_TIMEOUT_SECS` | deconfliction | `120` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | ingest, state, deconfliction | unset: traces are not exported. `http://localhost:4318` sends them to the local Jaeger |
| `RUST_LOG` | all | `info` |
| `BMS_API_URL` | ui | `http://localhost:9000` |
| `BMS_API_TOKEN` | ui | unset: requests get 401. The bearer token sent to the API |

### API authorization

Every request to `/api/v1/*` needs `Authorization: Bearer <token>` with one of the
tokens in `BMS_API_TOKENS`; otherwise the API answers `401 Unauthorized`. Swagger UI
and the OpenAPI document stay public; in Swagger UI, enter the token with **Authorize**.
Several tokens may be configured at once, so one can be replaced without downtime.

```bash
curl -H "Authorization: Bearer local-dev-token" http://localhost:9000/api/v1/hives
```

In the cloud the token is generated by Terraform: `tofu -chdir=infra/cloud output -raw api_token`.

### Tracing

Each event carries the W3C trace context of the span that created it, so a consumer
continues the producer's trace. Sagas store the context of their alert, so the commands
the saga engine sends later stay in the same trace. In Jaeger, pick the service
`bms-deconfliction` and the operation `saga start attempt` to see a whole chain:

```
bms-ingest         receive HiveChannel
bms-ingest         publish hive.telemetry
bms-state          consume hive.telemetry
bms-state          publish alert.proximity        (transactional outbox)
bms-deconfliction  consume alert.proximity        (saga created)
bms-deconfliction  saga start attempt             (engine tick)
bms-deconfliction  PATCH /api/v1/client/hive      (command to Communication Control)
bms-state          consume saga.command_sent
```

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets   # pedantic + nursery lints, no unwrap/expect/panic outside tests
```

Diagrams are rendered with PlantUML:

```bash
plantuml -tpng -o ../png docs/diagrams/plantuml/*.puml
```

## License

MIT (declared in `Cargo.toml`).
