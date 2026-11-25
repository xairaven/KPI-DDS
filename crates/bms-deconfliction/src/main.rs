mod config;
mod projection;

use crate::config::Config;
use bms_common::broker::{self, QueueSpec};
use bms_common::db;
use std::process::ExitCode;
use tracing::{error, info};

const QUEUE: QueueSpec = QueueSpec {
    name: "bms.deconfliction",
    bindings: &["alert.proximity", "hive.telemetry", "hive.moved", "zone.*"],
};

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
    info!("Service started");

    let pool =
        match db::connect(&config.database_url, &sqlx::migrate!("./migrations")).await {
            Ok(pool) => pool,
            Err(error) => {
                error!(%error, "Database migration failed");
                return ExitCode::FAILURE;
            },
        };

    tokio::select! {
        () = broker::consume(&config.rabbitmq_url, QUEUE, |envelope| {
            let pool = pool.clone();
            async move { projection::apply(&pool, &envelope).await }
        }) => {},
        _ = tokio::signal::ctrl_c() => info!("Shutting down"),
    }
    ExitCode::SUCCESS
}
