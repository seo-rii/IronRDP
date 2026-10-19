#![expect(unused_crate_dependencies, reason = "only the canvas alpha helper is under test")]

#[expect(dead_code, reason = "the browser canvas methods are not used by the pixel tests")]
#[path = "../src/canvas.rs"]
mod canvas;

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
