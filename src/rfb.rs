// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright 2022 Oxide Computer Company

use anyhow::{anyhow, Result};
use bitflags::bitflags;
use futures::future::BoxFuture;
use futures::FutureExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::encodings::{Encoding, EncodingType};
use crate::keysym::KeySym;
use crate::pixel_formats::rgb_888;

#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
pub enum ProtoVersion {
    Rfb33,
    Rfb37,
    Rfb38,
}

impl ProtoVersion {
    pub fn read_from<'a>(stream: &'a mut TcpStream) -> BoxFuture<'a, Result<Self>> {
        async move {
            let mut buf = [0u8; 12];
            stream.read_exact(&mut buf).await?;

            match &buf {
                b"RFB 003.003\n" => Ok(ProtoVersion::Rfb33),
                b"RFB 003.007\n" => Ok(ProtoVersion::Rfb37),
                b"RFB 003.008\n" => Ok(ProtoVersion::Rfb38),
                _ => Err(anyhow!("invalid protocol version")),
            }
        }
        .boxed()
    }

    pub fn write_to<'a>(self, stream: &'a mut TcpStream) -> BoxFuture<'a, Result<()>> {
        async move {
            let s = match self {
                ProtoVersion::Rfb33 => b"RFB 003.003\n",
                ProtoVersion::Rfb37 => b"RFB 003.007\n",
                ProtoVersion::Rfb38 => b"RFB 003.008\n",
            };

            Ok(stream.write_all(s).await?)
        }
        .boxed()
    }
}

// Section 7.1.2
#[derive(Debug, Clone)]
pub struct SecurityTypes(pub Vec<SecurityType>);

#[derive(Clone, PartialEq, Debug)]
pub enum SecurityType {
    None,
    VncAuthentication,
}

impl SecurityTypes {
    pub fn write_to<'a>(self, stream: &'a mut TcpStream) -> BoxFuture<'a, Result<()>> {
        async move {
            // TODO: fix cast
            stream.write_u8(self.0.len() as u8).await?;
            for t in self.0.into_iter() {
                t.write_to(stream).await?;
            }

            Ok(())
        }
        .boxed()
    }
}

impl SecurityType {
    pub fn read_from<'a>(stream: &'a mut TcpStream) -> BoxFuture<'a, Result<Self>> {
        async move {
            let t = stream.read_u8().await?;
            match t {
                1 => Ok(SecurityType::None),
                2 => Ok(SecurityType::VncAuthentication),
                v => Err(anyhow!(format!("invalid security type={}", v))),
            }
        }
        .boxed()
    }
    pub fn write_to<'a>(self, stream: &'a mut TcpStream) -> BoxFuture<'a, Result<()>> {
        async move {
            let val = match self {
                SecurityType::None => 0,
                SecurityType::VncAuthentication => 1,
            };
            stream.write_u8(val).await?;

            Ok(())
        }
        .boxed()
    }
}

// Section 7.1.3
pub enum SecurityResult {
    Success,
    Failure(String),
}

impl SecurityResult {
    pub fn write_to<'a>(self, stream: &'a mut TcpStream) -> BoxFuture<'a, Result<()>> {
        async move {
            match self {
                SecurityResult::Success => {
                    stream.write_u32(0).await?;
                }
                SecurityResult::Failure(s) => {
                    stream.write_u32(1).await?;
                    stream.write_all(s.as_bytes()).await?;
                }
            };

            Ok(())
        }
        .boxed()
    }
}

// Section 7.3.1
#[derive(Debug)]
pub struct ClientInit {
    pub shared: bool,
}

impl ClientInit {
    pub fn read_from<'a>(stream: &'a mut TcpStream) -> BoxFuture<'a, Result<Self>> {
        async {
            let flag = stream.read_u8().await?;
            match flag {
                0 => Ok(ClientInit { shared: false }),
                _ => Ok(ClientInit { shared: true }),
            }
        }
        .boxed()
    }
}

// Section 7.3.2
#[derive(Debug)]
pub struct ServerInit {
    initial_res: Resolution,
    pixel_format: PixelFormat,
    name: String,
}

impl ServerInit {
    pub fn new(width: u16, height: u16, name: String, pixel_format: PixelFormat) -> Self {
        Self {
            initial_res: Resolution { width, height },
            pixel_format,
            name,
        }
    }
    pub fn write_to<'a>(self, stream: &'a mut TcpStream) -> BoxFuture<'a, Result<()>> {
        async move {
            self.initial_res.write_to(stream).await?;
            self.pixel_format.write_to(stream).await?;

            // TODO: cast properly
            stream.write_u32(self.name.len() as u32).await?;
            stream.write_all(self.name.as_bytes()).await?;

            Ok(())
        }
        .boxed()
    }
}

