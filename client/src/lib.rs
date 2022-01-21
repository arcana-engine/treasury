//! Library for importing assets into treasury.

use std::{
    io::ErrorKind,
    net::Ipv4Addr,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

use eyre::WrapErr;
use tokio::{io::BufReader, net::TcpStream};
use treasury_api::{
    get_port, recv_message, send_handshake, send_message, FetchUrlResponse, FindResponse,
    OpenRequest, OpenResponse, Request, StoreResponse,
};
use url::Url;

pub use treasury_id::AssetId;

#[derive(Debug, serde::Deserialize)]
enum Treasury {
    // Remote(Url),
    Local(PathBuf),
}

#[derive(Debug)]
pub struct Client {
    #[allow(unused)]
    treasury: Treasury,
    stream: BufReader<TcpStream>,
}

impl Client {
    pub async fn local(treasury: PathBuf, init: bool) -> eyre::Result<Self> {
        let path = treasury
            .to_str()
            .ok_or_else(|| eyre::eyre!("Treasury path must not contain non UTF8 characters"))?;

        let mut stream = BufReader::new(connect_local().await?);

        send_handshake(&mut stream)
            .await
            .wrap_err("Failed to send handshake to treasury server")?;

        send_message(
            &mut stream,
            OpenRequest {
                init,
                path: path.into(),
            },
        )
        .await
        .wrap_err("Failed to send Open message to treasury server")?;

        match recv_message(&mut stream)
            .await
            .wrap_err("Failed to receive response for Open request")?
        {
            None => {
                return Err(eyre::eyre!(
                    "Failed to receive response for Open request. Connection lost."
                ));
            }
            Some(OpenResponse::Success) => {}
            Some(OpenResponse::Failure { description }) => {
                return Err(eyre::eyre!("Open request failure. {}", description));
            }
        }

        Ok(Client {
            stream,
            treasury: Treasury::Local(treasury),
        })
    }

    /// Store asset into treasury from specified URL.
    #[tracing::instrument]
    pub async fn store_asset(
        &mut self,
        source: &Url,
        format: Option<&str>,
        target: &str,
    ) -> eyre::Result<AssetId> {
        send_message(
            &mut self.stream,
            Request::Store {
                source: source.as_str().into(),
                format: format.map(|f| f.into()),
                target: target.into(),
            },
        )
        .await
        .wrap_err("Failed to send Store request")?;

        match recv_message(&mut self.stream)
            .await
            .wrap_err("Failed to receive response for Store request")?
        {
            None => Err(eyre::eyre!(
                "Failed to receive response for Store request. Connection lost."
            )),
            Some(StoreResponse::Success { id }) => {
                tracing::info!("Store requested succeeded");
                Ok(id)
            }
            Some(StoreResponse::Failure { description }) => {
                Err(eyre::eyre!("Store request failure. {}", description))
            }
            Some(StoreResponse::NeedData { url }) => Err(eyre::eyre!(
                "Treasury requires access to '{}' to finish store operation",
                url
            )),
        }
    }

    /// Store asset into treasury from specified URL.
    #[tracing::instrument]
    pub async fn fetch(&mut self, id: AssetId) -> eyre::Result<Option<Url>> {
        send_message(&mut self.stream, Request::FetchUrl { id })
            .await
            .wrap_err("Failed to send Store request")?;

        match recv_message(&mut self.stream)
            .await
            .wrap_err("Failed to receive response for Find request")?
        {
            None => Err(eyre::eyre!(
                "Failed to receive response for Find request. Connection lost."
            )),
            Some(FetchUrlResponse::Success { artifact }) => {
                tracing::info!("Find requested succeeded");
                let url = Url::parse(&artifact).wrap_err_with(|| {
                    format!(
                        "Failed to parse URL from '{}' from server response",
                        artifact
                    )
                })?;

                Ok(Some(url))
            }
            Some(FetchUrlResponse::Failure { description }) => {
                Err(eyre::eyre!("Find request failure. {}", description))
            }
            Some(FetchUrlResponse::NotFound) => Ok(None),
        }
    }

    /// Store asset into treasury from specified URL.
    #[tracing::instrument]
    pub async fn find(&mut self, source: &Url, target: &str) -> eyre::Result<Option<AssetId>> {
        send_message(
            &mut self.stream,
            Request::FindAsset {
                source: source.as_str().into(),
                target: target.into(),
            },
        )
        .await
        .wrap_err("Failed to send Store request")?;

        match recv_message(&mut self.stream)
            .await
            .wrap_err("Failed to receive response for Find request")?
        {
            None => Err(eyre::eyre!(
                "Failed to receive response for Find request. Connection lost."
            )),
            Some(FindResponse::Success { id }) => {
                tracing::info!("Find requested succeeded");

                Ok(Some(id))
            }
            Some(FindResponse::Failure { description }) => {
                Err(eyre::eyre!("Find request failure. {}", description))
            }
            Some(FindResponse::NotFound) => Ok(None),
        }
    }
}

async fn connect_local() -> eyre::Result<TcpStream> {
    let port = get_port();

    match TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
        Ok(stream) => {
            tracing::info!("Connected to running server");
            Ok(stream)
        }
        Err(err) if err.kind() == ErrorKind::ConnectionRefused => {
            tracing::info!("Failed to connect to treasury server. Run provisional instance");

            match Command::new("treasury-server")
                .env("TREASURY_PENDING_TIMEOUT", "5")
                .spawn()
            {
                Err(err) => {
                    return Err(eyre::eyre!(
                        "Failed to spawn provisional treasury server. {:#}",
                        err
                    ));
                }
                Ok(mut child) => {
                    let ten_ms = Duration::from_millis(10);
                    let second = Duration::from_secs(10);
                    let now = Instant::now();
                    let deadline = now + second;

                    while Instant::now() < deadline {
                        // Dirty, I know.
                        tokio::time::sleep(ten_ms).await;

                        match TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
                            Ok(stream) => {
                                // Not recommended for long-running processes to do so on UNIX systems.
                                drop(child);
                                return Ok(stream);
                            }
                            Err(err) if err.kind() == ErrorKind::ConnectionRefused => continue,
                            Err(err) => {
                                let _ = child.kill();
                                return Err(eyre::eyre!(
                                    "Failed to connect to provisional treasury server. {:#}",
                                    err
                                ));
                            }
                        }
                    }

                    // Failed to connect. Kill the bastard.
                    let _ = child.kill();
                    return Err(eyre::eyre!(
                        "Failed to connect to provisional treasury server before timeout"
                    ));
                }
            }
        }
        Err(err) => Err(eyre::eyre!(
            "Failed to connect to treasury server. {:#}",
            err
        )),
    }
}
