//!
//! Treasury is an easy to use asset pipeline.
//!

mod server;

#[derive(Debug, serde::Deserialize)]
pub struct Config {
    /// Seconds to wait after last connection is closed.
    /// Timeout is reset if new connection is made.
    /// Negative values are treated as infinity.
    #[serde(default = "default_pending_timeout")]
    pub pending_timeout: i32,
}

fn default_pending_timeout() -> i32 {
    -1
}

pub fn run(cfg: Config) -> eyre::Result<()> {
    tracing::info!("Starting Treasury with cfg: {:#?}", cfg);
    server::run(cfg)
}
