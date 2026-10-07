//! One parser per command: the words after the command's name into its argument struct.

use super::{
    types::{
        ExportArgs, ExportFormat, InfoArgs, JobCommandKind, MaterialArg, MetricsArgs, Mode,
        OptimizeArgs, RenderJobArgs, ReportFormat, RetargetArgs, SolveArgs, SweepArgs,
        ValidateArgs,
    },
    values::{
        Cursor, parse_candidates, parse_crown_fraction, parse_export_format, parse_lighting,
        parse_mode, parse_preset, parse_range, parse_ri, set_material,
    },
};
use indicatrix_cut_core::{CANONICAL_LIGHTING_PRESET, ObjectivePreset};
use indicatrix_editor::{
    optimize_view::{
        DEFAULT_BUDGET, DEFAULT_CANDIDATES, DEFAULT_STARTS, parse_budget, parse_seed, parse_starts,
    },
    retarget::CrownShift,
};
use indicatrix_render_jobs::{ComputeChoice, LocalEngines, TransferChoice};
use std::path::PathBuf;

pub(super) fn parse_info(args: &[String]) -> Result<InfoArgs, String> {
    let mut cursor = Cursor::new("info", args);
    let (mut design, mut json, mut out, mut db) = (None, false, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--json" => json = true,
            "--out" => out = Some(cursor.path("--out")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut design)?,
        }
    }
    Ok(InfoArgs {
        design: cursor.require_design(design)?,
        json,
        out,
        db,
    })
}

pub(super) fn parse_solve(args: &[String]) -> Result<SolveArgs, String> {
    let mut cursor = Cursor::new("solve", args);
    let (mut design, mut json, mut out, mut db) = (None, false, None, None);
    while let Some(word) = cursor.token() {
        match word {
            "--json" => json = true,
            "--out" => out = Some(cursor.path("--out")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut design)?,
        }
    }
    Ok(SolveArgs {
        design: cursor.require_design(design)?,
        json,
        out,
        db,
    })
}

pub(super) fn parse_metrics(args: &[String]) -> Result<MetricsArgs, String> {
    let mut cursor = Cursor::new("metrics", args);
    let mut design = None;
    let mut material = None;
    let mut tilt = false;
    let (mut json, mut csv) = (false, false);
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let (mut out, mut db) = (None, None);
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
            "--out" => out = Some(cursor.path("--out")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut design)?,
        }
    }
    if json && csv {
        return Err("give one of --json and --csv, not both".to_string());
    }
    let format = if json {
        ReportFormat::Json
    } else if csv {
        ReportFormat::Csv
    } else {
        ReportFormat::Text
    };
    Ok(MetricsArgs {
        design: cursor.require_design(design)?,
        material,
        tilt,
        format,
        lighting,
        out,
        db,
    })
}

pub(super) fn parse_validate(args: &[String]) -> Result<ValidateArgs, String> {
    let mut cursor = Cursor::new("validate", args);
    let mut design = None;
    let mut material = None;
    let mut json = false;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let (mut out, mut db) = (None, None);
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
            "--out" => out = Some(cursor.path("--out")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut design)?,
        }
    }
    Ok(ValidateArgs {
        design: cursor.require_design(design)?,
        json,
        material,
        lighting,
        out,
        db,
    })
}

pub(super) fn parse_optimize(args: &[String]) -> Result<OptimizeArgs, String> {
    let mut cursor = Cursor::new("optimize", args);
    let mut design = None;
    let mut preset = ObjectivePreset::Balanced;
    let mut budget = DEFAULT_BUDGET;
    let mut starts = DEFAULT_STARTS;
    let mut seed = 0_u64;
    let mut vary_anchored = false;
    let mut candidates = DEFAULT_CANDIDATES;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let mut json = false;
    let (mut out, mut db) = (None, None);
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
            "--out" => out = Some(cursor.path("--out")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut design)?,
        }
    }
    Ok(OptimizeArgs {
        design: cursor.require_design(design)?,
        preset,
        budget,
        starts,
        seed,
        vary_anchored,
        candidates,
        lighting,
        json,
        out,
        db,
    })
}

