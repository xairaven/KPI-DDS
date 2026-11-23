use bms_common::config::{self, ConfigError};

#[derive(Debug, Clone)]
pub struct Config {
    /// Redis used by Communication Control as its message bus.
    pub redis_url: String,
    /// Pub/Sub channel Communication Control publishes hive events to.
    pub redis_channel: String,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            redis_url: config::optional(
                "REDIS_URL",
                "redis://localhost:6379".to_owned(),
            )?,
            redis_channel: config::optional("REDIS_CHANNEL", "HiveChannel".to_owned())?,
        })
    }
}
