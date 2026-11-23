//! RabbitMQ setup shared by all services.

use crate::events::{EXCHANGE, Envelope};
use futures_util::StreamExt;
use lapin::message::Delivery;
use lapin::options::{
    BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicQosOptions,
    BasicRejectOptions, ExchangeDeclareOptions, QueueBindOptions, QueueDeclareOptions,
};
use lapin::types::{AMQPValue, FieldTable};
use lapin::{Channel, Connection, ConnectionProperties, ExchangeKind};
use std::fmt::Display;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Delay before reconnecting after the connection to RabbitMQ was lost.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
/// Delay before returning a message that failed to be handled back to the queue.
const HANDLER_RETRY_DELAY: Duration = Duration::from_secs(1);
/// How many unacknowledged messages RabbitMQ may hand to a consumer at once.
const PREFETCH: u16 = 32;

/// Connects to RabbitMQ, e.g. `amqp://user:password@localhost:5672`.
pub async fn connect(url: &str) -> lapin::Result<Connection> {
    Connection::connect(url, ConnectionProperties::default()).await
}

/// Declares the `bms.events` topic exchange.
///
/// Declaring is idempotent ("create if missing"), so every service can do it
/// on startup without caring who was first. Durable: survives broker restarts.
pub async fn declare_exchange(channel: &Channel) -> lapin::Result<()> {
    channel
        .exchange_declare(
            EXCHANGE.into(),
            ExchangeKind::Topic,
            ExchangeDeclareOptions {
                durable: true,
                ..ExchangeDeclareOptions::default()
            },
            FieldTable::default(),
        )
        .await
}

/// A service's own queue and the routing keys it subscribes to.
#[derive(Debug, Clone, Copy)]
pub struct QueueSpec {
    pub name: &'static str,
    /// Topic patterns: `*` matches one word, `#` zero or more words.
    pub bindings: &'static [&'static str],
}

/// Consumes the queue forever, reconnecting when RabbitMQ becomes unavailable.
///
/// A message is acknowledged only after `handle` succeeded. If the service dies
/// before that, RabbitMQ delivers the message again (at-least-once), so handlers
/// must be idempotent.
pub async fn consume<F, Fut, E>(url: &str, queue: QueueSpec, mut handle: F)
where
    F: FnMut(Envelope) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Display,
{
    loop {
        match consume_once(url, queue, &mut handle).await {
            Ok(()) => warn!(queue = queue.name, "Consumer stopped"),
            Err(error) => error!(%error, queue = queue.name, "Consuming failed"),
        }
        warn!(delay = ?RECONNECT_DELAY, "Reconnecting to RabbitMQ");
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn consume_once<F, Fut, E>(
    url: &str, queue: QueueSpec, handle: &mut F,
) -> lapin::Result<()>
where
    F: FnMut(Envelope) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Display,
{
    // Kept in scope: dropping the connection would close the channel.
    let connection = connect(url).await?;
    let channel = connection.create_channel().await?;
    channel
        .basic_qos(PREFETCH, BasicQosOptions::default())
        .await?;
    declare_exchange(&channel).await?;
    declare_queue(&channel, queue).await?;

    let mut deliveries = channel
        .basic_consume(
            queue.name.into(),
            "".into(),
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await?;
    info!(queue = queue.name, bindings = ?queue.bindings, "Consuming");

    while let Some(delivery) = deliveries.next().await {
        process(delivery?, handle).await?;
    }
    Ok(())
}

/// Declares the durable queue and binds it to the exchange.
async fn declare_queue(channel: &Channel, queue: QueueSpec) -> lapin::Result<()> {
    // Quorum queue: replicated, durable queue type recommended by RabbitMQ 4.
    // It also counts redeliveries and drops a message that keeps failing.
    let mut arguments = FieldTable::default();
    arguments.insert(
        "x-queue-type".into(),
        AMQPValue::LongString("quorum".into()),
    );

    channel
        .queue_declare(
            queue.name.into(),
            QueueDeclareOptions {
                durable: true,
                ..QueueDeclareOptions::default()
            },
            arguments,
        )
        .await?;

    for routing_key in queue.bindings {
        channel
            .queue_bind(
                queue.name.into(),
                EXCHANGE.into(),
                (*routing_key).into(),
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .await?;
    }
    Ok(())
}

async fn process<F, Fut, E>(delivery: Delivery, handle: &mut F) -> lapin::Result<()>
where
    F: FnMut(Envelope) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Display,
{
    let envelope: Envelope = match serde_json::from_slice(&delivery.data) {
        Ok(envelope) => envelope,
        Err(error) => {
            // Will never parse, so retrying is pointless.
            error!(%error, routing_key = %delivery.routing_key, "Malformed message, discarded");
            delivery
                .reject(BasicRejectOptions { requeue: false })
                .await?;
            return Ok(());
        },
    };

    let event_id = envelope.event_id;
    match handle(envelope).await {
        Ok(()) => {
            delivery.ack(BasicAckOptions::default()).await?;
            debug!(%event_id, "Acknowledged");
        },
        Err(error) => {
            error!(%error, %event_id, "Handling failed, message returned to the queue");
            tokio::time::sleep(HANDLER_RETRY_DELAY).await;
            delivery
                .nack(BasicNackOptions {
                    requeue: true,
                    ..BasicNackOptions::default()
                })
                .await?;
        },
    }
    Ok(())
}
