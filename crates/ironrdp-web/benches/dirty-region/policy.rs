
pub(crate) fn extract_benchmark(
    image: &DecodedImage,
    region: InclusiveRectangle,
    buffer: &mut WriteBuf,
    policy: u8,
) -> InclusiveRectangle {
    if policy == 6 {
        return extract_partial_image(image, region, buffer);
    }
    let prefer_rows = region.height() > 64 || region.width() > 512;
    let image_width = u32::from(image.width());
    let region_width = u32::from(region.width());
    let rows = match policy {
        0 => prefer_rows,
        1 => prefer_rows && image_width * 4 <= region_width * 5,
        2 => prefer_rows && image_width * 2 <= region_width * 3,
        3 => prefer_rows && image_width <= region_width * 2,
        4 => false,
        5 => true,
        _ => panic!("unknown benchmark policy"),
    };
    if rows {
        extract_whole_rows(image, region, buffer)
    } else {
        extract_smallest_rectangle(image, region, buffer)
    }
}
