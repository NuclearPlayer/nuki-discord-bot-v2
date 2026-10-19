use tracing_subscriber::EnvFilter;

/// Installs the global `fmt` subscriber, filtered by `RUST_LOG`, or `info` when it is not set.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
