//! Opt-in browser fixtures for the real session loop; never included in normal builds.

#![expect(
    unreachable_pub,
    reason = "wasm-bindgen requires public functions inside this private fixture module"
)]

use core::cell::RefCell;
use std::borrow::Cow;

use futures_util::future::{AbortHandle, abortable};
use iron_remote_desktop::{IronError as _, Session as _, SessionTerminationInfo as _};
use ironrdp::connector::connection_activation::ConnectionActivationFactory;
use ironrdp::pdu::bitmap::{BitmapData, BitmapUpdateData, Compression};
use ironrdp::pdu::fast_path::{EncryptionFlags, FastPathHeader, FastPathUpdatePdu, Fragmentation, UpdateCode};
use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::pdu::mcs::{DisconnectProviderUltimatum, DisconnectReason, McsMessage, SendDataIndication};
use ironrdp::pdu::pointer::{ColorPointerAttribute, Point16, PointerAttribute};
use ironrdp::pdu::rdp::capability_sets::{Bitmap, BitmapDrawingFlags, CapabilitySet, DemandActive, ServerDemandActive};
use ironrdp::pdu::rdp::finalization_messages::FontPdu;
use ironrdp::pdu::rdp::headers::{
    CompressionFlags, ServerDeactivateAll, ShareControlHeader, ShareControlPdu, ShareDataHeader, ShareDataPdu,
    StreamPriority,
};
use ironrdp::pdu::x224::X224;
use ironrdp_core::encode_vec;
use wasm_bindgen::prelude::*;

use super::{SecurityConfig, Session, build_config, connector, mpsc, writer_task};

const USER_CHANNEL_ID: u16 = 1002;
const IO_CHANNEL_ID: u16 = 1003;
const SHARE_ID: u32 = 0x0001_0000;

#[wasm_bindgen]
pub struct SessionFixture {
    session: Session,
    abort: RefCell<Option<AbortHandle>>,
}

#[wasm_bindgen]
impl SessionFixture {
    /// Supplies an already-established connection. The browser provides scripted WebSocket events;
    /// framing, decoding, presentation, input dispatch and session cleanup are production code.
    #[wasm_bindgen(constructor)]
    pub fn new(
        canvas: web_sys::HtmlCanvasElement,
        url: &str,
        width: u16,
        height: u16,
        cursor_callback: Option<js_sys::Function>,
    ) -> Result<Self, JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("fixture dimensions must be nonzero"));
        }
        let config = build_config(
            String::new(),
            String::new(),
            None,
            "session-fixture".to_owned(),
            iron_remote_desktop::DesktopSize { width, height },
            true,
            SecurityConfig {
                enable_credssp: false,
                enable_standard_rdp_security: false,
            },
        );
        let desktop_size = connector::DesktopSize { width, height };
        let connection_result = connector::ConnectionResult {
            io_channel_id: IO_CHANNEL_ID,
            user_channel_id: USER_CHANNEL_ID,
            message_channel_id: None,
            share_id: SHARE_ID,
            static_channels: Default::default(),
            desktop_size,
            input_flags: ironrdp::pdu::rdp::capability_sets::InputFlags::empty(),
            enable_server_pointer: true,
            pointer_software_rendering: false,
            refresh_rect_support: false,
            suppress_output_support: false,
            window_support_level: None,
            monitor_layout: None,
            activation_factory: ConnectionActivationFactory::new(config, IO_CHANNEL_ID, USER_CHANNEL_ID),
            compression_type: None,
        };
        let socket = super::WebSocket::open(url).map_err(|error| JsValue::from_str(&error.to_string()))?;
        let (reader, writer) = futures_util::AsyncReadExt::split(socket);
        let (writer_tx, writer_rx) = mpsc::unbounded();
        wasm_bindgen_futures::spawn_local(writer_task(writer_rx, writer, None));
        let (input_events_tx, input_events_rx) = mpsc::unbounded();
        Ok(Self {
            session: Session {
                desktop_size,
                input_database: RefCell::new(ironrdp::input::Database::new()),
                writer_tx,
                input_events_tx,
                render_canvas: canvas,
                set_cursor_style_callback: cursor_callback.unwrap_or_else(|| js_sys::Function::new_no_args("")),
                set_cursor_style_callback_context: JsValue::NULL,
                input_events_rx: RefCell::new(Some(input_events_rx)),
                connection_result: RefCell::new(Some(connection_result)),
                rdp_reader: RefCell::new(Some(reader)),
                clipboard: RefCell::new(Some(None)),
                printer: RefCell::new(Some(None)),
            },
            abort: RefCell::new(None),
        })
    }

    pub async fn run(&self) -> Result<String, JsValue> {
        let (run, handle) = abortable(self.session.run());
        *self.abort.borrow_mut() = Some(handle);
        let outcome = run.await;
        self.abort.borrow_mut().take();
        match outcome {
            Ok(Ok(info)) => Ok(info.reason()),
            Ok(Err(error)) => Err(JsValue::from_str(&error.backtrace())),
            Err(_) => Ok("fixture aborted".to_owned()),
        }
    }

    /// Exercises the production single-run guard without replacing the first run's abort handle.
    pub async fn run_again(&self) -> Result<String, JsValue> {
        self.session
            .run()
            .await
            .map(|info| info.reason())
            .map_err(|error| JsValue::from_str(&error.backtrace()))
    }

    pub fn synchronize_lock_keys(&self) -> Result<(), JsValue> {
        self.session
            .synchronize_lock_keys(false, false, false, false)
            .map_err(|error| JsValue::from_str(&error.backtrace()))
    }

    /// Cancels the actual run future, exercising the production timer/MessageChannel Drop paths.
    pub fn abort(&self) {
        if let Some(handle) = self.abort.borrow().as_ref() {
            handle.abort();
        }
    }

    pub fn shutdown(&self) -> Result<(), JsValue> {
        self.session
            .shutdown()
            .map_err(|error| JsValue::from_str(&error.backtrace()))
    }
}

