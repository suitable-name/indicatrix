//! One parser per command: the words after the command's name into its argument struct.

use super::{
    batch::{Fan, fan_out},
    types::{
        CensusArgs, CensusBuiltins, CensusOutput, Command, ExportArgs, ExportFormat, InfoArgs,
        JobCommandKind, MaterialArg, MetricsArgs, Mode, OptimizeArgs, RenderJobArgs, ReportFormat,
        ReportStyle, RetargetArgs, SolveArgs, SweepArgs, ValidateArgs,
    },
    values::{
        Cursor, parse_candidates, parse_count, parse_crown_fraction, parse_crown_mode,
        parse_export_format, parse_ids, parse_lighting, parse_mode, parse_modes, parse_preset,
        parse_range, parse_ri, parse_targets, set_material,
    },
};
use indicatrix_cut_core::{CANONICAL_LIGHTING_PRESET, ObjectivePreset};
use indicatrix_editor::{
    optimize_view::{
        DEFAULT_BUDGET, DEFAULT_CANDIDATES, DEFAULT_STARTS, parse_budget, parse_seed, parse_starts,
    },
    retarget::CrownShift,
};
use indicatrix_formats::native::design::{DESIGN_EXTENSION, DESIGN_EXTENSION_DOTTED};
use indicatrix_render_jobs::{ComputeChoice, LocalEngines, TransferChoice};
use std::path::PathBuf;

pub(super) fn parse_info(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("info", args);
    let (mut designs, mut json, mut out, mut out_dir, mut db) =
        (Vec::new(), false, None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--json" => json = true,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let fan = Fan {
        command: "info",
        designs: cursor.require_designs(designs)?,
        out,
        out_dir,
        extension: if json { "json" } else { "txt" },
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, out| {
        Command::Info(InfoArgs {
            design,
            json,
            out,
            db: db.clone(),
        })
    })
}

pub(super) fn parse_solve(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("solve", args);
    let (mut designs, mut json, mut out, mut out_dir, mut db) =
        (Vec::new(), false, None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--json" => json = true,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let fan = Fan {
        command: "solve",
        designs: cursor.require_designs(designs)?,
        out,
        out_dir,
        extension: DESIGN_EXTENSION,
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, out| {
        Command::Solve(SolveArgs {
            design,
            json,
            out,
            db: db.clone(),
        })
    })
}

pub(super) fn parse_metrics(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("metrics", args);
    let mut designs = Vec::new();
    let mut material = None;
    let mut tilt = false;
    let (mut json, mut csv) = (false, false);
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let (mut out, mut out_dir, mut db) = (None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--material" => {
                let name = cursor.value("--material")?;
                set_material(&mut material, MaterialArg::Named(name.to_string()))?;
            }
            "--ri" => {
                let ri = parse_ri(cursor.number("--ri")?)?;
                set_material(&mut material, MaterialArg::Ri(ri))?;
            }
            "--tilt" => tilt = true,
            "--json" => json = true,
            "--csv" => csv = true,
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    if json && csv {
        return Err("give one of --json and --csv, not both".to_string());
    }
    let (format, extension, style) = if json {
        (ReportFormat::Json, "json", ReportStyle::Json)
    } else if csv {
        (ReportFormat::Csv, "csv", ReportStyle::Csv)
    } else {
        (ReportFormat::Text, "txt", ReportStyle::Text)
    };
    let fan = Fan {
        command: "metrics",
        designs: cursor.require_designs(designs)?,
        out,
        out_dir,
        extension,
        style,
    };
    fan_out(fan, |design, out| {
        Command::Metrics(MetricsArgs {
            design,
            material: material.clone(),
            tilt,
            format,
            lighting,
            out,
            db: db.clone(),
        })
    })
}

