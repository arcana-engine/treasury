use std::{env::current_dir, path::Path};

use clap::{crate_authors, crate_description, crate_version, App, Arg};
use treasury_client::Client;
use url::Url;

fn make_app() -> App<'static, 'static> {
    App::new("treasury")
        .version(crate_version!())
        .author(crate_authors!())
        .about(crate_description!())
        .arg(
            Arg::with_name("base")
                .long("base")
                .short("b")
                .empty_values(false)
                .value_name("DIRECTORY"),
        )
        .subcommand(
            App::new("init")
            .help("Initialize new treasury instance")
            .arg(
                Arg::with_name("importers")
                    .help("Registers importers for the new treasury")
                    .required(false)
                    .multiple(true)
                    .takes_value(true)
                )
        )
        .subcommand(
            App::new("store")
            .help("Store assets into treasury")
            .arg(
                Arg::with_name("source_is_url")
                    .help("Treat source as URL instead of file path")
                    .long_help("Specifies how to treat source argument. By default source is file path. With this flag set it is an URL")
                    .long("url")
                    .short("u")
                    .required(false)
                    .takes_value(false),
            )
            .arg(
                Arg::with_name("source")
                    .help("Source of the asset")
                    .value_name("FILE")
                    .required(true),
            )
            .arg(
                Arg::with_name("target")
                    .help("Format of the asset artifact")
                    .value_name("TARGET-FORMAT")
                    .required(true),
            )
            .arg(
                Arg::with_name("format")
                    .help("Format of the source")
                    .long_help("Format of the source. If not present, storage will try to match target and source path extension to registered importers. Importing will fail if there isn't exactly single match")
                    .required(false)
                    .value_name("SOURCE-FORMAT"),
            ),
        )
}

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

    let matches = make_app().get_matches();

    let base = match matches.value_of_os("base") {
        None => current_dir().expect("Failed to fetch current directory"),
        Some(base) => Path::new(base).to_owned(),
    };

    match matches.subcommand() {
        ("", None) => {
            make_app().print_long_help().unwrap();
        }
        ("store", Some(store)) => {
            let source = store.value_of_os("source").unwrap();
            let format = store.value_of("format");
            let target = store.value_of("target").unwrap();

            let source_is_url = store.is_present("source_is_url");

            let source = match source_is_url {
                false => {
                    let source = Path::new(source);

                    let source = match dunce::canonicalize(source) {
                        Ok(source) => source,
                        Err(err) => {
                            eprintln!(
                                "Failed to canonicalize path '{}'. {:#}",
                                source.display(),
                                err
                            );
                            return;
                        }
                    };

                    match Url::from_file_path(&source) {
                        Ok(url) => url,
                        Err(()) => {
                            eprintln!(
                                "Failed to convert source path '{:?}' to URL",
                                source.display()
                            );
                            return;
                        }
                    }
                }
                true => {
                    let source = match source.to_str() {
                        Some(source) => source,
                        None => {
                            eprintln!("Failed to parse non UTF8 url");
                            return;
                        }
                    };
                    match Url::parse(source) {
                        Ok(url) => url,
                        Err(err) => {
                            eprintln!("Failed to parse url. {:#}", err);
                            return;
                        }
                    }
                }
            };

            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to init tokio");
            let result: eyre::Result<_> = runtime.block_on(async move {
                let mut client = Client::local(base, false).await?;
                client.store_asset(&source, format, target).await
            });

            match result {
                Err(err) => {
                    eprintln!("{:#}", err);
                }
                Ok((id, path)) => {
                    println!("Successfully stored asset");
                    println!("{} @ '{}'", id, path);
                }
            }
        }
        ("init", Some(_)) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to init tokio");
            let result: eyre::Result<_> = runtime.block_on(async move {
                let _ = Client::local(base, true).await?;
                Ok(())
            });

            match result {
                Err(err) => {
                    eprintln!("{:#}", err);
                }
                Ok(()) => {
                    println!("Successfully initialized treasury");
                }
            }
        }
        _ => unreachable!(),
    }
}
