//! Map overlay: EW zones, hives and proximity alerts drawn over the map tiles.

use crate::api::Snapshot;
use egui::{Align2, Color32, FontId, Pos2, Response, Stroke, Ui};
use walkers::{MapMemory, Plugin, Projector, lat_lon};

pub const ZONE_FILL: Color32 = Color32::from_rgba_premultiplied(120, 0, 0, 60);
pub const ZONE_BORDER: Color32 = Color32::from_rgb(200, 0, 0);
pub const PROXIMITY: Color32 = Color32::from_rgb(255, 140, 0);

/// Color of a hive by its status.
pub fn status_color(status: &str) -> Color32 {
    match status {
        "moving" => Color32::from_rgb(0, 150, 0),
        "stopped" => Color32::from_rgb(30, 100, 220),
        "inactive" => Color32::from_gray(140),
        _ => Color32::from_gray(60),
    }
}

pub struct Overlay<'a> {
    pub snapshot: &'a Snapshot,
}

impl Plugin for Overlay<'_> {
    fn run(
        self: Box<Self>, ui: &mut Ui, _response: &Response, projector: &Projector,
        _map_memory: &MapMemory,
    ) {
        let painter = ui.painter();
        let screen = |latitude: f64, longitude: f64| -> Pos2 {
            projector.project(lat_lon(latitude, longitude)).to_pos2()
        };

        for zone in &self.snapshot.zones {
            let center = lat_lon(zone.latitude, zone.longitude);
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Screen pixels need no f64 precision"
            )]
            let radius_m = (zone.radius_km * 1000.0) as f32;
            let radius_px = projector.scale_pixel_per_meter(center) * radius_m;
            let at = screen(zone.latitude, zone.longitude);
            painter.circle(at, radius_px, ZONE_FILL, Stroke::new(1.5, ZONE_BORDER));
        }

        let position = |hive_id: &str| {
            self.snapshot
                .hives
                .iter()
                .find(|hive| hive.hive_id == hive_id)
                .and_then(|hive| Some(screen(hive.latitude?, hive.longitude?)))
        };
        for alert in self
            .snapshot
            .alerts
            .iter()
            .filter(|a| a.kind == "proximity")
        {
            let other = alert.other_hive_id.as_deref().unwrap_or_default();
            if let (Some(a), Some(b)) = (position(&alert.hive_id), position(other)) {
                painter.line_segment([a, b], Stroke::new(2.0, PROXIMITY));
            }
        }

        for hive in &self.snapshot.hives {
            let (Some(latitude), Some(longitude)) = (hive.latitude, hive.longitude)
            else {
                continue;
            };
            let at = screen(latitude, longitude);
            // A red ring marks a hive inside an EW zone.
            let ring = if hive.ew_zone_ids.is_empty() {
                Stroke::new(1.0, Color32::WHITE)
            } else {
                Stroke::new(3.0, ZONE_BORDER)
            };
            painter.circle(at, 7.0, status_color(&hive.status), ring);
            painter.text(
                at + egui::vec2(10.0, -10.0),
                Align2::LEFT_BOTTOM,
                &hive.hive_id,
                FontId::proportional(14.0),
                Color32::BLACK,
            );
        }
    }
}
