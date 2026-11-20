-- Database-per-service: each service gets its own database and user,
-- and cannot connect to the other service's database.

CREATE USER bms_state WITH PASSWORD 'bms_state';
CREATE DATABASE state OWNER bms_state;
REVOKE CONNECT ON DATABASE state FROM PUBLIC;

CREATE USER bms_deconfliction WITH PASSWORD 'bms_deconfliction';
CREATE DATABASE deconfliction OWNER bms_deconfliction;
REVOKE CONNECT ON DATABASE deconfliction FROM PUBLIC;
