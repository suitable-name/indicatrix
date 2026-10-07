//! A design's own lighting, remembered in the library database.
//!
//! The design file holds the design and nothing else, so a lighting set-up the cutter
//! likes for one design lives in the local library, keyed by the design's UUID
//! (`indicatrix_vault`'s `design_lighting` table). Two buttons in the settings dialog
//! write it ("Use this lighting for this design") and clear it ("Forget for this
//! design"); opening a design that has some shows it in the live view.
//!
//! # The normal lighting is never overwritten
//!
//! A design's lighting is applied to the live view only (the render context and the
//! controls that mirror it), never to the persisted settings. The first time a saved
//! lighting is shown, the app's normal lighting (what the settings file holds, plus the
//! HDR map in use) is remembered; opening a design that has none, or forgetting the
//! lighting, puts it back. The light controls stay wired to the settings as before, so a
//! change made while a design's lighting is showing would otherwise leak into them. Two
//! things undo that: the remembered values are pinned over the lighting fields of every
//! write to the settings file for as long as the design's lighting shows
//! ([`SettingsPersister::set_disk_override`], so closing the app, or killing it, leaves the
//! normal lighting in the file), and the restore writes them back into the in-memory
//! settings too. Applying a named lighting preset is the exception: it is a choice of
//! normal lighting, so [`normal_lighting_chosen`] drops the remembered values and the pin.
//!
//! # Nothing here waits on the library or the disk
//!
//! A design opens on the UI thread. The saved lighting is read with a lock that never
//! waits; when the library is busy the read moves to a thread and the lighting is applied
//! when it lands. An HDR map the lighting names is reused when it is the one already loaded,
//! and decoded on a thread otherwise. Both answers carry the number of the opening they were
//! asked for ([`Session::open_generation`]) and are dropped when another design has opened,
//! or the lighting was forgotten or replaced, since. The dialog keeps showing the earlier map
//! until a decode lands, so "Use this lighting for this design" pressed meanwhile stores the
//! map being decoded ([`Session::pending_env_path`]), not the one on screen. A map the cutter
//! loads or clears by hand in that time is the newer choice ([`env_map_chosen`]): it voids the
//! decode, so the decode does not land over it and the map the dialog shows is the one stored.
//! The same holds earlier, while the lookup itself waits for a busy library: a map chosen by
//! hand since the opening began ([`map_chosen_since_open`]) is kept when the lookup lands, and
//! the saved lighting's map is neither decoded nor allowed to clear it.
//!
//! # What is stored
//!
//! [`LightingValues`] as JSON: the lighting rig, the light's position, exposure, surface
//! glare, backdrop and the HDR environment map path, if one is in use. The camera is not
//! part of it. The decisions (what a stored row means, what to do when a design opens)
//! are pure functions, tested without a window; the rest is thin glue over Slint.

mod opening;
mod stored;

use crate::{
    DesignLightingModel, MainWindow, SettingsModel, ViewportModel,
    bridge::render_thread::{RenderContext, load_env_map},
    gui::{env_map_status_text, show_toast, tutorial_events::raise},
    settings::{SettingsPersister, model::percent_from_surface_glare},
};
use indicatrix::{optics::LightingPreset, renderer::env_map::EnvironmentMap};
use indicatrix_editor::guide::viewing_events as events;
use indicatrix_vault::{db::sqlite::Database, model::design_key::normalize_design_uuid};
use opening::{
    Lookup, Note, Plan, decide, first_time_for, lookup_from_row, may_show_over, note_text,
};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    collections::HashSet,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, PoisonError, TryLockError},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use stored::{
    EnvSource, LightingValues, PITCH_RANGE_RAD, answer_is_current, env_path_for_capture,
    env_path_to_store, env_source_for_opening,
};

/// How long the notes about opening a design wait, so the "Loaded ..." message the open
/// itself shows can be read first.
const NOTE_DELAY: Duration = Duration::from_millis(1200);

// ---------------------------------------------------------------------------------------
// The database.
// ---------------------------------------------------------------------------------------

