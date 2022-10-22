// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright 2022 Oxide Computer Company

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::rfb::{
    ClientInit, PixelFormat, ProtoVersion, SecurityResult, SecurityType,
    SecurityTypes, ServerInit,
};

#[derive(Error, Debug)]
pub enum InitError {
    #[error("unsupported client version {0:?}")]
    UnsupportedVersion(ProtoVersion),

    #[error("unsupported security type {0:?}")]
    UnsupportedSecurityType(SecurityType),

    #[error("protocol error {source}")]
    Protocol {
        #[from]
        source: crate::rfb::ProtoError,
    },

    #[error("IO error {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, InitError>;

pub struct InitParams {
    /// Supported protocol version
    pub version: ProtoVersion,
    /// Supported security types
    pub sec_types: SecurityTypes,

    /// Server name
    pub name: String,

    /// Initial framebuffer width
    pub width: u16,
    /// Initial framebuffer height
    pub height: u16,
    /// Initial framebuffer pixel format
    pub format: PixelFormat,
}

async fn rfb_handshake(
    s: &mut (impl AsyncRead + AsyncWrite + Unpin),
    version: ProtoVersion,
    sec_types: SecurityTypes,
) -> Result<()> {
    // ProtocolVersion handshake
    version.write_to(s).await?;

    let client_version = ProtoVersion::read_from(s).await?;
    if client_version < version {
        return Err(InitError::UnsupportedVersion(client_version));
    }

    // Security Handshake
    let supported_types = sec_types.clone();
    supported_types.write_to(s).await?;
    let client_choice = SecurityType::read_from(s).await?;
    if !sec_types.0.contains(&client_choice) {
        let failure =
            SecurityResult::Failure("unsupported security type".to_string());
        failure.write_to(s).await?;
        return Err(InitError::UnsupportedSecurityType(client_choice));
    }

    let res = SecurityResult::Success;
    res.write_to(s).await?;

    Ok(())
}

async fn rfb_initialization(
    s: &mut (impl AsyncRead + AsyncWrite + Unpin),
    width: u16,
    height: u16,
    format: PixelFormat,
    name: String,
) -> Result<ClientInit> {
    let client_init = ClientInit::read_from(s).await?;

    let server_init = ServerInit::new(width, height, name, format);
    server_init.write_to(s).await?;

    Ok(client_init)
}

/// Perform server initialization handshake with client
pub async fn initialize(
    sock: &mut (impl AsyncRead + AsyncWrite + Unpin),
    params: InitParams,
) -> Result<ClientInit> {
    assert!(
        params.sec_types.0.len() > 0,
        "at least one security type must be defined"
    );

    rfb_handshake(sock, params.version, params.sec_types).await?;
    rfb_initialization(
        sock,
        params.width,
        params.height,
        params.format,
        params.name,
    )
    .await
}
