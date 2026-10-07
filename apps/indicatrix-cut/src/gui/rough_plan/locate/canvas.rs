//! Turning the window-free drawing and report data of [`crate::locate_io`] into the Slint
//! models of the two windows, and showing one photo with its drawing on a canvas.

use super::photo::Photo;
use crate::{
    CanvasMarker, CanvasPath, PhotoSlot, ReportRow,
    locate_io::{
        overlay::{Drawing, MarkerSpec, PathSpec},
        report::Row,
    },
};
use slint::{Image, ModelRc, SharedString, VecModel};

/// A model of strings.
pub(super) fn strings_model(items: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// A model of report lines.
pub(super) fn rows_model(rows: Vec<Row>) -> ModelRc<ReportRow> {
    ModelRc::new(VecModel::from(
        rows.into_iter()
            .map(|row| ReportRow {
                text: row.text.into(),
                warn: row.warn,
            })
            .collect::<Vec<_>>(),
    ))
}

/// A model of photo slots.
pub(super) fn slots_model(slots: Vec<PhotoSlot>) -> ModelRc<PhotoSlot> {
    ModelRc::new(VecModel::from(slots))
}

fn marker(spec: MarkerSpec) -> CanvasMarker {
    CanvasMarker {
        fx: spec.fraction[0],
        fy: spec.fraction[1],
        kind: spec.kind,
        label: spec.label.into(),
    }
}

fn path(spec: PathSpec) -> CanvasPath {
    CanvasPath {
        commands: spec.commands.into(),
        kind: spec.kind,
    }
}

/// One slot of a window's photo list.
pub(super) fn slot(name: &str, photo: Option<&Photo>, status: String, active: bool) -> PhotoSlot {
    PhotoSlot {
        name: name.into(),
        file: photo.map(Photo::file_name).unwrap_or_default().into(),
        status: status.into(),
        active,
        loaded: photo.is_some(),
    }
}

/// The properties of a canvas, set on whichever window shows it.
pub(super) struct CanvasView {
    /// The picture (empty without a photo).
    pub image: Image,
    /// The photo's own width, 1 without a photo.
    pub width: i32,
    /// The photo's own height, 1 without a photo.
    pub height: i32,
    /// Whether there is a photo.
    pub has_photo: bool,
    /// The markers over it.
    pub markers: ModelRc<CanvasMarker>,
    /// The paths over it.
    pub paths: ModelRc<CanvasPath>,
}

impl CanvasView {
    /// The canvas for `photo` (if any) with `drawing` over it.
    pub(super) fn new(photo: Option<&Photo>, drawing: Drawing) -> Self {
        let (markers, paths) = drawing;
        Self {
            image: photo.map(|p| p.image.clone()).unwrap_or_default(),
            width: photo.map_or(1, |p| i32::try_from(p.size[0]).unwrap_or(i32::MAX)),
            height: photo.map_or(1, |p| i32::try_from(p.size[1]).unwrap_or(i32::MAX)),
            has_photo: photo.is_some(),
            markers: ModelRc::new(VecModel::from(
                markers.into_iter().map(marker).collect::<Vec<_>>(),
            )),
            paths: ModelRc::new(VecModel::from(
                paths.into_iter().map(path).collect::<Vec<_>>(),
            )),
        }
    }
}
