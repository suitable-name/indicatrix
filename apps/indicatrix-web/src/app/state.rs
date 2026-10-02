//! [`WebApp`]: everything the browser app knows, held once in an
//! `Rc<RefCell<WebApp>>` ([`super::Ctx::state`]). The UI is refreshed from it by the
//! small `push_*` functions in [`super::push`]; nothing reads state back out of the
//! Slint window except the values the user just edited there.

use super::settings::RenderSettings;
use crate::render::viewport::ViewState;
use indicatrix::{
    geometry::{meet_solver::SolvedTier, stone_metrics::ExternalProportions},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::native::{AttachmentBlob, CustomMaterialSnapshot, DesignMetadata};
use indicatrix_editor::EditorSession;
use indicatrix_web_core::{
    custom_material::{builtin_name_conflict, builtin_name_message},
    settings::DEFAULT_AUTO_SOLVE_BUDGET_MS,
    solve::TierWarning,
};
use std::time::Duration;

/// The longest value, in characters, the dock's design-info text shows for one field.
const INFO_VALUE_CHARS: usize = 200;

/// Where the current design came from, for the status strip and the dock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesignSource {
    /// A New Design from the gallery; carries the template's card name.
    Template(String),
    /// A bare `.asc` file.
    Asc,
    /// An older `.asc` + `.indicatrix.toml` sidecar pair.
    OlderPair,
    /// A self-contained `.indicatrix` design file (or an older self-contained
    /// `.indicatrix.toml`), no `.asc`.
    DesignFile,
    /// A `.gem` or `.gcs` design converted to `.asc`; carries `".gem"`/`".gcs"`.
    Converted(&'static str),
    /// Restored from this tab's `sessionStorage` after a reload.
    Restored,
}

impl DesignSource {
    /// The short label the status strip and the dock show.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Template(name) => format!("New design: {name}"),
            Self::Asc => "Opened .asc".to_string(),
            Self::OlderPair => "Opened older .asc + sidecar pair".to_string(),
            Self::DesignFile => "Opened design file".to_string(),
            Self::Converted(ext) => format!("Converted from {ext}"),
            Self::Restored => "Restored from this tab".to_string(),
        }
    }
}

/// The loaded design and everything a save needs to write it back the way the
/// desktop would.
pub struct DesignState {
    /// The design, its undo/redo history, selection and dirty tracking.
    pub session: EditorSession,
    /// The design's `.asc` name (`round.asc`), `None` for a New design never saved.
    pub asc_filename: Option<String>,
    /// The opened `.asc`'s exact text, so a Save `.asc` + older sidecar can keep the
    /// `.asc` half byte-for-byte when the schedule did not change
    /// (`indicatrix_cut_core::native::save_paired`'s rule).
    pub original_asc_text: Option<String>,
    /// Where it came from.
    pub source: DesignSource,
    /// The printed proportions a design file carried in its `[source]` table.
    pub printed_proportions: Option<ExternalProportions>,
    /// The custom-material snapshot a design file carried, written back on the next
    /// save so the material survives the round trip.
    pub custom_material: Option<CustomMaterialSnapshot>,
    /// The `[meta]` table a design file carried (empty for any other source), written
    /// back on the next save with a fresh `modified_at`; unknown keys included.
    pub metadata: DesignMetadata,
    /// The attachments a design file carried, kept byte for byte and written back on
    /// the next save.
    pub attachments: Vec<AttachmentBlob>,
}

impl DesignState {
    /// A design with no file behind it yet.
    #[must_use]
    pub fn new(session: EditorSession, source: DesignSource) -> Self {
        Self {
            session,
            asc_filename: None,
            original_asc_text: None,
            source,
            printed_proportions: None,
            custom_material: None,
            metadata: DesignMetadata::default(),
            attachments: Vec::new(),
        }
    }

