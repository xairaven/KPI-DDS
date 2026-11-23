mod cc_message;
mod config;
mod publisher;
mod redis_listener;
mod zone_poller;

use crate::config::Config;
use crate::publisher::Publisher;
use bms_common::events::{Envelope, Event};
use std::process::ExitCode;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// Events waiting to be published; producers wait when it is full (backpressure).
const EVENT_BUFFER: usize = 1024;
/// Delay between attempts to publish an event while RabbitMQ is unavailable.
const PUBLISH_RETRY_DELAY: Duration = Duration::from_secs(2);

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
    // The RabbitMQ URL contains a password, so the config is not logged as a whole.
    info!(
        redis = %config.redis_url,
        channel = %config.redis_channel,
        cc = %config.cc_url,
        zone_poll_interval = ?config.zone_poll_interval,
        "Service started"
    );

    // Two producers (Redis bus, zone polling) feed one consumer.
    let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);

    tokio::select! {
        () = redis_listener::run(&config, events_tx.clone()) => {},
        () = zone_poller::run(&config, events_tx) => {},
        () = publish_events(Publisher::new(config.rabbitmq_url.clone()), events_rx) => {},
        _ = tokio::signal::ctrl_c() => info!("Shutting down"),
    }
    ExitCode::SUCCESS
}

async fn publish_events(mut publisher: Publisher, mut events: mpsc::Receiver<Event>) {
    while let Some(event) = events.recv().await {
        // The envelope (and its event_id) is created once, so retries publish the
        // same event and consumers can recognize a duplicate.
        let envelope = Envelope::new(event);

        while let Err(error) = publisher.publish(&envelope).await {
            error!(%error, routing_key = envelope.routing_key(), "Publishing failed, retrying");
            tokio::time::sleep(PUBLISH_RETRY_DELAY).await;
        }
        debug!(routing_key = envelope.routing_key(), event_id = %envelope.event_id, "Published");
    }
}