pub enum _ServerMessage {
    FramebufferUpdate(FramebufferUpdate),
    SetColorMapEntries(SetColorMapEntries),
    Bell,
    ServerCutText(CutText),
}

pub struct FramebufferUpdate {
    rectangles: Vec<Rectangle>,
}

impl FramebufferUpdate {
    pub fn new(rectangles: Vec<Rectangle>) -> Self {
        FramebufferUpdate { rectangles }
    }

    pub fn transform(&self, input_pf: &PixelFormat, output_pf: &PixelFormat) -> Self {
        let mut rectangles = Vec::new();

        for r in self.rectangles.iter() {
            rectangles.push(r.transform(input_pf, output_pf));
        }

        FramebufferUpdate { rectangles }
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Position {
    x: u16,
    y: u16,
}

impl Position {
    pub fn read_from<'a>(stream: &'a mut TcpStream) -> BoxFuture<'a, Result<Self>> {
        async {
            let x = stream.read_u16().await?;
            let y = stream.read_u16().await?;

            Ok(Position { x, y })
        }
        .boxed()
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Resolution {
    width: u16,
    height: u16,
}

impl Resolution {
    pub fn read_from<'a>(stream: &'a mut TcpStream) -> BoxFuture<'a, Result<Self>> {
        async {
            let width = stream.read_u16().await?;
            let height = stream.read_u16().await?;

            Ok(Resolution { width, height })
        }
        .boxed()
    }
    pub fn write_to<'a>(self, stream: &'a mut TcpStream) -> BoxFuture<'a, Result<()>> {
        async move {
            stream.write_u16(self.width).await?;
            stream.write_u16(self.height).await?;
            Ok(())
        }
        .boxed()
    }
}

pub struct Rectangle {
    position: Position,
    dimensions: Resolution,
    data: Box<dyn Encoding>,
}

impl Rectangle {
    pub fn new(x: u16, y: u16, width: u16, height: u16, data: Box<dyn Encoding>) -> Self {
        Rectangle {
            position: Position { x, y },
            dimensions: Resolution { width, height },
            data,
        }
    }

    pub fn transform(&self, input_pf: &PixelFormat, output_pf: &PixelFormat) -> Self {
        Rectangle {
            position: self.position,
            dimensions: self.dimensions,
            data: self.data.transform(input_pf, output_pf),
        }
    }
}

impl Rectangle {
    pub fn write_to<'a>(self, stream: &'a mut TcpStream) -> BoxFuture<'a, Result<()>> {
        async move {
            let encoding_type: i32 = self.data.get_type().into();

            stream.write_u16(self.position.x).await?;
            stream.write_u16(self.position.y).await?;
            stream.write_u16(self.dimensions.width).await?;
            stream.write_u16(self.dimensions.height).await?;
            stream.write_i32(encoding_type).await?;

            let data = self.data.encode();
            stream.write_all(data).await?;

            Ok(())
        }
        .boxed()
    }
}

impl FramebufferUpdate {
    pub fn write_to<'a>(self, stream: &'a mut TcpStream) -> BoxFuture<'a, Result<()>> {
        async move {
            // TODO: type function?
            stream.write_u8(0).await?;

            // 1 byte of padding
            stream.write_u8(0).await?;

            // number of rectangles
            let n_rect = self.rectangles.len() as u16;
            stream.write_u16(n_rect).await?;

            // rectangles
            for r in self.rectangles.into_iter() {
                r.write_to(stream).await?;
            }

            Ok(())
        }
        .boxed()
    }
}

#[derive(Debug)]
pub struct SetColorMapEntries {
    _colors: Vec<_ColorMapEntry>,
}

#[derive(Debug)]
pub struct _ColorMapEntry {
    _color: u16,
    _red: u16,
    _blue: u16,
    _green: u16,
}

// TODO: only ISO 8859-1 (Latin-1) text supported
// used for client and server
#[derive(Debug)]
pub struct CutText {
    _text: String,
}

// Section 7.4
#[derive(Debug, Clone, PartialEq)]
pub struct PixelFormat {
    pub bits_per_pixel: u8, // TODO: must be 8, 16, or 32
    pub depth: u8,          // TODO: must be < bits_per_pixel
    pub big_endian: bool,
    pub color_spec: ColorSpecification,
}

