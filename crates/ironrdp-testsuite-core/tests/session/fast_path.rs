//! Regression tests for FastPath graphics updates and frame acknowledgements.
//!
//! A FastPath `Processor` always owns a bulk decompressor: the library builds
//! one in `ProcessorBuilder::build`. A server can send a compressed FastPath
//! update (for example a full-frame redraw after a resize) regardless of what
//! the client advertised, so the decompressor must always be present. These
//! tests pin that a compressed update decodes to exactly the same pixels as its
//! uncompressed twin rather than being dropped or aborting the session.

use ironrdp_bulk::{BulkCompressor, CompressionType as BulkCompressionType, flags as bulk_flags};
use ironrdp_core::{ReadCursor, WriteBuf, decode, decode_cursor, encode_vec};
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_pdu::bitmap::{BitmapData, BitmapUpdateData, Compression};
use ironrdp_pdu::fast_path::{
    EncryptionFlags, FastPathHeader, FastPathUpdate, FastPathUpdatePdu, Fragmentation, UpdateCode,
};
use ironrdp_pdu::geometry::{ExclusiveRectangle, InclusiveRectangle};
use ironrdp_pdu::mcs::McsMessage;
use ironrdp_pdu::rdp::client_info::CompressionType as PduCompressionType;
use ironrdp_pdu::rdp::headers::{CompressionFlags, ShareControlHeader, ShareControlPdu, ShareDataPdu};
use ironrdp_pdu::surface_commands::{
    ExtendedBitmapDataPdu, FrameAction, FrameMarkerPdu, SurfaceBitsPdu, SurfaceCommand,
};
use ironrdp_pdu::x224::X224;
use ironrdp_session::fast_path::{Processor, ProcessorBuilder, UpdateKind};
use ironrdp_session::image::DecodedImage;

const IMAGE_DIM: u16 = 64;
const RECT_DIM: u16 = 8;
const USER_CHANNEL_ID: u16 = 1002;
const IO_CHANNEL_ID: u16 = 1003;
const SHARE_ID: u32 = 0x0001_0000;

fn processor() -> Processor {
    ProcessorBuilder {
        io_channel_id: IO_CHANNEL_ID,
        user_channel_id: USER_CHANNEL_ID,
        share_id: SHARE_ID,
        enable_server_pointer: false,
        pointer_software_rendering: false,
    }
    .build()
}

/// Encodes the inner FastPath Bitmap update: a single uncompressed 8x8 32bpp
/// rectangle filled with a repetitive (and therefore compressible) pattern.
fn bitmap_update_payload() -> Vec<u8> {
    let pixels: Vec<u8> = (0..RECT_DIM * RECT_DIM)
        .flat_map(|_| [0x11u8, 0x22, 0x33, 0xff])
        .collect();

    let update = BitmapUpdateData {
        rectangles: vec![BitmapData {
            rectangle: InclusiveRectangle {
                left: 0,
                top: 0,
                right: RECT_DIM - 1,
                bottom: RECT_DIM - 1,
            },
            width: RECT_DIM,
            height: RECT_DIM,
            bits_per_pixel: 32,
            compression_flags: Compression::empty(),
            compressed_data_header: None,
            bitmap_data: &pixels,
        }],
    };

    encode_vec(&update).expect("encode bitmap update")
}

/// Wraps an encoded `FastPathUpdatePdu` in a FastPath output frame (the
/// `FastPathHeader` that `Processor::process` expects to read first).
fn fastpath_frame(update_pdu: &[u8]) -> Vec<u8> {
    let header = FastPathHeader::new(EncryptionFlags::empty(), update_pdu.len());
    let mut frame = encode_vec(&header).expect("encode FastPath header");
    frame.extend_from_slice(update_pdu);
    frame
}

fn surface_commands_frame(commands: Vec<SurfaceCommand<'_>>) -> Vec<u8> {
    let payload = encode_vec(&FastPathUpdate::SurfaceCommands(commands)).expect("encode surface commands");
    fastpath_frame(
        &encode_vec(&FastPathUpdatePdu {
            fragmentation: Fragmentation::Single,
            update_code: UpdateCode::SurfaceCommands,
            compression_flags: None,
            compression_type: None,
            data: &payload,
        })
        .expect("encode surface commands update"),
    )
}