/// What the library holds for design `uuid`, reading it on the caller's thread with the
/// library's lock held for the one-row read.
fn read_with(db: &Database, uuid: &str) -> Lookup {
    match db.design_lighting(uuid) {
        Ok(row) => lookup_from_row(row.as_ref().map(|row| row.settings_json.as_str())),
        Err(err) => Lookup::Unreadable(format!("{err:#}")),
    }
}

/// What the library holds for design `uuid`. Waits for the library's lock: call it on a
/// thread of its own, or in a test.
fn read_lookup(db: &Mutex<Database>, uuid: &str) -> Lookup {
    read_with(&db.lock().unwrap_or_else(PoisonError::into_inner), uuid)
}

/// [`read_lookup`] for the UI thread: `None` when the library is busy right now, instead of
/// waiting for it.
fn try_read_lookup(db: &Mutex<Database>, uuid: &str) -> Option<Lookup> {
    let guard = match db.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return None,
    };
    Some(read_with(&guard, uuid))
}

/// Stores `values` as design `uuid`'s lighting. `now` is Unix seconds.
fn store_values(
    db: &Mutex<Database>,
    uuid: &str,
    values: &LightingValues,
    now: i64,
) -> Result<(), String> {
    let json = values.to_json()?;
    db.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .set_design_lighting(uuid, &values.lighting_rig, &json, now)
        .map_err(|err| format!("{err:#}"))
}

