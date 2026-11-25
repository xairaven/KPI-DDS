mod api;
mod app;
mod overlay;

use crate::app::App;

/// `bms-state` API used when `BMS_API_URL` is not set.
const DEFAULT_API_URL: &str = "http://localhost:9000";

fn main() -> eframe::Result {
    let api_url =
        std::env::var("BMS_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.to_owned());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("BMS")
            .with_inner_size([1280.0, 800.0]),
        ..eframe::NativeOptions::default()
    };

    eframe::run_native(
        "BMS",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc, api_url)))),
    )
}
