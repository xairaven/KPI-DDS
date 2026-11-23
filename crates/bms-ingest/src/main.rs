mod cc_message;
mod config;
mod redis_listener;

use crate::config::Config;
use std::process::ExitCode;
use tracing::{error, info};

#[tokio::main]
async fn main() -> ExitCode {
    bms_common::config::load_dotenv();
    bms_common::logging::init();

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            error!(%error, "Invalid configuration");
            return ExitCode::FAILURE;
        },
    };
    info!(?config, "Service started");

    tokio::select! {
        () = redis_listener::run(&config, |event| info!(?event, "Event")) => {},
        _ = tokio::signal::ctrl_c() => info!("Shutting down"),
    }
    ExitCode::SUCCESS
}
