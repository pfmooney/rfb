// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright 2022 Oxide Computer Company

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;

use anyhow::{bail, Result};
use clap::Parser;
use slog::{info, Drain};
use tokio::net::TcpListener;

use rfb::encodings::RawEncoding;
use rfb::rfb::{
    FramebufferUpdate, KeyEvent, PixelFormat, ProtoVersion, Rectangle, SecurityType, SecurityTypes,
};
use rfb::{self, pixel_formats::rgb_888};

mod shared;
use shared::{order_to_shift, ExampleBackend, Image};

const WIDTH: usize = 1024;
const HEIGHT: usize = 768;

#[derive(Parser, Debug)]
/// A simple VNC server that displays a single image or color, in a given pixel format
///
/// By default, the server will display the Oxide logo image using little-endian RGBx as its pixel format. To specify an alternate image or color, use the `-i` flag:
/// ./example-server -i test-tubes
/// ./example-server -i red
///
/// To specify an alternate pixel format, use the `--big-endian` flag and/or the ordering flags. The
/// server will transform the input image/color to the requested pixel format and use the format
/// for the RFB protocol.
///
/// For example, to use big-endian xRGB:
/// ./example-server --big-endian true -r 1 -g 2 -b 3
///
struct Args {
    /// Image/color to display from the server
    #[clap(value_enum, short, long, default_value_t = Image::Oxide)]
    image: Image,

    /// Pixel endianness
    #[clap(long, default_value_t = false, action = clap::ArgAction::Set)]
    big_endian: bool,

    /// Byte mapping to red (4-byte RGB pixel, endian-agnostic)
    #[clap(short, long, default_value_t = 0)]
    red_order: u8,

    /// Byte mapping to green (4-byte RGB pixel, endian-agnostic)
    #[clap(short, long, default_value_t = 1)]
    green_order: u8,

    /// Byte mapping to blue (4-byte RGB pixel, endian-agnostic)
    #[clap(short, long, default_value_t = 2)]
    blue_order: u8,
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
    validate_order(args.red_order, args.green_order, args.blue_order)?;

    let pf = PixelFormat::new_colorformat(
        rgb_888::BITS_PER_PIXEL,
        rgb_888::DEPTH,
        args.big_endian,
        order_to_shift(args.red_order),
        rgb_888::MAX_VALUE,
        order_to_shift(args.green_order),
        rgb_888::MAX_VALUE,
        order_to_shift(args.blue_order),
        rgb_888::MAX_VALUE,
    );
    info!(
        log,
        "Starting server: image: {:?}, pixel format; {:#?}", args.image, pf
    );

    let backend = ExampleBackend {
        display: args.image,
        rgb_order: (args.red_order, args.green_order, args.blue_order),
        big_endian: args.big_endian,
    };

    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 9000))
        .await
        .unwrap();

    loop {
        let (mut sock, addr) = listener.accept().await.unwrap();

        info!(log, "New connection from {:?}", addr);

        let server = rfb::Server::new(WIDTH as u16, HEIGHT as u16, pf.clone());
        server
            .initialize(
                &mut sock,
                &log,
                ProtoVersion::Rfb38,
                SecurityTypes(vec![SecurityType::None, SecurityType::VncAuthentication]),
                "rfb-example-server".to_string(),
            )
            .await
            .unwrap();

        let be_clone = backend.clone();
        let log_child = log.new(slog::o!("sock" => addr));
        tokio::spawn(async move {
            server
                .process(&mut sock, &log_child, || be_clone.generate(WIDTH, HEIGHT))
                .await;
        });
    }
}

fn validate_order(r: u8, g: u8, b: u8) -> Result<()> {
    if r > 3 || g > 3 || b > 3 {
        bail!("r/g/b must have ordering of 0, 1, 2, or 3");
    }

    if r == g || r == b || g == b {
        bail!("r/g/b must have unique orderings");
    }

    Ok(())
}
