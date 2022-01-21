use std::{
    collections::{hash_map::Entry, HashMap},
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};

use eyre::Context;
use futures_util::{stream::FuturesUnordered, StreamExt};
use parking_lot::Mutex;
use tokio::{
    io::BufReader,
    net::{TcpListener, TcpStream},
    pin,
    time::sleep,
};
use treasury_api::{
    get_port, recv_handshake, recv_message, send_message, FetchUrlResponse, FindResponse,
    OpenRequest, OpenResponse, Request, StoreResponse,
};
use treasury_store::Treasury;
use url::Url;

use crate::Config;

pub fn run(cfg: Config) -> eyre::Result<()> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();

    let runtime = builder.build()?;
    runtime.block_on(async move {
        let port = get_port();

        let mut listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?;
        let local_addr = listener.local_addr()?;

        if port == 0 {
            // If no particular port was requested - output assigned port to stdout
            let port = local_addr.port();
            println!("{}", port);
        }

        let treasuries = Mutex::new(HashMap::new());
        let mut tasks = FuturesUnordered::new();

        // let (stream, addr) = listener.accept().await?;
        // tasks.push(serve(&treasuries, stream, addr));

        tracing::debug!("Ready to serve at {}", local_addr);

        'outer: loop {
            tokio::select! {
                _ = tasks.next() => {}
                result = listener.accept() => match result {
                    Ok((stream, addr)) => {
                        tasks.push(serve(&treasuries, stream, addr));
                    }
                    Err(err) => {
                        tracing::error!("TcpListener failed: {:#?}", err);

                        match TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).await {
                            Ok(l) => listener = l,
                            Err(err) => {
                                tracing::error!("Failed to rebind TcpListener. {:#}", err);
                                break;
                            }
                        }
                    }
                }
            }

            if cfg.pending_timeout >= 0 && tasks.is_empty() {
                let timeout = sleep(Duration::from_secs(cfg.pending_timeout as u64));
                pin!(timeout);

                loop {
                    tokio::select! {
                        _ = timeout.as_mut() => {
                            break 'outer;
                        }
                        result = listener.accept() => match result {
                            Ok((stream, addr)) => {
                                tasks.push(serve(&treasuries, stream, addr));
                                continue 'outer;
                            }
                            Err(err) => {
                                tracing::error!("TcpListener failed: {:#?}", err);

                                match TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).await {
                                    Ok(l) => listener = l,
                                    Err(err) => {
                                        tracing::error!("Failed to rebind TcpListener. {:#}", err);
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                };
            }
        }

        Ok(tasks.collect().await)
    })
}

#[tracing::instrument(skip(treasuries))]
async fn serve(
    treasuries: &Mutex<HashMap<Box<str>, Arc<Treasury>>>,
    stream: TcpStream,
    addr: SocketAddr,
) {
    tracing::info!("Serving for '{}'", addr);

    match try_serve(treasuries, stream).await {
        Err(err) => tracing::error!("Error occurred while serving '{}'. '{:#}'", addr, err),
        Ok(()) => tracing::info!("Client '{}' disconnected", addr),
    }
}

async fn try_serve(
    treasuries: &Mutex<HashMap<Box<str>, Arc<Treasury>>>,
    stream: TcpStream,
) -> eyre::Result<()> {
    let mut stream = BufReader::new(stream);
    recv_handshake(&mut stream).await?;

    let open: OpenRequest = match recv_message(&mut stream).await {
        Err(err) => {
            send_message(
                &mut stream,
                OpenResponse::Failure {
                    description: format!("{:#}", err).into_boxed_str(),
                },
            )
            .await?;

            return Err(err);
        }
        Ok(None) => return Err(eyre::eyre!("Client didn't send Open message")),
        Ok(Some(open)) => open,
    };

    tracing::debug!("Got open request {:?}", open);

    let treasury = match treasuries.lock().entry(open.path.clone()) {
        Entry::Occupied(entry) => {
            if open.init {
                send_message(
                    &mut stream,
                    OpenResponse::Failure {
                        description: "Could not init treasury where it is already open"
                            .to_owned()
                            .into_boxed_str(),
                    },
                )
                .await?;

                return Err(eyre::eyre!(
                    "Could not init treasury where it is already open"
                ));
            } else {
                send_message(&mut stream, OpenResponse::Success).await?;
                entry.get().clone()
            }
        }
        Entry::Vacant(entry) => {
            let path = Path::new(&*open.path);
            let result = if open.init {
                Treasury::init_in(path, None, None, None, &[])
                    .wrap_err("Failed to initialize treasury")
            } else {
                Treasury::find_from(path).wrap_err("Failed to find treasury")
            };

            match result {
                Err(err) => {
                    send_message(
                        &mut stream,
                        OpenResponse::Failure {
                            description: format!("{:#}", err).into_boxed_str(),
                        },
                    )
                    .await?;

                    return Err(err);
                }
                Ok(treasury) => {
                    send_message(&mut stream, OpenResponse::Success).await?;
                    entry.insert(Arc::new(treasury)).clone()
                }
            }
        }
    };

    let treasury = &*treasury;

    loop {
        match recv_message(&mut stream)
            .await
            .wrap_err("Failed to read next request")?
        {
            None => return Ok(()),
            Some(Request::Store {
                source,
                format,
                target,
            }) => match treasury.store(&source, format.as_deref(), &target).await {
                Ok(id) => send_message(&mut stream, StoreResponse::Success { id }).await?,
                Err(err) => {
                    send_message(
                        &mut stream,
                        StoreResponse::Failure {
                            description: format!("{:#}", err).into_boxed_str(),
                        },
                    )
                    .await?
                }
            },
            Some(Request::FetchUrl { id }) => match treasury.fetch(id) {
                None => send_message(&mut stream, FetchUrlResponse::NotFound).await?,
                Some(path) => match Url::from_file_path(&path) {
                    Ok(url) => {
                        send_message(
                            &mut stream,
                            FetchUrlResponse::Success {
                                artifact: url.to_string().into_boxed_str(),
                            },
                        )
                        .await?
                    }
                    Err(()) => {
                        send_message(
                            &mut stream,
                            FetchUrlResponse::Failure {
                                description: format!(
                                    "Failed to convert path '{}' to URL",
                                    path.display(),
                                )
                                .into_boxed_str(),
                            },
                        )
                        .await?
                    }
                },
            },
            Some(Request::FindAsset { source, target }) => {
                match treasury.find_asset(&source, &target).await {
                    Err(err) => {
                        send_message(
                            &mut stream,
                            FindResponse::Failure {
                                description: format!("Failed to fetch asset. {:#}", err)
                                    .into_boxed_str(),
                            },
                        )
                        .await?
                    }
                    Ok(None) => send_message(&mut stream, FindResponse::NotFound).await?,
                    Ok(Some(id)) => send_message(&mut stream, FindResponse::Success { id }).await?,
                }
            }
        }
    }
}
