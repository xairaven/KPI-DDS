use bms_common::broker;
use bms_common::config::{self, ConfigError};

#[derive(Debug, Clone)]
pub struct Config {
    pub rabbitmq_url: String,
    /// The service's own database, e.g. `postgres://user:password@host:5432/deconfliction`.
    pub database_url: String,
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
        })
    }
}
