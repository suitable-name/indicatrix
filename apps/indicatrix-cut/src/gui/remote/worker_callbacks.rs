//! The "Remote coordinator" form (save/remove; there is only one remote endpoint),
//! "Test connection", token-based enrollment, the global denoise toggle, and the live
//! compute-target / live-transfer pickers.
//!
//! Split out of `gui::remote` purely to keep that module (already sizeable) from
//! growing further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`
//! itself.

use super::{live_compute_target_from_index, worker_settings::from_worker_item};
use crate::{
    MainWindow, RemoteWorkerModel, SettingsModel, WorkerItem,
    bridge::{export_thread, remote::remote_render, render_thread::RenderContext},
    gui::{
        remote::{RemoteOrchestratorHandle, refresh_remote_ui},
        show_toast,
    },
    settings::{LiveTransfer, SettingsPersister, WorkerSettings},
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, PoisonError};

// ---- Remote endpoint form + "Test connection" + denoise toggle -------------------

/// Wires the "Remote coordinator" form's save/remove, "Test connection", and the
/// global denoise-toggle callbacks. Split out of `setup_remote_rendering` purely to
/// keep that function shorter. `orchestrator` is the handle `setup_remote_rendering`
/// itself returns -- see `setup_denoise_toggle_callback`'s own doc comment for why the
/// denoise toggle needs it.
pub fn setup_worker_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    orchestrator: &RemoteOrchestratorHandle,
) {
    setup_save_remote_callback(ui, settings_store);
    setup_remove_remote_callback(ui, settings_store);
    setup_denoise_toggle_callback(ui, render_ctx, settings_store, orchestrator);
    setup_live_compute_target_callback(ui, render_ctx, settings_store);
    setup_live_transfer_callback(ui, render_ctx, settings_store);
    setup_contribute_to_final_picture_callback(ui, settings_store);
    setup_claim_token_callback(ui);
    setup_test_worker_connection_callback(ui);
    setup_cert_dir_picker_callback(ui);
}

/// Wires `on_save_remote`: the form replaces the one endpoint (keeping its live
/// transfer, which the settings dialog owns). Forgets any remembered "this remote
/// refuses final pictures" (the address may now point at an upgraded coordinator).
fn setup_save_remote_callback(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let settings_store_save = settings_store.clone();
    let ui_weak_save = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_save_remote(move |item: WorkerItem| {
            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            settings_store_save.update(|s| {
                let endpoint = from_worker_item(&item, s.settings.remote.as_ref());
                s.settings.remote = Some(endpoint);
            });
            export_thread::forget_final_picture_refusals();
            refresh_remote_ui(&ui, settings_store_save.snapshot().settings.remote.as_ref());
            show_toast(&ui, "Remote coordinator saved.", "success");
        });
}

/// Wires `on_remove_remote`: renders stay local afterwards.
fn setup_remove_remote_callback(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let settings_store_remove = settings_store.clone();
    let ui_weak_remove = ui.as_weak();
    ui.global::<RemoteWorkerModel>().on_remove_remote(move || {
        let Some(ui) = ui_weak_remove.upgrade() else {
            return;
        };
        settings_store_remove.update(|s| s.settings.remote = None);
        refresh_remote_ui(&ui, None);
        show_toast(
            &ui,
            "Remote coordinator removed -- renders stay local.",
            "info",
        );
    });
}

/// Wires the settings dialog's "Live Transfer" pills (full data or final picture only), stored on
/// the remote endpoint. An actual change while a settled epoch is live releases it
/// (`RenderContext::release_remote`), exactly like a live-compute-target change: the
/// orchestrator re-dispatches the settled view with the new transfer on its next tick.
fn setup_live_transfer_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store = settings_store.clone();
    let render_ctx = render_ctx.clone();
    ui.global::<SettingsModel>()
        .on_live_transfer_changed(move |index: i32| {
            let transfer = LiveTransfer::from_index(index);
            let mut changed = false;
            settings_store.update(|s| {
                if let Some(remote) = s.settings.remote.as_mut() {
                    changed = remote.live_transfer != transfer;
                    remote.live_transfer = transfer;
                }
            });
            if changed {
                let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
                if ctx.remote_active || ctx.live_epoch.is_some() {
                    ctx.release_remote();
                }
            }
        });
}

