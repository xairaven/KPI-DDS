//! Subscription to Communication Control's Redis Pub/Sub channel.

use crate::cc_message::CcMessage;
use crate::config::Config;
use bms_common::events::Event;
use futures_util::StreamExt;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Delay before reconnecting after the connection to Redis was lost.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Listens to the channel forever, reconnecting when Redis becomes unavailable.
/// Each recognized message is translated and passed to `on_event`.
pub async fn run(config: &Config, mut on_event: impl FnMut(Event)) {
    loop {
        match listen(config, &mut on_event).await {
            Ok(()) => warn!("Redis subscription ended"),
            Err(error) => error!(%error, "Redis subscription failed"),
        }
        // Pub/Sub keeps no history: everything published until we resubscribe is lost.
        warn!(delay = ?RECONNECT_DELAY, "Reconnecting to Redis");
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn listen(
    config: &Config, on_event: &mut impl FnMut(Event),
) -> redis::RedisResult<()> {
    let client = redis::Client::open(config.redis_url.as_str())?;
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.subscribe(&config.redis_channel).await?;
    info!(channel = %config.redis_channel, "Subscribed to Redis");

    let mut messages = pubsub.on_message();
    while let Some(message) = messages.next().await {
        let payload: String = match message.get_payload() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(%error, "Skipping non-text Redis message");
                continue;
            },
        };

        let message = match CcMessage::parse(&payload) {
            Ok(message) => message,
            Err(error) => {
                warn!(%error, %payload, "Unrecognized message, skipped");
                continue;
            },
        };

        if let Some(event) = message.into_event() {
            on_event(event);
        } else {
            debug!(%payload, "Message reports a failed operation, skipped");
        }
    }
    Ok(())
}
