//! Synthetic notification and ready-queue workloads using production presentation helpers.
use core::num::NonZeroU32;
use core::time::Duration;

use futures_util::future::{Fuse, FusedFuture, ready};
use futures_util::{FutureExt as _, select};
use gloo_timers::future::TimeoutFuture;
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::pdu::bitmap::{BitmapData, BitmapUpdateData, Compression};
use ironrdp::pdu::geometry::{InclusiveRectangle, Rectangle as _};
use ironrdp::session::fast_path::{Processor, ProcessorBuilder};
use ironrdp::session::image::DecodedImage;
use ironrdp_core::WriteBuf;
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use super::browser_yield::BrowserYield;
use super::canvas::Canvas;
use super::frame_pacing::FramePacing;
use super::pending_regions::{PendingRegions, benchmark_queue_len};
use super::{Fixture, image, rectangle};

/// Exercises production wakeups plus cancellation, reuse, and dropping a pending channel.
#[wasm_bindgen]
pub async fn probe_browser_yield(count: u32) -> Result<u32, JsValue> {
    let error = |error: anyhow::Error| JsValue::from_str(&error.to_string());
    let mut browser_yield = BrowserYield::new().map_err(error)?;
    for _ in 0..count {
        browser_yield.yield_now().await.map_err(error)?;
    }
    if browser_yield.yield_now().now_or_never().is_some() {
        return Err(JsValue::from_str(
            "a cooperative yield completed without a browser task",
        ));
    }
    browser_yield.yield_now().await.map_err(error)?;
    {
        let mut dropped = BrowserYield::new().map_err(error)?;
        if dropped.yield_now().now_or_never().is_some() {
            return Err(JsValue::from_str("drop probe did not leave a pending yield"));
        }
    }
    // Give queued message tasks another opportunity to expose an invalid dropped callback.
    browser_yield.yield_now().await.map_err(error)?;
    Ok(count)
}

#[derive(Default)]
struct Counts {
    updates: u32,
    draws: u32,
    bytes: u64,
    batches: u32,
    forced: u32,
    max_queue: usize,
    terminal_flushes: u32,
}

impl Counts {
    fn values(&self) -> Vec<f64> {
        vec![
            f64::from(self.updates),
            f64::from(self.draws),
            self.bytes as f64,
            f64::from(self.batches),
            f64::from(self.forced),
            self.max_queue as f64,
            f64::from(self.terminal_flushes),
        ]
    }
}

fn draw(canvas: &Canvas, frame: &DecodedImage, buffer: &mut WriteBuf, counts: &mut Counts, region: InclusiveRectangle) {
    buffer.clear();
    canvas.draw_image(frame, region, buffer).expect("draw synthetic update");
    counts.draws += 1;
    counts.bytes += buffer.filled_len() as u64;
}

fn flush(
    canvas: &Canvas,
    frame: &DecodedImage,
    buffer: &mut WriteBuf,
    pending: &mut PendingRegions,
    counts: &mut Counts,
) {
    if pending.is_empty() {
        return;
    }
    counts.batches += 1;
    for region in pending.drain() {
        draw(canvas, frame, buffer, counts, region);
    }
}

fn packet_regions(name: &str, width: u16, height: u16) -> Vec<Option<InclusiveRectangle>> {
    assert!(320 <= width && 240 <= height);
    let full = rectangle(0, 0, width, height);
    match name {
        "single" => vec![Some(rectangle(17, 19, 64, 128))],
        "repeat" => vec![Some(full); 8],
        "contained" => vec![
            Some(rectangle(17, 19, 32, 32)),
            Some(rectangle(9, 11, 80, 80)),
            Some(rectangle(5, 7, 128, 128)),
            Some(rectangle(17, 19, 32, 32)),
        ],
        "overlap" => vec![Some(rectangle(7, 9, 128, 96)), Some(rectangle(71, 41, 128, 96))],
        "disjoint" | "overflow" => (0..if name == "overflow" { 65 } else { 16 })
            .map(|index| Some(rectangle(5 + index % 10 * 24, 7 + index / 10 * 24, 16, 16)))
            .collect(),
        "barrier" => vec![Some(full.clone()), Some(full.clone()), None, Some(full)],
        _ => panic!("unknown packet fixture"),
    }
}

fn packet_fixture(width: u16, height: u16, regions: &[Option<InclusiveRectangle>], sparse: bool) -> Fixture {
    let mut fixture = Fixture::new(width, height);
    if !sparse {
        fixture.paint_common(rectangle(0, 0, width, height), 11);
    }
    for (index, region) in regions.iter().flatten().enumerate() {
        fixture.paint(region.clone(), 13 + index as u32);
    }
    fixture
}

