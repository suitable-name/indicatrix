//! The script writer: `run-render-jobs.sh` and `run-render-jobs.ps1`, which run each job
//! with `indicatrix-cli`, and the quoting that keeps every value literal.
//!
//! Every value goes into single quotes. In `sh` nothing inside single quotes is special
//! except the quote itself. In PowerShell the typographic single quotes (U+2018 to
//! U+201B) also close a string, so they are doubled too. A NUL character cannot be
//! quoted in either shell and is refused.

use crate::job::LocalEngines;
use std::fmt;

/// The folder of the job files, inside the script folder.
pub const JOBS_DIR: &str = "jobs";
/// The folder of copied HDR maps, inside [`JOBS_DIR`].
pub const ASSETS_DIR: &str = "assets";
/// The folder of finished pictures and videos, inside the script folder.
pub const RENDERS_DIR: &str = "renders";

/// Which shell a script is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptFlavor {
    /// A POSIX shell script.
    Sh,
    /// A PowerShell script.
    PowerShell,
}

impl ScriptFlavor {
    /// The script's file name.
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Sh => "run-render-jobs.sh",
            Self::PowerShell => "run-render-jobs.ps1",
        }
    }
}

/// Which `indicatrix-cli` command runs a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptCommand {
    /// `indicatrix-cli render`, for a still picture.
    Render,
    /// `indicatrix-cli tilt-video`.
    TiltVideo,
}

impl ScriptCommand {
    /// The command word.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Render => "render",
            Self::TiltVideo => "tilt-video",
        }
    }
}

/// One job line of a script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptJob {
    /// The command that runs the job.
    pub command: ScriptCommand,
    /// The job file, relative to the script, with `/` separators.
    pub job_file: String,
    /// The job label, written as a comment.
    pub label: String,
}

/// The remote worker a script renders with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteArgs {
    /// The worker address, `host:port`.
    pub address: String,
    /// The folder with the certificates.
    pub cert_dir: String,
}

/// What every job of a script shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSettings {
    /// The engines of the rendering computer.
    pub engines: LocalEngines,
    /// The remote worker, or `None` to render on this computer only.
    pub remote: Option<RemoteArgs>,
    /// A header line such as `Exported 2026-10-06 14:03 by Indicatrix Cut 0.6.2.`.
    pub header_note: String,
}

/// Why a script could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptError {
    /// A value holds a NUL character.
    Nul,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nul => write!(
                f,
                "A path or name contains a NUL character and cannot go into a script."
            ),
        }
    }
}

impl std::error::Error for ScriptError {}

