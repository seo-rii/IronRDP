#![expect(
    unused_crate_dependencies,
    reason = "only the pending regions and extraction modules are under test"
)]

use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::session::image::DecodedImage;
use ironrdp_core::WriteBuf;

#[path = "../src/image.rs"]
mod image;
#[path = "../src/presentation.rs"]
mod presentation;

use presentation::PendingRegions;

fn rectangle(left: u16, top: u16, width: u16, height: u16) -> InclusiveRectangle {
    InclusiveRectangle {
        left,
        top,
        right: left + width - 1,
        bottom: top + height - 1,
    }
}

#[test]
fn identical_and_contained_regions_need_only_one_draw_in_either_order() {
    let outer = rectangle(13, 17, 64, 65);
    let inner = rectangle(14, 18, 31, 32);
    for sequence in [
        [outer.clone(), inner.clone(), outer.clone()],
        [inner.clone(), inner.clone(), outer.clone()],
    ] {
        let mut pending = PendingRegions::default();
        assert!(pending.is_empty());
        for region in sequence {
            assert!(pending.push(region).is_none());
        }
        assert_eq!(pending.drain().collect::<Vec<_>>(), core::slice::from_ref(&outer));
        assert!(pending.is_empty());
        assert!(pending.push(inner.clone()).is_none());
        assert_eq!(pending.drain().collect::<Vec<_>>(), core::slice::from_ref(&inner));
    }
}

#[test]
fn disjoint_adjacent_and_partially_overlapping_regions_keep_their_footprints() {
    let regions = [
        rectangle(0, 0, 3, 3),
        rectangle(1, 1, 3, 3),
        rectangle(6, 0, 2, 2),
        rectangle(8, 0, 2, 2),
    ];
    let mut pending = PendingRegions::default();
    for region in &regions {
        assert!(pending.push(region.clone()).is_none());
    }
    assert_eq!(pending.drain().collect::<Vec<_>>(), regions);
}

#[test]
fn overflow_returns_the_unqueued_region_without_losing_pending_draws() {
    let regions: Vec<_> = (0..PendingRegions::CAPACITY)
        .map(|index| rectangle(u16::try_from(index * 2).expect("small coordinate"), 0, 1, 1))
        .collect();
    let mut pending = PendingRegions::default();
    for region in &regions {
        assert!(pending.push(region.clone()).is_none());
    }

    // A covered region needs no new slot even when the queue is full.
    assert!(pending.push(regions[17].clone()).is_none());
    let overflow = rectangle(500, 0, 1, 1);
    assert_eq!(pending.push(overflow.clone()), Some(overflow.clone()));
    assert_eq!(pending.drain().collect::<Vec<_>>(), regions);
    assert!(pending.push(overflow.clone()).is_none());
    assert_eq!(pending.drain().collect::<Vec<_>>(), [overflow]);

    // A covering update also fits a full queue by replacing the existing draws.
    for region in regions {
        assert!(pending.push(region).is_none());
    }
    let covering = rectangle(0, 0, 128, 1);
    assert!(pending.push(covering.clone()).is_none());
    assert_eq!(pending.drain().collect::<Vec<_>>(), [covering]);
}

#[test]
fn containment_does_not_overflow_at_maximum_coordinates() {
    let outer = InclusiveRectangle {
        left: u16::MAX - 5,
        top: u16::MAX - 5,
        right: u16::MAX,
        bottom: u16::MAX,
    };
    let inner = InclusiveRectangle {
        left: u16::MAX,
        top: u16::MAX,
        right: u16::MAX,
        bottom: u16::MAX,
    };
    let mut pending = PendingRegions::default();
    assert!(pending.push(inner).is_none());
    assert!(pending.push(outer.clone()).is_none());
    assert_eq!(pending.drain().collect::<Vec<_>>(), [outer]);
}

#[test]
fn every_three_rectangle_sequence_on_a_small_canvas_preserves_exact_coverage() {
    fn coverage(region: &InclusiveRectangle) -> u16 {
        let mut pixels = 0;
        for y in region.top..=region.bottom {
            for x in region.left..=region.right {
                pixels |= 1 << (y * 3 + x);
            }
        }
        pixels
    }

    let mut rectangles = Vec::new();
    for top in 0..3 {
        for bottom in top..3 {
            for left in 0..3 {
                for right in left..3 {
                    let region = InclusiveRectangle {
                        left,
                        top,
                        right,
                        bottom,
                    };
                    rectangles.push((region.clone(), coverage(&region)));
                }
            }
        }
    }

    let mut pending = PendingRegions::default();
    for (first, first_pixels) in &rectangles {
        for (second, second_pixels) in &rectangles {
            for (third, third_pixels) in &rectangles {
                for region in [first, second, third] {
                    assert!(pending.push(region.clone()).is_none());
                }
                let actual = pending.drain().fold(0, |pixels, region| pixels | coverage(&region));
                assert_eq!(
                    actual,
                    first_pixels | second_pixels | third_pixels,
                    "coverage changed for {first:?}, {second:?}, {third:?}"
                );
            }
        }
    }
}

#[test]
fn extraction_thresholds_preserve_the_opaque_pixels_on_a_sparse_canvas() {
    for (image_width, widths) in [(640, [511, 512, 513]), (1920, [1535, 1536, 1537])] {
        let source = DecodedImage::new(PixelFormat::RgbA32, image_width, 96);
        let mut buffer = WriteBuf::new();
        let mut rectangles = Vec::new();
        for width in widths {
            for height in [1, 64, 65] {
                for (left, top) in [(7, 9), (8, 10)] {
                    let region = rectangle(left, top, width, height);
                    buffer.clear();
                    let selected = image::extract_partial_image(&source, region.clone(), &mut buffer);
                    let mut pixels = vec![0u64; (usize::from(image_width) * 96).div_ceil(64)];
                    for y in selected.top..=selected.bottom {
                        for x in selected.left..=selected.right {
                            let offset = usize::from(y) * usize::from(image_width) + usize::from(x);
                            pixels[offset / 64] |= 1 << (offset % 64);
                        }
                    }
                    rectangles.push((region, pixels));
                }
            }
        }

        // Every draw makes its selected pixels opaque. Equal masks prove that initial transparent
        // pixels stay transparent, including when only one of the input regions widens to whole rows.
        for (first, first_pixels) in &rectangles {
            for (second, second_pixels) in &rectangles {
                let mut pending = PendingRegions::default();
                assert!(pending.push(first.clone()).is_none());
                assert!(pending.push(second.clone()).is_none());
                let mut actual = vec![0u64; first_pixels.len()];
                for region in pending.drain() {
                    let (_, selected_pixels) = rectangles
                        .iter()
                        .find(|(original, _)| *original == region)
                        .expect("coalescing keeps an original rectangle");
                    for (actual, selected) in actual.iter_mut().zip(selected_pixels) {
                        *actual |= selected;
                    }
                }
                for ((actual, first_pixels), second_pixels) in actual.iter().zip(first_pixels).zip(second_pixels) {
                    assert_eq!(
                        *actual,
                        first_pixels | second_pixels,
                        "opaque footprint changed for {first:?}, {second:?} on width {image_width}"
                    );
                }
            }
        }
    }
}
