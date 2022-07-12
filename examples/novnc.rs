// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright 2022 Oxide Computer Company

use core::pin::Pin;
use core::task::{Context, Poll};
use std::io;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use futures_util::{Sink, Stream};
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

#[tokio::main]
async fn main() -> Result<()> {
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

    let state = Arc::new((backend, pf));
    let state_add = warp::any().map(move || state.clone());

    let routes = warp::path("websockify").and(warp::ws()).and(state_add).map(
        |ws: warp::ws::Ws, state: Arc<(ExampleBackend, PixelFormat)>| {
            ws.on_upgrade(move |websocket| async move {
                let mut wrapped = WsWrap::new(websocket);

                let be_clone = state.0.clone();
                let pf_clone = state.1.clone();

                let server = rfb::Server::new(WIDTH as u16, HEIGHT as u16, pf_clone);
                server
                    .initialize(
                        &mut wrapped,
                        ProtoVersion::Rfb38,
                        SecurityTypes(vec![SecurityType::None, SecurityType::VncAuthentication]),
                        "rfb-example-server".to_string(),
                    )
                    .await
                    .unwrap();

                server
                    .process(&mut wrapped, || be_clone.generate(WIDTH, HEIGHT))
                    .await
            })
        },
    );

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;

    Ok(())
}