impl PixelFormat {
    /// Constructor for a PixelFormat that uses a color format to specify colors.
    pub fn new_colorformat(
        bbp: u8,
        depth: u8,
        big_endian: bool,
        red_shift: u8,
        red_max: u16,
        green_shift: u8,
        green_max: u16,
        blue_shift: u8,
        blue_max: u16,
    ) -> Self {
        PixelFormat {
            bits_per_pixel: bbp,
            depth,
            big_endian,
            color_spec: ColorSpecification::ColorFormat(ColorFormat {
                red_max,
                green_max,
                blue_max,
                red_shift,
                green_shift,
                blue_shift,
            }),
        }
    }

    /// Returns true if the pixel format is RGB888 (8-bits per color and 32 bits per pixel).
    pub fn is_rgb_888(&self) -> bool {
        if self.bits_per_pixel != rgb_888::BITS_PER_PIXEL || self.depth != rgb_888::DEPTH {
            return false;
        }

        match &self.color_spec {
            ColorSpecification::ColorFormat(cf) => {
                (cf.red_max == rgb_888::MAX_VALUE)
                    && (cf.green_max == rgb_888::MAX_VALUE)
                    && (cf.blue_max == rgb_888::MAX_VALUE)
                    && (rgb_888::valid_shift(cf.red_shift))
                    && (rgb_888::valid_shift(cf.green_shift))
                    && (rgb_888::valid_shift(cf.blue_shift))
            }
            ColorSpecification::ColorMap(_) => false,
        }
    }
}

impl PixelFormat {
    pub fn read_from<'a>(stream: &'a mut TcpStream) -> BoxFuture<'a, Result<Self>> {
        async {
            let bits_per_pixel = stream.read_u8().await?;
            let depth = stream.read_u8().await?;
            let be_flag = stream.read_u8().await?;
            let big_endian = match be_flag {
                0 => false,
                _ => true,
            };
            let color_spec = ColorSpecification::read_from(stream).await?;

            // 3 bytes of padding
            let mut buf = [0u8; 3];
            stream.read_exact(&mut buf).await?;

            Ok(Self {
                bits_per_pixel,
                depth,
                big_endian,
                color_spec,
            })
        }
        .boxed()
    }
    pub fn write_to<'a>(self, stream: &'a mut TcpStream) -> BoxFuture<'a, Result<()>> {
        async move {
            stream.write_u8(self.bits_per_pixel).await?;
            stream.write_u8(self.depth).await?;
            stream.write_u8(if self.big_endian { 1 } else { 0 }).await?;
            self.color_spec.write_to(stream).await?;

            // 3 bytes of padding
            let buf = [0u8; 3];
            stream.write_all(&buf).await?;

            Ok(())
        }
        .boxed()
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum ColorSpecification {
    ColorFormat(ColorFormat),
    ColorMap(ColorMap), // TODO: implement
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColorFormat {
    // TODO: maxes must be 2^N - 1 for N bits per color
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColorMap {}

impl ColorSpecification {
    pub fn read_from<'a>(stream: &'a mut TcpStream) -> BoxFuture<'a, Result<Self>> {
        async {
            let tc_flag = stream.read_u8().await?;
            match tc_flag {
                0 => {
                    // ColorMap
                    unimplemented!()
                }
                _ => {
                    // ColorFormat
                    let red_max = stream.read_u16().await?;
                    let green_max = stream.read_u16().await?;
                    let blue_max = stream.read_u16().await?;

                    let red_shift = stream.read_u8().await?;
                    let green_shift = stream.read_u8().await?;
                    let blue_shift = stream.read_u8().await?;

                    Ok(ColorSpecification::ColorFormat(ColorFormat {
                        red_max,
                        green_max,
                        blue_max,
                        red_shift,
                        green_shift,
                        blue_shift,
                    }))
                }
            }
        }
        .boxed()
    }
    pub fn write_to<'a>(self, stream: &'a mut TcpStream) -> BoxFuture<'a, Result<()>> {
        async move {
            match self {
                ColorSpecification::ColorFormat(cf) => {
                    stream.write_u8(1).await?; // true color
                    stream.write_u16(cf.red_max).await?;
                    stream.write_u16(cf.green_max).await?;
                    stream.write_u16(cf.blue_max).await?;

                    stream.write_u8(cf.red_shift).await?;
                    stream.write_u8(cf.green_shift).await?;
                    stream.write_u8(cf.blue_shift).await?;
                }
                ColorSpecification::ColorMap(_cm) => {
                    unimplemented!()
                }
            };

            Ok(())
        }
        .boxed()
    }
}