pub(super) fn parse_validate(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("validate", args);
    let mut designs = Vec::new();
    let mut material = None;
    let mut json = false;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let (mut out, mut out_dir, mut db) = (None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--material" => {
                let name = cursor.value("--material")?;
                set_material(&mut material, MaterialArg::Named(name.to_string()))?;
            }
            "--ri" => {
                let ri = parse_ri(cursor.number("--ri")?)?;
                set_material(&mut material, MaterialArg::Ri(ri))?;
            }
            "--json" => json = true,
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let fan = Fan {
        command: "validate",
        designs: cursor.require_designs(designs)?,
        out,
        out_dir,
        extension: if json { "json" } else { "txt" },
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, out| {
        Command::Validate(ValidateArgs {
            design,
            json,
            material: material.clone(),
            lighting,
            out,
            db: db.clone(),
        })
    })
}

pub(super) fn parse_optimize(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("optimize", args);
    let mut designs = Vec::new();
    let mut preset = ObjectivePreset::Balanced;
    let mut budget = DEFAULT_BUDGET;
    let mut starts = DEFAULT_STARTS;
    let mut seed = 0_u64;
    let mut vary_anchored = false;
    let mut candidates = DEFAULT_CANDIDATES;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let mut json = false;
    let (mut out, mut out_dir, mut db) = (None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--preset" => preset = parse_preset(cursor.value("--preset")?)?,
            "--budget" => budget = parse_budget(cursor.value("--budget")?)?,
            "--starts" => starts = parse_starts(cursor.value("--starts")?)?,
            "--seed" => seed = parse_seed(cursor.value("--seed")?)?,
            "--vary-anchored" => vary_anchored = true,
            "--candidates" => candidates = parse_candidates(cursor.value("--candidates")?)?,
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--json" => json = true,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let fan = Fan {
        command: "optimize",
        designs: cursor.require_designs(designs)?,
        out,
        out_dir,
        extension: DESIGN_EXTENSION,
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, out| {
        Command::Optimize(OptimizeArgs {
            design,
            preset,
            budget,
            starts,
            seed,
            vary_anchored,
            candidates,
            lighting,
            json,
            out,
            db: db.clone(),
        })
    })
}

