// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright 2022 Oxide Computer Company

#![allow(dead_code)]

use clap::ValueEnum;
use image::io::Reader as ImageReader;
use image::GenericImageView;

use rfb::encodings::RawEncoding;
use rfb::pixel_formats::rgb_888;
use rfb::rfb::{FramebufferUpdate, Rectangle};

#[derive(ValueEnum, Debug, Copy, Clone)]
pub enum Image {
    Oxide,
    TestTubes,
    Red,
    Green,
    Blue,
    White,
    Black,
}
#[derive(Clone)]
pub struct ExampleBackend {
    pub display: Image,
    pub rgb_order: (u8, u8, u8),
    pub big_endian: bool,
}
impl ExampleBackend {
    pub async fn generate(
        &self,
        width: usize,
        height: usize,
    ) -> FramebufferUpdate {
        let size = Size { width, height };
        let pixels = generate_pixels(
            size,
            self.display,
            self.big_endian,
            self.rgb_order,
        );
        let r = Rectangle::new(
            0,
            0,
            width as u16,
            height as u16,
            Box::new(RawEncoding::new(pixels)),
        );
        FramebufferUpdate::new(vec![r])
    }
}

#[derive(Copy, Clone)]
struct Size {
    width: usize,
    height: usize,
}
impl Size {
    const fn len(&self, bytes_per_pixel: usize) -> usize {
        self.width * self.height * bytes_per_pixel
    }
}

fn generate_image(
    size: Size,
    name: &str,
    big_endian: bool,
    rgb_order: (u8, u8, u8),
) -> Vec<u8> {
    let mut pixels = vec![0xffu8; size.len(rgb_888::BYTES_PER_PIXEL)];

    let img = ImageReader::open(name).unwrap().decode().unwrap();

    let (r, g, b) = rgb_order;
    let r_idx = order_to_index(r, big_endian) as usize;
    let g_idx = order_to_index(g, big_endian) as usize;
    let b_idx = order_to_index(b, big_endian) as usize;
    let x_idx = rgb_888::unused_index(r_idx, g_idx, b_idx);

    // Convert the input image pixels to the requested pixel format.
    for (x, y, pixel) in img.pixels() {
        let ux = x as usize;
        let uy = y as usize;

        let y_offset = size.width * rgb_888::BYTES_PER_PIXEL;
        let x_offset = ux * rgb_888::BYTES_PER_PIXEL;

        pixels[uy * y_offset + x_offset + r_idx] = pixel[0];
        pixels[uy * y_offset + x_offset + g_idx] = pixel[1];
        pixels[uy * y_offset + x_offset + b_idx] = pixel[2];
        pixels[uy * y_offset + x_offset + x_idx] = pixel[3];
    }

    pixels
}

fn generate_pixels(
    size: Size,
    img: Image,
    big_endian: bool,
    rgb_order: (u8, u8, u8),
) -> Vec<u8> {
    let (r, g, b) = rgb_order;
    match img {
        Image::Oxide => generate_image(
            size,
            "examples/images/oxide.jpg",
            big_endian,
            rgb_order,
        ),
        Image::TestTubes => generate_image(
            size,
            "examples/images/test-tubes.jpg",
            big_endian,
            rgb_order,
        ),
        Image::Red => generate_color(size, r, big_endian),
        Image::Green => generate_color(size, g, big_endian),
        Image::Blue => generate_color(size, b, big_endian),
        Image::White => vec![0xffu8; size.len(rgb_888::BYTES_PER_PIXEL)],
        Image::Black => vec![0x0u8; size.len(rgb_888::BYTES_PER_PIXEL)],
    }
}

fn generate_color(size: Size, index: u8, big_endian: bool) -> Vec<u8> {
    let mut pixels = vec![0x0u8; size.len(rgb_888::BYTES_PER_PIXEL)];

    let idx = order_to_index(index, big_endian);
    for (n, val) in pixels.iter_mut().enumerate() {
        if n as u8 % 4 == idx {
            *val = 0xff;
        }
    }

    pixels
}

pub fn order_to_shift(order: u8) -> u8 {
    assert!(order <= 3);
    (3 - order) * rgb_888::BITS_PER_COLOR
}

fn order_to_index(order: u8, big_endian: bool) -> u8 {
    assert!(order <= 3);

    if big_endian {
        order
    } else {
        4 - order - 1
    }
}