fn assert_frame_ack(output: &[u8], frame_id: u32) {
    let mut cursor = ReadCursor::new(output);
    let X224(McsMessage::SendDataRequest(request)) = decode_cursor(&mut cursor).expect("decode acknowledgement") else {
        panic!("expected a frame acknowledgement in an MCS send data request");
    };
    assert!(cursor.is_empty(), "expected exactly one acknowledgement");
    assert_eq!(request.initiator_id, USER_CHANNEL_ID);
    assert_eq!(request.channel_id, IO_CHANNEL_ID);

    let header: ShareControlHeader = decode(&request.user_data).expect("decode acknowledgement share header");
    assert_eq!(header.share_id, SHARE_ID);
    let ShareControlPdu::Data(data) = header.share_control_pdu else {
        panic!("expected a share data PDU");
    };
    let ShareDataPdu::FrameAcknowledge(ack) = data.share_data_pdu else {
        panic!("expected a frame acknowledgement PDU");
    };
    assert_eq!(ack.frame_id, frame_id);
}

/// Runs one FastPath frame through a fresh processor and returns the resulting
/// framebuffer.
fn render(frame: &[u8]) -> DecodedImage {
    let mut image = DecodedImage::new(PixelFormat::RgbA32, IMAGE_DIM, IMAGE_DIM);
    let mut output = WriteBuf::new();
    let mut bulk_decompressor = Some(BulkCompressor::new(BulkCompressionType::Rdp5));
    processor()
        .process(&mut image, frame, &mut output, &mut bulk_decompressor)
        .expect("process FastPath frame");
    image
}

#[test]
fn compressed_fastpath_update_decompresses_like_its_uncompressed_twin() {
    let payload = bitmap_update_payload();

    let uncompressed = fastpath_frame(
        &encode_vec(&FastPathUpdatePdu {
            fragmentation: Fragmentation::Single,
            update_code: UpdateCode::Bitmap,
            compression_flags: None,
            compression_type: None,
            data: &payload,
        })
        .expect("encode uncompressed FastPath update"),
    );

    // Bulk-compress the same payload (RDP5 / MPPC) and wrap it as a compressed
    // FastPath update.
    let mut compressor = BulkCompressor::new(BulkCompressionType::Rdp5);
    let (size, flags) = compressor.compress(&payload).expect("bulk compress payload");
    assert_ne!(
        flags & bulk_flags::PACKET_COMPRESSED,
        0,
        "test payload must actually compress; adjust it if MPPC declines to compress"
    );
    let compressed_data = compressor.compressed_data(size).to_vec();

    let compressed_pdu = encode_vec(&FastPathUpdatePdu {
        fragmentation: Fragmentation::Single,
        update_code: UpdateCode::Bitmap,
        // Carry the compressor's control bits (notably PACKET_AT_FRONT) so the
        // decompressor resets its history at the start of the stream.
        compression_flags: Some(CompressionFlags::from_bits_retain(
            u8::try_from(flags & 0xe0).expect("control-flag byte fits in u8"),
        )),
        compression_type: Some(PduCompressionType::K64),
        data: &compressed_data,
    })
    .expect("encode compressed FastPath update");
    let compressed = fastpath_frame(&compressed_pdu);

    let from_uncompressed = render(&uncompressed);
    let from_compressed = render(&compressed);

    assert_eq!(
        from_uncompressed.data(),
        from_compressed.data(),
        "compressed FastPath update rendered differently from its uncompressed twin"
    );

    // Sanity check: the update actually drew something, so the equality above
    // compares real pixels rather than two blank frames.
    let blank = DecodedImage::new(PixelFormat::RgbA32, IMAGE_DIM, IMAGE_DIM);
    assert_ne!(
        from_uncompressed.data(),
        blank.data(),
        "the bitmap update should have modified the framebuffer"
    );
}

