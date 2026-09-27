//! Tests for the single-endpoint settings model and its transfer preferences (full data
//! or final picture only): the legacy multi-worker migration and the transfer-choice
//! defaults.

use super::{
    AppSettings, ExportTransfer, LiveTransfer, RemoteEndpoint, SettingsFile, WorkerSettings,
};

/// An old (multi-worker era) settings file carrying TWO workers.
const OLD_FILE_WITH_TWO_WORKERS: &str = r#"
[settings]
exposure = 1.3
live_compute_target = "RemoteOnly"

[[settings.remote_workers]]
name = "Office"
address = "office.lan:7878"
cert_dir = "C:/certs/office"
cadence_ms = 250

[[settings.remote_workers]]
name = "Cloud"
address = "cloud.example:7878"
cert_dir = "C:/certs/cloud"
"#;

#[test]
fn an_old_file_with_two_workers_keeps_the_first_as_the_remote_and_drops_the_second() {
    let mut file: SettingsFile = toml::from_str(OLD_FILE_WITH_TWO_WORKERS).expect("parse");
    assert_eq!(file.settings.legacy_remote_workers.len(), 2);
    assert_eq!(file.settings.remote, None);

    let migration = file
        .settings
        .migrate_legacy_remote_workers()
        .expect("there was a list to migrate");
    assert_eq!(migration.kept.as_deref(), Some("office.lan:7878"));
    assert_eq!(migration.dropped, vec!["cloud.example:7878".to_string()]);

    let remote = file.settings.remote.as_ref().expect("first entry kept");
    assert_eq!(remote.connection.name, "Office");
    assert_eq!(remote.connection.cert_dir, "C:/certs/office");
    assert_eq!(remote.connection.cadence_ms, 250);
    assert_eq!(remote.export_transfer, ExportTransfer::FullData);
    assert_eq!(remote.live_transfer, LiveTransfer::FullData);
    assert_eq!(file.settings.legacy_remote_workers.len(), 0);
    // Everything else in the file is untouched.
    assert!((file.settings.exposure - 1.3).abs() < 1e-6);

    // Idempotent, and the retired key is never written back.
    assert_eq!(file.settings.migrate_legacy_remote_workers(), None);
    let rewritten = toml::to_string_pretty(&file).expect("serialize");
    assert!(!rewritten.contains("remote_workers"), "{rewritten}");
    let reparsed: SettingsFile = toml::from_str(&rewritten).expect("reparse");
    assert_eq!(reparsed.settings.remote, file.settings.remote);
}

/// A file that somehow carries both an endpoint and a legacy list keeps the endpoint
/// and drops the whole list (logged), never overwriting the newer setting.
#[test]
fn a_configured_endpoint_wins_over_a_leftover_legacy_list() {
    let mut settings = AppSettings {
        remote: Some(RemoteEndpoint::new(WorkerSettings {
            address: "coordinator:7878".to_string(),
            ..WorkerSettings::default()
        })),
        legacy_remote_workers: vec![WorkerSettings {
            address: "old:7878".to_string(),
            ..WorkerSettings::default()
        }],
        ..AppSettings::default()
    };
    let migration = settings.migrate_legacy_remote_workers().expect("migrated");
    assert_eq!(migration.kept, None);
    assert_eq!(migration.dropped, vec!["old:7878".to_string()]);
    assert_eq!(
        settings.remote_worker().map(|w| w.address),
        Some("coordinator:7878".to_string())
    );
}

#[test]
fn nothing_to_migrate_reports_none() {
    assert_eq!(AppSettings::default().migrate_legacy_remote_workers(), None);
}

/// Both transfer preferences default to full data (today's behaviour, and the only
/// transfer a plain worker supports); the pill indices round-trip.
#[test]
fn transfer_choices_default_to_full_data_and_their_indices_round_trip() {
    let endpoint = RemoteEndpoint::default();
    assert_eq!(endpoint.export_transfer, ExportTransfer::FullData);
    assert_eq!(endpoint.live_transfer, LiveTransfer::FullData);
    for t in [ExportTransfer::FullData, ExportTransfer::FinalPicture] {
        assert_eq!(ExportTransfer::from_index(t.index()), t);
    }
    for t in [LiveTransfer::FullData, LiveTransfer::FinalPicture] {
        assert_eq!(LiveTransfer::from_index(t.index()), t);
    }
    assert_eq!(ExportTransfer::from_index(-1), ExportTransfer::FullData);
    assert_eq!(LiveTransfer::from_index(7), LiveTransfer::FullData);
}
