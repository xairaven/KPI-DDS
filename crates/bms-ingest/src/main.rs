mod cc_message;
mod config;
mod redis_listener;
mod zone_poller;

use crate::config::Config;
use bms_common::events::Event;
use std::process::ExitCode;
use tokio::sync::mpsc;
use tracing::{error, info};

/// Events waiting to be handled; producers wait when it is full (backpressure).
const EVENT_BUFFER: usize = 1024;

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

    // Two producers (Redis bus, zone polling) feed one consumer.
    let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);

    tokio::select! {
        () = redis_listener::run(&config, events_tx.clone()) => {},
        () = zone_poller::run(&config, events_tx) => {},
        () = handle_events(events_rx) => {},
        _ = tokio::signal::ctrl_c() => info!("Shutting down"),
    }
    ExitCode::SUCCESS
}

async fn handle_events(mut events: mpsc::Receiver<Event>) {
    while let Some(event) = events.recv().await {
        info!(routing_key = event.routing_key(), ?event, "Event");
    }
}