/// A value in single quotes for `sh`; an embedded `'` becomes `'\''`.
///
/// # Errors
///
/// [`ScriptError::Nul`] when the value holds a NUL character.
pub fn sh_quote(value: &str) -> Result<String, ScriptError> {
    if value.contains('\0') {
        return Err(ScriptError::Nul);
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

/// A value in single quotes for PowerShell; each `'` and each of U+2018, U+2019, U+201A
/// and U+201B is doubled.
///
/// # Errors
///
/// [`ScriptError::Nul`] when the value holds a NUL character.
pub fn ps_quote(value: &str) -> Result<String, ScriptError> {
    if value.contains('\0') {
        return Err(ScriptError::Nul);
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        out.push(ch);
        if matches!(ch, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(ch);
        }
    }
    out.push('\'');
    Ok(out)
}

/// Text that is safe after `# `: control characters, including CR and LF, and the
/// Unicode line and paragraph separators become spaces.
#[must_use]
pub fn comment_text(label: &str) -> String {
    label
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

/// `1 job`, `2 jobs`.
fn count_phrase(count: usize) -> String {
    if count == 1 {
        "1 job".to_string()
    } else {
        format!("{count} jobs")
    }
}

/// `1 render job`, `2 render jobs`.
fn render_jobs_phrase(count: usize) -> String {
    if count == 1 {
        "1 render job".to_string()
    } else {
        format!("{count} render jobs")
    }
}

/// The values of the settings block, quoted for one shell.
struct Quoted {
    engines: String,
    remote: String,
    cert_dir: String,
}

fn quoted(
    settings: &ScriptSettings,
    quote: fn(&str) -> Result<String, ScriptError>,
) -> Result<Quoted, ScriptError> {
    let (remote, cert_dir) = settings.remote.as_ref().map_or(("", ""), |remote| {
        (remote.address.as_str(), remote.cert_dir.as_str())
    });
    Ok(Quoted {
        engines: quote(settings.engines.cli_word())?,
        remote: quote(remote)?,
        cert_dir: quote(cert_dir)?,
    })
}

fn header(jobs: &[ScriptJob], settings: &ScriptSettings, run_line: &str) -> Vec<String> {
    let mut lines = vec![format!(
        "# Indicatrix render jobs: {}.",
        count_phrase(jobs.len())
    )];
    if !settings.header_note.trim().is_empty() {
        lines.push(format!("# {}", comment_text(&settings.header_note)));
    }
    lines.push(run_line.to_string());
    lines.push(
        "# Each job runs with indicatrix-cli. Set INDICATRIX_CLI to its full path if it is not"
            .to_string(),
    );
    lines.push(
        "# on your PATH. A tilt video that stopped part way continues where it stopped."
            .to_string(),
    );
    lines.push(String::new());
    lines
}

fn sh_text(jobs: &[ScriptJob], settings: &ScriptSettings) -> Result<String, ScriptError> {
    let values = quoted(settings, sh_quote)?;
    let mut lines = vec!["#!/bin/sh".to_string()];
    lines.extend(header(
        jobs,
        settings,
        "# Run it with:  sh run-render-jobs.sh",
    ));
    lines.push(r#"cd "$(dirname "$0")" || exit 1"#.to_string());
    lines.push(r#"CLI="${INDICATRIX_CLI:-indicatrix-cli}""#.to_string());
    lines.push(String::new());
    lines.push("# Which engines of this computer render: cpu, gpu or cpu+gpu.".to_string());
    lines.push(format!("ENGINES={}", values.engines));
    lines.push(
        "# A remote worker (host:port) and its certificate folder. Leave REMOTE empty to render"
            .to_string(),
    );
    lines.push("# on this computer only.".to_string());
    lines.push(format!("REMOTE={}", values.remote));
    lines.push(format!("CERT_DIR={}", values.cert_dir));
    lines.push(String::new());
    lines.extend(
        [
            "failed=0",
            "run_job() {",
            r#"    if [ -n "$REMOTE" ]; then"#,
            r#"        "$CLI" "$@" --local "$ENGINES" --remote "$REMOTE" --cert-dir "$CERT_DIR""#,
            "    else",
            r#"        "$CLI" "$@" --local "$ENGINES""#,
            "    fi || failed=$((failed + 1))",
            "}",
            "",
        ]
        .map(str::to_string),
    );
    for (index, job) in jobs.iter().enumerate() {
        lines.push(format!("# {}. {}", index + 1, comment_text(&job.label)));
        lines.push(format!(
            "run_job {} {}",
            job.command.word(),
            sh_quote(&job.job_file)?
        ));
    }
    lines.push(String::new());
    lines.push(r#"if [ "$failed" -gt 0 ]; then"#.to_string());
    lines.push(format!(
        r#"    echo "$failed of {} failed." >&2"#,
        render_jobs_phrase(jobs.len())
    ));
    lines.push("    exit 1".to_string());
    lines.push("fi".to_string());
    lines.push(format!(
        r#"echo "All {} finished.""#,
        render_jobs_phrase(jobs.len())
    ));
    Ok(finish(&lines))
}

fn ps_text(jobs: &[ScriptJob], settings: &ScriptSettings) -> Result<String, ScriptError> {
    let values = quoted(settings, ps_quote)?;
    let mut lines = header(
        jobs,
        settings,
        "# Run it with:  powershell -ExecutionPolicy Bypass -File run-render-jobs.ps1",
    );
    lines.push("Set-Location -LiteralPath $PSScriptRoot".to_string());
    lines.push(
        "$Cli = if ($env:INDICATRIX_CLI) { $env:INDICATRIX_CLI } else { 'indicatrix-cli' }"
            .to_string(),
    );
    lines.push(String::new());
    lines.push("# Which engines of this computer render: cpu, gpu or cpu+gpu.".to_string());
    lines.push(format!("$Engines = {}", values.engines));
    lines.push(
        "# A remote worker (host:port) and its certificate folder. Leave $Remote empty to render"
            .to_string(),
    );
    lines.push("# on this computer only.".to_string());
    lines.push(format!("$Remote = {}", values.remote));
    lines.push(format!("$CertDir = {}", values.cert_dir));
    lines.push(String::new());
    lines.extend(
        [
            "$Failed = 0",
            "function Invoke-RenderJob([string[]] $JobArgs) {",
            "    $Extra = @('--local', $Engines)",
            "    if ($Remote -ne '') { $Extra += @('--remote', $Remote, '--cert-dir', $CertDir) }",
            "    & $Cli @JobArgs @Extra",
            "    if ($LASTEXITCODE -ne 0) { $script:Failed++ }",
            "}",
            "",
        ]
        .map(str::to_string),
    );
    for (index, job) in jobs.iter().enumerate() {
        lines.push(format!("# {}. {}", index + 1, comment_text(&job.label)));
        lines.push(format!(
            "Invoke-RenderJob @('{}', {})",
            job.command.word(),
            ps_quote(&job.job_file)?
        ));
    }
    lines.push(String::new());
    lines.push("if ($Failed -gt 0) {".to_string());
    lines.push(format!(
        r#"    Write-Host "$Failed of {} failed.""#,
        render_jobs_phrase(jobs.len())
    ));
    lines.push("    exit 1".to_string());
    lines.push("}".to_string());
    lines.push(format!(
        "Write-Host 'All {} finished.'",
        render_jobs_phrase(jobs.len())
    ));
    Ok(finish(&lines))
}

fn finish(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The text of a script, with `\n` line ends.
///
/// # Errors
///
/// [`ScriptError::Nul`] when a job file, the engines, the address or the certificate
/// folder holds a NUL character.
pub fn script_text(
    flavor: ScriptFlavor,
    jobs: &[ScriptJob],
    settings: &ScriptSettings,
) -> Result<String, ScriptError> {
    match flavor {
        ScriptFlavor::Sh => sh_text(jobs, settings),
        ScriptFlavor::PowerShell => ps_text(jobs, settings),
    }
}

/// The bytes to write for a script.
///
/// PowerShell: a UTF-8 byte order mark and `\r\n` line ends (Windows PowerShell 5.1
/// reads a file without the mark as the ANSI code page). Shell: UTF-8, `\n` line ends,
/// no mark.
///
/// # Errors
///
/// The same as [`script_text`].
pub fn script_bytes(
    flavor: ScriptFlavor,
    jobs: &[ScriptJob],
    settings: &ScriptSettings,
) -> Result<Vec<u8>, ScriptError> {
    let text = script_text(flavor, jobs, settings)?;
    Ok(match flavor {
        ScriptFlavor::Sh => text.into_bytes(),
        ScriptFlavor::PowerShell => {
            let mut bytes = vec![0xEF, 0xBB, 0xBF];
            bytes.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
            bytes
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER_NOTE: &str = "Exported 2026-10-06 14:03 by Indicatrix Cut 0.6.2.";

    fn jobs() -> Vec<ScriptJob> {
        vec![
            ScriptJob {
                command: ScriptCommand::Render,
                job_file: "jobs/01-barion-oval-current-view-1920x1080.job.json".to_string(),
                label: "Barion Oval · Current view · 1920×1080".to_string(),
            },
            ScriptJob {
                command: ScriptCommand::TiltVideo,
                job_file: "jobs/02-barion-oval-tilt-video-axis-45.job.json".to_string(),
                label: "Barion Oval · tilt video · axis 45°".to_string(),
            },
        ]
    }

    fn settings(cert_dir: &str) -> ScriptSettings {
        ScriptSettings {
            engines: LocalEngines::CpuGpu,
            remote: Some(RemoteArgs {
                address: "render-box.local:7878".to_string(),
                cert_dir: cert_dir.to_string(),
            }),
            header_note: HEADER_NOTE.to_string(),
        }
    }

    #[test]
    fn sh_quoting_is_exact() {
        let cases = [
            ("plain", "'plain'"),
            ("with space", "'with space'"),
            ("it's", r"'it'\''s'"),
            ("$HOME", "'$HOME'"),
            ("`cmd`", "'`cmd`'"),
            ("\"q\"", "'\"q\"'"),
            (r"C:\a b\c", r"'C:\a b\c'"),
            ("\u{2018}smart\u{2019}", "'\u{2018}smart\u{2019}'"),
            ("ü·×", "'ü·×'"),
            ("", "''"),
        ];
        for (input, want) in cases {
            assert_eq!(sh_quote(input).as_deref(), Ok(want), "{input}");
        }
    }

    #[test]
    fn ps_quoting_is_exact() {
        let cases = [
            ("plain", "'plain'"),
            ("with space", "'with space'"),
            ("it's", "'it''s'"),
            ("$HOME", "'$HOME'"),
            ("`cmd`", "'`cmd`'"),
            ("\"q\"", "'\"q\"'"),
            (r"C:\a b\c", r"'C:\a b\c'"),
            (
                "\u{2018}smart\u{2019}",
                "'\u{2018}\u{2018}smart\u{2019}\u{2019}'",
            ),
            (
                "\u{201A}low\u{201B}",
                "'\u{201A}\u{201A}low\u{201B}\u{201B}'",
            ),
            ("ü·×", "'ü·×'"),
            ("", "''"),
        ];
        for (input, want) in cases {
            assert_eq!(ps_quote(input).as_deref(), Ok(want), "{input}");
        }
    }

    #[test]
    fn a_nul_character_is_refused_everywhere() {
        assert_eq!(sh_quote("a\0b"), Err(ScriptError::Nul));
        assert_eq!(ps_quote("a\0b"), Err(ScriptError::Nul));
        let mut bad = jobs();
        bad[0].job_file = "jobs/a\0.job.json".to_string();
        for flavor in [ScriptFlavor::Sh, ScriptFlavor::PowerShell] {
            assert_eq!(
                script_text(flavor, &bad, &settings("c")),
                Err(ScriptError::Nul)
            );
        }
        let mut remote = settings("c\0d");
        assert_eq!(
            script_bytes(ScriptFlavor::Sh, &jobs(), &remote),
            Err(ScriptError::Nul)
        );
        remote.remote = None;
        assert!(script_bytes(ScriptFlavor::Sh, &jobs(), &remote).is_ok());
        assert_eq!(
            ScriptError::Nul.to_string(),
            "A path or name contains a NUL character and cannot go into a script."
        );
    }

    #[test]
    fn comment_text_flattens_control_characters() {
        assert_eq!(comment_text("a\r\nb\tc\u{0}d"), "a  b c d");
        assert_eq!(comment_text("a\u{2028}b\u{85}c"), "a b c");
        assert_eq!(comment_text("Oval · 45°"), "Oval · 45°");
    }

    const SH_EXPECTED: &str = r#"#!/bin/sh
# Indicatrix render jobs: 2 jobs.
# Exported 2026-10-06 14:03 by Indicatrix Cut 0.6.2.
# Run it with:  sh run-render-jobs.sh
# Each job runs with indicatrix-cli. Set INDICATRIX_CLI to its full path if it is not
# on your PATH. A tilt video that stopped part way continues where it stopped.

cd "$(dirname "$0")" || exit 1
CLI="${INDICATRIX_CLI:-indicatrix-cli}"

# Which engines of this computer render: cpu, gpu or cpu+gpu.
ENGINES='cpu+gpu'
# A remote worker (host:port) and its certificate folder. Leave REMOTE empty to render
# on this computer only.
REMOTE='render-box.local:7878'
CERT_DIR='/home/me/indicatrix certs'

failed=0
run_job() {
    if [ -n "$REMOTE" ]; then
        "$CLI" "$@" --local "$ENGINES" --remote "$REMOTE" --cert-dir "$CERT_DIR"
    else
        "$CLI" "$@" --local "$ENGINES"
    fi || failed=$((failed + 1))
}

# 1. Barion Oval · Current view · 1920×1080
run_job render 'jobs/01-barion-oval-current-view-1920x1080.job.json'
# 2. Barion Oval · tilt video · axis 45°
run_job tilt-video 'jobs/02-barion-oval-tilt-video-axis-45.job.json'

if [ "$failed" -gt 0 ]; then
    echo "$failed of 2 render jobs failed." >&2
    exit 1
fi
echo "All 2 render jobs finished."
"#;

    const PS_EXPECTED: &str = r#"# Indicatrix render jobs: 2 jobs.
# Exported 2026-10-06 14:03 by Indicatrix Cut 0.6.2.
# Run it with:  powershell -ExecutionPolicy Bypass -File run-render-jobs.ps1
# Each job runs with indicatrix-cli. Set INDICATRIX_CLI to its full path if it is not
# on your PATH. A tilt video that stopped part way continues where it stopped.

Set-Location -LiteralPath $PSScriptRoot
$Cli = if ($env:INDICATRIX_CLI) { $env:INDICATRIX_CLI } else { 'indicatrix-cli' }

# Which engines of this computer render: cpu, gpu or cpu+gpu.
$Engines = 'cpu+gpu'
# A remote worker (host:port) and its certificate folder. Leave $Remote empty to render
# on this computer only.
$Remote = 'render-box.local:7878'
$CertDir = 'C:\Users\me\indicatrix certs'

$Failed = 0
function Invoke-RenderJob([string[]] $JobArgs) {
    $Extra = @('--local', $Engines)
    if ($Remote -ne '') { $Extra += @('--remote', $Remote, '--cert-dir', $CertDir) }
    & $Cli @JobArgs @Extra
    if ($LASTEXITCODE -ne 0) { $script:Failed++ }
}

# 1. Barion Oval · Current view · 1920×1080
Invoke-RenderJob @('render', 'jobs/01-barion-oval-current-view-1920x1080.job.json')
# 2. Barion Oval · tilt video · axis 45°
Invoke-RenderJob @('tilt-video', 'jobs/02-barion-oval-tilt-video-axis-45.job.json')

if ($Failed -gt 0) {
    Write-Host "$Failed of 2 render jobs failed."
    exit 1
}
Write-Host 'All 2 render jobs finished.'
"#;

    #[test]
    fn the_sh_script_matches_the_spec_example() {
        let text = script_text(
            ScriptFlavor::Sh,
            &jobs(),
            &settings("/home/me/indicatrix certs"),
        )
        .unwrap();
        assert_eq!(text, SH_EXPECTED);
        for job in jobs() {
            let line = format!("run_job {} '{}'", job.command.word(), job.job_file);
            assert_eq!(text.matches(&line).count(), 1, "{line}");
        }
    }

    #[test]
    fn the_powershell_script_matches_the_spec_example() {
        let text = script_text(
            ScriptFlavor::PowerShell,
            &jobs(),
            &settings(r"C:\Users\me\indicatrix certs"),
        )
        .unwrap();
        assert_eq!(text, PS_EXPECTED);
        assert!(!text.contains("$Local"));
    }

    #[test]
    fn an_empty_remote_writes_empty_strings() {
        let mut local_only = settings("ignored");
        local_only.remote = None;
        let sh = script_text(ScriptFlavor::Sh, &jobs(), &local_only).unwrap();
        assert!(sh.contains("\nREMOTE=''\nCERT_DIR=''\n"));
        let ps = script_text(ScriptFlavor::PowerShell, &jobs(), &local_only).unwrap();
        assert!(ps.contains("\n$Remote = ''\n$CertDir = ''\n"));
    }

    #[test]
    fn one_job_reads_naturally_and_an_empty_note_is_skipped() {
        let mut one = settings("c");
        one.header_note = String::new();
        let sh = script_text(ScriptFlavor::Sh, &jobs()[..1], &one).unwrap();
        assert!(sh.starts_with("#!/bin/sh\n# Indicatrix render jobs: 1 job.\n# Run it with:"));
        assert!(sh.contains("echo \"$failed of 1 render job failed.\" >&2"));
        assert!(sh.contains("echo \"All 1 render job finished.\"\n"));
    }

    #[test]
    fn hostile_values_stay_literal() {
        let mut hostile = jobs();
        hostile[0].job_file = "jobs/it's $(rm -rf x).job.json".to_string();
        hostile[0].label = "line one\nrm -rf /".to_string();
        let sh = script_text(ScriptFlavor::Sh, &hostile, &settings("c")).unwrap();
        assert!(sh.contains("# 1. line one rm -rf /\n"));
        assert!(sh.contains(r"run_job render 'jobs/it'\''s $(rm -rf x).job.json'"));
        let ps = script_text(ScriptFlavor::PowerShell, &hostile, &settings("c")).unwrap();
        assert!(ps.contains("@('render', 'jobs/it''s $(rm -rf x).job.json')"));
    }

    #[test]
    fn powershell_bytes_have_a_bom_and_only_crlf() {
        let bytes = script_bytes(
            ScriptFlavor::PowerShell,
            &jobs(),
            &settings(r"C:\Users\me\indicatrix certs"),
        )
        .unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        let body = &bytes[3..];
        assert!(!body.starts_with(&[0xEF, 0xBB, 0xBF]));
        for (index, byte) in body.iter().enumerate() {
            if *byte == b'\n' {
                assert!(index > 0 && body[index - 1] == b'\r', "bare LF at {index}");
            }
            if *byte == b'\r' {
                assert_eq!(body.get(index + 1), Some(&b'\n'), "bare CR at {index}");
            }
        }
        assert!(String::from_utf8(body.to_vec()).is_ok());
    }

    #[test]
    fn shell_bytes_are_plain_lf_utf8() {
        let bytes = script_bytes(ScriptFlavor::Sh, &jobs(), &settings("c")).unwrap();
        assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
        assert!(!bytes.contains(&b'\r'));
        assert!(bytes.starts_with(b"#!/bin/sh\n"));
        assert!(bytes.ends_with(b"\n"));
    }

    #[test]
    fn names_are_fixed() {
        assert_eq!(ScriptFlavor::Sh.file_name(), "run-render-jobs.sh");
        assert_eq!(ScriptFlavor::PowerShell.file_name(), "run-render-jobs.ps1");
        assert_eq!(ScriptCommand::Render.word(), "render");
        assert_eq!(ScriptCommand::TiltVideo.word(), "tilt-video");
        assert_eq!(
            (JOBS_DIR, ASSETS_DIR, RENDERS_DIR),
            ("jobs", "assets", "renders")
        );
    }
}