/// Removes design `uuid`'s stored lighting. Nothing stored is not an error.
fn forget_values(db: &Mutex<Database>, uuid: &str) -> Result<(), String> {
    db.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear_design_lighting(uuid)
        .map(|_| ())
        .map_err(|err| format!("{err:#}"))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

// ---------------------------------------------------------------------------------------
// The window side.
// ---------------------------------------------------------------------------------------

/// The shared handles the callbacks and the open hook work through.
#[derive(Clone)]
struct Handles {
    db: Arc<Mutex<Database>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    settings_store: Arc<SettingsPersister>,
}

/// The HDR controls' state: what the settings dialog shows about the environment map.
#[derive(Clone)]
struct EnvUi {
    loaded: bool,
    status: String,
    path_text: String,
}

/// The app's normal lighting, remembered while a design's own lighting is showing.
struct Normal {
    values: LightingValues,
    /// The decoded HDR map in use, so putting it back needs no file read.
    env_map: Option<Arc<EnvironmentMap>>,
    env_ui: EnvUi,
}

/// What this window remembers between design opens.
#[derive(Default)]
struct Session {
    handles: Option<Handles>,
    /// The open design's UUID (lowercase); `None` when no design is open.
    design: Option<String>,
    /// `Some` exactly while a design's own lighting is showing.
    normal: Option<Normal>,
    /// Designs whose unusable saved lighting has been reported already.
    told: HashSet<String>,
    /// The number of the latest design opening, forget or normal-lighting choice. A lookup
    /// or an HDR decode started for an earlier number is stale when it lands and is dropped.
    open_generation: u64,
    /// The HDR map the lighting now showing names while a thread is still decoding it. The
    /// settings dialog keeps showing the earlier map until the decode lands, so "Use this
    /// lighting for this design" reads the map from here first ([`env_path_to_store`]).
    /// Cleared when the decode answers, fails to start, or any newer opening or reset begins,
    /// or when the cutter loads or clears a map by hand ([`env_map_chosen`]).
    pending_env_path: Option<String>,
    /// The number of the latest HDR decode started or manual map choice made. A decode that
    /// finds its number outdated when it lands is dropped, so the newest of the cutter's own
    /// actions and the saved lighting's map is the one that stays.
    env_decode: u64,
    /// What [`Self::env_decode`] read when the latest opening, forget or normal-lighting
    /// choice began. A different number now means the cutter loaded or cleared a map by hand
    /// since ([`map_chosen_since_open`]), which a lookup that lands late must not undo.
    env_decode_at_open: u64,
}

thread_local! {
    /// The window's session. Slint callbacks and the editor run on one thread, so a
    /// thread-local needs no lock, the same way the editor's other per-window state works.
    static SESSION: RefCell<Session> = RefCell::new(Session::default());
}

fn handles() -> Option<Handles> {
    SESSION.with(|session| session.borrow().handles.clone())
}

fn open_design() -> Option<String> {
    SESSION.with(|session| session.borrow().design.clone())
}

fn override_active() -> bool {
    SESSION.with(|session| session.borrow().normal.is_some())
}

/// Makes everything still on its way for an earlier opening (a lookup waiting for the
/// library, an HDR map being decoded) stale, and returns the number of what starts now.
fn begin_generation() -> u64 {
    SESSION.with(|session| {
        let mut session = session.borrow_mut();
        session.open_generation = session.open_generation.wrapping_add(1);
        // A decode for an earlier number is void, so the map it was decoding is not the one
        // the lighting names any more.
        session.pending_env_path = None;
        // Map choices from here on are newer than this opening.
        session.env_decode_at_open = session.env_decode;
        session.open_generation
    })
}

/// Whether the cutter loaded or cleared an HDR map by hand ([`env_map_chosen`]) since the
/// latest opening began. Read when the saved lighting lands: a lookup that waited for a busy
/// library finds the cutter's map newer than the saved one.
fn map_chosen_since_open() -> bool {
    SESSION.with(|session| {
        let session = session.borrow();
        session.env_decode != session.env_decode_at_open
    })
}

/// The HDR map being decoded for the lighting now showing, if a decode is on its way.
fn pending_env_path() -> Option<String> {
    SESSION.with(|session| session.borrow().pending_env_path.clone())
}

/// Remembers (`Some`) or forgets (`None`) the HDR map being decoded, see
/// [`Session::pending_env_path`].
fn set_pending_env_path(path: Option<String>) {
    SESSION.with(|session| session.borrow_mut().pending_env_path = path);
}

/// Starts the books on a decode of the HDR map at `path`: it is the map being decoded now, and
/// the returned number is what its answer must still match when it lands.
fn begin_env_decode(path: &str) -> u64 {
    SESSION.with(|session| {
        let mut session = session.borrow_mut();
        session.env_decode = session.env_decode.wrapping_add(1);
        session.pending_env_path = Some(path.to_owned());
        // This decode is the opening's own action, not a choice of the cutter's: it must not
        // read as "a map chosen by hand since the opening began".
        session.env_decode_at_open = session.env_decode;
        session.env_decode
    })
}

/// Whether a decode started under number `asked` is still the newest map action.
fn env_decode_is_current(asked: u64) -> bool {
    SESSION.with(|session| answer_is_current(asked, session.borrow().env_decode))
}

/// The cutter loaded an HDR map by hand, or cleared it: that is the newest word on which map is
/// in use. A decode for the design's saved lighting that is still on its way is void (it would
/// land over the cutter's choice), and so is the map it was decoding, so "Use this lighting for
/// this design" stores the map the dialog shows. The rest of the design's lighting, and a lookup
/// still waiting for the library, are not affected.
pub(in crate::gui) fn env_map_chosen() {
    SESSION.with(|session| {
        let mut session = session.borrow_mut();
        session.env_decode = session.env_decode.wrapping_add(1);
        session.pending_env_path = None;
    });
}

/// Whether an answer asked for under `asked` still counts.
fn generation_is_current(asked: u64) -> bool {
    SESSION.with(|session| answer_is_current(asked, session.borrow().open_generation))
}

/// Wires the settings dialog's two buttons and remembers the handles the open hook
/// ([`design_opened`]) needs. Called once, when the window is built.
pub(in crate::gui) fn setup_design_lighting_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    SESSION.with(|session| {
        session.borrow_mut().handles = Some(Handles {
            db: Arc::clone(db),
            render_ctx: Arc::clone(render_ctx),
            settings_store: Arc::clone(settings_store),
        });
    });

    let weak = ui.as_weak();
    ui.global::<DesignLightingModel>()
        .on_use_for_design(move || {
            if let Some(ui) = weak.upgrade() {
                use_lighting_for_design(&ui);
            }
        });
    let weak = ui.as_weak();
    ui.global::<DesignLightingModel>()
        .on_forget_for_design(move || {
            if let Some(ui) = weak.upgrade() {
                forget_lighting_for_design(&ui);
            }
        });
}

