//! The types of the direct-invoke table: one [`Row`] per global callback.

use indicatrix_cut::{CompareWindow, HelpWindow, MainWindow, RoughPlannerWindow};

/// The state a group of rows runs in; the app is relaunched into it once per group (and again
/// before every `fresh` row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    /// The freshly started app: no design, an empty library.
    Startup,
    /// The four fixture designs in the library, the first one selected.
    Library,
    /// `Library`, plus the first design loaded into the editor.
    Editor,
    /// `Library`, plus the Rough Planner window open.
    Planner,
    /// `Editor`, plus a snapshot and the visual-compare window open.
    Compare,
    /// The Help window open.
    Help,
}

impl Setup {
    /// Run order of the groups.
    pub const ORDER: [Self; 6] = [
        Self::Startup,
        Self::Library,
        Self::Editor,
        Self::Planner,
        Self::Compare,
        Self::Help,
    ];

    /// Name for messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Library => "library",
            Self::Editor => "editor",
            Self::Planner => "planner",
            Self::Compare => "compare",
            Self::Help => "help",
        }
    }
}

/// How a row reaches its callback: through the global of one window.
#[derive(Clone, Copy)]
pub enum Invoke {
    /// The main window's global.
    Main(fn(&MainWindow)),
    /// The Rough Planner window's own global instance.
    Planner(fn(&RoughPlannerWindow)),
    /// The visual-compare window's own global instance.
    Compare(fn(&CompareWindow)),
    /// The Help window's own global instance.
    Help(fn(&HelpWindow)),
    /// The main window's global, called with arguments that make it DO something, followed by a
    /// check of one observable effect. `Err` is a `coverage-effect` finding (the callback still
    /// counts as invoked). The function pumps and waits itself.
    Checked(fn(&MainWindow) -> Result<(), String>),
}

/// One callback and how to invoke it.
pub struct Row {
    /// The global (`EditorModel`).
    pub model: &'static str,
    /// The generated method (`invoke_solve`).
    pub callback: &'static str,
    /// The state the row needs.
    pub setup: Setup,
    /// Whether the row changes the state so much that the group's state is rebuilt before it.
    pub fresh: bool,
    /// The call.
    pub invoke: Invoke,
}

impl Row {
    /// The recorder / inventory key: `Model.name`.
    #[must_use]
    pub fn key(&self) -> String {
        format!(
            "{}.{}",
            self.model,
            self.callback
                .strip_prefix("invoke_")
                .unwrap_or(self.callback)
        )
    }
}

/// Builds a `&[Row]` of [`Invoke::Checked`] rows from `Model.method => function;` lines. A checked
/// row replaces the generated row with the same key (see `all_rows`).
macro_rules! checked_rows {
    ($setup:ident, $fresh:expr;
     $( $model:ident . $method:ident => $check:path ; )*) => {
        &[ $( Row {
            model: stringify!($model),
            callback: stringify!($method),
            setup: Setup::$setup,
            fresh: $fresh,
            invoke: Invoke::Checked($check),
        } ),* ]
    };
}

/// Builds a `&[Row]` from `Model.invoke_method(args);` lines. `$win` is the window type the
/// closures take, `$variant` the [`Invoke`] variant, `$setup` the [`Setup`] variant.
macro_rules! rows {
    ($win:ident, $variant:ident, $setup:ident, $fresh:expr;
     $( $model:ident . $method:ident ( $($arg:expr),* ) ; )*) => {
        &[ $( Row {
            model: stringify!($model),
            callback: stringify!($method),
            setup: Setup::$setup,
            fresh: $fresh,
            invoke: Invoke::$variant(|w: &$win| {
                std::hint::black_box(w.global::<$model>().$method($($arg),*));
            }),
        } ),* ]
    };
}
