//! The pieces every command's parser shares: the [`Cursor`] along its words and the checks of
//! one flag's value.

use super::types::{ExportFormat, LIGHTING_NAMES, MaterialArg, Mode, PRESET_NAMES};
use indicatrix::optics::LightingPreset;
use indicatrix_cut_core::ObjectivePreset;
use indicatrix_editor::optimize_view::MAX_CANDIDATES;
use std::path::PathBuf;

/// The greatest refractive index `--ri` takes. Rutile, the highest gem, is 2.9.
const MAX_RI: f64 = 5.0;

/// The widest search range `--range` takes, in degrees either side of each angle.
const MAX_RANGE_DEG: f64 = 45.0;

/// A walk along one command's words.
pub(super) struct Cursor<'a> {
    command: &'static str,
    args: &'a [String],
    next: usize,
}

impl<'a> Cursor<'a> {
    pub(super) const fn new(command: &'static str, args: &'a [String]) -> Self {
        Self {
            command,
            args,
            next: 0,
        }
    }

    /// The next word, if any.
    pub(super) fn token(&mut self) -> Option<&'a str> {
        let word = self.args.get(self.next)?;
        self.next += 1;
        Some(word.as_str())
    }

    /// The value of `flag`: the next word, whatever it looks like (`-43` is a value).
    pub(super) fn value(&mut self, flag: &str) -> Result<&'a str, String> {
        let word = self
            .args
            .get(self.next)
            .ok_or_else(|| format!("{flag} needs a value (see --help)"))?;
        self.next += 1;
        Ok(word.as_str())
    }

    pub(super) fn path(&mut self, flag: &str) -> Result<PathBuf, String> {
        self.value(flag).map(PathBuf::from)
    }

    pub(super) fn number(&mut self, flag: &str) -> Result<f64, String> {
        let text = self.value(flag)?;
        text.trim()
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .ok_or_else(|| format!("{flag} needs a number, got {text:?}"))
    }

    fn unknown(&self, word: &str) -> String {
        format!(
            "unknown flag {word:?} for \"{}\" (see --help)",
            self.command
        )
    }

    /// Takes `word` as the design file, or says why it cannot be one.
    pub(super) fn design_word(
        &self,
        word: &str,
        design: &mut Option<PathBuf>,
    ) -> Result<(), String> {
        if word.starts_with('-') {
            return Err(self.unknown(word));
        }
        if design.is_some() {
            return Err(format!(
                "unexpected extra argument {word:?} for \"{}\": give one design file (see --help)",
                self.command
            ));
        }
        *design = Some(PathBuf::from(word));
        Ok(())
    }

    pub(super) fn require_design(&self, design: Option<PathBuf>) -> Result<PathBuf, String> {
        design.ok_or_else(|| format!("\"{}\" needs a design file (see --help)", self.command))
    }
}

/// `ri` as a refractive index, or why it is not one.
pub(super) fn parse_ri(ri: f64) -> Result<f64, String> {
    if ri > 1.0 && ri <= MAX_RI {
        Ok(ri)
    } else {
        Err(format!(
            "--ri needs a refractive index above 1 and at most {MAX_RI}, got {ri}"
        ))
    }
}

/// `fraction` as the share of the pavilion's shift the crown follows.
pub(super) fn parse_crown_fraction(fraction: f64) -> Result<f64, String> {
    if (0.0..=1.0).contains(&fraction) {
        Ok(fraction)
    } else {
        Err(format!(
            "--crown-fraction needs a number from 0 to 1, got {fraction}"
        ))
    }
}

/// `range` as a search range in degrees.
pub(super) fn parse_range(range: f64) -> Result<f64, String> {
    if range > 0.0 && range <= MAX_RANGE_DEG {
        Ok(range)
    } else {
        Err(format!(
            "--range needs degrees above 0 and at most {MAX_RANGE_DEG}, got {range}"
        ))
    }
}

/// The algorithm a `--mode` word names.
pub(super) fn parse_mode(word: &str) -> Result<Mode, String> {
    match word.to_ascii_lowercase().as_str() {
        "shift" => Ok(Mode::Shift),
        "optimize" => Ok(Mode::Optimize),
        other => Err(format!(
            "unknown mode {other:?} (expected \"shift\" or \"optimize\")"
        )),
    }
}

/// The format a `--format` word names.
pub(super) fn parse_export_format(word: &str) -> Result<ExportFormat, String> {
    match word.to_ascii_lowercase().as_str() {
        "asc" => Ok(ExportFormat::Asc),
        "indicatrix" => Ok(ExportFormat::Indicatrix),
        "gcs" => Ok(ExportFormat::Gcs),
        "html" => Ok(ExportFormat::Html),
        other => Err(format!(
            "unknown format {other:?} (expected asc, indicatrix, gcs or html)"
        )),
    }
}

/// Records a `--material` or `--ri`, refusing a second one.
pub(super) fn set_material(slot: &mut Option<MaterialArg>, arg: MaterialArg) -> Result<(), String> {
    if slot.is_some() {
        return Err("give one of --material NAME or --ri N, and give it once".to_string());
    }
    *slot = Some(arg);
    Ok(())
}

/// The preset a `--lighting` name stands for.
pub(super) fn parse_lighting(name: &str) -> Result<LightingPreset, String> {
    let lower = name.to_ascii_lowercase();
    LIGHTING_NAMES
        .iter()
        .find(|(known, _)| *known == lower)
        .map(|(_, preset)| *preset)
        .ok_or_else(|| {
            let names: Vec<&str> = LIGHTING_NAMES.iter().map(|(known, _)| *known).collect();
            format!(
                "unknown lighting {name:?} (expected one of: {})",
                names.join(", ")
            )
        })
}

/// The objective a `--preset` name stands for.
pub(super) fn parse_preset(name: &str) -> Result<ObjectivePreset, String> {
    let lower = name.to_ascii_lowercase();
    PRESET_NAMES
        .iter()
        .find(|(known, _)| *known == lower)
        .map(|(_, preset)| *preset)
        .ok_or_else(|| {
            let names: Vec<&str> = PRESET_NAMES.iter().map(|(known, _)| *known).collect();
            format!(
                "unknown preset {name:?} (expected one of: {})",
                names.join(", ")
            )
        })
}

/// The candidate count `--candidates` names.
pub(super) fn parse_candidates(text: &str) -> Result<usize, String> {
    text.trim()
        .parse::<usize>()
        .ok()
        .filter(|count| (1..=MAX_CANDIDATES).contains(count))
        .ok_or_else(|| {
            format!("--candidates needs a whole number from 1 to {MAX_CANDIDATES}, got {text:?}")
        })
}
