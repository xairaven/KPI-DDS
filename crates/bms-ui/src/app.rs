use crate::api::{self, Snapshot};
use crate::overlay::{self, Overlay};
use egui::{Color32, RichText, ScrollArea};
use std::sync::{Arc, Mutex};
use walkers::sources::OpenStreetMap;
use walkers::{HttpTiles, Map, MapMemory, Position, lat_lon};

/// Center of the operational area of the lecturer's system.
const START_POSITION: (f64, f64) = (48.697_19, 38.066_246);
const START_ZOOM: f64 = 13.0;

pub struct App {
    tiles: HttpTiles,
    map_memory: MapMemory,
    snapshot: Arc<Mutex<Snapshot>>,
    api_url: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, api_url: String) -> Self {
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));

        let poller_snapshot = Arc::clone(&snapshot);
        let poller_url = api_url.clone();
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || api::poll(&poller_url, &poller_snapshot, &ctx));

        let mut map_memory = MapMemory::default();
        // A valid zoom level, so this cannot fail.
        map_memory.set_zoom(START_ZOOM).ok();

        Self {
            tiles: HttpTiles::new(OpenStreetMap, cc.egui_ctx.clone()),
            map_memory,
            snapshot,
            api_url,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Copy, so the poller thread is never blocked by drawing.
        let snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        egui::Panel::top("status")
            .show(ui, |ui| status_bar(ui, &snapshot, &self.api_url));

        egui::Panel::left("lists")
            .default_size(360.0)
            .show(ui, |ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    hives(ui, &snapshot);
                    ui.separator();
                    zones(ui, &snapshot);
                    ui.separator();
                    alerts(ui, &snapshot);
                    ui.separator();
                    sagas(ui, &snapshot);
                });
            });

        egui::CentralPanel::no_frame().show(ui, |ui| {
            let start: Position = lat_lon(START_POSITION.0, START_POSITION.1);
            ui.add(
                Map::new(Some(&mut self.tiles), &mut self.map_memory, start).with_plugin(
                    Overlay {
                        snapshot: &snapshot,
                    },
                ),
            );
        });
    }
}

fn status_bar(ui: &mut egui::Ui, snapshot: &Snapshot, api_url: &str) {
    ui.horizontal(|ui| {
        ui.heading("Battlefield Management System");
        ui.separator();
        ui.label(format!("API: {api_url}"));
        match (&snapshot.error, snapshot.updated_at) {
            (Some(error), _) => {
                ui.colored_label(Color32::RED, format!("Unavailable: {error}"));
            },
            (None, Some(updated)) => {
                ui.label(format!(
                    "Updated {:.0} s ago",
                    updated.elapsed().as_secs_f64()
                ));
            },
            (None, None) => {
                ui.label("Connecting…");
            },
        }
    });
}

fn hives(ui: &mut egui::Ui, snapshot: &Snapshot) {
    ui.heading(format!("Hives ({})", snapshot.hives.len()));
    egui::Grid::new("hives").striped(true).show(ui, |ui| {
        ui.strong("Hive");
        ui.strong("Status");
        ui.strong("Position");
        ui.strong("EW zone");
        ui.end_row();

        for hive in &snapshot.hives {
            ui.label(&hive.hive_id);
            ui.label(
                RichText::new(&hive.status).color(overlay::status_color(&hive.status)),
            );
            match (hive.latitude, hive.longitude) {
                (Some(latitude), Some(longitude)) => {
                    ui.label(format!("{latitude:.5}, {longitude:.5}"))
                },
                _ => ui.label("unknown"),
            };
            if hive.ew_zone_ids.is_empty() {
                ui.label("—");
            } else {
                ui.colored_label(overlay::ZONE_BORDER, "inside");
            }
            ui.end_row();
        }
    });
}

fn zones(ui: &mut egui::Ui, snapshot: &Snapshot) {
    ui.heading(format!("Active EW zones ({})", snapshot.zones.len()));
    for zone in &snapshot.zones {
        ui.label(format!(
            "{}  ({:.4}, {:.4}), r = {} km",
            short(&zone.zone_id),
            zone.latitude,
            zone.longitude,
            zone.radius_km
        ));
    }
}

fn alerts(ui: &mut egui::Ui, snapshot: &Snapshot) {
    ui.heading(format!("Open alerts ({})", snapshot.alerts.len()));
    for alert in &snapshot.alerts {
        let text = match alert.kind.as_str() {
            "proximity" => format!(
                "#{} Hives {} and {} too close: {:.0} m",
                alert.alert_id,
                alert.hive_id,
                alert.other_hive_id.as_deref().unwrap_or("?"),
                alert.distance_m.unwrap_or_default()
            ),
            _ => format!(
                "#{} Hive {} in EW zone {}",
                alert.alert_id,
                alert.hive_id,
                short(alert.zone_id.as_deref().unwrap_or("?"))
            ),
        };
        let color = if alert.kind == "proximity" {
            overlay::PROXIMITY
        } else {
            overlay::ZONE_BORDER
        };
        ui.colored_label(color, text);
    }
}

fn sagas(ui: &mut egui::Ui, snapshot: &Snapshot) {
    ui.heading("Deconfliction sagas");
    for saga in &snapshot.sagas {
        let color = match saga.status.as_str() {
            "completed" => Color32::from_rgb(0, 150, 0),
            "failed" => Color32::RED,
            "compensating" => overlay::PROXIMITY,
            _ => ui.visuals().text_color(),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "#{} ({}, {})",
                saga.saga_id, saga.hive_id, saga.other_hive_id
            ));
            ui.colored_label(color, &saga.status);
            ui.weak(&saga.note);
        });
    }
}

/// First 8 characters of an id.
fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}
