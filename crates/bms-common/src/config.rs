//! Configuration from environment variables.
//!
//! Services are configured the same way locally, in Docker and in Kubernetes:
//! through environment variables. For local runs, a `.env` file in the working
//! directory is loaded first (variables already set in the environment win).

use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Environment variable {0} is not set")]
    Missing(&'static str),
    #[error("Environment variable {name} has invalid value \"{value}\"")]
    Invalid { name: &'static str, value: String },
}

/// Loads variables from `.env`, if the file exists.
pub fn load_dotenv() {
    // A missing `.env` is normal (e.g. inside containers), so the error is ignored.
    let _ = dotenvy::dotenv();
}

/// Reads a required variable.
pub fn required<T: FromStr>(name: &'static str) -> Result<T, ConfigError> {
    let value = std::env::var(name).map_err(|_| ConfigError::Missing(name))?;
    parse(name, value)
}

/// Reads an optional variable, falling back to `default`.
pub fn optional<T: FromStr>(name: &'static str, default: T) -> Result<T, ConfigError> {
    std::env::var(name).map_or_else(|_| Ok(default), |value| parse(name, value))
}

fn parse<T: FromStr>(name: &'static str, value: String) -> Result<T, ConfigError> {
    value
        .parse()
        .map_err(|_| ConfigError::Invalid { name, value })
}
