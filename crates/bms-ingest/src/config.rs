use bms_common::config::{self, ConfigError};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    /// Redis used by Communication Control as its message bus.
    pub redis_url: String,
    /// Pub/Sub channel Communication Control publishes hive events to.
    pub redis_channel: String,
    /// Base URL of Communication Control, e.g. `http://localhost:8080`.
    pub cc_url: String,
    /// How often EW zones are polled from Communication Control.
    pub zone_poll_interval: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            redis_url: config::optional(
                "REDIS_URL",
                "redis://localhost:6379".to_owned(),
            )?,
            redis_channel: config::optional("REDIS_CHANNEL", "HiveChannel".to_owned())?,
            cc_url: config::optional("CC_URL", "http://localhost:8080".to_owned())?,
            zone_poll_interval: Duration::from_millis(config::optional(
                "ZONE_POLL_INTERVAL_MS",
                2000,
            )?),
        })
    }
}
