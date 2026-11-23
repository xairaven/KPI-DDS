//! Messages published by Communication Control to Redis `HiveChannel`,
//! and their translation into BMS events.
//!
//! The lecturer's format has no type field: the message type is recognized
//! by the one field that only that message has (see `docs/hive-emulator-protocol.md`).

use bms_common::events::{
    Event, HiveMoveCommanded, HiveRef, HiveTelemetry, ReportedState,
};
use bms_common::geo::GeoPoint;
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// One Communication Control message. `untagged` tries the variants in order
/// and picks the first whose required fields are all present.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CcMessage {
    Connected(Connected),
    Reconnected(Reconnected),
    Disconnected(Disconnected),
    Telemetry(Telemetry),
    Move(Move),
    Stop(Stop),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Connected {
    #[serde(rename = "HiveID")]
    hive_id: String,
    is_successfully_connected: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Reconnected {
    #[serde(rename = "HiveID")]
    hive_id: String,
    is_successfully_reconnected: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Disconnected {
    #[serde(rename = "HiveID")]
    hive_id: String,
    is_successfully_disconnected: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Telemetry {
    telemetry: TelemetryModel,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TelemetryModel {
    #[serde(rename = "HiveID")]
    hive_id: String,
    location: Location,
    speed: f64,
    height: f64,
    state: i32,
    timestamp: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Move {
    #[serde(rename = "HiveID")]
    hive_id: String,
    destination: Location,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Stop {
    #[serde(rename = "HiveID")]
    hive_id: String,
    // Required only to recognize the message; StopTimestamp is never filled by CC.
    #[expect(dead_code, reason = "Field identifies the message type")]
    is_immediate_stop: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Location {
    latitude: f64,
    longitude: f64,
}

impl From<Location> for GeoPoint {
    fn from(location: Location) -> Self {
        Self {
            latitude: location.latitude,
            longitude: location.longitude,
        }
    }
}

/// Numeric `HiveMindState` enum of Communication Control.
const fn reported_state(value: i32) -> ReportedState {
    match value {
        1 => ReportedState::Stop,
        2 => ReportedState::Move,
        3 => ReportedState::Error,
        _ => ReportedState::Unknown,
    }
}

impl CcMessage {
    pub fn parse(payload: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(payload)
    }

    /// Translates the message into a BMS event.
    /// `None` for messages reporting a failed operation: nothing actually happened.
    pub fn into_event(self) -> Option<Event> {
        match self {
            Self::Connected(m) => m
                .is_successfully_connected
                .then_some(Event::HiveConnected(HiveRef { hive_id: m.hive_id })),
            Self::Reconnected(m) => m
                .is_successfully_reconnected
                .then_some(Event::HiveReconnected(HiveRef { hive_id: m.hive_id })),
            Self::Disconnected(m) => m
                .is_successfully_disconnected
                .then_some(Event::HiveDisconnected(HiveRef { hive_id: m.hive_id })),
            Self::Telemetry(m) => {
                let t = m.telemetry;
                Some(Event::HiveTelemetry(HiveTelemetry {
                    hive_id: t.hive_id,
                    location: t.location.into(),
                    speed: t.speed,
                    height: t.height,
                    state: reported_state(t.state),
                    reported_at: t.timestamp,
                }))
            },
            Self::Move(m) => Some(Event::HiveMoveCommanded(HiveMoveCommanded {
                hive_id: m.hive_id,
                destination: m.destination.into(),
            })),
            Self::Stop(m) => {
                Some(Event::HiveStopCommanded(HiveRef { hive_id: m.hive_id }))
            },
        }
    }
}

#[cfg(test)]
mod tests {
    //! Payloads are real messages captured from `HiveChannel`.
    use super::*;

    fn translate(payload: &str) -> Option<Event> {
        CcMessage::parse(payload).unwrap().into_event()
    }

    #[test]
    fn telemetry() {
        let event = translate(
            r#"{"IsSuccessfullySent":true,"Telemetry":{"HiveID":"1","Location":{"Latitude":48.68248,"Longitude":38.04585},"Speed":15,"Height":5,"State":2,"Timestamp":"2026-11-18T17:27:27.9135011+00:00"},"HiveID":"1"}"#,
        );

        let Some(Event::HiveTelemetry(t)) = event else {
            panic!("expected telemetry, got {event:?}");
        };
        assert_eq!(t.hive_id, "1");
        assert!((t.location.latitude - 48.68248).abs() < 1e-9);
        assert_eq!(t.state, ReportedState::Move);
    }

    #[test]
    fn move_is_not_mistaken_for_telemetry() {
        let event = translate(
            r#"{"IsSuccessfullySent":true,"Destination":{"Latitude":48.7,"Longitude":38.07},"HiveID":"1"}"#,
        );

        assert!(matches!(event, Some(Event::HiveMoveCommanded(m)) if m.hive_id == "1"));
    }

    #[test]
    fn reconnected() {
        let event = translate(
            r#"{"IsSuccessfullyReconnected":true,"Hive":{"HiveID":"2","HiveIP":"hive-2","HivePort":5149,"HiveSchema":"http","Telemetry":null},"InitialOperationalArea":{"RadiusKM":5,"InitialLocation":{"Latitude":48.69719,"Longitude":38.066246},"InitialHeight":1,"Speed":5,"TelemetryIntervalMs":30000,"PingIntervalMs":15000},"HiveID":"2"}"#,
        );

        assert!(matches!(event, Some(Event::HiveReconnected(h)) if h.hive_id == "2"));
    }

    #[test]
    fn stop() {
        let event = translate(
            r#"{"IsImmediateStop":true,"StopTimestamp":"0001-01-01T00:00:00","HiveID":"1"}"#,
        );

        assert!(matches!(event, Some(Event::HiveStopCommanded(h)) if h.hive_id == "1"));
    }

    #[test]
    fn failed_disconnect_produces_no_event() {
        assert_eq!(
            translate(r#"{"IsSuccessfullyDisconnected":false,"HiveID":"9"}"#),
            None
        );
    }

    #[test]
    fn unknown_message_is_an_error() {
        assert!(CcMessage::parse(r#"{"Something":"else"}"#).is_err());
    }
}
