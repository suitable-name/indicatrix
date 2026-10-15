//! Tests of the HDR map: downscaling keeps the energy, the stored file round trips, damaged
//! files are ignored, and a map survives a restart.

use std::sync::Arc;

use indicatrix::renderer::env_map::EnvironmentMap;

use super::{file, scale, *};
use crate::{
    platform::{PickedFile, Platform},
    platform_mock::MockPlatform,
};

/// A state on a mock platform whose directories the caller can reuse; the mock removes
/// them when it is dropped.
fn state_on(mock: &Arc<MockPlatform>) -> AppState {
    let platform: Arc<dyn Platform> = mock.clone();
    AppState::new(platform)
}

/// A smooth, non-uniform, positive texel field with one bright patch (a "sun").
fn synthetic_texels(width: usize, height: usize) -> Vec<[f32; 3]> {
    (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let u = x as f32 / width as f32;
                let v = y as f32 / height as f32;
                let sun = if (u - 0.3).abs() < 0.05 && (v - 0.3).abs() < 0.08 {
                    40.0
                } else {
                    0.0
                };
                let half_v = 0.5 * v;
                [
                    0.2 + u + sun,
                    0.3 + half_v + sun,
                    0.1 + (u * 6.0).sin().abs() + sun,
                ]
            })
        })
        .collect()
}

/// A flat (uncompressed) Radiance file: pixels avoid the run-length markers, so the decoder
/// reads them as plain RGBE.
fn radiance_bytes(width: usize, height: usize) -> Vec<u8> {
    let mut bytes =
        format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes();
    for y in 0..height {
        for x in 0..width {
            bytes.push(100 + u8::try_from(x * 3 % 100).expect("below 100"));
            bytes.push(100 + u8::try_from(y * 2 % 100).expect("below 100"));
            bytes.push(120);
            bytes.push(128 + u8::try_from(x % 3).expect("below 3"));
        }
    }
    bytes
}

fn picked(bytes: Vec<u8>) -> PickedFile {
    PickedFile {
        display_name: "sky.hdr".to_owned(),
        bytes,
    }
}

fn relative_difference(a: f64, b: f64) -> f64 {
    (a - b).abs() / a.abs().max(b.abs()).max(1e-12)
}

#[test]
fn downscaling_keeps_the_mean_radiance_within_one_percent() {
    let (width, height) = (256, 128);
    let texels = synthetic_texels(width, height);
    let map = EnvironmentMap::from_rgb(width, height, texels.clone()).expect("a valid map");
    let before = scale::mean_radiance(&texels);

    let small = scale::resample_map(&map, 64, 32);
    assert_eq!(small.len(), 64 * 32);
    let after = scale::mean_radiance(&small);
    for channel in 0..3 {
        let difference = relative_difference(before[channel], after[channel]);
        assert!(
            difference < 0.01,
            "channel {channel}: {} -> {} ({difference})",
            before[channel],
            after[channel]
        );
    }

    // A non-integer ratio keeps the energy too.
    let odd = scale::resample_map(&map, 100, 37);
    let after = scale::mean_radiance(&odd);
    for channel in 0..3 {
        assert!(relative_difference(before[channel], after[channel]) < 0.01);
    }
}

#[test]
fn reading_the_texels_back_reproduces_the_map() {
    let (width, height) = (64, 32);
    let texels = synthetic_texels(width, height);
    let map = EnvironmentMap::from_rgb(width, height, texels.clone()).expect("a valid map");
    let back = scale::read_texels(&map);
    assert_eq!(back.len(), texels.len());
    let (a, b) = (scale::mean_radiance(&texels), scale::mean_radiance(&back));
    for channel in 0..3 {
        assert!(relative_difference(a[channel], b[channel]) < 0.01);
    }
}