#[test]
fn fragmented_compressed_fastpath_update_decompresses_before_reassembly() {
    let payload = bitmap_update_payload();
    let split = payload.len() / 2;
    let mut sender = BulkCompressor::new(BulkCompressionType::Rdp5);

    let (first_size, first_flags) = sender.compress(&payload[..split]).expect("compress first fragment");
    assert_ne!(
        first_flags & bulk_flags::PACKET_COMPRESSED,
        0,
        "first fragment must compress"
    );
    let first_data = sender.compressed_data(first_size).to_vec();
    let first = fastpath_frame(
        &encode_vec(&FastPathUpdatePdu {
            fragmentation: Fragmentation::First,
            update_code: UpdateCode::Bitmap,
            compression_flags: Some(CompressionFlags::from_bits_retain(
                u8::try_from(first_flags & 0xe0).expect("control-flag byte fits in u8"),
            )),
            compression_type: Some(PduCompressionType::K64),
            data: &first_data,
        })
        .expect("encode first FastPath fragment"),
    );

    let (last_size, last_flags) = sender.compress(&payload[split..]).expect("compress last fragment");
    assert_ne!(
        last_flags & bulk_flags::PACKET_COMPRESSED,
        0,
        "last fragment must compress"
    );
    let last_data = sender.compressed_data(last_size).to_vec();
    let last = fastpath_frame(
        &encode_vec(&FastPathUpdatePdu {
            fragmentation: Fragmentation::Last,
            update_code: UpdateCode::Bitmap,
            compression_flags: Some(CompressionFlags::from_bits_retain(
                u8::try_from(last_flags & 0xe0).expect("control-flag byte fits in u8"),
            )),
            compression_type: Some(PduCompressionType::K64),
            data: &last_data,
        })
        .expect("encode last FastPath fragment"),
    );

    let uncompressed = fastpath_frame(
        &encode_vec(&FastPathUpdatePdu {
            fragmentation: Fragmentation::Single,
            update_code: UpdateCode::Bitmap,
            compression_flags: None,
            compression_type: None,
            data: &payload,
        })
        .expect("encode uncompressed FastPath update"),
    );
    let expected = render(&uncompressed);

    let mut image = DecodedImage::new(PixelFormat::RgbA32, IMAGE_DIM, IMAGE_DIM);
    let mut output = WriteBuf::new();
    let mut bulk_decompressor = Some(BulkCompressor::new(BulkCompressionType::Rdp5));
    let mut processor = processor();

    assert!(
        processor
            .process(&mut image, &first, &mut output, &mut bulk_decompressor)
            .expect("process first FastPath fragment")
            .is_empty()
    );
    assert!(
        !processor
            .process(&mut image, &last, &mut output, &mut bulk_decompressor)
            .expect("process last FastPath fragment")
            .is_empty()
    );
    assert_eq!(image.data(), expected.data());
}

#[test]
fn frame_markers_do_not_report_dirty_pixels_and_still_acknowledge_frames() {
    let frame_id = 0x1234_5678;
    for actions in [
        vec![FrameAction::Begin],
        vec![FrameAction::End],
        vec![FrameAction::Begin, FrameAction::End],
    ] {
        let expects_ack = actions.contains(&FrameAction::End);
        let frame = surface_commands_frame(
            actions
                .into_iter()
                .map(|frame_action| {
                    SurfaceCommand::FrameMarker(FrameMarkerPdu {
                        frame_action,
                        frame_id: Some(frame_id),
                    })
                })
                .collect(),
        );
        let mut image = DecodedImage::new(PixelFormat::RgbA32, IMAGE_DIM, IMAGE_DIM);
        let before = image.data().to_vec();
        let mut output = WriteBuf::new();
        let updates = processor()
            .process(&mut image, &frame, &mut output, &mut None)
            .expect("process frame markers");

        assert!(updates.is_empty(), "frame markers alone must not report dirty pixels");
        assert_eq!(image.data(), before);
        if expects_ack {
            assert_frame_ack(output.filled(), frame_id);
        } else {
            assert!(output.filled().is_empty(), "begin markers must not acknowledge frames");
        }
    }
}

#[test]
fn frame_markers_preserve_a_real_top_left_single_pixel_surface_update() {
    let frame_id = 42;
    let bits = SurfaceBitsPdu {
        destination: ExclusiveRectangle {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        },
        extended_bitmap_data: ExtendedBitmapDataPdu {
            bpp: 32,
            codec_id: 0,
            width: 1,
            height: 1,
            header: None,
            data: &[0x33, 0x22, 0x11, 0xff],
        },
    };
    for surface_bits in [
        SurfaceCommand::SetSurfaceBits(bits.clone()),
        SurfaceCommand::StreamSurfaceBits(bits),
    ] {
        let frame = surface_commands_frame(vec![
            SurfaceCommand::FrameMarker(FrameMarkerPdu {
                frame_action: FrameAction::Begin,
                frame_id: Some(frame_id),
            }),
            surface_bits,
            SurfaceCommand::FrameMarker(FrameMarkerPdu {
                frame_action: FrameAction::End,
                frame_id: Some(frame_id),
            }),
        ]);
        let mut image = DecodedImage::new(PixelFormat::RgbA32, IMAGE_DIM, IMAGE_DIM);
        let mut expected = image.data().to_vec();
        expected[..4].copy_from_slice(&[0x11, 0x22, 0x33, 0xff]);
        let mut output = WriteBuf::new();
        let updates = processor()
            .process(&mut image, &frame, &mut output, &mut None)
            .expect("process marked single-pixel surface update");

        assert!(matches!(
            updates.as_slice(),
            [UpdateKind::Region(InclusiveRectangle {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            })]
        ));
        assert_eq!(image.data(), expected);
        assert_frame_ack(output.filled(), frame_id);
    }
}
