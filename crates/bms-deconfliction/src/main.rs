use bms_common::broker::{self, QueueSpec};
use bms_common::config;
use std::convert::Infallible;
use std::process::ExitCode;
use tracing::{error, info};

const QUEUE: QueueSpec = QueueSpec {
    name: "bms.deconfliction",
    bindings: &["alert.proximity", "hive.telemetry", "hive.moved", "zone.*"],
};

#[tokio::main]
async fn main() -> ExitCode {
    config::load_dotenv();
    bms_common::logging::init();

    let rabbitmq_url = match broker::url_from_env() {
        Ok(url) => url,
        Err(error) => {
            error!(%error, "Invalid configuration");
            return ExitCode::FAILURE;
        },
    };
    info!("Service started");

    tokio::select! {
        () = broker::consume(&rabbitmq_url, QUEUE, |envelope| async move {
            info!(
                event_id = %envelope.event_id,
                routing_key = envelope.routing_key(),
                event = ?envelope.event,
                "Received"
            );
            Ok::<(), Infallible>(())
        }) => {},
        _ = tokio::signal::ctrl_c() => info!("Shutting down"),
    }
    ExitCode::SUCCESS
}