#[wasm_bindgen]
pub fn session_fixture_pacing_enabled() -> bool {
    cfg!(feature = "experimental-presentation-pacing")
}

#[wasm_bindgen]
pub fn session_fixture_simd_enabled() -> bool {
    cfg!(target_feature = "simd128")
}

fn bitmap_update(left: u16, top: u16, width: u16, height: u16, color: u32) -> Result<Vec<u8>, JsValue> {
    let right = left
        .checked_add(width)
        .and_then(|end| end.checked_sub(1))
        .ok_or_else(|| JsValue::from_str("invalid fixture width"))?;
    let bottom = top
        .checked_add(height)
        .and_then(|end| end.checked_sub(1))
        .ok_or_else(|| JsValue::from_str("invalid fixture height"))?;
    let count = usize::from(width)
        .checked_mul(usize::from(height))
        .filter(|&count| count > 0 && count <= 512 * 512)
        .ok_or_else(|| JsValue::from_str("invalid fixture pixel count"))?;
    // The wire bitmap is bottom-up BGRX. Solid colors keep the oracle independent of decoding.
    let [blue, green, red, _] = color.to_le_bytes();
    let pixels = [blue, green, red, 0].repeat(count);
    let bitmap = encode_vec(&BitmapUpdateData {
        rectangles: vec![BitmapData {
            rectangle: InclusiveRectangle {
                left,
                top,
                right,
                bottom,
            },
            width,
            height,
            bits_per_pixel: 32,
            compression_flags: Compression::empty(),
            compressed_data_header: None,
            bitmap_data: &pixels,
        }],
    })
    .map_err(|error| JsValue::from_str(&error.to_string()))?;
    encode_vec(&FastPathUpdatePdu {
        fragmentation: Fragmentation::Single,
        update_code: UpdateCode::Bitmap,
        compression_flags: None,
        compression_type: None,
        data: &bitmap,
    })
    .map_err(|error| JsValue::from_str(&error.to_string()))
}

fn fast_path(payload: &[u8]) -> Result<Vec<u8>, JsValue> {
    let mut frame = encode_vec(&FastPathHeader::new(EncryptionFlags::empty(), payload.len()))
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    frame.extend_from_slice(payload);
    Ok(frame)
}

#[wasm_bindgen]
pub fn session_fixture_bitmap(left: u16, top: u16, width: u16, height: u16, color: u32) -> Result<Vec<u8>, JsValue> {
    fast_path(&bitmap_update(left, top, width, height, color)?)
}

