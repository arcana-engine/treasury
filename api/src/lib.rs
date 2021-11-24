//! Defines API to communicate between client and server.

use std::{
    mem::{size_of, size_of_val},
    sync::atomic::{AtomicU32, Ordering},
};

use bincode::Options;
use eyre::WrapErr;
use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use treasury_id::AssetId;

/// First message that must be sent by client after connection to treasury server.
#[repr(C)]
pub struct Handshake {
    /// Magic value that must be equal to [`MAGIC`]. Otherwise server SHOULD drop the connection.
    pub magic: u32,

    /// Major version of the crate used by client. If versions used by client and server mismatch, then server SHOULD drop the connection.
    pub version: u32,
}

/// First request that must follow handshake.
/// Opens particular treasury the client is going to work with.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct OpenRequest {
    /// Path to directory that contains Treasury.toml or any in descendants.
    pub path: Box<str>,

    /// Specifies that new treasury must be init. Fails if treasury directory already contains `Treasury.toml`
    /// But succeeds in descendant directories.
    pub init: bool,
}

/// Response to the `OpenRequest`
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum OpenResponse {
    Success,

    /// Failure.
    /// Payload contains description.
    Failure {
        description: Box<str>,
    },
}

/// Requests to Treasury instance.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum Request {
    /// Stores new asset into treasury.
    Store {
        /// Url for source file.
        source: Box<str>,

        /// Source format.
        format: Option<Box<str>>,

        /// Targe format.
        target: Box<str>,
    },

    /// Fetches url of the artifact for the specified asset.
    FetchUrl { id: AssetId },

    /// Fetches url of the artifact for the specified asset.
    FindAsset { source: Box<str>, target: Box<str> },
}

