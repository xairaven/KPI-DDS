//! EW zones are never announced on the Redis bus, so they are polled from
//! Communication Control's client API and turned into events by comparing lists.

use crate::cc_message::CcLocation;
use crate::config::Config;
use bms_common::events::{Envelope, Event, Zone, ZoneId, ZoneRef, ZoneSnapshot};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, info_span, warn};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Zone as returned by `GET /api/v1/client/interferences`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CcInterference {
    id: ZoneId,
    #[serde(rename = "RadiusKM")]
    radius_km: f64,
    location: CcLocation,
    created_at: DateTime<Utc>,
}

impl From<CcInterference> for Zone {
    fn from(zone: CcInterference) -> Self {
        Self {
            zone_id: zone.id,
            center: zone.location.into(),
            radius_km: zone.radius_km,
            created_at: zone.created_at,
        }
    }
}

type Zones = BTreeMap<ZoneId, Zone>;

/// Polls zones forever. The first successful poll is sent as a snapshot,
/// every later one as the differences to the previous poll.
pub async fn run(config: &Config, events: mpsc::Sender<Envelope>) {
    let url = format!("{}/api/v1/client/interferences", config.cc_url);
    let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            warn!(%error, "Cannot create HTTP client, zone polling disabled");
            return;
        },
    };

    let mut ticker = tokio::time::interval(config.zone_poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut known: Option<Zones> = None;

    loop {
        ticker.tick().await;

        let current = match fetch(&client, &url).await {
            Ok(zones) => zones,
            Err(error) => {
                // No answer is not "no zones": skip the round, keep the known list.
                warn!(%error, "Polling zones failed, round skipped");
                continue;
            },
        };

        let new_events = known
            .as_ref()
            .map_or_else(|| snapshot(&current), |previous| diff(previous, &current));
        known = Some(current);

        if new_events.is_empty() {
            continue;
        }
        // A span only for rounds that found changes: an unchanged poll every
        // few seconds would flood the traces with empty ones.
        let span = info_span!("zones changed", count = new_events.len());
        let envelopes: Vec<Envelope> = span.in_scope(|| {
            new_events.into_iter().map(Envelope::new).collect()
        });
        for envelope in envelopes {
            if events.send(envelope).await.is_err() {
                debug!("Event channel closed, zone polling stopped");
                return;
            }
        }
    }
}

async fn fetch(client: &reqwest::Client, url: &str) -> reqwest::Result<Zones> {
    let zones: Vec<CcInterference> = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(zones
        .into_iter()
        .map(|zone| (zone.id, Zone::from(zone)))
        .collect())
}

/// Full list of zones, sent once after startup.
fn snapshot(current: &Zones) -> Vec<Event> {
    info!(count = current.len(), "Initial zone snapshot");
    vec![Event::ZoneSnapshot(ZoneSnapshot {
        zones: current.values().cloned().collect(),
    })]
}

/// Events describing how `current` differs from `previous`.
fn diff(previous: &Zones, current: &Zones) -> Vec<Event> {
    let activated = current
        .iter()
        .filter(|(id, _)| !previous.contains_key(id))
        .map(|(_, zone)| Event::ZoneActivated(zone.clone()));

    let deactivated = previous
        .keys()
        .filter(|id| !current.contains_key(id))
        .map(|&zone_id| Event::ZoneDeactivated(ZoneRef { zone_id }));

    activated.chain(deactivated).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bms_common::geo::GeoPoint;
    use uuid::Uuid;

    fn zone(id: u128) -> Zone {
        Zone {
            zone_id: Uuid::from_u128(id),
            center: GeoPoint {
                latitude: 48.7,
                longitude: 38.07,
            },
            radius_km: 1.5,
            created_at: Utc::now(),
        }
    }

    fn zones(list: &[Zone]) -> Zones {
        list.iter().map(|z| (z.zone_id, z.clone())).collect()
    }

    #[test]
    fn no_changes_no_events() {
        let a = zone(1);
        let same = zones(&[a]);
        assert_eq!(diff(&same, &same), vec![]);
    }

    #[test]
    fn added_and_removed_zones() {
        let (a, b, c) = (zone(1), zone(2), zone(3));

        let events = diff(&zones(&[a.clone(), b.clone()]), &zones(&[b, c.clone()]));

        assert_eq!(
            events,
            vec![
                Event::ZoneActivated(c),
                Event::ZoneDeactivated(ZoneRef { zone_id: a.zone_id }),
            ]
        );
    }

    #[test]
    fn parses_communication_control_response() {
        let json = r#"[{"Id":"034682da-24df-493d-8d28-f1c2528f34da","RadiusKM":1.5,"Location":{"Latitude":48.7,"Longitude":38.07},"CreatedAt":"2026-11-17T12:32:47.2925525Z"}]"#;

        let parsed: Vec<CcInterference> = serde_json::from_str(json).unwrap();
        let zone = Zone::from(parsed.into_iter().next().unwrap());

        assert_eq!(
            zone.zone_id.to_string(),
            "034682da-24df-493d-8d28-f1c2528f34da"
        );
        assert!((zone.radius_km - 1.5).abs() < f64::EPSILON);
    }
}
