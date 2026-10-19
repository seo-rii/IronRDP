#![expect(unused_crate_dependencies, reason = "only the frame pacing state is under test")]

use core::time::Duration;

#[path = "../src/frame_pacing.rs"]
mod frame_pacing;

use frame_pacing::FramePacing;

#[test]
fn frame_pacing_draws_the_first_update_and_updates_after_idle_immediately() {
    let mut pacing = FramePacing::default();
    assert_eq!(pacing.delay_ms(0.0), 0);
    assert_eq!(pacing.delay_ms(100.0), 0);
    pacing.presented(100.0);
    assert_eq!(pacing.delay_ms(100.0), 17);
    assert_eq!(pacing.delay_ms(115.0), 2);
    assert_eq!(pacing.delay_ms(117.0), 0);
    assert_eq!(pacing.delay_ms(10_000.0), 0);
}

#[test]
fn frame_pacing_keeps_the_deadline_stable_while_updates_accumulate() {
    let mut pacing = FramePacing::new(Duration::from_millis(10));
    pacing.presented(100.0);
    for (now_ms, delay_ms) in [(101.0, 9), (104.0, 6), (109.0, 1), (110.0, 0)] {
        assert_eq!(pacing.delay_ms(now_ms), delay_ms);
        assert_eq!(pacing.delay_ms(now_ms), delay_ms);
    }
}

#[test]
fn frame_pacing_anchors_late_and_forced_draws_to_the_actual_presentation_time() {
    let mut pacing = FramePacing::new(Duration::from_millis(10));
    pacing.presented(100.0);
    assert_eq!(pacing.delay_ms(1_000.0), 0);
    pacing.presented(1_000.0);
    assert_eq!(pacing.delay_ms(1_000.0), 10);
    assert_eq!(pacing.delay_ms(1_001.0), 9);

    // An early capacity flush also starts a fresh interval instead of retaining a stale deadline.
    pacing.presented(1_005.0);
    assert_eq!(pacing.delay_ms(1_005.0), 10);
    assert_eq!(pacing.delay_ms(1_010.0), 5);
    assert_eq!(pacing.delay_ms(1_015.0), 0);
}

#[test]
fn frame_pacing_rounds_fractional_delays_up_and_bounds_timer_conversion() {
    let mut pacing = FramePacing::new(Duration::from_nanos(33_333_334));
    pacing.presented(0.0);
    assert_eq!(pacing.delay_ms(0.0), 34);
    assert_eq!(pacing.delay_ms(32.0), 2);
    assert_eq!(pacing.delay_ms(33.0), 1);
    assert_eq!(pacing.delay_ms(34.0), 0);

    let mut short_interval = FramePacing::new(Duration::from_nanos(1));
    short_interval.presented(0.0);
    assert_eq!(short_interval.delay_ms(0.0), 1);

    let mut long_interval = FramePacing::new(Duration::MAX);
    long_interval.presented(0.0);
    assert_eq!(long_interval.delay_ms(0.0), u32::MAX);

    let mut no_interval = FramePacing::new(Duration::ZERO);
    no_interval.presented(0.0);
    assert_eq!(no_interval.delay_ms(0.0), 0);
}

#[test]
fn frame_pacing_reset_makes_the_next_frame_immediate() {
    let mut pacing = FramePacing::default();
    pacing.presented(100.0);
    assert_eq!(pacing.delay_ms(101.0), 16);
    pacing.reset();
    assert_eq!(pacing.delay_ms(101.0), 0);
    pacing.presented(101.0);
    assert_eq!(pacing.delay_ms(101.0), 17);
}
