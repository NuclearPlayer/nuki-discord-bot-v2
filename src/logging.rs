use tracing_subscriber::EnvFilter;

/// Installs the global `fmt` subscriber with the `RUST_LOG` filter. If `RUST_LOG` is not set,
/// uses `info`.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
