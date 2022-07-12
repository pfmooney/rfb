// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright 2022 Oxide Computer Company

use core::pin::Pin;
use core::task::{Context, Poll};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Parser;
use futures_util::{Sink, Stream};
use slog::{info, Drain};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use warp::filters::ws::{Message, WebSocket};
use warp::{self, Filter};

use rfb::rfb::{PixelFormat, ProtoVersion, SecurityType, SecurityTypes};
use rfb::{self, pixel_formats::rgb_888};

mod shared;
use shared::{order_to_shift, ExampleBackend, Image};

const WIDTH: usize = 1024;
const HEIGHT: usize = 768;

#[derive(Parser, Debug)]
struct Args {
    /// Image/color to display from the server
    #[clap(value_enum, short, long, default_value_t = Image::Oxide)]
    image: Image,
}

fn warp_to_io(err: warp::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

struct WsWrap {
    ws: WebSocket,
    buf: Option<(Message, usize)>,
}
impl WsWrap {
    fn new(ws: WebSocket) -> Self {
        Self { ws, buf: None }
    }
}
impl AsyncWrite for WsWrap {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let ws = Pin::new(&mut self.ws);
        match ws.poll_ready(cx) {
            Poll::Ready(Ok(())) => {
                let ws = Pin::new(&mut self.ws);
                let msg = Message::binary(buf);
                if let Err(e) = ws.start_send(msg) {
                    Poll::Ready(Err(warp_to_io(e)))
                } else {
                    Poll::Ready(Ok(buf.len()))
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(warp_to_io(e))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let ws = Pin::new(&mut self.ws);
        ws.poll_flush(cx).map_err(warp_to_io)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        let ws = Pin::new(&mut self.ws);
        ws.poll_close(cx).map_err(warp_to_io)
    }
}
impl AsyncRead for WsWrap {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.buf.take() {
            None => {
                let ws = Pin::new(&mut self.ws);
                match ws.poll_next(cx) {
                    Poll::Ready(Some(Ok(msg))) => {
                        self.buf = Some((msg, 0));
                        self.poll_read(cx, buf)
                    }
                    Poll::Ready(Some(Err(e))) => Poll::Ready(Err(warp_to_io(e))),
                    Poll::Ready(None) => Poll::Ready(Ok(())),
                    Poll::Pending => Poll::Pending,
                }
            }
            Some((msg, consumed)) => {
                let (_used, remain) = msg.as_bytes().split_at(consumed);
                let to_write = buf.remaining().min(remain.len());
                buf.put_slice(&remain[..to_write]);
                if to_write < remain.len() {
                    self.buf = Some((msg, consumed + to_write))
                }
                Poll::Ready(Ok(()))
            }
        }
    }
}

struct App {
    be: ExampleBackend,
    pf: PixelFormat,
    log: slog::Logger,
}

#[tokio::main]
async fn main() -> Result<()> {
    let log = slog::Logger::root(
        Mutex::new(
            slog_envlogger::EnvLogger::new(
                slog_term::FullFormat::new(slog_term::TermDecorator::new().build())
                    .build()
                    .fuse(),
            )
            .fuse(),
        )
        .fuse(),
        slog::o!(),
    );

    let args = Args::parse();

    let pf = PixelFormat::new_colorformat(
        rgb_888::BITS_PER_PIXEL,
        rgb_888::DEPTH,
        false,
        order_to_shift(0),
        rgb_888::MAX_VALUE,
        order_to_shift(1),
        rgb_888::MAX_VALUE,
        order_to_shift(2),
        rgb_888::MAX_VALUE,
    );
    let backend = ExampleBackend {
        display: args.image,
        rgb_order: (0, 1, 2),
        big_endian: false,
    };
    let app = Arc::new(App {
        be: backend,
        pf,
        log,
    });

    let app_clone = app.clone();
    let app_ctx = warp::any().map(move || app_clone.clone());

    let routes = warp::path("websockify")
        .and(warp::addr::remote())
        .and(warp::ws())
        .and(app_ctx)
        .map(
            |addr: Option<SocketAddr>, ws: warp::ws::Ws, app: Arc<App>| {
                let addr = addr.unwrap();
                info!(app.log, "New connection from {}", addr);

                let child_log = app.log.new(slog::o!("sock" => addr));
                let be_clone = app.be.clone();
                let pf_clone = app.pf.clone();

                ws.on_upgrade(move |websocket| async move {
                    let mut wrapped = WsWrap::new(websocket);

                    let server = rfb::Server::new(WIDTH as u16, HEIGHT as u16, pf_clone);
                    server
                        .initialize(
                            &mut wrapped,
                            &child_log,
                            ProtoVersion::Rfb38,
                            SecurityTypes(vec![
                                SecurityType::None,
                                SecurityType::VncAuthentication,
                            ]),
                            "rfb-example-server".to_string(),
                        )
                        .await
                        .unwrap();

                    server
                        .process(&mut wrapped, &child_log, || {
                            be_clone.generate(WIDTH, HEIGHT)
                        })
                        .await
                })
            },
        );

    info!(app.log, "Starting server");
    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;

    Ok(())
}
