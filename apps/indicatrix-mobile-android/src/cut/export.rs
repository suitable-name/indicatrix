//! Sharing and printing the cutting sheet, and sharing the diagram picture.
//!
//! Building either takes a moment, so the state is copied into a `SheetJob` on the UI
//! thread, the bytes are made on a worker thread, and the platform (share sheet, print
//! dialog) is called back on the UI thread.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use indicatrix_mobile_core::{
    cutting::{self, SheetJob},
    platform::{OutgoingFile, Platform, PlatformError, mime},
};

use slint::ComponentHandle;

use crate::{AppWindow, design::Shared, push::toast};

/// What to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// The cutting sheet HTML, offered through the share sheet.
    Sheet,
    /// The cutting sheet HTML, handed to the print dialog.
    Print,
    /// The diagram PNG, offered through the share sheet.
    Diagram,
}

/// What the worker thread made.
enum Ready {
    Share(OutgoingFile),
    Print { title: String, html: String },
}

/// Starts making `kind` of the open design. A design without a current solve gets a
/// message instead.
pub(super) fn start(window: &AppWindow, state: &Shared, kind: Kind) {
    let (job, platform) = {
        let app = state.borrow();
        (
            cutting::sheet_job(&app, &cutting::date_text_now()),
            Arc::clone(app.platform()),
        )
    };
    let job = match job {
        Ok(job) => job,
        Err(error) => {
            toast::show(window, &error.to_string());
            return;
        }
    };
    toast::show(
        window,
        match kind {
            Kind::Sheet | Kind::Print => "Preparing the cutting sheet...",
            Kind::Diagram => "Drawing the diagram...",
        },
    );
    let weak = window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("cut-export".to_owned())
        .spawn(move || {
            let built = catch_unwind(AssertUnwindSafe(|| build(&job, kind)))
                .unwrap_or_else(|_| Err("The cutting sheet could not be built.".to_owned()));
            let delivered = weak.upgrade_in_event_loop(move |window| {
                deliver(&window, platform.as_ref(), built);
            });
            if let Err(error) = delivered {
                tracing::debug!(%error, "a cut export was dropped: the UI thread is gone");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "a cut export could not start");
        toast::show(window, "The cutting sheet could not be built.");
    }
}

/// Makes the file or the print job (on the worker thread).
fn build(job: &SheetJob, kind: Kind) -> Result<Ready, String> {
    match kind {
        Kind::Sheet => Ok(Ready::Share(OutgoingFile {
            suggested_name: cutting::file_name(job.title(), "_cutting_sheet.html"),
            mime: mime::HTML.to_owned(),
            bytes: job.html().into_bytes(),
        })),
        Kind::Print => Ok(Ready::Print {
            title: format!("{} Cutting Sheet", job.title()),
            html: job.html(),
        }),
        Kind::Diagram => job.diagram_png().map(|png| {
            Ready::Share(OutgoingFile {
                suggested_name: cutting::file_name(job.title(), "_diagram.png"),
                mime: mime::PNG.to_owned(),
                bytes: png,
            })
        }),
    }
}

/// Hands the result to the platform (on the UI thread) and reports a failure.
fn deliver(window: &AppWindow, platform: &dyn Platform, built: Result<Ready, String>) {
    let outcome: Result<(), PlatformError> = match built {
        Ok(Ready::Share(file)) => platform.share(file),
        Ok(Ready::Print { title, html }) => platform.print_html(&title, &html),
        Err(message) => {
            toast::show(window, &message);
            return;
        }
    };
    if let Err(error) = outcome {
        toast::show(window, &error.message);
    }
}
