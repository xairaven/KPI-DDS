mod cc_client;
mod config;
mod planner;
mod projection;
mod saga;

use crate::cc_client::{Area, CcClient};
use crate::config::Config;
use bms_common::broker::{self, Publisher, QueueSpec};
use bms_common::{db, outbox};
use std::process::ExitCode;
use std::time::Duration;
use tracing::{error, info, warn};

const QUEUE: QueueSpec = QueueSpec {
    name: "bms.deconfliction",
    bindings: &["alert.proximity", "hive.telemetry", "hive.moved", "zone.*"],
};

const AREA_RETRY_DELAY: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> ExitCode {
    bms_common::config::load_dotenv();
    // Kept until the end of main: dropping it sends the remaining spans.
    let _telemetry = bms_common::telemetry::init("bms-deconfliction");

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            error!(%error, "Invalid configuration");
            return ExitCode::FAILURE;
        },
    };
    info!(cc = %config.cc_url, saga = ?config.saga, "Service started");

    let cc = match CcClient::new(config.cc_url.clone()) {
        Ok(cc) => cc,
        Err(error) => {
            error!(%error, "Cannot create HTTP client");
            return ExitCode::FAILURE;
        },
    };

    let pool =
        match db::connect(&config.database_url, &sqlx::migrate!("./migrations")).await {
            Ok(pool) => pool,
            Err(error) => {
                error!(%error, "Database migration failed");
                return ExitCode::FAILURE;
            },
        };

    let settings = config.saga;
    tokio::select! {
        () = broker::consume(&config.rabbitmq_url, QUEUE, |envelope| {
            let pool = pool.clone();
            async move { projection::apply(&pool, settings, &envelope).await }
        }) => {},
        () = async {
            let area = operational_area(&cc).await;
            saga::run(pool.clone(), cc.clone(), area, settings).await;
        } => {},
        () = outbox::relay(pool.clone(), Publisher::new(config.rabbitmq_url.clone())) => {},
        _ = tokio::signal::ctrl_c() => info!("Shutting down"),
    }
    ExitCode::SUCCESS
}

/// The operational area, waiting until Communication Control answers.
async fn operational_area(cc: &CcClient) -> Area {
    loop {
        match cc.area().await {
            Ok(area) => {
                info!(?area, "Operational area received");
                return area;
            },
            Err(error) => {
                warn!(%error, "Communication Control unavailable, retrying");
                tokio::time::sleep(AREA_RETRY_DELAY).await;
            },
        }
    }
}