#[wasm_bindgen]
pub struct PacketBenchmark {
    canvas: Canvas,
    fixture: Fixture,
    regions: Vec<Option<InclusiveRectangle>>,
    pending: PendingRegions,
    counts: Counts,
    next_frame: usize,
    sparse: bool,
}

#[wasm_bindgen]
impl PacketBenchmark {
    #[wasm_bindgen(constructor)]
    pub fn new(element: HtmlCanvasElement, name: &str, width: u16, height: u16, sparse: bool) -> Self {
        let regions = packet_regions(name, width, height);
        let fixture = packet_fixture(width, height, &regions, sparse);
        let canvas = Canvas::new(
            element,
            NonZeroU32::new(u32::from(width)).expect("nonzero width"),
            NonZeroU32::new(u32::from(height)).expect("nonzero height"),
        )
        .expect("canvas");
        let mut result = Self {
            canvas,
            fixture,
            regions,
            pending: PendingRegions::default(),
            counts: Counts::default(),
            next_frame: 0,
            sparse,
        };
        if !sparse {
            result
                .canvas
                .draw_image(
                    &result.fixture.frames[0],
                    rectangle(0, 0, width, height),
                    &mut result.fixture.buffer,
                )
                .expect("initial full repaint");
        }
        result
    }

    /// Simulates already-applied packet output notifications, alternating prebuilt framebuffers.
    pub fn draw_batch(&mut self, coalesce: bool, iterations: u32) {
        for _ in 0..iterations {
            let frame = &self.fixture.frames[self.next_frame];
            for region in &self.regions {
                if let Some(region) = region {
                    self.counts.updates += 1;
                    if coalesce {
                        if let Some(overflow) = self.pending.push(region.clone()) {
                            self.counts.forced += 1;
                            flush(
                                &self.canvas,
                                frame,
                                &mut self.fixture.buffer,
                                &mut self.pending,
                                &mut self.counts,
                            );
                            // Packet coalescing preserves its flush-before-control/overflow ordering.
                            draw(
                                &self.canvas,
                                frame,
                                &mut self.fixture.buffer,
                                &mut self.counts,
                                overflow,
                            );
                        }
                        self.counts.max_queue = self.counts.max_queue.max(benchmark_queue_len(&self.pending));
                    } else {
                        draw(
                            &self.canvas,
                            frame,
                            &mut self.fixture.buffer,
                            &mut self.counts,
                            region.clone(),
                        );
                    }
                } else if coalesce {
                    flush(
                        &self.canvas,
                        frame,
                        &mut self.fixture.buffer,
                        &mut self.pending,
                        &mut self.counts,
                    );
                }
            }
            if coalesce {
                flush(
                    &self.canvas,
                    frame,
                    &mut self.fixture.buffer,
                    &mut self.pending,
                    &mut self.counts,
                );
            }
            self.next_frame ^= 1;
        }
    }

    pub fn stats(&self) -> Vec<f64> {
        self.counts.values()
    }

    pub fn clear_stats(&mut self) {
        self.counts = Counts::default();
    }

    pub fn source_pixels(&self, index: usize) -> Vec<u8> {
        self.fixture.frames[index].data().to_vec()
    }

    pub fn expected_pixels(&self) -> Vec<u8> {
        let frame = &self.fixture.frames[self.next_frame ^ 1];
        let mut pixels = frame.data().to_vec();
        if !self.sparse {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        } else {
            let mut buffer = WriteBuf::new();
            for region in self.regions.iter().flatten() {
                buffer.clear();
                let selected = image::extract_partial_image(frame, region.clone(), &mut buffer);
                for y in selected.top..=selected.bottom {
                    for x in selected.left..=selected.right {
                        pixels[(usize::from(y) * usize::from(frame.width()) + usize::from(x)) * 4 + 3] = 255;
                    }
                }
            }
        }
        pixels
    }

    pub fn resize(&mut self, name: &str, width: u16, height: u16) {
        self.canvas.resize(
            NonZeroU32::new(u32::from(width)).expect("nonzero width"),
            NonZeroU32::new(u32::from(height)).expect("nonzero height"),
        );
        self.regions = packet_regions(name, width, height);
        self.fixture = packet_fixture(width, height, &self.regions, true);
        self.pending.drain().for_each(drop);
        self.counts = Counts::default();
        self.next_frame = 0;
        self.sparse = true;
    }
}

fn processor() -> Processor {
    ProcessorBuilder {
        io_channel_id: 1003,
        user_channel_id: 1001,
        share_id: 1,
        enable_server_pointer: false,
        pointer_software_rendering: false,
    }
    .build()
}