/// The hook a design replacement calls once the new design is in place: shows the
/// design's saved lighting if it has some, or brings the normal lighting back.
///
/// `design_uuid` is the design's UUID (`EditorState::design_uuid`); `has_design` is false
/// for the empty startup placeholder, which has nothing to save lighting for. Does
/// nothing before [`setup_design_lighting_callbacks`] has run.
pub(in crate::gui) fn design_opened(ui: &MainWindow, design_uuid: &str, has_design: bool) {
    let Some(handles) = handles() else {
        return;
    };
    let uuid = if has_design {
        normalize_design_uuid(design_uuid)
    } else {
        None
    };
    // Whatever is still on its way for the design before this one is void now.
    let generation = begin_generation();
    SESSION.with(|session| session.borrow_mut().design.clone_from(&uuid));
    let Some(uuid) = uuid else {
        finish_open(ui, &handles, None, &Lookup::NoRow, generation);
        return;
    };
    // The saved lighting is one row, usually read at once. When the library is busy with
    // another job the read moves to a thread instead of freezing the window.
    match try_read_lookup(&handles.db, &uuid) {
        Some(lookup) => finish_open(ui, &handles, Some(&uuid), &lookup, generation),
        None => read_lookup_on_a_thread(ui, &handles, uuid, generation),
    }
}

/// Reads design `uuid`'s saved lighting on a thread and finishes the opening when it lands,
/// unless another design opened (or the lighting was forgotten) meanwhile.
fn read_lookup_on_a_thread(ui: &MainWindow, shared: &Handles, uuid: String, generation: u64) {
    let db = Arc::clone(&shared.db);
    let weak = ui.as_weak();
    let spawned = thread::Builder::new()
        .name("design-lighting-read".to_owned())
        .spawn(move || {
            let lookup = read_lookup(&db, &uuid);
            let _ = weak.upgrade_in_event_loop(move |ui| {
                if !generation_is_current(generation) {
                    return;
                }
                if let Some(handles) = handles() {
                    finish_open(&ui, &handles, Some(&uuid), &lookup, generation);
                }
            });
        });
    if spawned.is_err() {
        // No thread to read it on: say so, and keep the normal lighting.
        let unreadable = Lookup::Unreadable("the program could not start the work".to_owned());
        let uuid = open_design();
        finish_open(ui, shared, uuid.as_deref(), &unreadable, generation);
    }
}

/// The second half of [`design_opened`], once what the library holds for the design is known:
/// does what [`decide`] says, tells the dialog, and announces it.
fn finish_open(
    ui: &MainWindow,
    handles: &Handles,
    uuid: Option<&str>,
    lookup: &Lookup,
    generation: u64,
) {
    let showing_own = override_active();
    let decision = decide(lookup, showing_own);
    match decision.plan {
        Plan::Keep => {}
        Plan::Apply(values) => {
            if !showing_own {
                begin_override(ui, handles);
            }
            apply_values(ui, handles, &values, generation);
        }
        Plan::Restore => restore_normal_lighting(ui, handles),
    }
    publish_model(ui, uuid.is_some(), lookup);

    let announce = decision
        .note
        .as_ref()
        .filter(|note| should_announce(note, uuid));
    if let Some(note) = announce {
        let (text, kind) = note_text(note);
        toast_after_open(ui, text, kind);
    }
}

/// Whether `note` is to be shown now: a problem is shown once per design and session, the
/// ordinary notes every time.
fn should_announce(note: &Note, design_uuid: Option<&str>) -> bool {
    match (note, design_uuid) {
        (Note::UnknownRig(_) | Note::Unreadable(_), Some(uuid)) => {
            SESSION.with(|session| first_time_for(&mut session.borrow_mut().told, uuid))
        }
        _ => true,
    }
}