#[wasm_bindgen]
#[expect(clippy::arithmetic_side_effects, reason = "fixture dimensions are bounded to 1..=96")]
pub fn session_fixture_pointer(width: u16, height: u16, hotspot_x: u16, hotspot_y: u16) -> Result<Vec<u8>, JsValue> {
    if !(1..=96).contains(&width) || !(1..=96).contains(&height) || hotspot_x >= width || hotspot_y >= height {
        return Err(JsValue::from_str("invalid fixture pointer dimensions or hotspot"));
    }
    // MS-RDPBCGR New Pointer: bottom-up BGRA with 16-bit aligned mask scanlines.
    // Solid color keeps the PNG oracle independent of orientation and resampling.
    let pixels = [0x33, 0x66, 0xCC, 0xFF].repeat(usize::from(width) * usize::from(height));
    let and_mask = vec![0; usize::from(width).div_ceil(16) * 2 * usize::from(height)];
    let pointer = encode_vec(&PointerAttribute {
        xor_bpp: 32,
        color_pointer: ColorPointerAttribute {
            cache_index: 0,
            hot_spot: Point16 {
                x: hotspot_x,
                y: hotspot_y,
            },
            width,
            height,
            xor_mask: &pixels,
            and_mask: &and_mask,
        },
    })
    .map_err(|error| JsValue::from_str(&error.to_string()))?;
    let update = encode_vec(&FastPathUpdatePdu {
        fragmentation: Fragmentation::Single,
        update_code: UpdateCode::NewPointer,
        compression_flags: None,
        compression_type: None,
        data: &pointer,
    })
    .map_err(|error| JsValue::from_str(&error.to_string()))?;
    fast_path(&update)
}

#[wasm_bindgen]
pub fn session_fixture_tiles(count: u16) -> Result<Vec<u8>, JsValue> {
    if !(1..=256).contains(&count) {
        return Err(JsValue::from_str("fixture tile count must be between 1 and 256"));
    }
    let mut updates = Vec::new();
    for index in 0..count {
        let left = (index % 16).checked_mul(4).expect("bounded fixture coordinate");
        let top = (index / 16).checked_mul(4).expect("bounded fixture coordinate");
        updates.extend(bitmap_update(left, top, 1, 1, 0x00CC_5533)?);
    }
    fast_path(&updates)
}

#[wasm_bindgen]
pub fn session_fixture_invalid_frame() -> Result<Vec<u8>, JsValue> {
    // Reserved update code, with a valid outer frame and zero-length update body.
    fast_path(&[0x0F, 0, 0])
}

fn server_frame(pdu: ShareControlPdu) -> Result<Vec<u8>, JsValue> {
    let data = encode_vec(&ShareControlHeader {
        share_control_pdu: pdu,
        pdu_source: USER_CHANNEL_ID,
        share_id: SHARE_ID,
    })
    .map_err(|error| JsValue::from_str(&error.to_string()))?;
    encode_vec(&X224(McsMessage::SendDataIndication(SendDataIndication {
        initiator_id: USER_CHANNEL_ID,
        channel_id: IO_CHANNEL_ID,
        user_data: Cow::Owned(data),
    })))
    .map_err(|error| JsValue::from_str(&error.to_string()))
}

#[wasm_bindgen]
pub fn session_fixture_deactivate() -> Result<Vec<u8>, JsValue> {
    server_frame(ShareControlPdu::ServerDeactivateAll(ServerDeactivateAll))
}

#[wasm_bindgen]
pub fn session_fixture_disconnect() -> Result<Vec<u8>, JsValue> {
    encode_vec(&X224(McsMessage::DisconnectProviderUltimatum(
        DisconnectProviderUltimatum::from_reason(DisconnectReason::ProviderInitiated),
    )))
    .map_err(|error| JsValue::from_str(&error.to_string()))
}

#[wasm_bindgen]
pub fn session_fixture_activate(width: u16, height: u16) -> Result<Vec<u8>, JsValue> {
    let mut frames = server_frame(ShareControlPdu::ServerDemandActive(ServerDemandActive {
        pdu: DemandActive {
            source_descriptor: "fixture".to_owned(),
            capability_sets: vec![CapabilitySet::Bitmap(Bitmap {
                pref_bits_per_pix: 32,
                desktop_width: width,
                desktop_height: height,
                desktop_resize_flag: true,
                drawing_flags: BitmapDrawingFlags::empty(),
            })],
        },
    }))?;
    // FontMap completes activation after the client's normal ConfirmActive/finalization writes.
    frames.extend(server_frame(ShareControlPdu::Data(ShareDataHeader {
        share_data_pdu: ShareDataPdu::FontMap(FontPdu::default()),
        stream_priority: StreamPriority::Medium,
        compression_flags: CompressionFlags::empty(),
        compression_type: ironrdp::pdu::rdp::client_info::CompressionType::K8,
    }))?);
    Ok(frames)
}
