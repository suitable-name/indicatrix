//! The index wheel of a step: the gear's ring of teeth with the step's indices marked.
//!
//! Geometry only. Everything is in a square of [`BOX`] units with the wheel centred in it, tooth
//! 0 at the top and the numbers running clockwise, the way an index plate is read. The tick and
//! spoke strokes come as SVG path commands for one Slint `Path` each; the marks and the scale
//! numbers come as positions in fractions of the box, which the screen multiplies by its size.

use super::{CuttingStep, progress::Progress};
use std::fmt::Write as _;

/// The side of the square the wheel is drawn in.
pub const BOX: f64 = 200.0;
/// The centre of the wheel, in box units.
const CENTRE: f64 = BOX / 2.0;
/// The radius of the ring the teeth sit on.
const RING: f64 = 76.0;
/// The radius at which a minor tick starts (it ends on the ring).
const MINOR_INNER: f64 = 70.0;
/// The radius at which a major tick starts.
const MAJOR_INNER: f64 = 63.0;
/// The radius of the scale numbers, inside the ring.
const SCALE_RADIUS: f64 = 50.0;
/// The radius of the marked indices' own numbers, outside the ring.
const LABEL_RADIUS: f64 = 91.0;
/// Past this many marked indices their numbers would overlap; the dots alone are drawn.
const MAX_LABELLED_MARKS: usize = 16;
/// The most minor ticks drawn: a finer gear draws every few teeth.
const MAX_MINOR_TICKS: u32 = 192;

/// One marked index: its dot on the ring and its number outside it.
#[derive(Debug, Clone, PartialEq)]
pub struct WheelMark {
    /// The dot's position, as a fraction of the box from the left.
    pub x: f32,
    /// The dot's position, as a fraction of the box from the top.
    pub y: f32,
    /// The number's position from the left.
    pub label_x: f32,
    /// The number's position from the top.
    pub label_y: f32,
    /// The index as the sheet prints it; empty when too many are marked to label.
    pub label: String,
    /// Whether the index is ticked on the page.
    pub ticked: bool,
}

/// A number printed inside the ring at a major tick.
#[derive(Debug, Clone, PartialEq)]
pub struct WheelScale {
    /// From the left, as a fraction of the box.
    pub x: f32,
    /// From the top, as a fraction of the box.
    pub y: f32,
    /// The tooth number.
    pub label: String,
}

/// The index wheel with a step's indices marked.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexWheel {
    /// Path commands for the minor ticks, in box units.
    pub minor_ticks: String,
    /// Path commands for the major ticks.
    pub major_ticks: String,
    /// Path commands for the lines from the centre to each marked index.
    pub spokes: String,
    /// The marked indices.
    pub marks: Vec<WheelMark>,
    /// The numbers at the major ticks.
    pub scale: Vec<WheelScale>,
}

impl IndexWheel {
    /// The wheel of a `gear_teeth`-tooth gear with the indices of `step` marked, ticked ones
    /// as `progress` has them. A gear without teeth draws nothing.
    #[must_use]
    pub fn new(gear_teeth: u32, step: &CuttingStep, progress: &Progress) -> Self {
        let mut wheel = Self {
            minor_ticks: String::new(),
            major_ticks: String::new(),
            spokes: String::new(),
            marks: Vec::new(),
            scale: Vec::new(),
        };
        if gear_teeth == 0 {
            return wheel;
        }
        let major = major_step(gear_teeth);
        let minor = gear_teeth.div_ceil(MAX_MINOR_TICKS).max(1);
        for tooth in (0..gear_teeth).step_by(minor as usize) {
            if !tooth.is_multiple_of(major) {
                push_segment(&mut wheel.minor_ticks, tooth, gear_teeth, MINOR_INNER);
            }
        }
        for tooth in (0..gear_teeth).step_by(major as usize) {
            push_segment(&mut wheel.major_ticks, tooth, gear_teeth, MAJOR_INNER);
            let (x, y) = place(SCALE_RADIUS, f64::from(tooth), gear_teeth);
            wheel.scale.push(WheelScale {
                x: fraction(x),
                y: fraction(y),
                label: tooth.to_string(),
            });
        }
        let labelled = step.indices.len() <= MAX_LABELLED_MARKS;
        for (chip, index) in step.indices.iter().enumerate() {
            let (x, y) = place(RING, index.value, gear_teeth);
            let (label_x, label_y) = place(LABEL_RADIUS, index.value, gear_teeth);
            let _ = write!(wheel.spokes, "M {CENTRE:.2} {CENTRE:.2} L {x:.2} {y:.2} ");
            wheel.marks.push(WheelMark {
                x: fraction(x),
                y: fraction(y),
                label_x: fraction(label_x),
                label_y: fraction(label_y),
                label: if labelled {
                    index.text.clone()
                } else {
                    String::new()
                },
                ticked: progress.chip_ticked(step, chip),
            });
        }
        wheel
    }
}

/// How far apart the major ticks are, in teeth: the gear split into up to 16 equal arcs, or
/// about a dozen when the tooth count has no such divisor.
#[must_use]
pub fn major_step(gear_teeth: u32) -> u32 {
    [16, 12, 10, 8, 6, 4, 2]
        .into_iter()
        .find(|arcs| gear_teeth.is_multiple_of(*arcs) && gear_teeth / arcs >= 1)
        .map_or_else(|| (gear_teeth / 12).max(1), |arcs| gear_teeth / arcs)
}

/// The point at `radius` for wheel position `position` of a `gear_teeth`-tooth gear, in box
/// units: position 0 at the top, increasing clockwise.
fn place(radius: f64, position: f64, gear_teeth: u32) -> (f64, f64) {
    let turn = position.rem_euclid(f64::from(gear_teeth)) / f64::from(gear_teeth);
    let angle = turn * std::f64::consts::TAU;
    (
        radius.mul_add(angle.sin(), CENTRE),
        (-radius).mul_add(angle.cos(), CENTRE),
    )
}

/// Appends the tick of `tooth`, from `inner` out to the ring, as `M x y L x y `.
fn push_segment(commands: &mut String, tooth: u32, gear_teeth: u32, inner: f64) {
    let (x0, y0) = place(inner, f64::from(tooth), gear_teeth);
    let (x1, y1) = place(RING, f64::from(tooth), gear_teeth);
    let _ = write!(commands, "M {x0:.2} {y0:.2} L {x1:.2} {y1:.2} ");
}

/// A coordinate in box units as a fraction of the box.
fn fraction(coordinate: f64) -> f32 {
    (coordinate / BOX) as f32
}