pub(super) fn parse_retarget(args: &[String]) -> Result<RetargetArgs, String> {
    let mut cursor = Cursor::new("retarget", args);
    let mut design = None;
    let mut target = None;
    let mut mode = Mode::Shift;
    let mut crown = CrownShift::default();
    let (mut fraction_given, mut ratio_given, mut follow_given) = (false, false, false);
    let mut preset = ObjectivePreset::Balanced;
    let mut range_deg = None;
    let mut budget = None;
    let mut seed = 0_u64;
    let mut keep_look = true;
    let mut lighting = CANONICAL_LIGHTING_PRESET;
    let mut json = false;
    let (mut out, mut db) = (None, None);
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
            "--lighting" => lighting = parse_lighting(cursor.value("--lighting")?)?,
            "--json" => json = true,
            "--out" => out = Some(cursor.path("--out")?),
            "--db" => db = Some(cursor.path("--db")?),
            other => cursor.design_word(other, &mut design)?,
        }
    }
    if usize::from(fraction_given) + usize::from(ratio_given) + usize::from(follow_given) > 1 {
        return Err(
            "give one of --crown-follow, --crown-fraction F and --crown-ratio, not several"
                .to_string(),
        );
    }
    let design = cursor.require_design(design)?;
    let target = target.ok_or_else(|| {
        "\"retarget\" needs the material to retarget for: --material NAME or --ri N (see --help)"
            .to_string()
    })?;
    Ok(RetargetArgs {
        design,
        target,
        mode,
        crown,
        preset,
        range_deg,
        budget,
        seed,
        keep_look,
        lighting,
        json,
        out,
        db,
    })
}

pub(super) fn parse_sweep(args: &[String]) -> Result<SweepArgs, String> {
    let mut cursor = Cursor::new("sweep", args);
    let mut design = None;
    let mut tier = None;
    let (mut from_deg, mut to_deg, mut step_deg) = (None, None, None);
    let mut tilt = false;
    let mut csv = None;
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
            "--csv" => csv = Some(cursor.path("--csv")?),
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
            other => cursor.design_word(other, &mut design)?,
        }
    }
    let design = cursor.require_design(design)?;
    let missing = |flag: &str| format!("\"sweep\" needs {flag} (see --help)");
    Ok(SweepArgs {
        design,
        tier: tier.ok_or_else(|| missing("--tier NAME"))?,
        from_deg: from_deg.ok_or_else(|| missing("--from ANGLE"))?,
        to_deg: to_deg.ok_or_else(|| missing("--to ANGLE"))?,
        step_deg: step_deg.ok_or_else(|| missing("--step DEGREES"))?,
        tilt,
        csv,
        json,
        material,
        lighting,
        db,
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

pub(super) fn parse_export(args: &[String]) -> Result<ExportArgs, String> {
    let mut cursor = Cursor::new("export", args);
    let mut design = None;
    let mut format = None;
    let mut out: Option<PathBuf> = None;
    let mut db = None;
    let mut date = None;
    while let Some(word) = cursor.token() {
        match word {
            "--format" => format = Some(parse_export_format(cursor.value("--format")?)?),
            "--out" => out = Some(cursor.path("--out")?),
            "--db" => db = Some(cursor.path("--db")?),
            "--date" => date = Some(cursor.value("--date")?.to_string()),
            other => cursor.design_word(other, &mut design)?,
        }
    }
    let design = cursor.require_design(design)?;
    let out = out.ok_or_else(|| "\"export\" needs --out FILE (see --help)".to_string())?;
    let format = format
        .or_else(|| ExportFormat::from_file_name(&out.to_string_lossy()))
        .ok_or_else(|| {
            "\"export\" needs --format asc|indicatrix|gcs|html, or an --out file ending in \
             .asc, .indicatrix, .gcs or .html (see --help)"
                .to_string()
        })?;
    Ok(ExportArgs {
        design,
        format,
        out,
        db,
        date,
    })
}