fn apply(processor: &mut Processor, image: &mut DecodedImage, region: InclusiveRectangle, payload: &[u8]) {
    processor
        .process_bitmap_update(
            image,
            BitmapUpdateData {
                rectangles: vec![BitmapData {
                    width: region.width(),
                    height: region.height(),
                    rectangle: region,
                    bits_per_pixel: 32,
                    compression_flags: Compression::empty(),
                    compressed_data_header: None,
                    bitmap_data: payload,
                }],
            },
        )
        .expect("apply synthetic bitmap payload");
}

#[wasm_bindgen]
pub struct ReadyQueueBenchmark {
    canvas: Canvas,
    image: DecodedImage,
    processor: Processor,
    buffer: WriteBuf,
    regions: Vec<InclusiveRectangle>,
    payloads: [Vec<u8>; 2],
    expected: Vec<u8>,
    pending: PendingRegions,
    pacing: FramePacing,
    performance: web_sys::Performance,
    counts: Counts,
    total: u32,
}

#[wasm_bindgen]
impl ReadyQueueBenchmark {
    #[wasm_bindgen(constructor)]
    pub fn new(element: HtmlCanvasElement, mode: &str, width: u16, height: u16, total: u32, fps: u32) -> Self {
        assert!(total > 0 && fps > 0);
        let regions: Vec<InclusiveRectangle> = match mode {
            "partial" => packet_regions("overflow", width, height)
                .into_iter()
                .flatten()
                .collect(),
            "full" => vec![rectangle(0, 0, width, height)],
            "narrow" => {
                assert!(width >= 81 && height >= 739);
                vec![rectangle(17, 19, 64, 720)]
            }
            _ => panic!("unknown ready-queue fixture"),
        };
        let bytes = usize::from(regions[0].width()) * usize::from(regions[0].height()) * 4;
        let payloads = [19_u8, 97].map(|seed| {
            (0..bytes)
                .map(|index| {
                    if index % 4 == 3 {
                        0
                    } else {
                        (index as u8).wrapping_mul(37).wrapping_add(seed)
                    }
                })
                .collect::<Vec<_>>()
        });
        let mut reference = DecodedImage::new(PixelFormat::RgbA32, width, height);
        let mut reference_processor = processor();
        for index in 0..total as usize {
            apply(
                &mut reference_processor,
                &mut reference,
                regions[index % regions.len()].clone(),
                &payloads[index % 2],
            );
        }
        Self {
            canvas: Canvas::new(
                element,
                NonZeroU32::new(u32::from(width)).expect("width"),
                NonZeroU32::new(u32::from(height)).expect("height"),
            )
            .expect("canvas"),
            image: DecodedImage::new(PixelFormat::RgbA32, width, height),
            processor: processor(),
            buffer: WriteBuf::new(),
            regions,
            payloads,
            expected: reference.data().to_vec(),
            pending: PendingRegions::default(),
            pacing: FramePacing::new(Duration::from_nanos(1_000_000_000_u64.div_ceil(u64::from(fps)))),
            performance: web_sys::window()
                .expect("window")
                .performance()
                .expect("performance clock"),
            counts: Counts::default(),
            total,
        }
    }

    /// Applies exactly one prebuilt synthetic bitmap update; all input records start ready.
    pub fn step(&mut self, paced: bool) -> u32 {
        assert!(self.counts.updates < self.total);
        let before = self.counts.draws;
        let index = self.counts.updates as usize;
        let region = self.regions[index % self.regions.len()].clone();
        apply(
            &mut self.processor,
            &mut self.image,
            region.clone(),
            &self.payloads[index % 2],
        );
        self.counts.updates += 1;
        if paced {
            if let Some(region) = self.pending.push(region) {
                self.counts.forced += 1;
                self.pacing.presented(self.performance.now());
                flush(
                    &self.canvas,
                    &self.image,
                    &mut self.buffer,
                    &mut self.pending,
                    &mut self.counts,
                );
                assert!(self.pending.push(region).is_none());
            }
            self.counts.max_queue = self.counts.max_queue.max(benchmark_queue_len(&self.pending));
            // A forced batch may take longer than an interval; check again after rendering.
            self.flush_due(self.performance.now());
        } else {
            draw(&self.canvas, &self.image, &mut self.buffer, &mut self.counts, region);
            self.counts.batches += 1;
        }
        self.counts.draws - before
    }