/// Lets go of a design's own lighting that is showing, without changing what is on screen:
/// the lighting now in the settings is the normal lighting.
///
/// Called when the cutter applies a named lighting preset, which is a choice of normal
/// lighting (it is written to the settings) and so must not be undone the next time a
/// design without saved lighting opens. The design's saved lighting stays saved.
pub(in crate::gui) fn normal_lighting_chosen() {
    // A lookup or an HDR decode on its way for the design is void: the cutter chose.
    begin_generation();
    let showing_own = SESSION.with(|session| session.borrow_mut().normal.take().is_some());
    if showing_own && let Some(handles) = handles() {
        // The settings now hold the preset's look, and that is what the file should get.
        handles.settings_store.set_disk_override(None);
    }
}

/// Puts the app's normal lighting back, if a design's own is showing.
fn restore_normal_lighting(ui: &MainWindow, handles: &Handles) {
    // A lookup or an HDR decode on its way for the design is void now.
    begin_generation();
    let normal = SESSION.with(|session| session.borrow_mut().normal.take());
    if let Some(normal) = normal {
        restore_normal(ui, handles, &normal);
        // The settings hold the normal lighting again: let the file take what they hold.
        handles.settings_store.set_disk_override(None);
    }
}

/// Starts showing a design's own lighting: remembers the normal lighting for the day it
/// ends, and pins it over the lighting fields of every write to the settings file until then
/// ([`SettingsPersister::set_disk_override`]), so the light controls, which write the
/// settings as they are moved, cannot leave the design's values there.
fn begin_override(ui: &MainWindow, handles: &Handles) {
    let normal = capture_normal(ui, handles);
    handles
        .settings_store
        .set_disk_override(Some(normal.values.disk_pin()));
    SESSION.with(|session| session.borrow_mut().normal = Some(normal));
}

/// Reads the app's normal lighting: the settings file's values, the HDR map in use and
/// what the HDR controls show.
fn capture_normal(ui: &MainWindow, handles: &Handles) -> Normal {
    let values = LightingValues::from_app_settings(&handles.settings_store.snapshot().settings);
    let env_map = RenderContext::lock(&handles.render_ctx).env_map.clone();
    let settings = ui.global::<SettingsModel>();
    let env_ui = EnvUi {
        loaded: settings.get_env_map_loaded(),
        status: settings.get_env_map_status().to_string(),
        path_text: settings.get_env_map_path().to_string(),
    };
    Normal {
        values,
        env_map,
        env_ui,
    }
}

/// Shows `values` in the live view: the render context and the controls that mirror it.
/// The settings file is not touched.
///
/// The HDR map the lighting names is never decoded here. When it is the map already loaded,
/// it stays as it is; when it is another file, the old environment stays until a thread has
/// decoded the new one ([`start_env_decode`]); when the lighting names none, the studio rig
/// comes back.
fn apply_values(ui: &MainWindow, handles: &Handles, values: &LightingValues, generation: u64) {
    let rig = values.rig().unwrap_or_default();
    let loaded = loaded_env_path(ui, handles);
    let source = env_source_for_opening(
        map_chosen_since_open(),
        values.env_map_path.as_deref(),
        loaded.as_deref(),
    );
    {
        let mut ctx = RenderContext::lock(&handles.render_ctx);
        push_values_to_context(&mut ctx, values, rig);
        if source == EnvSource::Studio {
            ctx.env_map = None;
        }
        ctx.dirty = true;
    }
    push_values_to_controls(ui, values, rig);
    match source {
        EnvSource::Studio => {
            let settings = ui.global::<SettingsModel>();
            settings.set_env_map_status(String::new().into());
            settings.set_env_map_loaded(false);
        }
        // The controls already show this map, or the cutter's own choice stays (the rest of
        // the saved lighting is applied either way).
        EnvSource::AlreadyLoaded | EnvSource::ChosenByHand => {}
        EnvSource::Decode(path) => start_env_decode(ui, path, generation),
    }
}

/// The path of the HDR map in use: the one in the settings dialog's field, but only while a
/// map is actually loaded (see [`env_path_for_capture`]).
fn loaded_env_path(ui: &MainWindow, handles: &Handles) -> Option<String> {
    let has_map = RenderContext::lock(&handles.render_ctx).env_map.is_some();
    let settings = ui.global::<SettingsModel>();
    env_path_for_capture(
        has_map && settings.get_env_map_loaded(),
        settings.get_env_map_path().as_str(),
    )
}

