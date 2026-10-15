//! Tests for the settings model and its store. Each test works in its own temp directory.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use super::*;
use crate::{layout::Destination, platform::AppDirs, platform_mock::unique_temp_dir};

/// A temp directory that is removed when the test ends.
struct TempFiles(PathBuf);

impl TempFiles {
    fn new(tag: &str) -> Self {
        Self(unique_temp_dir(tag))
    }

    fn dirs(&self) -> AppDirs {
        AppDirs {
            files: self.0.join("files"),
            cache: self.0.join("cache"),
        }
    }

    fn settings_path(&self) -> PathBuf {
        self.0.join("files").join("settings.toml")
    }
}

impl Drop for TempFiles {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait_for_file(path: &Path, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if path.is_file() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    path.is_file()
}

fn recent(title: &str, kind: RecentKind, at: i64) -> RecentItem {
    RecentItem {
        title: title.to_owned(),
        opened_at_unix: at,
        kind,
    }
}

#[test]
fn defaults_match_the_brief() {
    let settings = MobileSettings::default();
    assert_eq!(settings.theme, ThemeChoice::System);
    assert!(settings.large_handles && settings.snap_to_gear);
    assert_eq!(settings.auto_solve_budget_ms, 300);
    assert_eq!(settings.material_name, "Diamond");
    assert_eq!(settings.last_destination, Destination::Design);
    assert_eq!(settings.render.bounces, 8);
    assert_eq!(settings.render.quality, RenderQuality::Normal);
    assert_eq!(settings.render.export_cap_px, 1024);
    assert_eq!(settings.lighting.preset_name, "Light tent + black cards");
    assert_eq!(settings.lighting.backdrop, Backdrop::Grey);
    assert_eq!(settings.lighting.head_shadow_deg, 16.0);
    assert_eq!(settings.remote.cadence_ms, 500);
    assert_eq!(settings.remote.transfer, RemoteTransfer::DisplayOnly);
    assert_eq!(settings.library.sort, LibrarySort::CatalogueOrder);
    assert_eq!(settings.recent, Vec::<RecentItem>::new());
}

#[test]
fn toml_round_trip_of_a_changed_value() {
    let mut settings = MobileSettings {
        theme: ThemeChoice::Dark,
        battery_saver: true,
        last_destination: Destination::Cut,
        ..MobileSettings::default()
    };
    settings.render.quality = RenderQuality::Fine;
    settings.lighting.backdrop = Backdrop::White;
    settings.remote.address = "192.168.1.5:7000".to_owned();
    settings.library.sort = LibrarySort::RecentlyEdited;
    settings.push_recent(recent("A", RecentKind::Library { entry_id: 7 }, 100));
    settings.push_recent(recent(
        "B",
        RecentKind::Document {
            display_name: "b.asc".to_owned(),
        },
        200,
    ));
    let text = toml::to_string_pretty(&settings).expect("serialize");
    let back: MobileSettings = toml::from_str(&text).expect("parse");
    assert_eq!(back, settings);
}

#[test]
fn surface_glare_defaults_to_unscaled_and_round_trips() {
    assert_eq!(
        LightingSettings::default().surface_glare_pct,
        crate::design_props::DEFAULT_SURFACE_GLARE_PCT
    );
    let mut settings = MobileSettings::default();
    settings.lighting.surface_glare_pct = 35.0;
    let text = toml::to_string_pretty(&settings).expect("serialize");
    let back: MobileSettings = toml::from_str(&text).expect("parse");
    assert_eq!(back.lighting.surface_glare_pct, 35.0);
    assert_eq!(back, settings);
}

#[test]
fn a_file_without_surface_glare_reads_as_unscaled() {
    let old = "[lighting]\nexposure = 1.5\nazimuth_deg = 10.0\n";
    let settings: MobileSettings = toml::from_str(old).expect("parse");
    assert_eq!(settings.lighting.exposure, 1.5);
    assert_eq!(settings.lighting.surface_glare_pct, 100.0);
}

#[test]
fn missing_file_gives_defaults() {
    let temp = TempFiles::new("settings-missing");
    let (_store, settings) = SettingsStore::open(&temp.dirs());
    assert_eq!(settings, MobileSettings::default());
}

#[test]
fn corrupt_file_is_renamed_aside() {
    let temp = TempFiles::new("settings-corrupt");
    let dirs = temp.dirs();
    fs::create_dir_all(&dirs.files).expect("mkdir");
    fs::write(temp.settings_path(), "this is [not toml").expect("write");
    let (_store, settings) = SettingsStore::open(&dirs);
    assert_eq!(settings, MobileSettings::default());
    assert!(!temp.settings_path().exists());
    let aside: Vec<_> = fs::read_dir(&dirs.files)
        .expect("read_dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("settings.toml.corrupt-"))
        .collect();
    assert_eq!(aside.len(), 1, "found {aside:?}");
}

#[test]
fn unknown_keys_are_ignored() {
    let temp = TempFiles::new("settings-unknown");
    let dirs = temp.dirs();
    fs::create_dir_all(&dirs.files).expect("mkdir");
    fs::write(
        temp.settings_path(),
        "theme = \"Light\"\nfuture_key = 5\n\n[render]\nbounces = 12\nfuture_render_key = true\n",
    )
    .expect("write");
    let (_store, settings) = SettingsStore::open(&dirs);
    assert_eq!(settings.theme, ThemeChoice::Light);
    assert_eq!(settings.render.bounces, 12);
}

#[test]
fn partial_file_keeps_defaults_for_missing_keys() {
    let temp = TempFiles::new("settings-partial");
    let dirs = temp.dirs();
    fs::create_dir_all(&dirs.files).expect("mkdir");
    fs::write(temp.settings_path(), "[render]\nbounces = 3\n").expect("write");
    let (_store, settings) = SettingsStore::open(&dirs);
    assert_eq!(settings.render.bounces, 3);
    assert_eq!(settings.render.export_cap_spp, 256);
    assert_eq!(settings.material_name, "Diamond");
    assert!(settings.large_handles);
}

#[test]
fn flush_writes_immediately_and_reloads() {
    let temp = TempFiles::new("settings-flush");
    let dirs = temp.dirs();
    let (store, mut settings) = SettingsStore::open(&dirs);
    assert_eq!(store.path(), temp.settings_path());
    settings.theme = ThemeChoice::Dark;
    settings.render.bounces = 5;
    store.save(&settings);
    store.flush();
    assert!(temp.settings_path().is_file());
    assert!(!dirs.files.join("settings.toml.tmp").exists());
    let (_second, reloaded) = SettingsStore::open(&dirs);
    assert_eq!(reloaded, settings);
}

#[test]
fn debounce_coalesces_saves_into_the_final_content() {
    let temp = TempFiles::new("settings-debounce");
    let dirs = temp.dirs();
    let (store, mut settings) = SettingsStore::open(&dirs);
    for bounces in 1..=10 {
        settings.render.bounces = bounces;
        store.save(&settings);
    }
    // Nothing is written during the 600 ms quiet period.
    assert!(!temp.settings_path().exists());
    assert!(wait_for_file(&temp.settings_path(), Duration::from_secs(5)));
    let text = fs::read_to_string(temp.settings_path()).expect("read");
    let written: MobileSettings = toml::from_str(&text).expect("parse");
    assert_eq!(written.render.bounces, 10);
}

#[test]
fn drop_flushes_a_pending_save() {
    let temp = TempFiles::new("settings-drop");
    let dirs = temp.dirs();
    let (store, mut settings) = SettingsStore::open(&dirs);
    settings.snap_to_gear = false;
    store.save(&settings);
    drop(store);
    let (_second, reloaded) = SettingsStore::open(&dirs);
    assert!(!reloaded.snap_to_gear);
}

#[test]
fn push_recent_dedups_orders_and_caps() {
    let mut settings = MobileSettings::default();
    for id in 0..12 {
        settings.push_recent(recent(
            &format!("L{id}"),
            RecentKind::Library { entry_id: id },
            id,
        ));
    }
    assert_eq!(settings.recent.len(), MAX_RECENT);
    assert_eq!(settings.recent[0].title, "L11");

    // Re-opening an entry moves it to the front without duplicating it.
    settings.push_recent(recent("L5 again", RecentKind::Library { entry_id: 5 }, 99));
    assert_eq!(settings.recent.len(), MAX_RECENT);
    assert_eq!(settings.recent[0].title, "L5 again");
    assert_eq!(
        settings
            .recent
            .iter()
            .filter(|item| item.kind == RecentKind::Library { entry_id: 5 })
            .count(),
        1
    );

    // A document with the same name as a library title is a different target.
    settings.push_recent(recent(
        "doc",
        RecentKind::Document {
            display_name: "x.asc".to_owned(),
        },
        100,
    ));
    settings.push_recent(recent(
        "doc2",
        RecentKind::Document {
            display_name: "x.asc".to_owned(),
        },
        101,
    ));
    assert_eq!(settings.recent[0].title, "doc2");
    assert_eq!(settings.recent[1].title, "L5 again");
}
