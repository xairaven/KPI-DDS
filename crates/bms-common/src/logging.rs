use tracing_subscriber::EnvFilter;

/// Log level used when `RUST_LOG` is not set.
const DEFAULT_FILTER: &str = "info";

/// Initializes logging to stdout.
///
/// The level is taken from the `RUST_LOG` environment variable
/// (e.g. `RUST_LOG=debug` or `RUST_LOG=bms_ingest=trace`), defaulting to `info`.
pub fn init() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    tracing_subscriber::fmt().with_env_filter(filter).init();
}
