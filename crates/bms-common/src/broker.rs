//! RabbitMQ setup shared by all services.

use crate::events::EXCHANGE;
use lapin::options::ExchangeDeclareOptions;
use lapin::types::FieldTable;
use lapin::{Channel, Connection, ConnectionProperties, ExchangeKind};

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
