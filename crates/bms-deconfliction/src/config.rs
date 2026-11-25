use crate::saga;
use bms_common::broker;
use bms_common::config::{self, ConfigError};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub rabbitmq_url: String,
    /// The service's own database, e.g. `postgres://user:password@host:5432/deconfliction`.
    pub database_url: String,
    /// Base URL of Communication Control, e.g. `http://localhost:8080`.
    pub cc_url: String,
    pub saga: saga::Settings,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            rabbitmq_url: broker::url_from_env()?,
            database_url: config::optional(
                "DATABASE_URL",
                // Local development database from infra/bms (credentials are local-only).
                "postgres://bms_deconfliction:bms_deconfliction@localhost:5432/deconfliction"
                    .to_owned(),
            )?,
            cc_url: config::optional("CC_URL", "http://localhost:8080".to_owned())?,
            saga: saga::Settings {
                safe_distance_m: config::optional("SAFE_DISTANCE_M", 1000.0)?,
                // Above the 600 m at which bms-state resolves a proximity alert
                // (500 m threshold, hysteresis factor 1.2).
                separation_m: config::optional("SEPARATION_M", 600.0)?,
                command_timeout: Duration::from_secs(config::optional(
                    "COMMAND_TIMEOUT_SECS",
                    15,
                )?),
                separation_timeout: Duration::from_secs(config::optional(
                    "SEPARATION_TIMEOUT_SECS",
                    120,
                )?),
            },
        })
    }
}
