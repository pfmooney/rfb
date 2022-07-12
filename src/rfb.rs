// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright 2022 Oxide Computer Company

use anyhow::{anyhow, Result};
use bitflags::bitflags;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::encodings::{Encoding, EncodingType};
use crate::keysym::Keysym;
use crate::pixel_formats::rgb_888;

#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
pub enum ProtoVersion {
    Rfb33,
    Rfb37,
    Rfb38,
}

impl ProtoVersion {
    pub async fn read_from(
        stream: &mut (impl AsyncRead + Unpin),
    ) -> Result<Self> {
        let mut buf = [0u8; 12];
        stream.read_exact(&mut buf).await?;

        match &buf {
            b"RFB 003.003\n" => Ok(ProtoVersion::Rfb33),
            b"RFB 003.007\n" => Ok(ProtoVersion::Rfb37),
            b"RFB 003.008\n" => Ok(ProtoVersion::Rfb38),
            _ => Err(anyhow!("invalid protocol version")),
        }
    }

    pub async fn write_to(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        let s = match self {
            ProtoVersion::Rfb33 => b"RFB 003.003\n",
            ProtoVersion::Rfb37 => b"RFB 003.007\n",
            ProtoVersion::Rfb38 => b"RFB 003.008\n",
        };

        Ok(stream.write_all(s).await?)
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
    pub async fn write_to(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        // TODO: fix cast
        stream.write_u8(self.0.len() as u8).await?;
        for t in self.0.into_iter() {
            t.write_to(stream).await?;
        }

        Ok(())
    }
}

impl SecurityType {
    pub async fn read_from(
        stream: &mut (impl AsyncRead + Unpin),
    ) -> Result<Self> {
        let t = stream.read_u8().await?;
        match t {
            1 => Ok(SecurityType::None),
            2 => Ok(SecurityType::VncAuthentication),
            v => Err(anyhow!(format!("invalid security type={}", v))),
        }
    }
    pub async fn write_to(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        let val = match self {
            SecurityType::None => 0,
            SecurityType::VncAuthentication => 1,
        };
        stream.write_u8(val).await?;

        Ok(())
    }
}

// Section 7.1.3
pub enum SecurityResult {
    Success,
    Failure(String),
}

impl SecurityResult {
    pub async fn write_to(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
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
}

// Section 7.3.1
#[derive(Debug)]
pub struct ClientInit {
    pub shared: bool,
}

impl ClientInit {
    pub async fn read_from(
        stream: &mut (impl AsyncRead + Unpin),
    ) -> Result<Self> {
        let flag = stream.read_u8().await?;
        match flag {
            0 => Ok(ClientInit { shared: false }),
            _ => Ok(ClientInit { shared: true }),
        }
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
    pub fn new(
        width: u16,
        height: u16,
        name: String,
        pixel_format: PixelFormat,
    ) -> Self {
        Self { initial_res: Resolution { width, height }, pixel_format, name }
    }
    pub async fn write_to(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        self.initial_res.write_to(stream).await?;
        self.pixel_format.write_to(stream).await?;

        // TODO: cast properly
        stream.write_u32(self.name.len() as u32).await?;
        stream.write_all(self.name.as_bytes()).await?;

        Ok(())
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

    pub fn transform(
        &self,
        input_pf: &PixelFormat,
        output_pf: &PixelFormat,
    ) -> Self {
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
    pub async fn read_from(
        stream: &mut (impl AsyncRead + Unpin),
    ) -> Result<Self> {
        let x = stream.read_u16().await?;
        let y = stream.read_u16().await?;

        Ok(Position { x, y })
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Resolution {
    width: u16,
    height: u16,
}

impl Resolution {
    pub async fn read_from(
        stream: &mut (impl AsyncRead + Unpin),
    ) -> Result<Self> {
        let width = stream.read_u16().await?;
        let height = stream.read_u16().await?;

        Ok(Resolution { width, height })
    }
    pub async fn write_to(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        stream.write_u16(self.width).await?;
        stream.write_u16(self.height).await?;
        Ok(())
    }
}

pub struct Rectangle {
    position: Position,
    dimensions: Resolution,
    data: Box<dyn Encoding>,
}

impl Rectangle {
    pub fn new(
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        data: Box<dyn Encoding>,
    ) -> Self {
        Rectangle {
            position: Position { x, y },
            dimensions: Resolution { width, height },
            data,
        }
    }

    pub fn transform(
        &self,
        input_pf: &PixelFormat,
        output_pf: &PixelFormat,
    ) -> Self {
        Rectangle {
            position: self.position,
            dimensions: self.dimensions,
            data: self.data.transform(input_pf, output_pf),
        }
    }
}

impl Rectangle {
    pub async fn write_to(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
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
}

impl FramebufferUpdate {
    pub async fn write_to<'a>(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
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
        cf: ColorFormat,
    ) -> Self {
        PixelFormat {
            bits_per_pixel: bbp,
            depth,
            big_endian,
            color_spec: ColorSpecification::ColorFormat(cf),
        }
    }

    /// Returns true if the pixel format is RGB888 (8-bits per color and 32 bits per pixel).
    pub fn is_rgb_888(&self) -> bool {
        if self.bits_per_pixel != rgb_888::BITS_PER_PIXEL
            || self.depth != rgb_888::DEPTH
        {
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
    pub async fn read_from(
        stream: &mut (impl AsyncRead + Unpin),
    ) -> Result<Self> {
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

        Ok(Self { bits_per_pixel, depth, big_endian, color_spec })
    }
    pub async fn write_to(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
        stream.write_u8(self.bits_per_pixel).await?;
        stream.write_u8(self.depth).await?;
        stream.write_u8(if self.big_endian { 1 } else { 0 }).await?;
        self.color_spec.write_to(stream).await?;

        // 3 bytes of padding
        let buf = [0u8; 3];
        stream.write_all(&buf).await?;

        Ok(())
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
    pub async fn read_from(
        stream: &mut (impl AsyncRead + Unpin),
    ) -> Result<Self> {
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
    pub async fn write_to(
        self,
        stream: &mut (impl AsyncWrite + Unpin),
    ) -> Result<()> {
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
    pub async fn read_from(
        stream: &mut (impl AsyncRead + Unpin),
    ) -> Result<ClientMessage> {
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
                    let e: EncodingType =
                        EncodingType::try_from(stream.read_i32().await?)?;
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

                let key = Keysym::try_from(stream.read_u32().await?)?;

                let key_event = KeyEvent { is_pressed, key };

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
            unknown => Err(anyhow!(format!(
                "unknown client message type: {}",
                unknown
            ))),
        };

        res
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct FramebufferUpdateRequest {
    incremental: bool,
    position: Position,
    resolution: Resolution,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct KeyEvent {
    is_pressed: bool,
    key: Keysym,
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
    pub async fn read_from(
        stream: &mut (impl AsyncRead + Unpin),
    ) -> Result<Self> {
        let button_mask = stream.read_u8().await?;
        let pressed = MouseButtons::from_bits_truncate(button_mask);
        let position = Position::read_from(stream).await?;

        Ok(PointerEvent { position, pressed })
    }
}
