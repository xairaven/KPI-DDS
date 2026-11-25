use crate::monitoring;
use bms_common::broker;
use bms_common::config::{self, ConfigError};
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub rabbitmq_url: String,
    /// The service's own database, e.g. `postgres://user:password@host:5432/state`.
    pub database_url: String,
    /// A hive not heard from for this long is considered inactive.
    pub hive_inactive_after: Duration,
    /// Address the HTTP API listens on.
    pub api_addr: SocketAddr,
    pub monitoring: monitoring::Settings,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            rabbitmq_url: broker::url_from_env()?,
            database_url: config::optional(
                "DATABASE_URL",
                // Local development database from infra/bms (credentials are local-only).
                "postgres://bms_state:bms_state@localhost:5432/state".to_owned(),
            )?,
            // 30 s as in requirement CC-7 ("not updated for more than 30 seconds").
            hive_inactive_after: Duration::from_secs(config::optional(
                "HIVE_INACTIVE_AFTER_SECS",
                30,
            )?),
            // 0.0.0.0: reachable from other machines and containers, not only localhost.
            api_addr: config::optional(
                "API_ADDR",
                SocketAddr::from(([0, 0, 0, 0], 9000)),
            )?,
            monitoring: monitoring::Settings {
                proximity_threshold_m: config::optional("PROXIMITY_THRESHOLD_M", 500.0)?,
            },
        })
    }
}