#[test]
fn the_stored_file_round_trips_exactly() {
    let dir = crate::platform_mock::unique_temp_dir("hdr-file");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("current.envmap");
    let texels = synthetic_texels(48, 24);
    file::write(&path, 48, 24, &texels).expect("written");
    assert_eq!(file::read_size(&path), Ok((48, 24)));
    let (width, height, back) = file::read(&path).expect("read back");
    assert_eq!((width, height), (48, 24));
    assert_eq!(back, texels);
    assert!(
        !dir.join("current.envmap.tmp").exists(),
        "the temporary file is renamed away"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn damaged_files_are_refused() {
    let dir = crate::platform_mock::unique_temp_dir("hdr-bad");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("current.envmap");
    let texels = synthetic_texels(8, 4);
    file::write(&path, 8, 4, &texels).expect("written");
    let good = std::fs::read(&path).expect("bytes");

    // Truncated.
    std::fs::write(&path, &good[..good.len() - 5]).expect("write");
    assert!(file::read(&path).is_err());
    // Wrong magic.
    let mut wrong = good.clone();
    wrong[0] = b'X';
    std::fs::write(&path, &wrong).expect("write");
    assert!(file::read(&path).is_err());
    assert!(file::read_size(&path).is_err());
    // A texel that is not a number.
    let mut nan = good.clone();
    nan[20..24].copy_from_slice(&f32::NAN.to_le_bytes());
    std::fs::write(&path, &nan).expect("write");
    assert!(file::read(&path).is_err());
    // Trailing bytes.
    let mut long = good;
    long.push(0);
    std::fs::write(&path, &long).expect("write");
    assert!(file::read(&path).is_err());
    // Missing.
    std::fs::remove_file(&path).expect("remove");
    assert!(matches!(file::read(&path), Err(file::FileError::Io(_))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_damaged_stored_map_is_ignored() {
    let mock = Arc::new(MockPlatform::default());
    let dirs = mock.app_dirs().clone();
    let hdr_dir = dirs.files.join("hdr");
    std::fs::create_dir_all(&hdr_dir).expect("hdr dir");
    // A good header followed by too few texels.
    let texels = synthetic_texels(8, 4);
    file::write(&hdr_dir.join("current.envmap"), 8, 4, &texels).expect("written");
    let path = hdr_dir.join("current.envmap");
    let bytes = std::fs::read(&path).expect("bytes");
    std::fs::write(&path, &bytes[..bytes.len() - 12]).expect("truncate");
    std::fs::write(hdr_dir.join("selected"), b"").expect("flag");

    let mut state = state_on(&mock);
    restore(&mut state);
    assert!(state.hdr.is_available(), "the header still reads");
    // The map itself does not: it is ignored, and the lighting preset applies.
    assert!(state.hdr.active().is_none());
    assert!(state.hdr.map().is_none());
    assert!(!state.hdr.is_available(), "a failed load forgets the size");
}

#[test]
fn importing_a_radiance_file_installs_and_selects_it() {
    let mock = Arc::new(MockPlatform::default());
    let mut state = state_on(&mock);
    assert!(state.hdr.active().is_none());
    let description = import(&mut state, &picked(radiance_bytes(64, 32))).expect("imports");
    assert_eq!(description, "64 x 32 HDR map");
    assert!(state.hdr.is_available());
    assert!(state.hdr.is_selected());
    let map = state.hdr.active().expect("the chosen map is active");
    assert_eq!((map.width(), map.height()), (64, 32));
    let view = state.hdr.view();
    assert!(view.available && view.selected);
    assert_eq!(view.description, "64 x 32 HDR map");
}

#[test]
fn a_map_and_the_choice_survive_a_restart() {
    let mock = Arc::new(MockPlatform::default());
    let mut first = state_on(&mock);
    import(&mut first, &picked(radiance_bytes(32, 16))).expect("imports");
    let original = first.hdr.active().expect("active");

    let second_mock = Arc::new(MockPlatform::new(mock.app_dirs().clone()));
    let mut second = state_on(&second_mock);
    restore(&mut second);
    assert!(second.hdr.is_selected());
    assert_eq!(second.hdr.size(), Some((32, 16)));
    // The worker path: a loader fills the slot, then `active` just hands it out.
    let loader = second.hdr.loader().expect("there is a stored map to load");
    loader.run();
    assert!(second.hdr.loader().is_none(), "loaded once");
    let reloaded = second.hdr.active().expect("active after the restart");
    assert!(original.bitwise_eq(&reloaded), "a reload lights the same");
}

#[test]
fn active_loads_lazily_without_a_loader() {
    let mock = Arc::new(MockPlatform::default());
    let mut first = state_on(&mock);
    import(&mut first, &picked(radiance_bytes(32, 16))).expect("imports");
    let second_mock = Arc::new(MockPlatform::new(mock.app_dirs().clone()));
    let mut second = state_on(&second_mock);
    restore(&mut second);
    assert!(second.hdr.active().is_some());
}

#[test]
fn choosing_needs_a_map_and_clearing_removes_everything() {
    let mock = Arc::new(MockPlatform::default());
    let mut state = state_on(&mock);
    assert_eq!(
        set_selected(&mut state, true),
        Err("Import an HDR map first.".to_owned())
    );
    import(&mut state, &picked(radiance_bytes(32, 16))).expect("imports");
    set_selected(&mut state, false).expect("unchoose");
    assert!(state.hdr.active().is_none(), "unchosen: the preset applies");
    assert!(state.hdr.is_available(), "but the map is still there");
    set_selected(&mut state, true).expect("choose again");
    assert!(state.hdr.active().is_some());

    clear(&mut state);
    assert!(!state.hdr.is_available());
    assert!(state.hdr.active().is_none());
    let hdr_dir = mock.app_dirs().files.join("hdr");
    assert!(!hdr_dir.join("current.envmap").exists());
    assert!(!hdr_dir.join("selected").exists());
    // A fresh start finds nothing.
    let second_mock = Arc::new(MockPlatform::new(mock.app_dirs().clone()));
    let mut second = state_on(&second_mock);
    restore(&mut second);
    assert!(!second.hdr.is_available());
}

#[test]
fn bad_files_are_refused_with_a_sentence() {
    let mock = Arc::new(MockPlatform::default());
    let mut state = state_on(&mock);
    let not_hdr = import(&mut state, &picked(b"hello".to_vec())).expect_err("not a Radiance file");
    assert!(not_hdr.contains("not a Radiance"), "{not_hdr}");

    // Declared larger than the limits: refused from the header alone (no pixels follow).
    let big = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 100 +X 5000\n".to_vec();
    let too_big = import(&mut state, &picked(big)).expect_err("too large");
    assert!(too_big.contains("larger than"), "{too_big}");

    // Black everywhere: nothing to light with.
    let mut black = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 4 +X 16\n".to_vec();
    black.extend(std::iter::repeat_n(0_u8, 4 * 16 * 4));
    let message = import(&mut state, &picked(black)).expect_err("black");
    assert!(message.contains("black"), "{message}");
    assert!(!state.hdr.is_available());
}

#[test]
fn picking_a_lighting_preset_turns_the_map_off() {
    let mock = Arc::new(MockPlatform::default());
    let mut state = state_on(&mock);
    import(&mut state, &picked(radiance_bytes(32, 16))).expect("imports");
    assert!(state.hdr.is_selected());
    let name = crate::design_props::lighting_rows()[0].name.clone();
    crate::design_props::set_lighting(&mut state, &name).expect("a real rig");
    assert!(!state.hdr.is_selected());
    assert!(state.hdr.active().is_none());
    assert!(state.hdr.is_available(), "the map stays imported");
    let view = crate::design_props::lighting_view(&state);
    assert!(view.hdr.available && !view.hdr.selected);
}

#[test]
fn the_debug_output_does_not_print_texels() {
    let mock = Arc::new(MockPlatform::default());
    let mut state = state_on(&mock);
    import(&mut state, &picked(radiance_bytes(32, 16))).expect("imports");
    let text = format!("{:?}", state.hdr);
    assert!(text.len() < 400, "{text}");
}
