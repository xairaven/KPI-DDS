//! Subscription to Communication Control's Redis Pub/Sub channel.

use crate::cc_message::CcMessage;
use crate::config::Config;
use bms_common::events::Envelope;
use futures_util::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, info_span, warn};

/// Delay before reconnecting after the connection to Redis was lost.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Listens to the channel forever, reconnecting when Redis becomes unavailable.
/// Each recognized message is translated and sent to `events`.
pub async fn run(config: &Config, events: mpsc::Sender<Envelope>) {
    loop {
        match listen(config, &events).await {
            Ok(()) => warn!("Redis subscription ended"),
            Err(error) => error!(%error, "Redis subscription failed"),
        }
        if events.is_closed() {
            debug!("Event channel closed, Redis listener stopped");
            return;
        }
        // Pub/Sub keeps no history: everything published until we resubscribe is lost.
        warn!(delay = ?RECONNECT_DELAY, "Reconnecting to Redis");
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn listen(config: &Config, events: &mpsc::Sender<Envelope>) -> redis::RedisResult<()> {
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
            // The root span of the event's trace: everything that happens because
            // of this bus message, in any service, is attached to it.
            let span = info_span!(
                "receive",
                otel.name = %format!("receive {}", config.redis_channel),
                otel.kind = "consumer",
                messaging.system = "redis",
                event = event.routing_key(),
            );
            let envelope = span.in_scope(|| Envelope::new(event));
            if events.send(envelope).await.is_err() {
                return Ok(());
            }
        } else {
            debug!(%payload, "Message reports a failed operation, skipped");
        }
    }
    Ok(())
}
