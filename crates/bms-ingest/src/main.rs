use tracing::info;

#[tokio::main]
async fn main() {
    bms_common::logging::init();
    info!("Service started");
}
