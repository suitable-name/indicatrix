//! Getting to a state: selecting a library design, showing a tab, loading a design into the
//! editor, selecting a tier, choosing the stone's material and the light.

use crate::{kit::Sc, support::app::pump};
use indicatrix_cut::{EditorModel, LibraryModel, SolidPreviewModel, ViewportModel};
use slint::{ComponentHandle, Model};

/// The design of the library shots (the dodecagon of the main screen).
pub const STUDIO: &str = "45.148 SplitChecker 12";
/// The design of the editor shots.
pub const FVS: &str = "41.029B FVS-65";
/// The crown tier the editor shots select.
pub const CROWN_TIER: &str = "C3";

/// A library row.
#[derive(Clone, Debug)]
pub struct Design {
    /// The catalogue entry id.
    pub id: i32,
    /// The title as the list shows it.
    pub title: String,
}

/// The listed design called `title` (exactly, else the first whose title contains it).
fn find_row(ui: &indicatrix_cut::MainWindow, title: &str) -> Option<Design> {
    let wanted = title.to_lowercase();
    let rows: Vec<Design> = ui
        .global::<LibraryModel>()
        .get_diagram_list()
        .iter()
        .map(|row| Design {
            id: row.id,
            title: row.title.to_string(),
        })
        .collect();
    rows.iter()
        .find(|row| row.title.to_lowercase() == wanted)
        .or_else(|| {
            rows.iter()
                .find(|row| row.title.to_lowercase().contains(&wanted))
        })
        .cloned()
}

impl Sc<'_, '_> {
    /// Selects the library design called `title`, searching for it when the first page of the
    /// list does not hold it (the search is cleared again afterwards).
    ///
    /// # Errors
    ///
    /// The library has no such design, or its detail does not load.
    pub fn select_design(&self, title: &str) -> Result<Design, String> {
        let ui = self.ui();
        let library = ui.global::<LibraryModel>();
        let mut searched = false;
        let mut found = find_row(&ui, title);
        if found.is_none() {
            searched = true;
            library.set_search_text(title.into());
            library.invoke_search_changed(title.into());
            let _ = Self::wait(30, || find_row(&ui, title).is_some());
            found = find_row(&ui, title);
        }
        let design = found.ok_or_else(|| format!("the library has no design called {title:?}"))?;
        library.invoke_select_diagram(design.id);
        let selected = Self::wait(30, || {
            library.get_selected_entry_id() == design.id
                && !library.get_current_detail().title.is_empty()
        });
        if !selected {
            return Err(format!("the detail of {title:?} did not load"));
        }
        if searched {
            library.set_search_text("".into());
            library.invoke_search_changed("".into());
            let _ = Self::wait(30, || library.get_diagram_list().row_count() > 1);
        }
        pump(400);
        self.ctx.app.clear_notice();
        Ok(design)
    }

    /// Shows catalogue tab `tab` (0 3D Spectral Preview, 1 Cutting Instructions, 2 Files).
    pub fn show_tab(&self, tab: i32) {
        let ui = self.ui();
        ui.set_active_tab(tab);
        ui.invoke_active_tab_changed(tab);
        pump(400);
    }

    /// Shows the 3D tab's Live Render view.
    pub fn show_live_view(&self) {
        self.show_tab(0);
        let ui = self.ui();
        ui.set_render_view_tab(0);
        ui.invoke_render_view_tab_changed(0);
        pump(400);
        self.fit_camera();
    }

    /// Shows the 3D tab's Edit view.
    pub fn show_edit_view(&self) {
        self.show_tab(0);
        let ui = self.ui();
        ui.set_render_view_tab(1);
        ui.invoke_render_view_tab_changed(1);
        pump(600);
        self.fit_camera();
    }

    /// Frames the stone with the viewport's Fit pose (the pose pills' "Fit": the whole stone in
    /// the picture on screen, orbit angle kept). Fit redraws the picture itself.
    pub fn fit_camera(&self) {
        let ui = self.ui();
        let solid = ui.global::<SolidPreviewModel>();
        let _ = Self::wait(15, || solid.get_has_solid());
        pump(300);
        ui.global::<ViewportModel>().invoke_set_view(5);
        pump(800);
    }

    /// Selects the library design `title`, loads it into the editor (what Load Selected does),
    /// shows the Edit view and waits for the solve and the solid.
    ///
    /// # Errors
    ///
    /// The design is not found, does not load, or does not solve.
    pub fn open_in_editor(&mut self, title: &str) -> Result<(), String> {
        self.select_design(title)?;
        let ui = self.ui();
        let editor = ui.global::<EditorModel>();
        editor.invoke_load_selected();
        let loaded = Self::wait(90, || {
            editor.get_has_design() && editor.get_tiers().row_count() > 0
        });
        if !loaded {
            return Err(format!("{title:?} did not load into the editor"));
        }
        if editor.get_unsaved_dialog_open() {
            return Err("the editor asked about unsaved changes on a fresh app".to_owned());
        }
        self.show_edit_view();
        let solved = Self::wait(180, || {
            !editor.get_solve_running()
                && editor.get_facet_count() > 0
                && editor.get_solve_state().as_str() == "solved"
        });
        if !solved {
            return Err(format!(
                "{title:?} is not solved: state {:?}, status {:?}",
                editor.get_solve_state().to_string(),
                editor.get_status_text().to_string()
            ));
        }
        let solid = ui.global::<SolidPreviewModel>();
        let _ = Self::wait(60, || solid.get_has_solid());
        // Framed again now that the solid is the solved design's.
        self.fit_camera();
        // The offer to set the material of the catalogue's refractive index is a banner in the
        // design panel; dismissing it changes nothing in the design.
        editor.invoke_material_suggestion_dismiss();
        pump(600);
        self.ctx.app.clear_notice();
        Ok(())
    }