// Section 7.5
pub enum ClientMessage {
    SetPixelFormat(PixelFormat),
    SetEncodings(Vec<EncodingType>),
    FramebufferUpdateRequest(FramebufferUpdateRequest),
    KeyEvent(KeyEvent),
    PointerEvent(PointerEvent),
    ClientCutText(String),
}

impl ClientMessage {
    pub fn read_from<'a>(stream: &'a mut TcpStream) -> BoxFuture<'a, Result<ClientMessage>> {
        async {
            let t = stream.read_u8().await?;
            let res = match t {
                0 => {
                    // SetPixelFormat
                    let mut padding = [0u8; 3];
                    stream.read_exact(&mut padding).await?;
                    let pixel_format = PixelFormat::read_from(stream).await?;
                    Ok(ClientMessage::SetPixelFormat(pixel_format))
                }

                2 => {
                    // SetEncodings
                    stream.read_u8().await?; // 1 byte of padding
                    let num_encodings = stream.read_u16().await?;

                    // TODO: what to do if num_encodings is 0

                    let mut encodings = Vec::new();
                    for _ in 0..num_encodings {
                        let e: EncodingType = EncodingType::try_from(stream.read_i32().await?)?;
                        encodings.push(e);
                    }

                    Ok(ClientMessage::SetEncodings(encodings))
                }
                3 => {
                    // FramebufferUpdateRequest
                    let incremental = match stream.read_u8().await? {
                        0 => false,
                        _ => true,
                    };
                    let position = Position::read_from(stream).await?;
                    let resolution = Resolution::read_from(stream).await?;

                    let fbu_req = FramebufferUpdateRequest {
                        incremental,
                        position,
                        resolution,
                    };

                    Ok(ClientMessage::FramebufferUpdateRequest(fbu_req))
                }
                4 => {
                    // KeyEvent
                    let is_pressed = match stream.read_u8().await? {
                        0 => false,
                        _ => true,
                    };

                    // 2 bytes of padding
                    stream.read_u16().await?;

                    let keysym_raw = stream.read_u32().await?;
                    let keysym = KeySym::try_from(keysym_raw)?;

                    let key_event = KeyEvent {
                        is_pressed,
                        keysym,
                        keysym_raw,
                    };

                    Ok(ClientMessage::KeyEvent(key_event))
                }
                5 => {
                    // PointerEvent
                    let pointer_event = PointerEvent::read_from(stream).await?;
                    Ok(ClientMessage::PointerEvent(pointer_event))
                }
                6 => {
                    // ClientCutText

                    // 3 bytes of padding
                    let mut padding = [0u8; 3];
                    stream.read_exact(&mut padding).await?;

                    let len = stream.read_u32().await?;
                    let mut buf: Vec<u8> = Vec::with_capacity(len as usize);
                    stream.read_exact(&mut buf).await?;

                    // TODO: The encoding RFB uses is ISO 8859-1 (Latin-1), which is a subset of
                    // utf-8. Determine if this is the right approach.
                    let text = String::from_utf8(buf)?;

                    Ok(ClientMessage::ClientCutText(text))
                }
                unknown => Err(anyhow!(format!("unknown client message type: {}", unknown))),
            };

            res
        }
        .boxed()
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct FramebufferUpdateRequest {
    incremental: bool,
    position: Position,
    resolution: Resolution,
}

#[derive(Debug, Copy, Clone)]
pub struct KeyEvent {
    is_pressed: bool,
    keysym: KeySym,
    keysym_raw: u32,
}

impl KeyEvent {
    pub fn keysym_raw(&self) -> u32 {
        self.keysym_raw
    }

    pub fn keysym(&self) -> KeySym {
        self.keysym
    }

    pub fn is_pressed(&self) -> bool {
        self.is_pressed
    }
}

bitflags! {
    struct MouseButtons: u8 {
        const LEFT = 1 << 0;
        const MIDDLE = 1 << 1;
        const RIGHT = 1 << 2;
        const SCROLL_A = 1 << 3;
        const SCROLL_B = 1 << 4;
        const SCROLL_C = 1 << 5;
        const SCROLL_D = 1 << 6;
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct PointerEvent {
    position: Position,
    pressed: MouseButtons,
}

impl PointerEvent {
    pub fn read_from<'a>(stream: &'a mut TcpStream) -> BoxFuture<'a, Result<Self>> {
        async {
            let button_mask = stream.read_u8().await?;
            let pressed = MouseButtons::from_bits_truncate(button_mask);
            let position = Position::read_from(stream).await?;

            Ok(PointerEvent { position, pressed })
        }
        .boxed()
    }
}