pub(super) fn parse_retarget(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("retarget", args);
    let mut designs = Vec::new();
    let mut target = None;
    let mut mode = Mode::Shift;
    let mut crown = CrownShift::default();
    let (mut fraction_given, mut ratio_given, mut follow_given) = (false, false, false);
    let mut preset = ObjectivePreset::Balanced;
    let mut range_deg = None;
    let mut budget = None;
    let mut seed = 0_u64;
    let mut keep_look = true;
    let mut girdle_allowance = false;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let mut json = false;
    let (mut out, mut out_dir, mut db) = (None, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--material" => {
                let name = cursor.value("--material")?;
                set_material(&mut target, MaterialArg::Named(name.to_string()))?;
            }
            "--ri" => {
                let ri = parse_ri(cursor.number("--ri")?)?;
                set_material(&mut target, MaterialArg::Ri(ri))?;
            }
            "--mode" => mode = parse_mode(cursor.value("--mode")?)?,
            "--crown-fraction" => {
                crown.fraction = parse_crown_fraction(cursor.number("--crown-fraction")?)?;
                // An explicit rule: it replaces the default of following the pavilion.
                crown.follow_pavilion = false;
                fraction_given = true;
            }
            "--crown-ratio" => {
                crown.scale_by_ratio = true;
                crown.follow_pavilion = false;
                ratio_given = true;
            }
            // The default, accepted so a script can say so.
            "--crown-follow" => {
                crown.follow_pavilion = true;
                follow_given = true;
            }
            "--preset" => preset = parse_preset(cursor.value("--preset")?)?,
            "--range" => range_deg = Some(parse_range(cursor.number("--range")?)?),
            "--budget" => budget = Some(parse_budget(cursor.value("--budget")?)?),
            "--seed" => seed = parse_seed(cursor.value("--seed")?)?,
            "--no-keep-look" => keep_look = false,
            "--girdle-allowance" => girdle_allowance = true,
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--json" => json = true,
            "--out" => out = Some(("--out", cursor.path("--out")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    if usize::from(fraction_given) + usize::from(ratio_given) + usize::from(follow_given) > 1 {
        return Err(
            "give one of --crown-follow, --crown-fraction F and --crown-ratio, not several"
                .to_string(),
        );
    }
    let designs = cursor.require_designs(designs)?;
    let target = target.ok_or_else(|| {
        "\"retarget\" needs the material to retarget for: --material NAME or --ri N (see --help)"
            .to_string()
    })?;
    let fan = Fan {
        command: "retarget",
        designs,
        out,
        out_dir,
        extension: DESIGN_EXTENSION,
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, out| {
        Command::Retarget(RetargetArgs {
            design,
            target: target.clone(),
            mode,
            crown,
            preset,
            range_deg,
            budget,
            seed,
            keep_look,
            girdle_allowance,
            lighting,
            json,
            out,
            db: db.clone(),
        })
    })
}

/// The materials `census` retargets to when `--targets` is not given.
const DEFAULT_CENSUS_TARGETS: [&str; 5] = [
    "Diamond",
    "Sapphire",
    "Quartz",
    "Cubic Zirconia",
    "Tourmaline",
];

/// The search budget `census` gives the optimize mode when `--budget` is not given.
const DEFAULT_CENSUS_BUDGET: usize = 100;

pub(super) fn parse_census(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("census", args);
    let mut files = Vec::new();
    let mut builtins = CensusBuiltins::default();
    let mut output = CensusOutput::default();
    let mut all = false;
    let (mut db, mut out, mut out_dir) = (None, None, None);
    let mut ids = Vec::new();
    let (mut limit, mut offset) = (None, 0_usize);
    let mut targets = None;
    let mut modes = None;
    let mut crown = CrownShift::default();
    let mut budget = DEFAULT_CENSUS_BUDGET;
    while let Some(word) = cursor.token() {
        match word {
            "--templates" => builtins.templates = true,
            "--fixtures" => builtins.fixtures = true,
            "--all" => all = true,
            "--timings" => output.timings = true,
            "--json" => output.json = true,
            "--db" => db = Some(cursor.path("--db")?),
            "--ids" => ids = parse_ids(cursor.value("--ids")?)?,
            "--limit" => limit = Some(parse_count("--limit", cursor.value("--limit")?)?),
            "--offset" => offset = parse_count("--offset", cursor.value("--offset")?)?,
            "--targets" => targets = Some(parse_targets(cursor.value("--targets")?)?),
            "--modes" => modes = Some(parse_modes(cursor.value("--modes")?)?),
            "--crown" => crown = parse_crown_mode(cursor.value("--crown")?)?,
            "--budget" => budget = parse_budget(cursor.value("--budget")?)?,
            "--out" => out = Some(cursor.path("--out")?),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            other => cursor.design_word(other, &mut files)?,
        }
    }
    if all && !ids.is_empty() {
        return Err("give one of --all and --ids, not both".to_string());
    }
    if (all || !ids.is_empty()) && db.is_none() {
        return Err("--all and --ids read a design library: give --db FILE".to_string());
    }
    if db.is_some() && !all && ids.is_empty() {
        return Err("--db needs --all or --ids LIST for the designs to measure".to_string());
    }
    if files.is_empty() && builtins == CensusBuiltins::default() && db.is_none() {
        return Err(
            "\"census\" needs designs to measure: --templates, --fixtures, design files or \
             --db FILE with --all or --ids (see --help)"
                .to_string(),
        );
    }
    Ok(Command::Census(CensusArgs {
        files,
        builtins,
        db,
        all,
        ids,
        limit,
        offset,
        targets: targets.unwrap_or_else(|| {
            DEFAULT_CENSUS_TARGETS
                .iter()
                .map(|name| MaterialArg::Named((*name).to_string()))
                .collect()
        }),
        modes: modes.unwrap_or_else(|| vec![Mode::Shift]),
        crown,
        budget,
        output,
        out,
        out_dir,
    }))
}

pub(super) fn parse_sweep(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("sweep", args);
    let mut designs = Vec::new();
    let mut tier = None;
    let (mut from_deg, mut to_deg, mut step_deg) = (None, None, None);
    let mut tilt = false;
    let (mut csv, mut out_dir) = (None, None);
    let mut json = false;
    let mut material = None;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let mut db = None;
    while let Some(word) = cursor.token() {
        match word {
            "--tier" => tier = Some(cursor.value("--tier")?.to_string()),
            "--from" => from_deg = Some(cursor.number("--from")?),
            "--to" => to_deg = Some(cursor.number("--to")?),
            "--step" => step_deg = Some(cursor.number("--step")?),
            "--tilt" => tilt = true,
            "--csv" => csv = Some(("--csv", cursor.path("--csv")?)),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--json" => json = true,
            "--material" => {
                let name = cursor.value("--material")?;
                set_material(&mut material, MaterialArg::Named(name.to_string()))?;
            }
            "--ri" => {
                let ri = parse_ri(cursor.number("--ri")?)?;
                set_material(&mut material, MaterialArg::Ri(ri))?;
            }
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let designs = cursor.require_designs(designs)?;
    let missing = |flag: &str| format!("\"sweep\" needs {flag} (see --help)");
    let tier = tier.ok_or_else(|| missing("--tier NAME"))?;
    let from_deg = from_deg.ok_or_else(|| missing("--from ANGLE"))?;
    let to_deg = to_deg.ok_or_else(|| missing("--to ANGLE"))?;
    let step_deg = step_deg.ok_or_else(|| missing("--step DEGREES"))?;
    let fan = Fan {
        command: "sweep",
        designs,
        out: csv,
        out_dir,
        extension: "csv",
        style: ReportStyle::json_or_text(json),
    };
    fan_out(fan, |design, csv| {
        Command::Sweep(SweepArgs {
            design,
            tier: tier.clone(),
            from_deg,
            to_deg,
            step_deg,
            tilt,
            csv,
            json,
            material: material.clone(),
            lighting,
            db: db.clone(),
        })
    })
}

fn parse_local(word: &str) -> Result<LocalEngines, String> {
    LocalEngines::from_cli_word(word)
        .ok_or_else(|| format!("unknown engines {word:?} (expected cpu, gpu or cpu+gpu)"))
}

fn parse_compute(word: &str) -> Result<ComputeChoice, String> {
    ComputeChoice::from_cli_word(word)
        .ok_or_else(|| format!("unknown compute choice {word:?} (expected local, remote or both)"))
}

fn parse_transfer(word: &str) -> Result<TransferChoice, String> {
    TransferChoice::from_cli_word(word)
        .ok_or_else(|| format!("unknown transfer choice {word:?} (expected full or final)"))
}

/// Parses the words of `render` or `tilt-video`: one job file and the engine options.
pub(super) fn parse_render_job(
    kind: JobCommandKind,
    args: &[String],
) -> Result<RenderJobArgs, String> {
    let command = kind.word();
    let mut cursor = Cursor::new(command, args);
    let mut job: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut local = LocalEngines::default();
    let mut engine = None;
    let (mut compute, mut transfer) = (None, None);
    let (mut remote, mut cert_dir): (Option<String>, Option<PathBuf>) = (None, None);
    let (mut contribute_local, mut restart, mut quiet) = (false, false, false);
    while let Some(word) = cursor.token() {
        match word {
            "--out" if kind == JobCommandKind::Render => out = Some(cursor.path("--out")?),
            "--out" => {
                return Err(
                    "--out is for \"render\"; use --out-dir with \"tilt-video\" (see --help)"
                        .to_string(),
                );
            }
            "--out-dir" if kind == JobCommandKind::TiltVideo => {
                out = Some(cursor.path("--out-dir")?);
            }
            "--restart" if kind == JobCommandKind::TiltVideo => restart = true,
            "--out-dir" | "--restart" => {
                return Err(format!(
                    "{word} is for \"tilt-video\", not \"{command}\" (see --help)"
                ));
            }
            "--engine" => {
                engine = Some(match cursor.value("--engine")? {
                    "path" => indicatrix::renderer::engine::RenderEngine::PathTraced,
                    "ray" => indicatrix::renderer::engine::RenderEngine::RayTraced,
                    other => {
                        return Err(format!(
                            "unknown renderer {other:?} for --engine (expected path or ray)"
                        ));
                    }
                });
            }
            "--local" => local = parse_local(cursor.value("--local")?)?,
            "--compute" => compute = Some(parse_compute(cursor.value("--compute")?)?),
            "--transfer" => transfer = Some(parse_transfer(cursor.value("--transfer")?)?),
            "--remote" => {
                let address = cursor.value("--remote")?.trim();
                if address.is_empty() {
                    return Err("--remote needs HOST:PORT, got an empty value".to_string());
                }
                remote = Some(address.to_string());
            }
            "--cert-dir" => cert_dir = Some(cursor.path("--cert-dir")?),
            "--contribute-local" => contribute_local = true,
            "--quiet" => quiet = true,
            other if other.starts_with('-') => {
                return Err(format!(
                    "unknown flag {other:?} for \"{command}\" (see --help)"
                ));
            }
            other => {
                if job.is_some() {
                    return Err(format!(
                        "unexpected extra argument {other:?} for \"{command}\": give one job file \
                         (see --help)"
                    ));
                }
                job = Some(PathBuf::from(other));
            }
        }
    }
    let job = job.ok_or_else(|| format!("\"{command}\" needs a job file (see --help)"))?;
    if remote.is_some() != cert_dir.is_some() {
        return Err("--remote and --cert-dir go together: give both or neither".to_string());
    }
    if matches!(compute, Some(ComputeChoice::Remote | ComputeChoice::Both)) && remote.is_none() {
        return Err("--compute remote needs --remote and --cert-dir".to_string());
    }
    Ok(RenderJobArgs {
        engine,
        kind,
        job,
        out,
        local,
        compute,
        transfer,
        remote,
        cert_dir,
        contribute_local,
        restart,
        quiet,
    })
}

pub(super) fn parse_export(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("export", args);
    let mut designs = Vec::new();
    let mut format = None;
    let mut out: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut db = None;
    let mut date = None;
    while let Some(word) = cursor.token() {
        match word {
            "--format" => format = Some(parse_export_format(cursor.value("--format")?)?),
            "--out" => out = Some(cursor.path("--out")?),
            "--out-dir" => out_dir = Some(cursor.path("--out-dir")?),
            "--db" => db = Some(cursor.path("--db")?),
            "--date" => date = Some(cursor.value("--date")?.to_string()),
            other => cursor.design_word(other, &mut designs)?,
        }
    }
    let designs = cursor.require_designs(designs)?;
    if out.is_none() && out_dir.is_none() {
        return Err(
            "\"export\" needs --out FILE, or --out-dir DIR for several designs (see --help)"
                .to_string(),
        );
    }
    // `--out-dir` names no file, so the format cannot come from the name there.
    let format = format
        .or_else(|| {
            out.as_ref()
                .and_then(|path| ExportFormat::from_file_name(&path.to_string_lossy()))
        })
        .ok_or_else(|| {
            format!(
                "\"export\" needs --format asc|{DESIGN_EXTENSION}|gcs|html, or an --out file \
                 ending in .asc, {DESIGN_EXTENSION_DOTTED}, .gcs or .html (see --help)"
            )
        })?;
    let fan = Fan {
        command: "export",
        designs,
        out: out.map(|file| ("--out", file)),
        out_dir,
        extension: format.extension(),
        style: ReportStyle::Text,
    };
    fan_out(fan, |design, out| {
        Command::Export(ExportArgs {
            design,
            format,
            // Always given: the line named `--out`, or `--out-dir` made one for each design.
            out: out.unwrap_or_default(),
            db: db.clone(),
            date: date.clone(),
        })
    })
}
