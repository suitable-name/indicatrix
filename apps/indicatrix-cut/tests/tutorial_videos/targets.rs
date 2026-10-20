//! Where the cursor goes for a step: every `highlight_target` a guide step can name
//! (`crates/indicatrix-editor/src/guide/catalog.rs`, `HIGHLIGHT_TARGETS`), mapped to a query for
//! the element the app outlines for it.
//!
//! The table is data only. A query prefers the element id of the outlined control
//! (`Component::id`), then its accessible label; the first query that finds something visible
//! wins. A target with no entry, or whose queries find nothing on screen, is a note in the
//! report (once per name), never a finding: the cursor simply stays where it is.

use crate::pure_targets::nearest_within;
use crate::support::{ElementHandle, WindowView};
use uiprobe::AccessibleRole;

/// One way to find an element in the main window.
#[derive(Clone, Copy, Debug)]
pub enum Query {
    /// The element with this id (`Component::local-id`).
    Id(&'static str),
    /// An element with exactly this accessible label (or placeholder text).
    Label(&'static str),
    /// The command bar's File button (not the window's native File menu).
    FileButton,
    /// An element whose accessible label starts with this text.
    LabelStart(&'static str),
    /// A tab (`AccessibleRole::Tab`) with exactly this accessible label.
    Tab(&'static str),
    /// An element that is, or inherits, this component.
    Inherits(&'static str),
    /// An element with this label below the element labelled `.0` (a menu and its items).
    Within(&'static str, &'static str),
    /// The rectangle that holds the first visible match of every member (a group of controls).
    Group(&'static [Self]),
}

/// One row of the table.
pub struct Entry {
    /// The `highlight_target` name.
    pub name: &'static str,
    /// The queries, in order of preference.
    pub queries: &'static [Query],
    /// Where the app draws the outline: `ui/` file and line. Documentation only.
    #[expect(dead_code, reason = "documentation column, read by people")]
    pub source: &'static str,
}

/// Every target name the guides use. The `source` column is the `highlight_target ==` line.
pub const TABLE: &[Entry] = &[
    Entry {
        name: "new_design_dialog",
        queries: &[
            Query::Inherits("NewDesignDialog"),
            Query::Within("File Menu", "New Design..."),
            Query::Label("New Design"),
            Query::FileButton,
        ],
        source: "components/new_design_dialog.slint:135; editor_command_bar/file_menu.slint:239; empty_state.slint:83",
    },
    Entry {
        name: "inspector_tier",
        queries: &[Query::Tab("Tier")],
        source: "components/editor_inspector.slint:678",
    },
    Entry {
        name: "preform_tab",
        queries: &[Query::Tab("Preform")],
        source: "components/editor_inspector.slint:686",
    },
    Entry {
        name: "optimize_tab",
        queries: &[Query::Tab("Optimize")],
        source: "components/editor_inspector.slint:694",
    },
    Entry {
        name: "history_tab",
        queries: &[Query::Tab("History")],
        source: "components/editor_inspector.slint:709",
    },
    Entry {
        name: "design_settings",
        queries: &[
            Query::Inherits("EditorDesignSettings"),
            Query::Label("Stone Properties"),
        ],
        source: "components/editor_design_settings.slint:45; parameters_panel/toolbar_buttons.slint:22",
    },
    Entry {
        name: "tier_table",
        queries: &[Query::Inherits("EditorTierTable"), Query::Label("Tiers")],
        source: "components/editor_tier_table.slint:57",
    },
    Entry {
        name: "solve_button",
        queries: &[Query::Id("SolveGroup::solve_btn"), Query::Label("Solve")],
        source: "components/editor_command_bar/edit_groups.slint:127",
    },
    Entry {
        name: "auto_solve",
        queries: &[
            Query::Id("SolveGroup::auto_solve_combo"),
            Query::Label("Auto-Solve"),
        ],
        source: "components/editor_command_bar/edit_groups.slint:190",
    },
    Entry {
        name: "deep_solve_button",
        queries: &[
            Query::Id("SolveGroup::deep_solve_btn"),
            Query::Label("Deep Solve"),
        ],
        source: "components/editor_command_bar/edit_groups.slint:215",
    },
    Entry {
        name: "retarget_button",
        queries: &[
            Query::Id("AnalysisGroup::retarget_btn"),
            Query::Label("Retarget..."),
        ],
        source: "components/editor_command_bar/analysis_groups.slint:37",
    },
    Entry {
        name: "cutting_mode_button",
        queries: &[
            Query::Id("AnalysisGroup::cut_mode_btn"),
            Query::Label("Cut Mode"),
        ],
        source: "components/editor_command_bar/analysis_groups.slint:80",
    },
    Entry {
        name: "snapshot_button",
        queries: &[
            Query::Id("SnapshotGroup::snapshot_btn"),
            Query::Label("Snapshot"),
        ],
        source: "components/editor_command_bar/analysis_groups.slint:115",
    },
    Entry {
        name: "compare_button",
        queries: &[
            Query::Id("SnapshotGroup::compare_snapshot_btn"),
            Query::Label("Compare"),
        ],
        source: "components/editor_command_bar/analysis_groups.slint:145",
    },
    Entry {
        name: "verdict_badge",
        queries: &[Query::LabelStart("Overall Verdict")],
        source: "components/verdict_badge.slint:98",
    },
    Entry {
        name: "solid_viewport",
        queries: &[Query::Inherits("SolidViewportView")],
        source: "components/solid_viewport.slint:717",
    },
    Entry {
        name: "view_modes",
        queries: &[Query::Group(&[
            Query::Label("Solid"),
            Query::Label("Path-Traced"),
            Query::Label("Ray-Traced"),
            Query::Label("Both"),
            Query::Label("Diagram"),
        ])],
        source: "components/solid_viewport/toolbar.slint:148-172",
    },
    Entry {
        name: "cut_slider",
        queries: &[Query::Id("CutSlider::cut_slider"), Query::Label("Cut Step")],
        source: "components/solid_viewport/toolbar.slint:57,79",
    },
    Entry {
        name: "snap_pill",
        queries: &[Query::Label("Snap")],
        source: "components/solid_viewport/toolbar.slint:302",
    },
    Entry {
        name: "slice_pill",
        queries: &[Query::Label("Slice")],
        source: "components/solid_viewport/toolbar.slint:317",
    },
    Entry {
        name: "live_render_tab",
        queries: &[
            Query::Tab("Live Render"),
            Query::Label("Live Render"),
            Query::Id("TabViewPillRow::live_render_tab_touch"),
        ],
        source: "components/app/tab_pill_row.slint:114,116",
    },
    Entry {
        name: "lighting_combo",
        queries: &[
            Query::Id("GemViewportToolbarActions::combo-box-l371"),
            Query::Label("Lighting"),
        ],
        source: "components/gem_viewport/toolbar_actions.slint:414,422",
    },
    Entry {
        name: "stone_tab",
        queries: &[Query::Label("Stone Properties"), Query::Label("Stone")],
        source: "components/parameters_panel/toolbar_buttons.slint:22; parameters_panel.slint:142",
    },
    Entry {
        name: "lighting_tab",
        queries: &[
            Query::Label("Lighting and Environment"),
            Query::Label("Lighting"),
        ],
        source: "components/parameters_panel/toolbar_buttons.slint:23; parameters_panel.slint:150",
    },
    Entry {
        name: "render_output_tab",
        queries: &[Query::Label("Render and Output"), Query::Label("Render")],
        source: "components/parameters_panel/toolbar_buttons.slint:24; parameters_panel.slint:158",
    },
    Entry {
        name: "save_button",
        queries: &[Query::Within("File Menu", "Save"), Query::FileButton],
        source: "components/editor_command_bar/file_menu.slint:100,273",
    },
    Entry {
        name: "open_button",
        queries: &[Query::Within("File Menu", "Open"), Query::FileButton],
        source: "components/editor_command_bar/file_menu.slint:101,250",
    },
    Entry {
        name: "load_button",
        queries: &[
            Query::Within("File Menu", "Load Selected"),
            Query::FileButton,
        ],
        source: "components/editor_command_bar/file_menu.slint:102,259",
    },
    Entry {
        name: "export_asc_button",
        queries: &[Query::Within("File Menu", "Export .asc"), Query::FileButton],
        source: "components/editor_command_bar/file_menu.slint:103,287",
    },
    Entry {
        name: "export_menu",
        queries: &[
            Query::Within("File Menu", "Export Cutting Sheet (HTML)..."),
            Query::FileButton,
        ],
        source: "components/editor_command_bar/file_menu.slint:104,296",
    },
    Entry {
        name: "library_search",
        queries: &[Query::Label("Search the Library")],
        source: "components/header.slint:340,370",
    },
    Entry {
        name: "library_filters",
        queries: &[Query::Group(&[
            Query::Label("Shape Filter"),
            Query::LabelStart("Filters"),
        ])],
        source: "components/header.slint:422,447,481,494",
    },
    Entry {
        name: "library_import",
        queries: &[Query::Label("Import")],
        source: "components/header.slint:561",
    },
    Entry {
        name: "mode_switch",
        queries: &[Query::Group(&[
            Query::Label("Simple Interface"),
            Query::Label("Advanced Interface"),
        ])],
        source: "components/header.slint:211,223,272",
    },
];

/// A rectangle on screen in logical pixels, with the element it came from (a group has none).
#[derive(Clone)]
pub struct Spot {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
    /// The element, when the spot is one element.
    pub handle: Option<ElementHandle>,
}

impl Spot {
    /// The rectangle `element` covers.
    #[must_use]
    pub fn of(element: &ElementHandle) -> Self {
        let position = element.absolute_position();
        let size = element.size();
        Self {
            x: position.x,
            y: position.y,
            w: size.width,
            h: size.height,
            handle: Some(element.clone()),
        }
    }

    /// The centre.
    #[must_use]
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// The smallest rectangle holding both (without an element).
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            w: (self.x + self.w).max(other.x + other.w) - x,
            h: (self.y + self.h).max(other.y + other.h) - y,
            handle: None,
        }
    }

    /// Whether the rectangle is big enough to be a control and its centre is inside a window of
    /// `window` logical pixels.
    pub(crate) fn usable(&self, window: (f32, f32)) -> bool {
        let (cx, cy) = self.center();
        self.w >= 2.0 && self.h >= 2.0 && cx >= 0.0 && cy >= 0.0 && cx <= window.0 && cy <= window.1
    }

    /// Whether the centre of this spot lies between the top and the bottom of `area`: a scrolling
    /// area (this is a control on a row that is on view) or one row of a table (this is a control
    /// on that row). The horizontal position is not looked at.
    #[must_use]
    pub const fn level_with(&self, area: &Self) -> bool {
        let y = self.y + self.h / 2.0;
        y >= area.y && y <= area.y + area.h
    }

    /// Whether this spot can be clicked in the scrolling area `viewport`: it is level with it
    /// ([`Self::level_with`]) and lies wholly inside it, clear of the vertical scroll bar at its
    /// right edge. A control of a column that is scrolled out sideways still has a place inside
    /// the window (over whatever lies beside the area), so "inside the window"
    /// ([`Self::usable`]) is not enough: a click there would land on that neighbour.
    #[must_use]
    pub const fn in_view_of(&self, viewport: &Self) -> bool {
        self.level_with(viewport)
            && self.x >= viewport.x - 1.0
            && self.x + self.w <= viewport.x + viewport.w - SCROLL_BAR_ROOM
    }
}

/// The width at the right edge of a scrolling area that its vertical scroll bar may cover.
const SCROLL_BAR_ROOM: f32 = 20.0;

/// What looking up a target found.
pub enum Resolved {
    /// The target is on screen here.
    Found(Spot),
    /// The name has no table entry.
    Unmapped,
    /// The entry's queries found nothing visible.
    NotOnScreen,
}

/// Looks `target` up in the table and on screen.
#[must_use]
pub fn resolve(view: &WindowView, target: &str) -> Resolved {
    let Some(entry) = TABLE.iter().find(|entry| entry.name == target) else {
        return Resolved::Unmapped;
    };
    entry
        .queries
        .iter()
        .find_map(|query| run(view, query))
        .map_or(Resolved::NotOnScreen, Resolved::Found)
}

/// The first usable element among `elements`.
fn first_usable(elements: &[ElementHandle], window: (f32, f32)) -> Option<Spot> {
    elements
        .iter()
        .map(Spot::of)
        .find(|spot| spot.usable(window))
}

/// Runs one query.
#[must_use]
pub fn run(view: &WindowView, query: &Query) -> Option<Spot> {
    let window = view.size;
    match *query {
        Query::Id(id) => first_usable(
            &view.root.query_descendants().match_id(id).find_all(),
            window,
        ),
        Query::Label(label) => first_usable(&view.by_label(label), window),
        Query::FileButton => file_button(view),
        Query::LabelStart(prefix) => {
            let wanted = prefix.to_owned();
            first_usable(
                &view
                    .root
                    .query_descendants()
                    .match_predicate(move |e| {
                        e.accessible_label()
                            .is_some_and(|l| l.starts_with(wanted.as_str()))
                    })
                    .find_all(),
                window,
            )
        }
        Query::Tab(label) => {
            let wanted = label.to_owned();
            first_usable(
                &view
                    .root
                    .query_descendants()
                    .match_accessible_role(AccessibleRole::Tab)
                    .match_predicate(move |e| e.accessible_label().is_some_and(|l| l == wanted))
                    .find_all(),
                window,
            )
        }
        Query::Inherits(name) => first_usable(
            &view
                .root
                .query_descendants()
                .match_inherits(name)
                .find_all(),
            window,
        ),
        Query::Within(container, label) => {
            let wanted = container.to_owned();
            let parent = view
                .root
                .query_descendants()
                .match_predicate(move |e| e.accessible_label().is_some_and(|l| l == wanted))
                .find_first()?;
            let wanted = label.to_owned();
            first_usable(
                &parent
                    .query_descendants()
                    .match_predicate(move |e| e.accessible_label().is_some_and(|l| l == wanted))
                    .find_all(),
                window,
            )
        }
        Query::Group(members) => members
            .iter()
            .filter_map(|member| run(view, member))
            .reduce(|a, b| a.union(&b)),
    }
}

/// Finds the editable widget itself, never its adjacent `FieldCaption` text.
#[must_use]
pub fn text_input(view: &WindowView, label: &str) -> Option<Spot> {
    view.root
        .query_descendants()
        .match_accessible_role(AccessibleRole::TextInput)
        .find_all()
        .into_iter()
        .filter(|element| element.accessible_label().is_some_and(|name| name == label))
        .map(|element| Spot::of(&element))
        .find(|spot| spot.usable(view.size))
}

/// Finds a labelled editable widget whose label starts with `prefix`.
#[must_use]
pub fn text_input_start(view: &WindowView, prefix: &str) -> Option<Spot> {
    view.root
        .query_descendants()
        .match_accessible_role(AccessibleRole::TextInput)
        .find_all()
        .into_iter()
        .filter(|element| {
            element
                .accessible_label()
                .is_some_and(|name| name.starts_with(prefix))
        })
        .map(|element| Spot::of(&element))
        .find(|spot| spot.usable(view.size))
}

/// The first visible element with exactly this accessible label.
#[must_use]
pub fn run_label(view: &WindowView, label: &str) -> Option<Spot> {
    first_usable(&view.by_label(label), view.size)
}

/// The first visible element whose accessible label starts with `prefix`.
#[must_use]
pub fn run_label_start(view: &WindowView, prefix: &str) -> Option<Spot> {
    let wanted = prefix.to_owned();
    first_usable(
        &view
            .root
            .query_descendants()
            .match_predicate(move |e| {
                e.accessible_label()
                    .is_some_and(|l| l.starts_with(wanted.as_str()))
            })
            .find_all(),
        view.size,
    )
}

/// The visible element whose accessible label starts with `prefix` and whose centre lies within
/// `tolerance` logical pixels (vertically, inclusive) of `center_y`: the one on a given row where
/// every row has such a button. Returns `None` when no such element is that close, so the caller
/// must fail the scenario instead of clicking another row's button; pass the row's height as
/// `tolerance`.
///
/// Exact signature (used by `perform_actions.rs`):
/// `pub fn label_start_near_y(view: &WindowView, prefix: &str, center_y: f32, tolerance: f32) -> Option<Spot>`
#[must_use]
pub fn label_start_near_y(
    view: &WindowView,
    prefix: &str,
    center_y: f32,
    tolerance: f32,
) -> Option<Spot> {
    let wanted = prefix.to_owned();
    let spots: Vec<Spot> = view
        .root
        .query_descendants()
        .match_predicate(move |e| {
            e.accessible_label()
                .is_some_and(|l| l.starts_with(wanted.as_str()))
        })
        .find_all()
        .iter()
        .map(Spot::of)
        .filter(|spot| spot.usable(view.size))
        .collect();
    let centres: Vec<f32> = spots.iter().map(|spot| spot.center().1).collect();
    let index = nearest_within(&centres, center_y, tolerance)?;
    spots.into_iter().nth(index)
}

/// The Next / Finish / Skip Step button of the guide panel.
#[must_use]
pub fn panel_button(view: &WindowView) -> Option<Spot> {
    let by_id = run(view, &Query::Id("GuidePanel::action-button-offers-next"));
    by_id.or_else(|| {
        ["Next", "Finish", "Skip Step"]
            .into_iter()
            .find_map(|label| run(view, &Query::Label(label)))
    })
}

/// The menu item `label` of the open File menu.
#[must_use]
pub fn menu_item(view: &WindowView, label: &'static str) -> Option<Spot> {
    run(view, &Query::Within("File Menu", label))
}

/// The File button of the command bar.
#[must_use]
pub fn file_button(view: &WindowView) -> Option<Spot> {
    // Both the native menu bar and the editor command bar expose "File".
    // Only this component owns the in-window "File Menu" queried above.
    view.root
        .query_descendants()
        .match_inherits("FileMenu")
        .find_all()
        .into_iter()
        .find_map(|menu| {
            let buttons = menu
                .query_descendants()
                .match_accessible_role(AccessibleRole::Button)
                .match_predicate(|element| {
                    element
                        .accessible_label()
                        .is_some_and(|label| label == "File")
                })
                .find_all();
            first_usable(&buttons, view.size)
        })
}

/// The Edit view pill in the tab bar.
#[must_use]
pub fn edit_pill(view: &WindowView) -> Option<Spot> {
    run(view, &Query::Tab("Edit"))
        .or_else(|| run(view, &Query::Label("Edit")))
        .or_else(|| run(view, &Query::Id("TabViewPillRow::edit_tab_touch")))
}

/// The Live Render view pill in the tab bar.
#[must_use]
pub fn live_render_pill(view: &WindowView) -> Option<Spot> {
    run(view, &Query::Tab("Live Render"))
        .or_else(|| run(view, &Query::Label("Live Render")))
        .or_else(|| run(view, &Query::Id("TabViewPillRow::live_render_tab_touch")))
}

/// The button that collapses the library panel.
#[must_use]
pub fn hide_library_button(view: &WindowView) -> Option<Spot> {
    run(view, &Query::Label("Hide the Library List"))
        .or_else(|| run(view, &Query::Id("DiagramList::panel_collapse_btn")))
}

/// The rail button that expands the library panel.
#[must_use]
pub fn show_library_button(view: &WindowView) -> Option<Spot> {
    run(view, &Query::Label("Show the Library List"))
        .or_else(|| run(view, &Query::Id("LibraryRail::panel_expand_btn")))
}

/// The inspector tab pill for tab index `tab` (0 Tier, 1 Preform, 2 Optimize, 3 Schedule, 4 History).
#[must_use]
pub fn inspector_tab_pill(view: &WindowView, tab: i32) -> Option<Spot> {
    let name = match tab {
        0 => "Tier",
        1 => "Preform",
        2 => "Optimize",
        3 => "Schedule",
        4 => "History",
        _ => return None,
    };
    run(view, &Query::Tab(name)).or_else(|| run(view, &Query::Label(name)))
}

/// The Parameters dock tab pill/button (`"stone"`, `"lighting"`, `"render_output"`).
#[must_use]
pub fn dock_tab_button(view: &WindowView, tab: &str) -> Option<Spot> {
    match tab {
        "stone" => run(view, &Query::Label("Stone"))
            .or_else(|| run(view, &Query::Label("Stone Properties"))),
        "lighting" => run(view, &Query::Label("Lighting"))
            .or_else(|| run(view, &Query::Label("Lighting & Environment")))
            .or_else(|| run(view, &Query::Label("Lighting and Environment"))),
        "render_output" => run(view, &Query::Label("Render & Output"))
            .or_else(|| run(view, &Query::Label("Render and Output")))
            .or_else(|| run(view, &Query::Label("Render Output"))),
        _ => None,
    }
}

/// The solid preview mode pill on the viewport toolbar (0 Solid, 1 Path-Traced, 2 Both, 3 Diagram, 4 Ray-Traced).
#[must_use]
pub fn solid_view_mode_pill(view: &WindowView, mode: i32) -> Option<Spot> {
    let name = match mode {
        0 => "Solid",
        1 => "Path-Traced",
        2 => "Both",
        3 => "Diagram",
        4 => "Ray-Traced",
        _ => return None,
    };
    run(view, &Query::Label(name))
}