    /// The read-only "Design info" text for the dock: title, designer, source, notes,
    /// tags and licence, one line each for the fields that are set; empty when none is.
    #[must_use]
    pub fn info_text(&self) -> String {
        let meta = &self.metadata;
        let source = if meta.source_citation.is_empty() {
            &meta.source_url
        } else {
            &meta.source_citation
        };
        let tags = meta.tags.join(", ");
        [
            ("Title", meta.title.as_str()),
            ("Designer", meta.designer.as_str()),
            ("Source", source.as_str()),
            ("Notes", meta.notes.as_str()),
            ("Tags", tags.as_str()),
            ("License", meta.license.as_str()),
        ]
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(label, value)| {
            if value.chars().count() > INFO_VALUE_CHARS {
                let head: String = value.chars().take(INFO_VALUE_CHARS).collect();
                format!("{label}: {head}...")
            } else {
                format!("{label}: {value}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// The custom-material snapshot a native save should carry: [`Self::custom_material`]
    /// while the design's material is a custom one, `None` once it names a built-in (a
    /// snapshot left over from a material the design no longer uses must not be written
    /// under the new name).
    #[must_use]
    pub fn custom_snapshot(&self) -> Option<&CustomMaterialSnapshot> {
        let name = self.session.design.material.name.as_deref()?;
        let builtin = GemMaterial::all_materials()
            .iter()
            .any(|m| m.name.eq_ignore_ascii_case(name));
        if builtin {
            return None;
        }
        self.custom_material.as_ref()
    }

    /// The name the status strip shows: the `.asc` name, else "Untitled design".
    #[must_use]
    pub fn display_name(&self) -> String {
        self.asc_filename
            .clone()
            .unwrap_or_else(|| "Untitled design".to_string())
    }

    /// The `.asc` name a save proposes (`indicatrix_editor::files::suggested_asc_file_name`).
    #[must_use]
    pub fn save_asc_name(&self) -> String {
        indicatrix_editor::files::suggested_asc_file_name(
            self.asc_filename.as_deref(),
            &self.session.design.meta.headers,
        )
    }
}

/// The last solve, stamped with the design generation it was computed against.
#[derive(Default)]
pub enum SolveState {
    /// Nothing solved since the design was loaded or last edited.
    #[default]
    NotSolved,
    /// A solve that succeeded (it may still describe a solid that does not close;
    /// `status`/`problem` say so).
    Solved {
        /// `EditorSession::current_generation` at solve time.
        generation: u64,
        /// One entry per tier, in tier order.
        tiers: Vec<SolvedTier>,
        /// `status_text_and_is_problem_from_solved`'s banner text.
        status: String,
        /// Whether `status` describes a problem.
        problem: bool,
        /// Wall time of the solve.
        took: Duration,
        /// The viewport planes built from `tiers` (what the renderer traces).
        planes: Vec<indicatrix::geometry::GpuFacetPlane>,
        /// The tier-tagged manufacturability warnings of this solve
        /// (`SolveOutcome::warnings`); the tier table tints their rows and lists them.
        warnings: Vec<TierWarning>,
    },
    /// A solve that failed (a missing scale anchor, an unresolvable target).
    Failed {
        /// `EditorSession::current_generation` at solve time.
        generation: u64,
        /// The solver's own message.
        message: String,
    },
}

/// An uploaded `.hdr` environment map, held for the renderer, which sends it to the
/// render Workers and the analysis Worker to decode. Checked against the browser caps on
/// upload (see `crate::io::hdr`).
pub struct HdrUpload {
    /// The file name, shown in the render view.
    pub name: String,
    /// The raw file bytes.
    pub bytes: Vec<u8>,
    /// Width in texels, from the header.
    pub width: u32,
    /// Height in texels, from the header.
    pub height: u32,
}

/// Everything the browser app knows. See the module doc comment.
pub struct WebApp {
    /// The loaded design, `None` until one is created, opened or restored.
    pub design: Option<DesignState>,
    /// The last solve of [`Self::design`].
    pub solve: SolveState,
    /// Custom materials restored from opened design files, by name; offered after
    /// the built-ins in the material combo and consulted by every save.
    pub custom_materials: Vec<GemMaterial>,
    /// Material, lighting, exposure, light direction, backdrop, bounces, samples,
    /// camera and the current view tab.
    pub settings: RenderSettings,
    /// The render target size and camera pose the renderer uses.
    pub view: ViewState,
    /// An uploaded environment map, if any.
    pub hdr: Option<HdrUpload>,
    /// The tier selected in the tier list / Solid / Diagram views (the desktop's
    /// `EditorModel.selected_tier_index`, `None` for -1). A plain selection also
    /// empties `EditorSession::multi_selected` (the desktop's rule). The views
    /// redraw on their own when this or the multi-selection changes; call
    /// `crate::views::request_refresh` for an immediate redraw.
    pub selected_tier: Option<usize>,
    /// The auto-solve budget in milliseconds (the dock's "Auto-solve" combo: Off, 150 ms,
    /// 300 ms, 1 s, 3 s; persisted in the settings payload). After an edit a debounced
    /// re-solve is scheduled while the design's own last solve took less than this; `0`
    /// is off, and an edited design then reads "stale" until Solve is pressed.
    pub auto_solve_budget_ms: u32,
}

impl WebApp {
    /// A fresh app with restored (or default) `settings` and no design.
    #[must_use]
    pub const fn new(settings: RenderSettings) -> Self {
        let view = ViewState::from_settings(&settings);
        Self {
            design: None,
            solve: SolveState::NotSolved,
            custom_materials: Vec::new(),
            settings,
            view,
            hdr: None,
            selected_tier: None,
            auto_solve_budget_ms: DEFAULT_AUTO_SOLVE_BUDGET_MS,
        }
    }

    /// The current design's generation, or `None` without a design.
    #[must_use]
    pub fn generation(&self) -> Option<u64> {
        self.design.as_ref().map(|d| d.session.current_generation())
    }

    /// The cached solve's tiers when it matches the current generation.
    #[must_use]
    pub fn current_solved(&self) -> Option<&[SolvedTier]> {
        match (&self.solve, self.generation()) {
            (
                SolveState::Solved {
                    generation, tiers, ..
                },
                Some(current),
            ) if *generation == current => Some(tiers),
            _ => None,
        }
    }

    /// Installs `design` in place of the current one (continuing the generation
    /// counter, so any in-flight job sees the change) and forgets the old solve.
    pub fn replace_design(&mut self, mut design: DesignState) {
        if let Some(previous) = &self.design {
            design.session.continue_generation_from(&previous.session);
        }
        self.design = Some(design);
        self.solve = SolveState::NotSolved;
        self.selected_tier = None;
    }

    /// Registers (or replaces, by case-insensitive name) a custom material.
    ///
    /// # Errors
    ///
    /// The refusal text when `material`'s name is a built-in material's: the catalogue is
    /// tab-wide and custom materials win over built-ins of the same name, so such a
    /// material would silently replace the built-in everywhere it is selected.
    pub fn register_custom_material(&mut self, material: GemMaterial) -> Result<(), String> {
        if let Some(builtin) = builtin_name_conflict(&material.name) {
            return Err(builtin_name_message(&material.name, &builtin));
        }
        if let Some(existing) = self
            .custom_materials
            .iter_mut()
            .find(|m| m.name.eq_ignore_ascii_case(&material.name))
        {
            *existing = material;
        } else {
            self.custom_materials.push(material);
        }
        Ok(())
    }
}
