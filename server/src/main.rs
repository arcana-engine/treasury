use treasury_server::{run, Config};

fn main() {
    color_eyre::install().unwrap();

    use tracing_subscriber::layer::SubscriberExt as _;
    if let Err(err) = tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .finish()
            .with(tracing_error::ErrorLayer::default()),
    ) {
        panic!("Failed to install tracing subscriber: {}", err);
    }

    let cfg: Config = envy::prefixed("TREASURY_").from_env().unwrap();

    if let Err(err) = run(cfg) {
        tracing::error!("{:#?}", err);
    }
}
