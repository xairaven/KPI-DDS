# Design of Distributed Systems

University Labs.

**Author:** Kovalov Oleksandr

**Group:** ІМ-51мн

> [!NOTE]
> Work in Progress.

- Communication Channel Swagger: http://localhost:8080/swagger
- Map: http://localhost:8081

- RabbitMQ Admin: http://localhost:15672 (Dev Creds: bms / bms)
- BMS Swagger: http://localhost:9000/swagger/

### Docker

```bash
docker compose -f infra/hiveemulator/docker-compose.yml up -d   # start cc & hivemind
docker compose -f infra/hiveemulator/docker-compose.yml down    # stop cc & hivemind
```

```bash
docker compose -f infra/bms/docker-compose.yml up -d            # start bms
docker compose -f infra/bms/docker-compose.yml down             # stop (data kept)
docker compose -f infra/bms/docker-compose.yml down -v          # stop AND wipe data (re-runs init.sql next time)
```
