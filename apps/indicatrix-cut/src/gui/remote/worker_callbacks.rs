//! Worker-list CRUD, "Test connection", token-based worker enrollment, and the global
//! denoise toggle.
//!
//! Split out of `gui::remote` purely to keep that module (already sizeable) from
//! growing further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`
//! itself.

use super::{
    live_compute_target_from_index, remote_samples_exponent_to_count,
    worker_settings::from_worker_item,
};
use crate::{
    MainWindow, RemoteWorkerModel, SettingsModel, WorkerItem,
    bridge::{remote::remote_render, render_thread::RenderContext},
    gui::{remote::refresh_worker_options, show_toast},
    settings::{SettingsPersister, WorkerSettings},
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, PoisonError};

// ---- Worker-list CRUD + "Test connection" + denoise toggle ----------------------

/// Wires the worker-list panel's add/edit/remove, "Test connection", and the global
/// denoise-toggle callbacks. Split out of `setup_remote_rendering` purely to keep that
/// function shorter.
pub fn setup_worker_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    setup_save_worker_callback(ui, settings_store);
    setup_remove_worker_callback(ui, settings_store);
    setup_denoise_toggle_callback(ui, render_ctx, settings_store);
    setup_live_compute_target_callback(ui, render_ctx, settings_store);
    setup_remote_render_samples_callback(ui, render_ctx, settings_store);
    setup_claim_token_callback(ui);
    setup_test_worker_connection_callback(ui);
    setup_cert_dir_picker_callback(ui);
}