/// Decodes the HDR map at `path` on a thread and applies it when it lands, unless the opening
/// it was started for is over by then. A map that cannot be read is said so, and the
/// environment in use is kept.
fn start_env_decode(ui: &MainWindow, path: String, generation: u64) {
    // Until the map lands the dialog still shows the earlier one: saving the lighting in the
    // meantime must store this path, not that one. Unless the cutter picks a map of their own
    // meanwhile ([`env_map_chosen`]), which voids this decode.
    let decode = begin_env_decode(&path);
    let weak = ui.as_weak();
    let spawned = thread::Builder::new()
        .name("design-lighting-hdr".to_owned())
        .spawn(move || {
            let decoded = catch_unwind(AssertUnwindSafe(|| load_env_map(&path)))
                .unwrap_or_else(|_| Err("the file could not be decoded".to_owned()));
            let _ = weak.upgrade_in_event_loop(move |ui| {
                env_decoded(&ui, &path, decoded, generation, decode);
            });
        });
    if spawned.is_err() {
        // Nothing is decoding, so the earlier map is what stays in use.
        set_pending_env_path(None);
        toast_after_open(
            ui,
            "This design's saved lighting uses an HDR environment map that could not be \
             loaded (the program could not start the work). The current environment is kept."
                .to_owned(),
            "warning",
        );
    }
}

/// [`start_env_decode`]'s answer, back on the UI thread: shows the map, or says it could not
/// be loaded. Dropped when the design's lighting is not showing any more, another design
/// opened since the decode started, or the cutter chose a map by hand since ([`env_map_chosen`]):
/// the newest choice wins.
fn env_decoded(
    ui: &MainWindow,
    path: &str,
    decoded: Result<Arc<EnvironmentMap>, String>,
    generation: u64,
    decode: u64,
) {
    if !generation_is_current(generation) || !env_decode_is_current(decode) {
        return;
    }
    // The decode this answer belongs to is over, whatever it brought.
    set_pending_env_path(None);
    if !override_active() {
        return;
    }
    let Some(handles) = handles() else {
        return;
    };
    match decoded {
        Ok(map) => {
            {
                let mut ctx = RenderContext::lock(&handles.render_ctx);
                ctx.env_map = Some(Arc::clone(&map));
                ctx.dirty = true;
            }
            let settings = ui.global::<SettingsModel>();
            settings.set_env_map_status(env_map_status_text(&map, path).into());
            settings.set_env_map_loaded(true);
            settings.set_env_map_path(path.into());
        }
        Err(err) => toast_after_open(
            ui,
            format!(
                "This design's saved lighting uses an HDR environment map that could not be \
                 loaded ({err}). The current environment is kept."
            ),
            "warning",
        ),
    }
}

/// Puts `normal` back: the render context, the controls, and the settings file.
fn restore_normal(ui: &MainWindow, handles: &Handles, normal: &Normal) {
    let values = &normal.values;
    let rig = crate::gui::optics::offered_lighting::offered_from_label(&values.lighting_rig);
    {
        let mut ctx = RenderContext::lock(&handles.render_ctx);
        push_values_to_context(&mut ctx, values, rig);
        ctx.env_map.clone_from(&normal.env_map);
        ctx.dirty = true;
    }
    // Anything the light controls wrote while a design's own lighting was showing is
    // undone here, so the normal lighting is exactly what it was.
    handles
        .settings_store
        .update(|file| values.write_into(&mut file.settings));
    push_values_to_controls(ui, values, rig);
    let settings = ui.global::<SettingsModel>();
    settings.set_env_map_loaded(normal.env_ui.loaded);
    settings.set_env_map_status(normal.env_ui.status.clone().into());
    settings.set_env_map_path(normal.env_ui.path_text.clone().into());
}

/// The render-context half of showing `values`.
const fn push_values_to_context(
    ctx: &mut RenderContext,
    values: &LightingValues,
    rig: LightingPreset,
) {
    ctx.light_yaw = values.light_yaw_deg.to_radians();
    ctx.light_pitch = values
        .light_pitch_deg
        .to_radians()
        .clamp(PITCH_RANGE_RAD.0, PITCH_RANGE_RAD.1);
    ctx.exposure = values.exposure;
    ctx.lighting_preset = rig;
    ctx.surface_glare = values.surface_glare;
    ctx.backdrop = values.backdrop;
}