    /// Drives an already-ready synthetic queue entirely in Rust, using production yield and timer primitives.
    pub async fn run(&mut self, paced: bool) -> Result<Vec<f64>, JsValue> {
        assert_eq!(self.counts.updates, 0, "run requires a fresh or reset workload");
        let error = |error: anyhow::Error| JsValue::from_str(&error.to_string());
        let mut browser_yield = BrowserYield::new().map_err(error)?;
        let mut presentation_timer = Fuse::<TimeoutFuture>::terminated();
        let mut first_draw = None;
        let mut cooperative_yields = 0_u32;
        let mut timer_callbacks = 0_u32;
        let mut work_ms = 0.0;
        let started = self.performance.now();
        let mut active_ms = 0.0;

        let outcome: Result<(), JsValue> = async {
            while self.counts.updates < self.total {
                let work_started;
                let drawn = select! {
                    _ = ready(()).fuse() => {
                        work_started = self.performance.now();
                        self.step(paced)
                    }
                    _ = presentation_timer => {
                        work_started = self.performance.now();
                        timer_callbacks += 1;
                        self.flush_due(self.performance.now())
                    }
                };
                if drawn != 0 {
                    // Production cancels even a ready timer after every presentation.
                    presentation_timer = Fuse::terminated();
                    if first_draw.is_none() {
                        first_draw = Some((self.performance.now() - started, self.counts.updates));
                    }
                }
                if paced {
                    if let Some(delay) = self.pending_delay_ms(self.performance.now()) {
                        if delay == 0 {
                            presentation_timer = Fuse::terminated();
                            if self.flush_due(self.performance.now()) != 0 && first_draw.is_none() {
                                first_draw = Some((self.performance.now() - started, self.counts.updates));
                            }
                        } else if presentation_timer.is_terminated() {
                            presentation_timer = TimeoutFuture::new(delay).fuse();
                        }
                    }
                    // Both update work and timer-driven presentation count; browser waits do not.
                    let elapsed = self.performance.now() - work_started;
                    active_ms += elapsed;
                    work_ms += elapsed;
                    if work_ms >= 8.0 {
                        browser_yield.yield_now().await.map_err(error)?;
                        cooperative_yields += 1;
                        work_ms = 0.0;
                    }
                }
            }
            Ok(())
        }
        .await;
        let applied_at = self.performance.now();
        // Drop cancels its JS timeout, including on an error from the cooperative yield.
        drop(presentation_timer);
        if self.finish(self.performance.now()) != 0 && first_draw.is_none() {
            first_draw = Some((self.performance.now() - started, self.counts.updates));
        }
        let finished = self.performance.now();
        outcome?;
        let (first_draw_ms, first_draw_update) = first_draw.expect("nonempty workload presents its first update");
        Ok(vec![
            applied_at - started,
            finished - started,
            first_draw_ms,
            f64::from(first_draw_update),
            f64::from(cooperative_yields),
            f64::from(timer_callbacks),
            if paced {
                active_ms + finished - applied_at
            } else {
                finished - started
            },
        ])
    }

    pub fn pending_delay_ms(&self, now_ms: f64) -> Option<u32> {
        (!self.pending.is_empty()).then(|| self.pacing.delay_ms(now_ms))
    }

    pub fn flush_due(&mut self, now_ms: f64) -> u32 {
        let before = self.counts.draws;
        if !self.pending.is_empty() && self.pacing.delay_ms(now_ms) == 0 {
            self.pacing.presented(now_ms);
            flush(
                &self.canvas,
                &self.image,
                &mut self.buffer,
                &mut self.pending,
                &mut self.counts,
            );
        }
        self.counts.draws - before
    }

    /// Terminal flush preserves the newest pixels even before their normal presentation deadline.
    pub fn finish(&mut self, now_ms: f64) -> u32 {
        let before = self.counts.draws;
        if !self.pending.is_empty() {
            self.counts.terminal_flushes += 1;
            self.pacing.presented(now_ms);
            flush(
                &self.canvas,
                &self.image,
                &mut self.buffer,
                &mut self.pending,
                &mut self.counts,
            );
        }
        self.counts.draws - before
    }

    pub fn stats(&self) -> Vec<f64> {
        self.counts.values()
    }

    pub fn expected_pixels(&self) -> Vec<u8> {
        self.expected.clone()
    }

    pub fn source_pixels(&self) -> Vec<u8> {
        self.image.data().to_vec()
    }

    /// Replaces the framebuffer and clears the Canvas backing store at its current dimensions.
    pub fn restart_after_resize(&mut self) {
        let width = self.image.width();
        let height = self.image.height();
        self.canvas.resize(
            NonZeroU32::new(u32::from(width)).expect("width"),
            NonZeroU32::new(u32::from(height)).expect("height"),
        );
        self.image = DecodedImage::new(PixelFormat::RgbA32, width, height);
        self.pending.drain().for_each(drop);
        self.pacing.reset();
        self.counts = Counts::default();
    }
}
