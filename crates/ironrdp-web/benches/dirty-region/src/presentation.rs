//! Direct calls into independently compiled production/candidate sources.
use core::hint::black_box;
use core::num::NonZeroU32;

use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use super::{Fixture, canvas::Canvas, image, rectangle};

#[wasm_bindgen]
pub struct PresentationBenchmark {
    fixture: Fixture,
    canvas: Canvas,
    next_frame: usize,
}

#[wasm_bindgen]
impl PresentationBenchmark {
    #[wasm_bindgen(constructor)]
    pub fn new(
        element: HtmlCanvasElement,
        width: u16,
        height: u16,
        left: u16,
        top: u16,
        dirty_width: u16,
        dirty_height: u16,
    ) -> Result<PresentationBenchmark, JsValue> {
        let mut fixture = Fixture::new(width, height);
        fixture.paint_common(rectangle(0, 0, width, height), 11);
        fixture.paint(rectangle(left, top, dirty_width, dirty_height), 12);
        Ok(Self {
            fixture,
            canvas: Canvas::new(
                element,
                NonZeroU32::new(u32::from(width)).expect("nonzero width"),
                NonZeroU32::new(u32::from(height)).expect("nonzero height"),
            )
            .map_err(|error| JsValue::from_str(&error.to_string()))?,
            next_frame: 0,
        })
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.fixture = Fixture::new(width, height);
        self.next_frame = 0;
        self.canvas.resize(
            NonZeroU32::new(u32::from(width)).expect("nonzero width"),
            NonZeroU32::new(u32::from(height)).expect("nonzero height"),
        );
    }

    pub fn clear_source(&mut self) {
        self.fixture = Fixture::new(self.fixture.frames[0].width(), self.fixture.frames[0].height());
        self.next_frame = 0;
    }

    pub fn paint(&mut self, left: u16, top: u16, width: u16, height: u16, seed: u32) {
        self.fixture.paint(rectangle(left, top, width, height), seed);
    }

    pub fn copy_batch(&mut self, left: u16, top: u16, width: u16, height: u16, iterations: u32) -> usize {
        let region = rectangle(left, top, width, height);
        for iteration in 0..iterations {
            self.fixture.buffer.clear();
            let selected = image::extract_partial_image(
                black_box(&self.fixture.frames[iteration as usize % 2]),
                black_box(region.clone()),
                &mut self.fixture.buffer,
            );
            black_box(selected);
            black_box(self.fixture.buffer.filled());
        }
        self.fixture.buffer.filled_len()
    }

    pub fn draw_batch(
        &mut self,
        left: u16,
        top: u16,
        width: u16,
        height: u16,
        iterations: u32,
    ) -> Result<usize, JsValue> {
        let region = rectangle(left, top, width, height);
        for iteration in 0..iterations {
            self.fixture.buffer.clear();
            #[cfg(presentation_direct)]
            self.canvas
                .draw_image(
                    black_box(&self.fixture.frames[(self.next_frame + iteration as usize) % 2]),
                    black_box(region.clone()),
                    &mut self.fixture.buffer,
                )
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            #[cfg(not(presentation_direct))]
            {
                let selected = image::extract_partial_image(
                    black_box(&self.fixture.frames[(self.next_frame + iteration as usize) % 2]),
                    black_box(region.clone()),
                    &mut self.fixture.buffer,
                );
                self.canvas
                    .draw(self.fixture.buffer.filled_mut(), selected)
                    .map_err(|error| JsValue::from_str(&error.to_string()))?;
            }
        }
        self.next_frame = (self.next_frame + iterations as usize) % 2;
        Ok(self.fixture.buffer.filled_len())
    }

    pub fn expected_pixels(&self, frame_index: usize) -> Vec<u8> {
        let mut pixels = self.source_pixels(frame_index);
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        pixels
    }

    pub fn source_pixels(&self, frame_index: usize) -> Vec<u8> {
        self.fixture.frames[frame_index].data().to_vec()
    }
}

#[wasm_bindgen]
pub fn probe_alpha(mut bytes: Vec<u8>, offset: usize, len: usize) -> Vec<u8> {
    let buffer = &mut bytes[offset..offset + len];
    #[cfg(any(presentation_simd, presentation_direct))]
    super::canvas::make_opaque(buffer);
    #[cfg(not(any(presentation_simd, presentation_direct)))]
    for pixel in buffer.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    bytes
}

#[wasm_bindgen]
pub fn probe_copy(
    source: Vec<u8>,
    mut target: Vec<u8>,
    source_offset: usize,
    target_offset: usize,
    len: usize,
) -> Vec<u8> {
    let original = source.clone();
    let src = &source[source_offset..source_offset + len];
    let dst = &mut target[target_offset..target_offset + len];
    #[cfg(presentation_fused)]
    super::image::copy_opaque_rgba(src, dst);
    #[cfg(presentation_candidate)]
    super::canvas::copy_opaque_rgba(src, dst);
    #[cfg(not(any(presentation_fused, presentation_candidate)))]
    {
        dst.copy_from_slice(src);
        #[cfg(presentation_direct)]
        super::canvas::make_opaque(dst);
        #[cfg(not(presentation_direct))]
        for pixel in dst.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }
    assert_eq!(source, original, "copy must preserve the framebuffer");
    target
}
