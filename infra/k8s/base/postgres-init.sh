#!/bin/sh
# Database per service, as infra/bms/postgres/init.sql, but with the passwords
# taken from the Secret instead of being written in the file.
# Runs once, when the data volume is empty.
set -eu

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" \
    -v state_password="$STATE_DB_PASSWORD" \
    -v deconfliction_password="$DECONFLICTION_DB_PASSWORD" <<'SQL'
CREATE USER bms_state WITH PASSWORD :'state_password';
CREATE DATABASE state OWNER bms_state;
REVOKE CONNECT ON DATABASE state FROM PUBLIC;

CREATE USER bms_deconfliction WITH PASSWORD :'deconfliction_password';
CREATE DATABASE deconfliction OWNER bms_deconfliction;
REVOKE CONNECT ON DATABASE deconfliction FROM PUBLIC;
SQL
