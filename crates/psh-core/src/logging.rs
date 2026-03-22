use tracing_subscriber::EnvFilter;

/// Initialize tracing with env filter. Defaults to `info` level.
/// Override with `PSH_LOG` env var (e.g. `PSH_LOG=debug`).
pub fn init(component: &str) {
    let filter = EnvFilter::try_from_env("PSH_LOG")
        .unwrap_or_else(|_| EnvFilter::new(format!("warn,{component}=info")));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init()
        .ok();
}
