// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright 2022 Oxide Computer Company

use anyhow::{bail, Result};
use slog::{debug, error, info, Logger};
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;

use crate::rfb::ClientMessage::{
    ClientCutText, FramebufferUpdateRequest, KeyEvent, PointerEvent,
    SetEncodings, SetPixelFormat,
};
use crate::rfb::{
    ClientInit, ClientMessage, FramebufferUpdate, KeyEvent, PixelFormat,
    ProtoVersion, SecurityResult, SecurityType, SecurityTypes, ServerInit,
};

/// Mutable state
struct ServerState {
    width: u16,
    height: u16,

    /// The pixel format of the framebuffer data passed in to the server via
    /// get_framebuffer_update.
    input_format: PixelFormat,

    /// The pixel format of the framebuffer data passed in to the server via
    /// get_framebuffer_update.
    output_format: PixelFormat,
}

pub struct Server {
    state: Mutex<ServerState>,
}

impl Server {
    pub fn new(width: u16, height: u16, input_format: PixelFormat) -> Self {
        Self {
            state: Mutex::new(ServerState {
                width,
                height,
                output_format: input_format.clone(),
                input_format,
            }),
        }
    }

    pub async fn set_pixel_format(&self, pixel_format: PixelFormat) {
        let mut state = self.state.lock().await;
        state.input_format = pixel_format;
    }

    pub async fn set_resolution(&self, width: u16, height: u16) {
        let mut state = self.state.lock().await;
        state.width = width;
        state.height = height;
    }

    pub async fn initialize(
        &self,
        s: &mut (impl AsyncRead + AsyncWrite + Unpin),
        log: &Logger,
        version: ProtoVersion,
        sec_types: SecurityTypes,
        name: String,
    ) -> Result<()> {
        assert!(
            sec_types.0.len() > 0,
            "at least one security type must be defined"
        );

        self.rfb_handshake(s, log, version, sec_types).await?;
        self.rfb_initialization(s, log, name).await
    }

    async fn rfb_handshake(
        &self,
        s: &mut (impl AsyncRead + AsyncWrite + Unpin),
        log: &Logger,
        version: ProtoVersion,
        sec_types: SecurityTypes,
    ) -> Result<()> {
        // ProtocolVersion handshake
        info!(log, "Tx: ProtoVersion={:?}", version);
        version.write_to(s).await?;
        let client_version = ProtoVersion::read_from(s).await?;
        info!(log, "Rx: ClientVersion={:?}", client_version);

        if client_version < version {
            let err_str = format!(
                "unsupported client version={:?} (server version: {:?})",
                client_version, version
            );
            error!(log, "{}", err_str);
            bail!(err_str);
        }

        // Security Handshake
        let supported_types = sec_types.clone();
        info!(log, "Tx: SecurityTypes={:?}", supported_types);
        supported_types.write_to(s).await?;
        let client_choice = SecurityType::read_from(s).await?;
        info!(log, "Rx: SecurityType Choice={:?}", client_choice);
        if !sec_types.0.contains(&client_choice) {
            info!(log, "Tx: SecurityResult=Failure");
            let failure = SecurityResult::Failure(
                "unsupported security type".to_string(),
            );
            failure.write_to(s).await?;
            let err_str =
                format!("invalid security choice={:?}", client_choice);
            error!(log, "{}", err_str);
            bail!(err_str);
        }

        let res = SecurityResult::Success;
        info!(log, "Tx: SecurityResult=Success");
        res.write_to(s).await?;

        Ok(())
    }

    async fn rfb_initialization(
        &self,
        s: &mut (impl AsyncRead + AsyncWrite + Unpin),
        log: &Logger,
        name: String,
    ) -> Result<()> {
        let client_init = ClientInit::read_from(s).await?;
        info!(log, "Rx: ClientInit={:?}", client_init);
        // TODO: decide what to do in exclusive case
        match client_init.shared {
            true => {}
            false => {}
        }

        let data = self.state.lock().await;
        let server_init = ServerInit::new(
            data.width,
            data.height,
            name,
            data.input_format.clone(),
        );
        info!(log, "Tx: ServerInit={:#?}", server_init);
        server_init.write_to(s).await?;

        Ok(())
    }

    pub async fn process<F, R>(
        &self,
        s: &mut (impl AsyncRead + AsyncWrite + Unpin),
        log: &Logger,
        updatef: F,
    ) where
        F: Fn() -> R,
        R: Future<Output = FramebufferUpdate>,
    {
        loop {
            let req = ClientMessage::read_from(s).await;

            match req {
                Ok(client_msg) => match client_msg {
                    SetPixelFormat(pf) => {
                        debug!(log, "Rx: SetPixelFormat={:#?}", pf);

                        // TODO: invalid pixel formats?
                        let mut state = self.state.lock().await;
                        state.output_format = pf;
                        drop(state);
                    }
                    SetEncodings(e) => {
                        debug!(log, "Rx: SetEncodings={:?}", e);
                    }
                    FramebufferUpdateRequest(f) => {
                        debug!(log, "Rx: FramebufferUpdateRequest={:?}", f);

                        //let mut fbu = self.server.get_framebuffer_update().await;
                        let mut fbu = updatef().await;
                        let state = self.state.lock().await;

                        // We only need to change pixel formats if the client
                        // requested a different one than what's specified in
                        // the input.
                        //
                        // For now, we only support transformations between
                        // 4-byte RGB formats, so if the requested format isn't
                        // one of those, we'll just leave the pixels as is.
                        if state.input_format != state.output_format
                            && state.input_format.is_rgb_888()
                            && state.output_format.is_rgb_888()
                        {
                            debug!(
                                log,
                                "transforming: input={:#?}, output={:#?}",
                                state.input_format,
                                state.output_format
                            );
                            fbu = fbu.transform(
                                &state.input_format,
                                &state.output_format,
                            );
                        } else if !(state.input_format.is_rgb_888()
                            && state.output_format.is_rgb_888())
                        {
                            debug!(
                                log,
                                concat!(
                                    "cannot transform between pixel formats (not rgb888):",
                                    " input.is_rgb_888()={}, output.is_rgb_888()={}"
                                ),
                                state.input_format.is_rgb_888(),
                                state.output_format.is_rgb_888()
                            );
                        }

                        if let Err(e) = fbu.write_to(s).await {
                            error!(log, "could not write FramebufferUpdateRequest: {:?}", e);
                            return;
                        }
                        debug!(log, "Tx: FramebufferUpdate",);
                    }
                    KeyEvent(ke) => {
                        debug!(log, "Rx: KeyEvent={:?}", ke);
                    }
                    PointerEvent(pe) => {
                        debug!(log, "Rx: PointerEvent={:?}", pe);
                    }
                    ClientCutText(t) => {
                        debug!(log, "Rx: ClientCutText={:?}", t);
                    }
                },
                Err(e) => {
                    error!(log, "error reading client message: {}", e);
                    return;
                }
            }
        }
    }
}
