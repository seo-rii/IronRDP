use std::time::Instant;

use ironrdp_web_dirty_region_bench::{Fixture, POLICIES, rectangle};

fn main() {
    let samples: usize = std::env::args()
        .nth(1)
        .map_or(20, |value| value.parse().expect("sample count"));
    let cases: [(u16, u16, u16, u16); 12] = [
        (1920, 1080, 32, 32),
        (1920, 1080, 64, 720),
        (1920, 1080, 512, 720),
        (1920, 1080, 960, 720),
        (1920, 1080, 1280, 720),
        (1920, 1080, 1536, 720),
        (1920, 1080, 1800, 720),
        (1920, 1080, 1920, 1080),
        (1920, 1080, 640, 16),
        (1920, 1080, 1536, 16),
        (3840, 2160, 64, 1440),
        (3840, 2160, 3840, 2160),
    ];
    let mut random = 0x12345678_u32;
    println!("image_width,image_height,width,height,policy,sample,iterations,bytes,ns_per_copy");
    for (image_width, image_height, width, height) in cases {
        let mut fixture = Fixture::new(image_width, image_height);
        fixture.paint_common(rectangle(0, 0, image_width, image_height), 1);
        let region = rectangle((image_width - width) / 2, (image_height - height) / 2, width, height);
        fixture.paint(region.clone(), 2);
        let mut iterations = [1_u32; 7];
        for policy in 0..7_u8 {
            // Calibrate to >= 8 ms per batch, then warm the same allocation again.
            loop {
                let start = Instant::now();
                fixture.copy_batch(policy, region.clone(), iterations[usize::from(policy)]);
                if start.elapsed().as_secs_f64() >= 0.008 {
                    break;
                }
                iterations[usize::from(policy)] *= 2;
            }
            fixture.copy_batch(policy, region.clone(), iterations[usize::from(policy)]);
        }
        for sample in 0..samples {
            let mut policies = [0_u8, 1, 2, 3, 4, 5, 6];
            for index in (1..policies.len()).rev() {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                policies.swap(index, random as usize % (index + 1));
            }
            for policy in policies {
                let count = iterations[usize::from(policy)];
                let start = Instant::now();
                let bytes = fixture.copy_batch(policy, region.clone(), count);
                let nanoseconds = start.elapsed().as_secs_f64() * 1_000_000_000.0 / f64::from(count);
                println!(
                    "{image_width},{image_height},{width},{height},{},{sample},{count},{bytes},{nanoseconds:.3}",
                    POLICIES[usize::from(policy)]
                );
            }
        }
    }
}