/// Wires `on_save_worker` (add or update, by index). Split out of
/// `setup_worker_callbacks` purely to keep that function under clippy's
/// function-length lint -- same reasoning as `setup_remote_render_samples_callback`
/// below.
fn setup_save_worker_callback(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let settings_store_save = settings_store.clone();
    let ui_weak_save = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_save_worker(move |idx: i32, item: WorkerItem| {
            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            let worker = from_worker_item(&item);
            let idx = idx as usize;
            let mut result = Ok(());
            settings_store_save.update(|s| {
                result = if idx < s.settings.remote_workers.len() {
                    s.settings.update_worker(idx, worker.clone())
                } else {
                    s.settings.add_worker(worker.clone());
                    Ok(())
                };
            });
            match result {
                Ok(()) => {
                    refresh_worker_options(
                        &ui,
                        &settings_store_save.snapshot().settings.remote_workers,
                    );
                    show_toast(&ui, "Remote worker saved.", "success");
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

/// Wires `on_remove_worker`. Split out of `setup_worker_callbacks` purely to keep that
/// function under clippy's function-length lint -- same reasoning as
/// `setup_remote_render_samples_callback` below.
fn setup_remove_worker_callback(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let settings_store_remove = settings_store.clone();
    let ui_weak_remove = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_remove_worker(move |idx: i32| {
            let Some(ui) = ui_weak_remove.upgrade() else {
                return;
            };
            let mut result = Ok(());
            settings_store_remove.update(|s| result = s.settings.remove_worker(idx as usize));
            match result {
                Ok(()) => {
                    refresh_worker_options(
                        &ui,
                        &settings_store_remove.snapshot().settings.remote_workers,
                    );
                    show_toast(&ui, "Remote worker removed.", "info");
                }
                Err(err) => show_toast(&ui, &err, "error"),
            }
        });
}

/// Wires the global denoise toggle. Split out of `setup_worker_callbacks` purely to
/// keep that function under clippy's function-length lint -- same reasoning as
/// `setup_remote_render_samples_callback` below.
fn setup_denoise_toggle_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store_denoise = settings_store.clone();
    let render_ctx_denoise = render_ctx.clone();
    ui.global::<RemoteWorkerModel>()
        .on_denoise_toggled(move |enabled: bool| {
            // Live-updates `RenderContext` (governs the render loop immediately, both the
            // local readback in `render_thread` and the remote merged-accumulation
            // readback in `gui::remote::render_merged_frame`) in addition to persisting
            // the choice -- the same two-step pattern every other live render setting in
            // this module uses (see e.g. `on_target_samples_changed`/`on_bounces_changed`
            // in `gui::mod`), rather than only taking effect after the next app restart.
            render_ctx_denoise
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .denoise_enabled = enabled;
            settings_store_denoise.update(|s| s.settings.denoise_enabled = enabled);
        });
}

/// Wires `on_test_worker_connection`, spawning a background thread that probes the
/// candidate address/cert-dir and reports compatibility back on the UI thread. Split
/// out of `setup_worker_callbacks` purely to keep that function under clippy's
/// function-length lint -- same reasoning as `setup_remote_render_samples_callback`
/// below.
fn setup_test_worker_connection_callback(ui: &MainWindow) {
    let ui_weak_test = ui.as_weak();
    ui.global::<RemoteWorkerModel>().on_test_worker_connection(
        move |idx: i32, address: slint::SharedString, cert_dir: slint::SharedString| {
            let Some(ui) = ui_weak_test.upgrade() else {
                return;
            };
            ui.global::<RemoteWorkerModel>()
                .set_testing_worker_index(idx);
            let worker = WorkerSettings {
                address: address.to_string(),
                cert_dir: cert_dir.to_string(),
                ..WorkerSettings::default()
            };
            let ui_weak_result = ui.as_weak();
            std::thread::spawn(move || {
                let result = remote_render::test_connection(&worker);
                let _ = ui_weak_result.upgrade_in_event_loop(move |ui| {
                    ui.global::<RemoteWorkerModel>()
                        .set_testing_worker_index(-1);
                    ui.global::<RemoteWorkerModel>()
                        .set_test_connection_result_index(idx);
                    match result {
                        Ok(info) => {
                            ui.global::<RemoteWorkerModel>()
                                .set_test_connection_is_error(false);
                            ui.global::<RemoteWorkerModel>().set_test_connection_result(
                                format!(
                                    "Compatible -- {} (protocol v{})",
                                    backend_label(info.render.as_ref()),
                                    info.protocol_version
                                )
                                .into(),
                            );
                        }
                        Err(err) => {
                            ui.global::<RemoteWorkerModel>()
                                .set_test_connection_is_error(true);
                            ui.global::<RemoteWorkerModel>()
                                .set_test_connection_result(err.to_string().into());
                        }
                    }
                });
            });
        },
    );
}

/// Wires the native folder picker for `form_cert_dir` ("Browse..." beside the
/// certificate-bundle-folder field in `remote_worker_dialog.slint`). Split out of
/// `setup_worker_callbacks` purely to keep that function under clippy's
/// function-length lint -- same reasoning as `setup_remote_render_samples_callback`
/// above.
///
/// Fills the field, doesn't validate or use it itself; `save_worker`/`test_connection`
/// still do that, unchanged, whichever way the folder got typed, pasted, or claimed via
/// enrollment token. Returns the SAME text it was given when the user cancels, so the
/// Slint-side assignment (`root.form_cert_dir = root.pick_cert_dir(root.form_cert_dir)`)
/// is a no-op on cancel.
///
/// The dialog runs off the UI thread through `gui::pickers::pick`, like every other
/// picker in this app. Slint has no way to await a value-returning callback, so
/// `on_pick_cert_dir` is void (`ui/models/remote_worker.slint`) and this closure
/// instead pushes the chosen folder into `RemoteWorkerModel.picked_cert_dir` once the
/// picker completes; `RemoteWorkerDialog`'s own `changed picked_cert_dir` handler
/// (`ui/components/remote_worker_dialog.slint`) copies it into `form_cert_dir`. Left
/// unchanged when the user cancels.
fn setup_cert_dir_picker_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_pick_cert_dir(move |current: slint::SharedString| {
            use crate::gui::pickers::{PickerKind, PickerRequest, pick};

            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let starting_dir = crate::gui::starting_dir_from_picker_field(current.as_str());
            let request = PickerRequest {
                kind: PickerKind::PickFolder,
                title: None,
                filters: Vec::new(),
                default_file_name: None,
                starting_dir,
            };
            pick(&ui, request, |ui, picked| {
                if let Some(path) = picked {
                    ui.global::<RemoteWorkerModel>()
                        .set_picked_cert_dir(path.display().to_string().into());
                }
            });
        });
}

/// Wires the "Live Compute" picker (`settings_dialog.slint`'s Local/Remote/Local+Remote
/// pills). Split out of `setup_worker_callbacks` purely to keep that function under
/// clippy's function-length lint -- same reasoning as
/// `setup_remote_render_samples_callback` just below.
///
/// Live-updates `RenderContext` (read fresh by `gui::remote::orchestrator::poll_tick` at
/// the NEXT settle -- see `RenderContext::live_compute_target`'s own doc comment) in
/// addition to persisting the choice, the same two-step pattern `on_denoise_toggled`
/// above already uses. Never touches `ctx.dirty` -- unlike a scene-shaping setting, this
/// only changes how a FUTURE settle behaves, never what's already on screen.
fn setup_live_compute_target_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store_compute = settings_store.clone();
    let render_ctx_compute = render_ctx.clone();
    ui.global::<SettingsModel>()
        .on_live_compute_target_changed(move |index: i32| {
            let target = live_compute_target_from_index(index);
            render_ctx_compute
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .live_compute_target = target;
            settings_store_compute.update(|s| s.settings.live_compute_target = target);
        });
}

/// Wires the "Remote Render Samples" slider. Split out of
/// `setup_worker_callbacks` purely to keep that function under clippy's
/// function-length lint -- this is one more global remote-rendering setting alongside
/// the denoise toggle it already wires, not a functionally distinct group.
///
/// Carries the slider's EXPONENT, same int-discriminant treatment as
/// `gui::mod::on_target_samples_changed` for the local Target Samples slider --
/// `remote_samples_exponent_to_count` is the one boundary crossing from "slider
/// position" to the actual count that's stored/dispatched. No `ctx.dirty`/redraw
/// needed: this only affects the NEXT remote render `start_remote_render` dispatches
/// (it reads `remote_render_samples` live at dispatch time), never the image already
/// on screen.
fn setup_remote_render_samples_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_remote_samples = render_ctx.clone();
    let settings_store_remote_samples = settings_store.clone();
    ui.global::<RemoteWorkerModel>()
        .on_render_samples_changed(move |exponent: i32| {
            let samples = remote_samples_exponent_to_count(u32::try_from(exponent).unwrap_or(0));
            render_ctx_remote_samples
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remote_render_samples = samples;
            settings_store_remote_samples.update(|s| s.settings.remote_render_samples = samples);
        });
}

/// Wires the "Redeem token" action in the worker add/edit form: given a worker name, an
/// enrollment address, and a pasted `GW1-...` token, claims the token and writes the
/// resulting certificate bundle to disk -- so a user enrolling a new worker never has to
/// install `indicatrix-worker` or run `cert claim` in a terminal. See `bridge::enroll`'s
/// module doc comment for where the bundle is written and why the user never chooses the
/// path.
///
/// Follows `on_test_worker_connection`'s exact shape, just above: the actual claim
/// (`bridge::enroll::claim_and_write_bundle`, a blocking TCP connect and TLS handshake)
/// runs on a plain `std::thread::spawn` thread, with the result marshalled back to the
/// Slint event loop via `Weak::upgrade_in_event_loop` -- see `bridge::export_thread`'s
/// module doc comment for the general pattern this and `on_test_worker_connection` both
/// follow.
///
/// The token is read out of the callback's own argument and moved into the background
/// closure; nothing here logs it, stores it in `settings::WorkerSettings`, or otherwise
/// keeps a copy once the claim attempt (success or failure) completes -- `claim_result_index`/
/// `claim_result_cert_dir` report the OUTCOME back to the dialog, never the token itself,
/// and `remote_worker_dialog.slint`'s own handler clears its `form_token` field on
/// success (see that file's `changed claim_result_cert_dir` handler).
fn setup_claim_token_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>().on_claim_token(
        move |index: i32,
              worker_name: slint::SharedString,
              enroll_addr: slint::SharedString,
              token: slint::SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.global::<RemoteWorkerModel>().set_claiming_token(true);
            ui.global::<RemoteWorkerModel>()
                .set_claim_result_index(index);
            ui.global::<RemoteWorkerModel>()
                .set_claim_result_text("".into());
            ui.global::<RemoteWorkerModel>()
                .set_claimed_cert_dir("".into());
            ui.global::<RemoteWorkerModel>()
                .set_claimed_address("".into());

            let settings_dir = crate::settings::store::default_settings_path()
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            let worker_name = worker_name.to_string();
            let enroll_addr = enroll_addr.to_string();
            let token = token.to_string();
            let suggested_address =
                crate::bridge::remote::enroll::suggested_serve_address(&enroll_addr);
            let ui_weak_result = ui.as_weak();
            std::thread::spawn(move || {
                let bundle_dir =
                    crate::bridge::remote::enroll::bundle_dir_for(&settings_dir, &worker_name);
                let result = crate::bridge::remote::enroll::claim_and_write_bundle(
                    &token,
                    &enroll_addr,
                    &bundle_dir,
                );
                let _ = ui_weak_result.upgrade_in_event_loop(move |ui| {
                    ui.global::<RemoteWorkerModel>().set_claiming_token(false);
                    match result {
                        Ok(dir) => {
                            ui.global::<RemoteWorkerModel>()
                                .set_claim_result_is_error(false);
                            ui.global::<RemoteWorkerModel>().set_claim_result_text(
                                "Token redeemed -- certificate folder filled in.".into(),
                            );
                            ui.global::<RemoteWorkerModel>()
                                .set_claimed_cert_dir(dir.display().to_string().into());
                            ui.global::<RemoteWorkerModel>()
                                .set_claimed_address(suggested_address.into());
                        }
                        Err(message) => {
                            ui.global::<RemoteWorkerModel>()
                                .set_claim_result_is_error(true);
                            ui.global::<RemoteWorkerModel>()
                                .set_claim_result_text(message.into());
                        }
                    }
                });
            });
        },
    );
}

/// Human-readable description of what a connected server can render.
///
/// Takes the whole `Option` rather than a `Backend`, because a server legitimately may
/// have no render capacity at all: since the worker's render path moved behind its
/// `worker` feature, a library-only build advertises `render: None`. The viewer must say
/// so plainly rather than imply a renderer that is not there.
pub(super) fn backend_label(render: Option<&indicatrix_net::messages::RenderCapability>) -> String {
    match render.map(|r| &r.backend) {
        Some(indicatrix_net::messages::Backend::Cpu { threads }) => {
            format!("CPU, {threads} threads")
        }
        Some(indicatrix_net::messages::Backend::Gpu { adapter }) => format!("GPU ({adapter})"),
        None => "library only (no render capacity)".to_string(),
    }
}
