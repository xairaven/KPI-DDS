//! Internal events exchanged between BMS services through RabbitMQ.
//!
//! These are our own types, independent of the Communication Control message format.
//! `bms-ingest` translates lecturer messages into them; other services only see these.

use crate::geo::GeoPoint;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Name of the RabbitMQ topic exchange all events are published to.
pub const EXCHANGE: &str = "bms.events";

/// Wrapper around every event sent through the broker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Unique id of this event, used by consumers to detect redelivered duplicates.
    pub event_id: Uuid,
    /// When the event happened (or was observed by BMS).
    pub occurred_at: DateTime<Utc>,
    #[serde(flatten)]
    pub event: Event,
}

impl Envelope {
    pub fn new(event: Event) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            occurred_at: Utc::now(),
            event,
        }
    }

    pub const fn routing_key(&self) -> &'static str {
        self.event.routing_key()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    HiveConnected(HiveRef),
    HiveReconnected(HiveRef),
    HiveDisconnected(HiveRef),
    HiveTelemetry(HiveTelemetry),
    HiveMoveCommanded(HiveMoveCommanded),
    HiveStopCommanded(HiveRef),
    ZoneActivated(Zone),
    ZoneDeactivated(ZoneRef),
    ZoneSnapshot(ZoneSnapshot),
}

impl Event {
    /// RabbitMQ routing key: consumers bind their queues by these (e.g. `hive.*`).
    pub const fn routing_key(&self) -> &'static str {
        match self {
            Self::HiveConnected(_) => "hive.connected",
            Self::HiveReconnected(_) => "hive.reconnected",
            Self::HiveDisconnected(_) => "hive.disconnected",
            Self::HiveTelemetry(_) => "hive.telemetry",
            Self::HiveMoveCommanded(_) => "hive.moved",
            Self::HiveStopCommanded(_) => "hive.stopped",
            Self::ZoneActivated(_) => "zone.activated",
            Self::ZoneDeactivated(_) => "zone.deactivated",
            Self::ZoneSnapshot(_) => "zone.snapshot",
        }
    }
}

pub type HiveId = String;
pub type ZoneId = Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HiveRef {
    pub hive_id: HiveId,
}

/// State reported by the hive itself. Not fully reliable: an idle hive may report `Move`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportedState {
    Stop,
    Move,
    Error,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HiveTelemetry {
    pub hive_id: HiveId,
    pub location: GeoPoint,
    pub speed: f64,
    pub height: f64,
    pub state: ReportedState,
    /// Timestamp assigned by Communication Control when it accepted the report.
    pub reported_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HiveMoveCommanded {
    pub hive_id: HiveId,
    pub destination: GeoPoint,
}

/// An EW (electronic warfare) zone: a circle where radio links are jammed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Zone {
    pub zone_id: ZoneId,
    pub center: GeoPoint,
    pub radius_km: f64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneRef {
    pub zone_id: ZoneId,
}

/// Full list of zones active right now; consumers reconcile their state against it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZoneSnapshot {
    pub zones: Vec<Zone>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_json_has_explicit_type() {
        let envelope = Envelope::new(Event::HiveDisconnected(HiveRef {
            hive_id: "1".to_owned(),
        }));

        let json: serde_json::Value = serde_json::to_value(&envelope).unwrap();

        assert_eq!(json["type"], "hive_disconnected");
        assert_eq!(json["data"]["hive_id"], "1");
        assert!(json["event_id"].is_string());
    }

    #[test]
    fn envelope_round_trip() {
        let envelope = Envelope::new(Event::HiveTelemetry(HiveTelemetry {
            hive_id: "2".to_owned(),
            location: GeoPoint {
                latitude: 48.68,
                longitude: 38.04,
            },
            speed: 15.0,
            height: 5.0,
            state: ReportedState::Move,
            reported_at: Utc::now(),
        }));

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: Envelope = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed, envelope);
        assert_eq!(parsed.routing_key(), "hive.telemetry");
    }
}
