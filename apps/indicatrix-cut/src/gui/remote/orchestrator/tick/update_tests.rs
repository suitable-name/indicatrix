use super::*;
use indicatrix_net::messages::{DisplayEncoding, DisplayFrameHeader, StreamEvent};

// ---- display_frame_rgba: the final-picture live transfer's frame -> image path --

/// An accumulator holding `rgba` as request 1's latest display frame, encoded the
/// way a coordinator sends it.
fn with_display_frame(width: u32, height: u32, rgba: &[u8]) -> Accumulator {
    let png = indicatrix_net::display::encode_rgba8(DisplayEncoding::Png, width, height, rgba)
        .expect("encodes");
    let mut acc = Accumulator::new(width, height);
    acc.begin_request(1);
    let header = DisplayFrameHeader {
        request_id: 1,
        samples_done: 32,
        width,
        height,
        encoding: DisplayEncoding::Png,
        payload_len: u32::try_from(png.len()).expect("a test PNG fits u32"),
    };
    acc.apply(&StreamEvent::DisplayFrame(header), Some(&png))
        .expect("applies");
    acc
}

#[test]
fn a_display_frame_decodes_to_exactly_the_pixels_the_remote_sent() {
    let rgba: Vec<u8> = (0..3 * 2 * 4)
        .map(|i| u8::try_from(i * 7 % 256).expect("i * 7 % 256 is in 0..=255"))
        .collect();
    let acc = with_display_frame(3, 2, &rgba);
    assert_eq!(display_frame_rgba(&acc, 3, 2), Ok(Some(rgba)));
}

#[test]
fn no_display_frame_yet_shows_nothing_and_a_mis_sized_one_is_refused() {
    assert_eq!(display_frame_rgba(&Accumulator::new(2, 2), 2, 2), Ok(None));
    let acc = with_display_frame(2, 2, &[5_u8; 16]);
    assert!(
        display_frame_rgba(&acc, 4, 4).is_err(),
        "a frame for another view size never reaches the viewport"
    );
}

// ---- redraw_is_due: remote redraw rate limit ----

#[test]
fn no_prior_redraw_is_always_due() {
    assert!(redraw_is_due(
        None,
        REMOTE_REDRAW_MIN_INTERVAL,
        Instant::now()
    ));
}

#[test]
fn a_redraw_within_the_interval_is_not_due() {
    let last = Instant::now();
    let now = last + Duration::from_millis(10);
    assert!(
        !redraw_is_due(Some(last), REMOTE_REDRAW_MIN_INTERVAL, now),
        "10ms after the last redraw is well inside the ~33ms interval"
    );
}

#[test]
fn a_redraw_exactly_at_the_interval_boundary_is_due() {
    let last = Instant::now();
    let now = last + REMOTE_REDRAW_MIN_INTERVAL;
    assert!(
        redraw_is_due(Some(last), REMOTE_REDRAW_MIN_INTERVAL, now),
        "the boundary itself (elapsed >= min_interval) must count as due"
    );
}

#[test]
fn a_redraw_well_past_the_interval_is_due() {
    let last = Instant::now();
    let now = last + REMOTE_REDRAW_MIN_INTERVAL + Duration::from_secs(1);
    assert!(redraw_is_due(Some(last), REMOTE_REDRAW_MIN_INTERVAL, now));
}

/// A whole burst of updates arriving within one interval must only ever find one of
/// them due -- the time-based half of rate-limiting redraws (`RedrawGate`
/// separately bounds the queue to one pending closure; this bounds how often a
/// redraw is even attempted).
#[test]
fn a_burst_of_updates_within_one_interval_finds_at_most_one_due() {
    let start = Instant::now();
    let mut last_redraw_at: Option<Instant> = None;
    let mut due_count = 0;
    for ms in [0u64, 5, 10, 15, 20, 25, 30] {
        let now = start + Duration::from_millis(ms);
        if redraw_is_due(last_redraw_at, REMOTE_REDRAW_MIN_INTERVAL, now) {
            due_count += 1;
            last_redraw_at = Some(now);
        }
    }
    assert_eq!(
        due_count, 1,
        "only the FIRST update of a burst inside one ~33ms interval may redraw"
    );

    // Once the interval genuinely elapses, the next update must be due again.
    let now = start + REMOTE_REDRAW_MIN_INTERVAL + Duration::from_millis(1);
    assert!(redraw_is_due(
        last_redraw_at,
        REMOTE_REDRAW_MIN_INTERVAL,
        now
    ));
}