    /// Selects the tier row called `wanted`; when there is none, the first crown tier (a name
    /// starting with `C`). Returns the name of the tier selected.
    ///
    /// # Errors
    ///
    /// The design has neither.
    pub fn select_tier(&mut self, wanted: &str) -> Result<String, String> {
        let ui = self.ui();
        let editor = ui.global::<EditorModel>();
        let names: Vec<String> = editor
            .get_tiers()
            .iter()
            .map(|tier| tier.name.to_string())
            .collect();
        let is_wanted = |name: &String| name.split('/').any(|part| part.trim() == wanted);
        let position = names
            .iter()
            .position(is_wanted)
            .or_else(|| names.iter().position(|name| name.starts_with('C')))
            .ok_or_else(|| format!("no tier {wanted} and no crown tier among {names:?}"))?;
        editor.set_selected_tier_index(i32::try_from(position).unwrap_or(-1));
        pump(800);
        let chosen = names.get(position).cloned().unwrap_or_default();
        if chosen != wanted {
            self.note(format!(
                "tier {wanted} does not exist; {chosen} is selected"
            ));
        }
        Ok(chosen)
    }

    /// Shows the solid viewport's view mode `mode` (0 Solid, 3 Diagram) and waits for it.
    pub fn set_view_mode(&self, mode: i32) {
        let ui = self.ui();
        let solid = ui.global::<SolidPreviewModel>();
        solid.set_view_mode(mode);
        pump(300);
        let _ = Self::wait(60, || {
            if mode == 3 {
                solid.get_has_diagram()
            } else {
                solid.get_has_solid()
            }
        });
        pump(1000);
    }

    /// Shows inspector tab `tab` (0 Tier, 1 Preform, 2 Optimize, 3 Schedule, 4 History).
    pub fn set_inspector_tab(&self, tab: i32) {
        let ui = self.ui();
        let editor = ui.global::<EditorModel>();
        editor.set_inspector_collapsed(false);
        editor.set_inspector_tab(tab);
        pump(500);
    }

    /// Collapses the Design and Tiers sections of the Edit view so the inspector below them gets
    /// the column's height (at the shots' window size it is otherwise a strip of a few rows).
    pub fn expand_inspector(&self) {
        let ui = self.ui();
        let editor = ui.global::<EditorModel>();
        editor.set_settings_collapsed(true);
        editor.set_tier_table_collapsed(true);
        editor.set_inspector_collapsed(false);
        pump(500);
    }

    /// Makes the library view show the design with the material called `wanted` (the Stone
    /// tab's Material pick for a library design). Returns the name that was chosen.
    ///
    /// # Errors
    ///
    /// The app has no such material.
    pub fn choose_material(&self, wanted: &str) -> Result<String, String> {
        let ui = self.ui();
        let viewport = ui.global::<ViewportModel>();
        let options: Vec<String> = viewport
            .get_material_options()
            .iter()
            .map(String::from)
            .collect();
        let low = wanted.to_lowercase();
        let index = options
            .iter()
            .position(|name| name.to_lowercase() == low)
            .or_else(|| {
                options
                    .iter()
                    .position(|name| name.to_lowercase().contains(&low))
            })
            .ok_or_else(|| format!("no material called {wanted:?} among {options:?}"))?;
        let name = options.get(index).cloned().unwrap_or_default();
        viewport.invoke_library_material_chosen(name.as_str().into());
        pump(300);
        if viewport.get_traced_material_name().as_str() != name {
            viewport.set_selected_material_index(i32::try_from(index).unwrap_or(0));
            pump(300);
        }
        self.ctx.app.clear_notice();
        Ok(name)
    }

    /// Chooses the lighting rig whose name contains `wanted`.
    ///
    /// # Errors
    ///
    /// The app has no such rig.
    pub fn choose_lighting(&self, wanted: &str) -> Result<String, String> {
        let ui = self.ui();
        let viewport = ui.global::<ViewportModel>();
        let options: Vec<String> = viewport
            .get_lighting_options()
            .iter()
            .map(String::from)
            .collect();
        let low = wanted.to_lowercase();
        let index = options
            .iter()
            .position(|name| name.to_lowercase().contains(&low))
            .ok_or_else(|| format!("no lighting rig called {wanted:?} among {options:?}"))?;
        let number = i32::try_from(index).unwrap_or(0);
        viewport.set_selected_lighting_index(number);
        viewport.invoke_lighting_changed(number);
        pump(300);
        self.ctx.app.clear_notice();
        Ok(options.get(index).cloned().unwrap_or_default())
    }

    /// The look of the old main screen: a garnet under the ring lights. Either part that the
    /// app does not offer is left out and noted.
    pub fn studio_look(&mut self) {
        if let Err(reason) = self.choose_material("Grossular") {
            self.note(format!("studio look: {reason}"));
        }
        if let Err(reason) = self.choose_lighting("Ring Lights") {
            self.note(format!("studio look: {reason}"));
        }
    }
}
