use crate::{MainWindow, OpticalModel};
use slint::ComponentHandle;
#[derive(Clone, Copy)]
#[repr(i32)]
pub(super) enum Status {
    Off = 0,
    Updating = 1,
    Draft = 2,
    Ready = 3,
    Unavailable = 4,
    Suspended = 5,
}
/// The status line: the state's name, then the detail after a dash. Details are Title Case.
pub(super) fn text(status: Status, detail: &str) -> String {
    let label = match status {
        Status::Off => "Off",
        Status::Updating => "Updating",
        Status::Draft => "Draft / Refining",
        Status::Ready => "Ready",
        Status::Unavailable => "Unavailable",
        Status::Suspended => "Suspended",
    };
    if detail.is_empty() {
        label.to_string()
    } else {
        format!("{label} - {detail}")
    }
}
/// Whether the model already shows exactly this status line, so a repeat write is a no-op.
pub(super) fn is_current(ui: &MainWindow, status: Status, detail: &str) -> bool {
    let m = ui.global::<OpticalModel>();
    m.get_status_code() == status as i32 && m.get_status().as_str() == text(status, detail)
}
pub(super) fn set(ui: &MainWindow, status: Status, detail: &str) {
    let m = ui.global::<OpticalModel>();
    m.set_status_code(status as i32);
    m.set_status(text(status, detail).into());
    m.set_progress(match status {
        Status::Ready => 1.0,
        Status::Draft => 0.5,
        _ => 0.0,
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_lines_join_state_and_detail() {
        assert_eq!(text(Status::Off, ""), "Off");
        assert_eq!(
            text(Status::Updating, "Camera Moving"),
            "Updating - Camera Moving"
        );
        assert_eq!(text(Status::Draft, ""), "Draft / Refining");
    }
}