/// Wires the settings dialog's "Final-picture exports: this machine renders a share
/// too" switch (v16). Unlike [`setup_live_transfer_callback`]/
/// [`setup_live_compute_target_callback`] just above, this only PERSISTS the choice --
/// no `RenderContext` field to keep in step, because an export reads the settings
/// snapshot fresh every time it dispatches (`gui::render::render_export::wiring`,
/// `gui::tilt::video_export`), unlike the live viewport's own settled epoch, which a
/// live-affecting setting must actively release.
fn setup_contribute_to_final_picture_callback(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_contribute_to_final_picture_changed(move |on: bool| {
            settings_store.update(|s| s.settings.contribute_to_final_picture = on);
        });
}

/// Wires the global denoise toggle. Split out of `setup_worker_callbacks` purely to
/// keep that function under clippy's function-length lint -- same reasoning as
/// `setup_live_compute_target_callback` below.
fn setup_denoise_toggle_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    orchestrator: &RemoteOrchestratorHandle,
) {
    let settings_store_denoise = settings_store.clone();
    let render_ctx_denoise = render_ctx.clone();
    let orchestrator_denoise = orchestrator.clone();
    let ui_weak_denoise = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_denoise_toggled(move |enabled: bool| {
            // Live-updates `RenderContext` (governs the render loop immediately, both the
            // local readback in `render_thread`'s own frame loop and the remote
            // merged-accumulation readback in `orchestrator::tick::update::
            // redraw_from_epoch`) in addition to persisting the choice -- the same
            // two-step pattern every other live render setting in this module uses (see
            // e.g. `on_target_samples_changed`/`on_bounces_changed` in `gui::mod`),
            // rather than only taking effect after the next app restart.
            let mut ctx = render_ctx_denoise
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            ctx.denoise_enabled = enabled;
            // One-shot re-tonemap request for the `Both`/local-only render loop --
            // see `RenderContext::redisplay_requested`'s own doc comment for why this
            // is needed at all (accumulation may have already converged, so nothing
            // would otherwise ever read `denoise_enabled` again).
            ctx.redisplay_requested = true;
            drop(ctx);
            settings_store_denoise.update(|s| s.settings.denoise_enabled = enabled);
            // `RemoteOnly` suspends the render thread's loop entirely, so
            // `redisplay_requested` above has no frame-loop iteration to be read back
            // on -- force the orchestrator's own display path instead. A no-op in
            // every other live-compute-target (`request_redisplay`'s own doc comment).
            if let Some(ui) = ui_weak_denoise.upgrade() {
                orchestrator_denoise.request_redisplay(&ui, &render_ctx_denoise);
            }
        });
}

/// Wires `on_test_worker_connection`, spawning a background thread that probes the
/// candidate address/cert-dir and reports compatibility back on the UI thread. Split
/// out of `setup_worker_callbacks` purely to keep that function under clippy's
/// function-length lint -- same reasoning as `setup_live_compute_target_callback`
/// below.
fn setup_test_worker_connection_callback(ui: &MainWindow) {
    let ui_weak_test = ui.as_weak();
    ui.global::<RemoteWorkerModel>().on_test_worker_connection(
        move |address: slint::SharedString, cert_dir: slint::SharedString| {
            let Some(ui) = ui_weak_test.upgrade() else {
                return;
            };
            ui.global::<RemoteWorkerModel>()
                .set_testing_connection(true);
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
                        .set_testing_connection(false);
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
/// function-length lint -- same reasoning as `setup_live_compute_target_callback`
/// below.
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
            // Cleared before the dialog even opens, not just left for the
            // completion closure below to overwrite: picking the SAME folder
            // this field already holds `picked_cert_dir` from a previous pick
            // would otherwise be a same-value write, and `RemoteWorkerDialog`'s
            // own `changed picked_cert_dir` handler (`remote_worker_dialog.
            // slint`) -- like every Slint `changed` handler -- never fires for
            // one of those,
            // silently dropping a genuine re-pick of the same folder. Clearing
            // it here first guarantees the picker's own completion below is
            // always a real `"" -> <folder>` transition, and "" is already
            // documented (that handler's own comment) as never copied into
            // `form_cert_dir`, so a cancelled picker leaves nothing to notice.
            ui.global::<RemoteWorkerModel>()
                .set_picked_cert_dir("".into());
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
/// clippy's function-length lint.
///
/// Live-updates `RenderContext` (read fresh by `gui::remote::orchestrator::poll_tick` at
/// the NEXT settle -- see `RenderContext::live_compute_target`'s own doc comment) in
/// addition to persisting the choice, the same two-step pattern `on_denoise_toggled`
/// above already uses. An ACTUAL change while a settled epoch is live releases it
/// (`RenderContext::release_remote`, which restarts local from a clean buffer): the
/// render loop only claims from the epoch's cursor while combining, so switching modes
/// mid-epoch would otherwise re-trace indices local already took from it, and
/// `RemoteOnly` <-> `Both` also changes who owns the display. The orchestrator notices
/// the release on its next tick and cancels the epoch's remote lane.
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
            {
                let mut ctx = render_ctx_compute
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if ctx.live_compute_target != target
                    && (ctx.remote_active || ctx.live_epoch.is_some())
                {
                    ctx.release_remote();
                }
                ctx.live_compute_target = target;
            }
            settings_store_compute.update(|s| s.settings.live_compute_target = target);
        });
}

