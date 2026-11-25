//! RabbitMQ setup shared by all services.

use crate::config::{self, ConfigError};
use crate::events::{EXCHANGE, Envelope};
use futures_util::StreamExt;
use lapin::message::Delivery;
use lapin::options::{
    BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicPublishOptions,
    BasicQosOptions, BasicRejectOptions, ConfirmSelectOptions, ExchangeDeclareOptions,
    QueueBindOptions, QueueDeclareOptions,
};
use lapin::types::{AMQPValue, FieldTable};
use lapin::{
    BasicProperties, Channel, Confirmation, Connection, ConnectionProperties,
    ExchangeKind,
};
use std::fmt::Display;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, error, info, warn};

/// Delay before reconnecting after the connection to RabbitMQ was lost.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
/// Delay before returning a message that failed to be handled back to the queue.
const HANDLER_RETRY_DELAY: Duration = Duration::from_secs(1);
/// How many unacknowledged messages RabbitMQ may hand to a consumer at once.
const PREFETCH: u16 = 32;

/// Local development broker from `infra/bms` (its credentials are local-only).
const DEFAULT_URL: &str = "amqp://bms:bms@localhost:5672";

/// RabbitMQ URL from `RABBITMQ_URL`, defaulting to the local development broker.
pub fn url_from_env() -> Result<String, ConfigError> {
    config::optional("RABBITMQ_URL", DEFAULT_URL.to_owned())
}

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

/// AMQP delivery mode 2: the broker writes the message to disk.
const PERSISTENT: u8 = 2;

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("RabbitMQ error: {0}")]
    Amqp(#[from] lapin::Error),
    #[error("Cannot serialize event: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("RabbitMQ rejected the message (nack)")]
    Nack,
}

/// Lazily connected publisher. After an error the connection is dropped
/// and re-established on the next publish.
pub struct Publisher {
    url: String,
    /// Kept alive together with the channel: dropping it closes the connection.
    connection: Option<(Connection, Channel)>,
}

impl Publisher {
    pub const fn new(url: String) -> Self {
        Self {
            url,
            connection: None,
        }
    }

    /// Publishes the envelope and waits until RabbitMQ confirms it has stored it.
    pub async fn publish(&mut self, envelope: &Envelope) -> Result<(), PublishError> {
        let result = self.try_publish(envelope).await;
        if result.is_err() {
            self.connection = None;
        }
        result
    }

    async fn try_publish(&mut self, envelope: &Envelope) -> Result<(), PublishError> {
        let channel = self.channel().await?;
        let payload = serde_json::to_vec(envelope)?;
        let properties = BasicProperties::default()
            .with_delivery_mode(PERSISTENT)
            .with_content_type("application/json".into())
            .with_message_id(envelope.event_id.to_string().into());

        let confirmation = channel
            .basic_publish(
                EXCHANGE.into(),
                envelope.routing_key().into(),
                // Not mandatory: events no service subscribed to are dropped by the
                // exchange, which is normal for a topic exchange.
                BasicPublishOptions::default(),
                &payload,
                properties,
            )
            .await?
            .await?;

        match confirmation {
            Confirmation::Ack(_) | Confirmation::NotRequested => Ok(()),
            Confirmation::Nack(_) => Err(PublishError::Nack),
        }
    }

    async fn channel(&mut self) -> Result<&Channel, PublishError> {
        let connection = if let Some(existing) = self.connection.take() {
            existing
        } else {
            let connection = connect(&self.url).await?;
            let channel = connection.create_channel().await?;
            // Publisher confirms: every publish gets an ack once the broker has it.
            channel
                .confirm_select(ConfirmSelectOptions::default())
                .await?;
            declare_exchange(&channel).await?;
            info!("Connected to RabbitMQ");
            (connection, channel)
        };
        let (_, channel) = self.connection.insert(connection);
        Ok(channel)
    }
}
