#![expect(unused_crate_dependencies, reason = "only the canvas alpha helper is under test")]

#[expect(dead_code, reason = "the browser canvas methods are not used by the pixel tests")]
#[path = "../src/canvas.rs"]
mod canvas;

#[path = "../src/image.rs"]
mod image;

use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::session::image::DecodedImage;
use ironrdp_core::WriteBuf;

#[test]
fn complete_pixels_are_opaque_without_changing_rgb_partial_pixels_or_surrounding_bytes() {
    const PATTERN: [u8; 12] = [11, 41, 95, 0, 29, 97, 183, 127, 211, 137, 55, 255];

    // Cover every SIMD alignment and tail, including empty slices and incomplete RGBA pixels.
    for length in 0..=67 {
        for offset in 0..16 {
            let mut buffer: Vec<u8> = (0..length + 32).map(|index| PATTERN[index % PATTERN.len()]).collect();
            let original = buffer.clone();

            canvas::make_opaque(&mut buffer[offset..offset + length]);

            assert_eq!(&buffer[..offset], &original[..offset]);
            assert_eq!(&buffer[offset + length..], &original[offset + length..]);
            for (index, &byte) in original[offset..offset + length].iter().enumerate() {
                let expected = if index % 4 == 3 { 255 } else { byte };
                assert_eq!(
                    buffer[offset + index],
                    expected,
                    "length={length}, offset={offset}, byte={index}"
                );
            }
        }
    }
}

#[test]
fn opaque_copy_preserves_rgb_tails_and_surrounding_bytes_at_independent_offsets() {
    for length in 0..=67 {
        for source_offset in 0..16 {
            for target_offset in 0..16 {
                let source: Vec<u8> = (0..length + 32)
                    .map(|index| u8::try_from(index * 37 % 256).expect("byte pattern"))
                    .collect();
                let original = source.clone();
                let mut target = vec![0x5A; length + 32];
                canvas::copy_opaque_rgba(
                    &source[source_offset..source_offset + length],
                    &mut target[target_offset..target_offset + length],
                );
                assert_eq!(source, original);
                assert!(target[..target_offset].iter().all(|&byte| byte == 0x5A));
                assert!(target[target_offset + length..].iter().all(|&byte| byte == 0x5A));
                for index in 0..length {
                    let expected = if index % 4 == 3 {
                        255
                    } else {
                        source[source_offset + index]
                    };
                    assert_eq!(target[target_offset + index], expected);
                }
            }
        }
    }
}

#[test]
fn opaque_rows_append_and_reuse_without_mutating_a_transparent_framebuffer() {
    for width in [1919, 1920, 1921] {
        let source = DecodedImage::new(PixelFormat::RgbA32, width, 1080);
        let original = source.data().to_vec();
        let mut buffer = WriteBuf::new();
        for (top, bottom) in [(0, 1079), (17, 18), (23, 64)] {
            buffer.clear();
            buffer.write_slice(&[11, 23, 47]);
            canvas::copy_opaque_rows(
                &source,
                &InclusiveRectangle {
                    left: 0,
                    top,
                    right: width - 1,
                    bottom,
                },
                &mut buffer,
            );
            assert_eq!(&buffer.filled()[..3], &[11, 23, 47]);
            assert_eq!(
                buffer.filled_len(),
                3 + usize::from(width) * usize::from(bottom - top + 1) * 4
            );
            assert!(
                buffer.filled()[3..]
                    .chunks_exact(4)
                    .all(|pixel| pixel == [0, 0, 0, 255])
            );
            assert_eq!(source.data(), original);
        }
    }
}
