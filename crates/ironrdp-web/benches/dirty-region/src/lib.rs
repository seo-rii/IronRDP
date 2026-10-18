include!(concat!(env!("OUT_DIR"), "/module.rs"));

use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::pdu::bitmap::{BitmapData, BitmapUpdateData, Compression};
use ironrdp::pdu::geometry::{InclusiveRectangle, Rectangle as _};
use ironrdp::session::fast_path::ProcessorBuilder;
use ironrdp::session::image::DecodedImage;
use ironrdp_core::WriteBuf;

pub const POLICIES: [&str; 7] = ["baseline", "cap1.25", "cap1.5", "cap2", "exact", "whole", "production"];

pub fn rectangle(left: u16, top: u16, width: u16, height: u16) -> InclusiveRectangle {
    assert!(width > 0 && height > 0);
    InclusiveRectangle {
        left,
        top,
        right: left + width - 1,
        bottom: top + height - 1,
    }
}

pub struct Fixture {
    frames: [DecodedImage; 2],
    buffer: WriteBuf,
}

impl Fixture {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            frames: [
                DecodedImage::new(PixelFormat::RgbA32, width, height),
                DecodedImage::new(PixelFormat::RgbA32, width, height),
            ],
            buffer: WriteBuf::new(),
        }
    }

    pub fn paint(&mut self, region: InclusiveRectangle, seed: u32) {
        self.paint_frames(region, seed, true);
    }

    pub fn paint_common(&mut self, region: InclusiveRectangle, seed: u32) {
        self.paint_frames(region, seed, false);
    }

    fn paint_frames(&mut self, region: InclusiveRectangle, seed: u32, alternate: bool) {
        for (frame_index, frame) in self.frames.iter_mut().enumerate() {
            let mut source = Vec::with_capacity(usize::from(region.width()) * usize::from(region.height()) * 4);
            // BitmapData uses bottom-up BGRX. Fixture setup is outside all timers.
            for y in (region.top..=region.bottom).rev() {
                for x in region.left..=region.right {
                    let variation = if alternate { frame_index as u32 * 97 } else { 0 };
                    let value = (u32::from(x) * 31 + u32::from(y) * 71 + seed * 13 + variation).wrapping_mul(0x45d9f3b);
                    source.extend_from_slice(&[value as u8, (value >> 8) as u8, (value >> 16) as u8, 0]);
                }
            }
            let mut processor = ProcessorBuilder {
                io_channel_id: 1003,
                user_channel_id: 1001,
                share_id: 1,
                enable_server_pointer: false,
                pointer_software_rendering: false,
            }
            .build();
            processor
                .process_bitmap_update(
                    frame,
                    BitmapUpdateData {
                        rectangles: vec![BitmapData {
                            rectangle: region.clone(),
                            width: region.width(),
                            height: region.height(),
                            bits_per_pixel: 32,
                            compression_flags: Compression::empty(),
                            compressed_data_header: None,
                            bitmap_data: &source,
                        }],
                    },
                )
                .expect("populate synthetic framebuffer through the public bitmap API");
        }
    }

    pub fn copy_batch(&mut self, policy: u8, region: InclusiveRectangle, iterations: u32) -> usize {
        let mut bytes = 0;
        for iteration in 0..iterations {
            self.buffer.clear();
            let selected = image::extract_benchmark(
                std::hint::black_box(&self.frames[iteration as usize % 2]),
                std::hint::black_box(region.clone()),
                &mut self.buffer,
                policy,
            );
            std::hint::black_box(selected);
            std::hint::black_box(self.buffer.filled());
            bytes = self.buffer.filled_len();
        }
        bytes
    }
}

#[cfg(target_arch = "wasm32")]
#[path = "../../../src/canvas.rs"]
mod canvas;

#[cfg(target_arch = "wasm32")]
mod browser {
    use core::num::NonZeroU32;

    use wasm_bindgen::prelude::*;
    use web_sys::HtmlCanvasElement;

    use super::{Fixture, canvas::Canvas, image, rectangle};

    #[wasm_bindgen]
    pub struct Benchmark {
        fixture: Fixture,
        canvas: Canvas,
    }

    #[wasm_bindgen]
    impl Benchmark {
        #[wasm_bindgen(constructor)]
        pub fn new(element: HtmlCanvasElement, width: u16, height: u16) -> Result<Benchmark, JsValue> {
            Ok(Self {
                fixture: Fixture::new(width, height),
                canvas: Canvas::new(
                    element,
                    NonZeroU32::new(u32::from(width)).expect("nonzero width"),
                    NonZeroU32::new(u32::from(height)).expect("nonzero height"),
                )
                .map_err(|error| JsValue::from_str(&error.to_string()))?,
            })
        }

        pub fn resize(&mut self, width: u16, height: u16) {
            self.fixture = Fixture::new(width, height);
            self.canvas.resize(
                NonZeroU32::new(u32::from(width)).expect("nonzero width"),
                NonZeroU32::new(u32::from(height)).expect("nonzero height"),
            );
        }

        pub fn paint(&mut self, left: u16, top: u16, width: u16, height: u16, seed: u32) {
            self.fixture.paint(rectangle(left, top, width, height), seed);
        }

        pub fn paint_common(&mut self, width: u16, height: u16, seed: u32) {
            self.fixture.paint_common(rectangle(0, 0, width, height), seed);
        }

        pub fn copy_batch(
            &mut self,
            policy: u8,
            left: u16,
            top: u16,
            width: u16,
            height: u16,
            iterations: u32,
        ) -> usize {
            self.fixture
                .copy_batch(policy, rectangle(left, top, width, height), iterations)
        }

        pub fn draw_batch(
            &mut self,
            policy: u8,
            left: u16,
            top: u16,
            width: u16,
            height: u16,
            iterations: u32,
        ) -> Result<usize, JsValue> {
            let region = rectangle(left, top, width, height);
            for iteration in 0..iterations {
                self.fixture.buffer.clear();
                let selected = image::extract_benchmark(
                    &self.fixture.frames[iteration as usize % 2],
                    region.clone(),
                    &mut self.fixture.buffer,
                    policy,
                );
                self.canvas
                    .draw(self.fixture.buffer.filled_mut(), selected)
                    .map_err(|error| JsValue::from_str(&error.to_string()))?;
            }
            Ok(self.fixture.buffer.filled_len())
        }

        pub fn expected_pixels(&self, frame_index: usize) -> Vec<u8> {
            let mut pixels = self.fixture.frames[frame_index].data().to_vec();
            for pixel in pixels.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
            pixels
        }
    }
}