/// Response to store request.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum StoreResponse {
    /// Success.
    /// Payload contains asset id.
    Success { id: AssetId },

    /// Storing process requires to read data from URL, but can't access it from treasury host.
    NeedData { url: Box<str> },

    /// Failure.
    /// Payload contains description.
    Failure { description: Box<str> },
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum FetchUrlResponse {
    /// Success.
    /// Payload contains URL of the artifact.
    Success { artifact: Box<str> },

    /// Asset not found
    NotFound,

    /// Failure response to any store request.
    Failure { description: Box<str> },
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum FindResponse {
    /// Success.
    /// Payload contains URL of the artifact.
    Success { id: AssetId },

    /// Asset not found
    NotFound,

    /// Failure response to any store request.
    Failure { description: Box<str> },
}

pub const MAGIC: u32 = u32::from_be_bytes(*b"TRES");

pub fn version() -> u32 {
    static VERSION: AtomicU32 = AtomicU32::new(u32::MAX);

    #[cold]
    fn init_version() -> u32 {
        // Initialize
        env!("CARGO_PKG_VERSION_MAJOR")
            .parse()
            .expect("Bad major version")
    }

    let mut version = VERSION.load(Ordering::Relaxed);
    if version == u32::MAX {
        version = init_version();
        VERSION.store(version, Ordering::Relaxed);
    }
    version
}

#[derive(Debug)]
#[repr(C)]
pub struct MessageHeader {
    pub size: u32,
}

pub const DEFAULT_PORT: u16 = 12345;

pub fn get_port() -> u16 {
    match std::env::var("TREASURY_SERVICE_PORT") {
        Ok(port_string) => match port_string.parse() {
            Ok(port) => port,
            Err(_) => {
                tracing::error!(
                    "Failed to parse desired treasury port from env '{}'. Using default {}",
                    port_string,
                    DEFAULT_PORT
                );
                DEFAULT_PORT
            }
        },
        Err(_) => DEFAULT_PORT,
    }
}

const INLINE_MESSAGE_LIMIT: usize = 1 << 12; // 4 KiB
const MESSAGE_LIMIT: usize = 1 << 28; // 256 MiB

pub async fn send_message<T: Serialize>(
    stream: &mut (impl AsyncWrite + Unpin),
    message: T,
) -> eyre::Result<()> {
    let size = bincode_options()
        .serialized_size(&message)
        .wrap_err("Failed to determine serialized size of the message")?;

    eyre::ensure!(size <= MESSAGE_LIMIT as u64, "Message is too large");

    let size = size as u32;
    let header = MessageHeader { size };
    tracing::debug!("Sending message header {:?}", header);

    let mut buffer = [0; INLINE_MESSAGE_LIMIT];
    if size > INLINE_MESSAGE_LIMIT as u32 {
        let mut buffer = vec![0; size_of::<MessageHeader>() + size as usize];

        buffer[..size_of::<MessageHeader>()].copy_from_slice(&header.size.to_le_bytes());

        bincode_options()
            .serialize_into(&mut buffer[size_of::<MessageHeader>()..], &message)
            .wrap_err("Failed to serialize message")?;

        stream
            .write_all(&buffer)
            .await
            .wrap_err("Failed to send message")?;

        tracing::debug!("{} bytes sent", buffer.len());
    } else {
        let buffer = &mut buffer[..size_of::<MessageHeader>() + size as usize];

        buffer[..size_of::<MessageHeader>()].copy_from_slice(&header.size.to_le_bytes());

        bincode_options()
            .serialize_into(&mut buffer[size_of::<MessageHeader>()..], &message)
            .wrap_err("Failed to serialize message")?;

        stream
            .write_all(buffer)
            .await
            .wrap_err("Failed to send message")?;

        tracing::debug!("{} bytes sent", buffer.len());
    }

    Ok(())
}

async fn next_message_header(
    stream: &mut (impl AsyncRead + Unpin),
) -> std::io::Result<Option<MessageHeader>> {
    let mut buffer = [0; size_of::<MessageHeader>()];
    match stream.read_exact(&mut buffer).await {
        Ok(_) => Ok(Some(MessageHeader {
            size: u32::from_le_bytes(buffer),
        })),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(err) => Err(err),
    }
}

pub async fn recv_message<T: DeserializeOwned>(
    stream: &mut (impl AsyncRead + Unpin),
) -> eyre::Result<Option<T>> {
    let header = match next_message_header(stream).await? {
        None => {
            tracing::debug!("Connection closed");
            return Ok(None);
        }
        Some(header) => header,
    };

    tracing::debug!("Next message header {:?}", header);

    eyre::ensure!(header.size <= MESSAGE_LIMIT as u32, "Message is too large");

    let mut buffer = [0; INLINE_MESSAGE_LIMIT];

    if header.size > INLINE_MESSAGE_LIMIT as u32 {
        let mut buffer = vec![0; header.size as usize];
        stream.read_exact(&mut buffer).await?;

        tracing::debug!(
            "{} bytes received",
            size_of::<MessageHeader>() + header.size as usize
        );

        let message = bincode_options()
            .deserialize(&buffer)
            .wrap_err("Failed to parse request")?;

        Ok(Some(message))
    } else {
        let buffer = &mut buffer[..header.size as usize];
        stream.read_exact(buffer).await?;

        tracing::debug!(
            "{} bytes received",
            size_of::<MessageHeader>() + header.size as usize
        );

        let message = bincode_options()
            .deserialize(buffer)
            .wrap_err("Failed to parse request")?;

        Ok(Some(message))
    }
}

pub async fn recv_handshake(stream: &mut (impl AsyncRead + Unpin)) -> eyre::Result<()> {
    let mut buffer = [0; size_of::<Handshake>()];

    stream
        .read_exact(&mut buffer)
        .await
        .wrap_err("Handshake failed")?;

    let handshake = Handshake {
        magic: u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]),
        version: u32::from_le_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]),
    };

    tracing::debug!(
        "Handshake received {}:{}",
        handshake.magic,
        handshake.version
    );

    eyre::ensure!(
        handshake.magic == MAGIC,
        "Wrong MAGIC number. Expected '{}', found '{}'",
        MAGIC,
        handshake.magic
    );

    let version = version();

    eyre::ensure!(
        handshake.version == version,
        "Treasury API version mismatch. Expected '{}', found '{}'",
        version,
        handshake.version,
    );

    tracing::info!("Handshake valid");

    Ok(())
}

pub async fn send_handshake(stream: &mut (impl AsyncWrite + Unpin)) -> eyre::Result<()> {
    let mut buffer = [0; size_of::<Handshake>()];

    buffer[..size_of_val(&MAGIC)].copy_from_slice(&MAGIC.to_le_bytes());
    buffer[size_of_val(&MAGIC)..].copy_from_slice(&version().to_le_bytes());

    stream
        .write_all(&buffer)
        .await
        .wrap_err("Handshake failed")?;

    tracing::debug!("Handshake sent {}:{}", MAGIC, version());

    Ok(())
}

fn bincode_options() -> impl Options {
    bincode::options()
        .with_big_endian()
        .with_fixint_encoding()
        .allow_trailing_bytes()
}