/// Wires the "Redeem token" action in the worker add/edit form: given a worker name, an
/// enrollment address, and a pasted `GW1-...` token, claims the token and writes the
/// resulting certificate bundle to disk -- so a user enrolling a new worker never has to
/// install `indicatrix-worker` or run `cert claim` in a terminal. See `bridge::remote::enroll`'s
/// module doc comment for where the bundle is written and why the user never chooses the
/// path.
///
/// Follows `on_test_worker_connection`'s exact shape, just above: the actual claim
/// (`bridge::remote::enroll::claim_and_write_bundle`, a blocking TCP connect and TLS handshake)
/// runs on a plain `std::thread::spawn` thread, with the result marshalled back to the
/// Slint event loop via `Weak::upgrade_in_event_loop` -- see `bridge::export_thread`'s
/// module doc comment for the general pattern this and `on_test_worker_connection` both
/// follow.
///
/// The token is read out of the callback's own argument and moved into the background
/// closure; nothing here logs it, stores it in `settings::WorkerSettings`, or otherwise
/// keeps a copy once the claim attempt (success or failure) completes -- `claim_result_text`/
/// `claimed_cert_dir` report the OUTCOME back to the dialog, never the token itself,
/// and `remote_worker_dialog.slint`'s own handler clears its `form_token` field on
/// success (see that file's `changed claim_result_cert_dir` handler).
fn setup_claim_token_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>().on_claim_token(
        move |worker_name: slint::SharedString,
              enroll_addr: slint::SharedString,
              token: slint::SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.global::<RemoteWorkerModel>().set_claiming_token(true);
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
        Some(indicatrix_net::messages::Backend::Coordinator { workers, .. }) => {
            coordinator_label(*workers)
        }
        None => "library only (no render capacity)".to_string(),
    }
}

/// "coordinator (N workers)" -- what "served by" names a coordinator, counting the
/// workers joined to it (its own `--render` lane, if any, is not a worker).
fn coordinator_label(workers: u32) -> String {
    if workers == 1 {
        "coordinator (1 worker)".to_string()
    } else {
        format!("coordinator ({workers} workers)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_net::messages::{Backend, RenderCapability};

    fn capability(backend: Backend) -> RenderCapability {
        RenderCapability {
            backend,
            max_pixels: 1,
            min_cadence_ms: 100,
            hdr: false,
        }
    }

    #[test]
    fn served_by_names_a_coordinator_by_its_worker_count() {
        let three = capability(Backend::Coordinator {
            workers: 3,
            threads: 16,
            gpus: 1,
        });
        assert_eq!(backend_label(Some(&three)), "coordinator (3 workers)");
        let one = capability(Backend::Coordinator {
            workers: 1,
            threads: 0,
            gpus: 0,
        });
        assert_eq!(backend_label(Some(&one)), "coordinator (1 worker)");
        assert_eq!(
            backend_label(Some(&capability(Backend::Cpu { threads: 8 }))),
            "CPU, 8 threads"
        );
        assert_eq!(backend_label(None), "library only (no render capacity)");
    }
}
