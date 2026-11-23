//! Publishing events to the RabbitMQ `bms.events` exchange.

use bms_common::broker;
use bms_common::events::{EXCHANGE, Envelope};
use lapin::options::{BasicPublishOptions, ConfirmSelectOptions};
use lapin::{BasicProperties, Channel, Confirmation, Connection};
use thiserror::Error;
use tracing::info;

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
            let connection = broker::connect(&self.url).await?;
            let channel = connection.create_channel().await?;
            // Publisher confirms: every publish gets an ack once the broker has it.
            channel
                .confirm_select(ConfirmSelectOptions::default())
                .await?;
            broker::declare_exchange(&channel).await?;
            info!("Connected to RabbitMQ");
            (connection, channel)
        };
        let (_, channel) = self.connection.insert(connection);
        Ok(channel)
    }
}