/// The controls half of showing `values`: the sliders, the backdrop pills and the
/// lighting drop-down. Setting a property from Rust does not fire the control's own
/// "changed" callback, so nothing is written to the settings file by this.
fn push_values_to_controls(ui: &MainWindow, values: &LightingValues, rig: LightingPreset) {
    let settings = ui.global::<SettingsModel>();
    settings.set_light_yaw_deg(values.light_yaw_deg);
    settings.set_light_pitch_deg(values.light_pitch_deg);
    settings.set_exposure_val(values.exposure);
    settings.set_surface_glare_pct(percent_from_surface_glare(values.surface_glare));
    settings.set_backdrop_index(values.backdrop.index());
    ui.global::<ViewportModel>()
        .set_selected_lighting_index(rig.index());
}

/// Tells the settings dialog what it may offer for the open design.
fn publish_model(ui: &MainWindow, design_open: bool, lookup: &Lookup) {
    let model = ui.global::<DesignLightingModel>();
    model.set_available(design_open);
    model.set_has_saved(design_open && lookup.has_row());
    model.set_summary(lookup.summary_text().into());
}

/// Shows a note once the open's own message has had time to be read, unless a warning or
/// error is on screen by then.
fn toast_after_open(ui: &MainWindow, text: String, kind: &'static str) {
    let weak = ui.as_weak();
    slint::Timer::single_shot(NOTE_DELAY, move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if may_show_over(ui.get_toast_visible(), ui.get_toast_type().as_str()) {
            show_toast(&ui, &text, kind);
        }
    });
}

/// The "Use this lighting for this design" button: stores what the live view shows.
fn use_lighting_for_design(ui: &MainWindow) {
    let (Some(handles), Some(uuid)) = (handles(), open_design()) else {
        show_toast(
            ui,
            "Open or create a design first, then save its lighting.",
            "info",
        );
        return;
    };
    let settings = ui.global::<SettingsModel>();
    // A map still being decoded for the lighting on screen counts as the map in use: the
    // dialog keeps showing the earlier one until the decode lands.
    let env_path = env_path_to_store(
        pending_env_path().as_deref(),
        settings.get_env_map_loaded(),
        settings.get_env_map_path().as_str(),
    );
    let values = {
        let ctx = RenderContext::lock(&handles.render_ctx);
        LightingValues::from_live(&ctx, env_path)
    };
    if let Err(err) = store_values(&handles.db, &uuid, &values, unix_now()) {
        show_toast(
            ui,
            &format!("Could not save the lighting for this design: {err}"),
            "error",
        );
        return;
    }
    // From now on this design's lighting is what is showing, so the normal lighting is
    // remembered for the day another design opens. Already remembered when this design's
    // lighting was showing before.
    if !override_active() {
        begin_override(ui, &handles);
    }
    SESSION.with(|session| session.borrow_mut().told.remove(&uuid));
    publish_model(ui, true, &Lookup::Row(values));
    show_toast(
        ui,
        "Saved. This design will open with this lighting.",
        "success",
    );
    // A tutorial step may wait for the lighting to be saved.
    raise(ui, events::DESIGN_LIGHTING_SAVED);
}

/// The "Forget for this design" button: clears the stored lighting and puts the normal
/// lighting back.
fn forget_lighting_for_design(ui: &MainWindow) {
    let (Some(handles), Some(uuid)) = (handles(), open_design()) else {
        return;
    };
    if let Err(err) = forget_values(&handles.db, &uuid) {
        show_toast(
            ui,
            &format!("Could not forget the lighting for this design: {err}"),
            "error",
        );
        return;
    }
    restore_normal_lighting(ui, &handles);
    publish_model(ui, true, &Lookup::NoRow);
    show_toast(ui, "This design now uses your normal lighting.", "info");
    // A tutorial step may wait for the lighting to be forgotten.
    raise(ui, events::DESIGN_LIGHTING_FORGOTTEN);
}

#[cfg(test)]
mod tests;
