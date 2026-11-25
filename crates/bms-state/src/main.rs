mod api;
mod config;
mod inactivity;
mod monitoring;
mod outbox;
mod projection;

use crate::config::Config;
use bms_common::broker::{self, Publisher, QueueSpec};
use bms_common::db;
use std::process::ExitCode;
use tokio::net::TcpListener;
use tracing::{error, info};

const QUEUE: QueueSpec = QueueSpec {
    name: "bms.state",
    bindings: &["hive.*", "zone.*", "saga.*"],
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

    let listener = match TcpListener::bind(config.api_addr).await {
        Ok(listener) => listener,
        Err(error) => {
            error!(%error, addr = %config.api_addr, "Cannot listen for HTTP API");
            return ExitCode::FAILURE;
        },
    };
    info!(addr = %config.api_addr, "HTTP API listening, Swagger UI at /swagger");

    let settings = config.monitoring;
    tokio::select! {
        () = broker::consume(&config.rabbitmq_url, QUEUE, |envelope| {
            // The pool is a cheap handle (Arc inside); each event gets its own copy.
            let pool = pool.clone();
            async move { projection::apply(&pool, settings, &envelope).await }
        }) => {},
        () = inactivity::run(pool.clone(), config.hive_inactive_after, settings) => {},
        () = outbox::relay(pool.clone(), Publisher::new(config.rabbitmq_url.clone())) => {},
        result = axum::serve(listener, api::router(pool.clone())) => {
            if let Err(error) = result {
                error!(%error, "HTTP API stopped");
            }
        },
        _ = tokio::signal::ctrl_c() => info!("Shutting down"),
    }
    ExitCode::SUCCESS
}
