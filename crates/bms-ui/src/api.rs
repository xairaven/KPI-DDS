//! Client of the `bms-state` HTTP API. Polled from a background thread, so the
//! UI never waits for the network.

// DTO fields are named exactly like the JSON of the bms-state API.
#![expect(clippy::struct_field_names, reason = "Named like the API JSON")]

use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Deserialize)]
pub struct Hive {
    pub hive_id: String,
    pub status: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub ew_zone_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Zone {
    pub zone_id: String,
    pub latitude: f64,
    pub longitude: f64,
    pub radius_km: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Alert {
    pub alert_id: i64,
    pub kind: String,
    pub hive_id: String,
    pub zone_id: Option<String>,
    pub other_hive_id: Option<String>,
    pub distance_m: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Saga {
    pub saga_id: i64,
    pub hive_id: String,
    pub other_hive_id: String,
    pub status: String,
    pub note: String,
}

/// The latest data received from the API.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub hives: Vec<Hive>,
    pub zones: Vec<Zone>,
    pub alerts: Vec<Alert>,
    pub sagas: Vec<Saga>,
    pub updated_at: Option<Instant>,
    /// Error of the latest poll, if it failed; data of the previous poll is kept.
    pub error: Option<String>,
}

/// Polls the API forever, storing results in `snapshot` and asking egui to repaint.
pub fn poll(base_url: &str, snapshot: &Mutex<Snapshot>, ctx: &egui::Context) {
    let client = match reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            store(snapshot, |s| s.error = Some(error.to_string()));
            return;
        },
    };

    loop {
        let result = fetch_all(&client, base_url);
        store(snapshot, |s| match result {
            Ok(fresh) => *s = fresh,
            Err(error) => s.error = Some(error.to_string()),
        });
        ctx.request_repaint();
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn fetch_all(
    client: &reqwest::blocking::Client, base_url: &str,
) -> reqwest::Result<Snapshot> {
    let get = |path: &str| {
        client
            .get(format!("{base_url}/api/v1/{path}"))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
    };

    Ok(Snapshot {
        hives: get("hives")?.json()?,
        zones: get("zones")?.json()?,
        alerts: get("alerts")?.json()?,
        sagas: get("sagas")?.json()?,
        updated_at: Some(Instant::now()),
        error: None,
    })
}

fn store(snapshot: &Mutex<Snapshot>, change: impl FnOnce(&mut Snapshot)) {
    // A poisoned lock only means another thread panicked while holding it;
    // the data is still usable.
    let mut guard = snapshot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    change(&mut guard);
}
