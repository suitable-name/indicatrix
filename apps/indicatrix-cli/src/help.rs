//! The help pages: [`text`] returns the page for a [`Topic`].
//!
//! The pages are plain strings so a test can check that every flag the parser takes is on its
//! page, and the README can quote them.

use crate::args::Topic;

const ROOT: &str = "\
indicatrix-cli: solve, check, optimize, retarget, sweep and export faceting designs without a window.

Usage:
  indicatrix-cli <command> <design> [options]
  indicatrix-cli <command> --help

A <design> is a .indicatrix, .asc, .gem or .gcs file.

Commands:
  info      Name, gear, symmetry, tier list and material of a design.
  solve     Solve the design, report closure and warnings, optionally save it.
  metrics   Windowing, brilliance, extinction, fire and the proportions of the stone.
  validate  Manufacturability warnings and the overall Good / Check / Problem verdict.
  optimize  Search for better facet angles and optionally save the best result.
  retarget  Adapt the angles to another material with the dialog's validity gate.
  sweep     Score one tier over a range of angles.
  export    Write .asc, .indicatrix, .gcs or the cutting sheet (.html).
  render      Render a still picture from a render job file (*.job.json).
  tilt-video  Render a tilt video from a render job file; resumes from its frames.

render and tilt-video take a job file instead of a design: indicatrix-cli render --help.

Options for every command:
  -h, --help     Show the page of a command (indicatrix-cli retarget --help).
  -V, --version  Show the version.

Material and lighting:
  --material NAME   A built-in material, or a custom one of the --db library.
  --ri N            A bare refractive index instead (a flat, non-dispersive material).
  --db FILE         A design library (facet_diagrams.sqlite), opened read-only, for its
                    custom materials.
  --lighting NAME   daylight, incandescent, ring, spotlight, iso (or grading: the grading
                    tray, the default), tent, dome (sky, no sun), sun (sky plus direct sun),
                    tray (white tray), shop, window, illuminant-a or aset.

Exit codes:
  0  Done.
  1  The command line is wrong.
  2  The design cannot be used, or a result was refused (nothing is written then).
  3  A file could not be read or written.
  4  validate only: the verdict is Problem.
  5  render and tilt-video only: the render failed or was stopped.

Output is deterministic: fixed decimals, stable order, JSON with sorted keys. (render and
tilt-video print their progress and an estimate of the time left to standard error.)

Angles: the text reports show every facet angle as a positive number, and the block
(crown, pavilion, girdle) says which side of the girdle it is on. JSON keeps the stored
signed convention instead, the one the design files use: a pavilion angle is negative.
";

const INFO: &str = "\
Usage: indicatrix-cli info <design> [--json] [--out FILE] [--db FILE]

Prints the design's name, gear, symmetry, preform (shape, size and offset), material and the
tier list (standard code, the tier's own name, block, angle as a positive number, index
positions, what each tier meets or follows). Does not solve.

  --json       Print JSON instead of text.
  --out FILE   Write the report to FILE instead of printing it.
  --db FILE    A design library, for custom materials.
";

const SOLVE: &str = "\
Usage: indicatrix-cli solve <design> [--out FILE.indicatrix] [--json] [--db FILE]

Solves every tier, checks that the facets enclose a stone and lists the manufacturability
warnings. Exit code 2 when the design does not solve or does not close.

  --out FILE   Save the design as a .indicatrix file (also converts .asc, .gem and .gcs).
               Nothing is written when the design does not solve and close.
  --json       Print JSON instead of text.
  --db FILE    A design library, for custom materials.
";

const METRICS: &str = "\
Usage: indicatrix-cli metrics <design> [--material NAME | --ri N] [--tilt]
                              [--json | --csv] [--lighting NAME] [--out FILE] [--db FILE]

Table-up windowing, brilliance, extinction, fire and scintillation, the proportions of the
stone and its yield. With --tilt the tilt performance is averaged over four axes and 181
angles too (about a second and a half).

Without --material or --ri the design's own material is used; a design with none is scored
with its refractive index as a flat material, and the report says so.

  --material NAME   Score in this material.
  --ri N            Score in a material of this refractive index.
  --tilt            Also average the tilt performance.
  --json | --csv    Print JSON, or a header line and one row, instead of text.
  --lighting NAME   The light the score is taken under (default grading, the grading tray).
  --out FILE        Write the report to FILE instead of printing it.
  --db FILE         A design library, for custom materials.
";

const VALIDATE: &str = "\
Usage: indicatrix-cli validate <design> [--json] [--material NAME | --ri N]
                               [--lighting NAME] [--out FILE] [--db FILE]

Lists the manufacturability warnings and the overall verdict of the desktop editor's status
strip: Good, Check or Problem, with the reasons. Exit code 4 when the verdict is Problem,
2 when the design cannot be read.

  --json            Print JSON instead of text.
  --material NAME   Judge the optics in this material instead of the design's own.
  --ri N            Judge the optics in a material of this refractive index.
  --lighting NAME   The light the optics are measured under (default grading, the grading tray).
  --out FILE        Write the report to FILE instead of printing it.
  --db FILE         A design library, for custom materials.
";

const OPTIMIZE: &str = "\
Usage: indicatrix-cli optimize <design> [--preset NAME] [--budget N] [--starts N] [--seed N]
                               [--vary-anchored] [--candidates N] [--lighting NAME]
                               [--json] [--out FILE.indicatrix] [--db FILE]

Runs the Optimize tab's search on the design's own material and lists the ranked candidates.
The best one is saved with --out. When the search finds nothing better, nothing is written.

  --preset NAME      balanced (default), brilliance, low-windowing, low-extinction,
                     keep-weight, lighten-dark or intensify-pale. The last two also steer the
                     face-up tone (lighter, or a stronger colour) of a coloured material,
                     sized by the design's girdle diameter.
  --budget N         Evaluations of the coordinate stage (default 800), shared by all starts.
  --starts N         Starting arrangements to try, 1 to 32 (default 8). Each start gets at least
                     four sweeps of the free tiers; a budget too small for that runs fewer
                     starts, and --starts 1 is the plain single descent.
  --seed N           The search seed (default 0). The same inputs give the same result.
  --vary-anchored    Turn pinned (scale-reference) tiers about their girdle edges too. On by
                     default when every tier is pinned, as in the tab.
  --candidates N     How many ranked candidates to keep, 1 to 5 (default 3).
  --lighting NAME    The light the search scores and tones under (default grading, the canonical
                     preset: a run without --lighting is scored under the grading tray).
  --json             Print JSON instead of text.
  --out FILE         Save the best candidate as a .indicatrix file.
  --db FILE          A design library, for custom materials.
";

const RETARGET: &str = "\
Usage: indicatrix-cli retarget <design> (--material NAME | --ri N) [--mode shift|optimize]
                               [--crown-follow | --crown-fraction F | --crown-ratio]
                               [--preset NAME]
                               [--range DEG] [--budget N] [--seed N] [--no-keep-look]
                               [--lighting NAME]
                               [--json] [--out FILE.indicatrix] [--db FILE]

Adapts the facet angles to another material with the Retarget dialog's engine and validity
gate: every facet turns about its girdle-side edge, and the result is checked for a closed
stone, a girdle that survives, a table that stays flat and facets that do not vanish.
A result the gate refuses is never written: the reasons are printed and the exit code is 2.
The design's material is set to the target in the saved file.

  --material NAME      The material to retarget for.
  --ri N               A bare refractive index instead.
  --mode shift         The critical-angle shift only (default).
  --mode optimize      The shift, then a search around it for better angles.
  --crown-follow       Scale every crown angle's tangent by the pavilion's vertical stretch, so
                       the stone keeps its silhouette and table size (default: the crown
                       follows the pavilion's stretch).
  --crown-fraction F   Move the crown by this share (0 to 1) of the pavilion's shift
                       instead (0 leaves the crown where it is).
  --crown-ratio        Scale the crown angle by the ratio of the two critical angles instead.
  --preset NAME        What the search favours (optimize mode; default balanced).
  --range DEG          Degrees either side of each angle the search may move (default 6).
  --budget N           Evaluations of the search (default 300).
  --seed N             The search seed (default 0).
  --no-keep-look       Do not penalise options that drift from the design's table size and
                       crown-to-pavilion ratio (optimize mode; by default every option is
                       scored with that penalty).
  --lighting NAME      The light everything is scored under (default grading, the grading tray).
  --json               Print JSON instead of text.
  --out FILE           Save the retargeted design as a .indicatrix file.
  --db FILE            A design library, for custom materials.
";

const SWEEP: &str = "\
Usage: indicatrix-cli sweep <design> --tier NAME --from A --to B --step S [--tilt]
                            [--csv FILE] [--json] [--material NAME | --ri N]
                            [--lighting NAME] [--db FILE]

Sets one tier to every angle from A to B in steps of S, solves and scores each one, and
prints the table, from the flattest angle to the steepest. The design's own angle is always
a row. Angles are positive numbers, and the side of the girdle comes from the tier:
--from 39 --to 43 --step 0.5 sweeps a pavilion tier. A sign in front is ignored, so
--from -43 --to -39 does the same. The text table and the CSV show positive angles; the
JSON keeps the stored signed ones (a pavilion angle is negative).

  --tier NAME       The tier: its name (P1, G1/G2), or #N for the Nth row of the tier list.
  --from A --to B   The range in degrees from flat, either end first.
  --step S          The distance between angles; at most 200 angles in all.
  --tilt            Also average the tilt performance of every row (about 1.4 s a row).
  --csv FILE        Write the rows as CSV to FILE.
  --json            Print JSON instead of text.
  --material NAME   Score in this material instead of the design's own.
  --ri N            Score in a material of this refractive index.
  --lighting NAME   The light the score is taken under (default grading, the grading tray).
  --db FILE         A design library, for custom materials.
";

const EXPORT: &str = "\
Usage: indicatrix-cli export <design> --format asc|indicatrix|gcs|html --out FILE [--db FILE]
                             [--date TEXT]

Writes the solved design in another form. The format may be left out when FILE ends in
.asc, .indicatrix, .gcs or .html. The design must solve and close; nothing is written
otherwise.

  --format asc         GemCAD cutting instructions, the file the editor's Export writes
                       (CRLF line ends).
  --format indicatrix  A self-contained .indicatrix design file.
  --format gcs         A Gem Cut Studio file (experimental).
  --format html        The cutting instructions as a web page, headed by the design's
                       title, designer and notes.
  --out FILE           Where to write it.
  --db FILE            A design library, for custom materials.
  --date TEXT          The date the html page prints under its title, for example
                       'October 2026'. Without it no date is printed, so the page is the
                       same every time.
";

const RENDER: &str = "\
Usage: indicatrix-cli render JOB.job.json [--out FILE.png] [--local cpu|gpu|cpu+gpu]
                             [--remote HOST:PORT --cert-dir FOLDER]
                             [--compute local|remote|both] [--transfer full|final]
                             [--contribute-local] [--quiet]

Renders the still picture a render job file describes. The file is written by the desktop
app's render queue (File > Render Jobs...), which also exports scripts that call this command.
It renders with the app's own engine, so the picture is the one the app would have made.

  --out FILE.png          Write the picture here instead of where the job says.
  --local ENGINES         The engines of this computer: cpu, gpu or cpu+gpu (default cpu+gpu).
                          gpu needs a build with the gpu feature; without it everything renders
                          on the processor and a note says so.
  --remote HOST:PORT      Also use a remote worker. Needs --cert-dir.
  --cert-dir FOLDER       The folder with ca.pem, client.pem and client.key. Needs --remote.
  --compute WHERE         Replace the job's choice: local, remote or both. remote and both
                          need --remote.
  --transfer WHAT         Replace the job's choice: full (raw sample data) or final (the
                          finished picture only).
  --contribute-local      With --transfer final: this computer renders a share too.
  --quiet                 No progress lines (notes and errors are still printed).

Output: standard output is the path of the picture written, one line. Standard error carries
the progress and the estimated time left (one rewritten line on a terminal, a line per 10
percent otherwise), notes, and the line 'wrote PATH'. A name that is taken is not
overwritten: the job's name gets ' (2)' and so on.

Exit codes: 0 done; 1 the command line is wrong, or the job file holds a tilt video;
2 the job file cannot be used (not a job file, newer format, invalid values, an HDR map that
is missing or changed); 3 a file could not be read or written; 5 the render failed or was
stopped.
";

const TILT_VIDEO: &str = "\
Usage: indicatrix-cli tilt-video JOB.job.json [--out-dir FOLDER] [--restart]
                                  [--local cpu|gpu|cpu+gpu] [--remote HOST:PORT --cert-dir FOLDER]
                                  [--compute local|remote|both] [--transfer full|final]
                                  [--contribute-local] [--quiet]

Renders the tilt performance video a render job file describes: every frame as a picture,
then the video. The file is written by the desktop app's render queue (File > Render Jobs...),
which also exports scripts that call this command. It renders with the app's own engine.

A video resumes by default. Frames the same job already wrote to its folder are kept, and
only the missing ones are rendered, so a run that stopped part way (Ctrl+C, a power cut)
continues where it stopped. Frames are written atomically. --restart deletes this job's
frames first and starts at frame one. A folder that holds another job's frames is refused.

  --out-dir FOLDER        The frame folder, instead of the one the job names.
  --restart               Start again from the first frame.
  --local ENGINES         The engines of this computer: cpu, gpu or cpu+gpu (default cpu+gpu).
                          gpu needs a build with the gpu feature; without it everything renders
                          on the processor and a note says so.
  --remote HOST:PORT      Also use a remote worker. Needs --cert-dir.
  --cert-dir FOLDER       The folder with ca.pem, client.pem and client.key. Needs --remote.
  --compute WHERE         Replace the job's choice: local, remote or both. remote and both
                          need --remote.
  --transfer WHAT         Replace the job's choice: full (raw sample data) or final (the
                          finished picture only).
  --contribute-local      With --transfer final: this computer renders a share too.
  --quiet                 No progress lines (notes and errors are still printed).

Output: standard output is the path written, one line: the MP4 or GIF, or the frame folder
when no video could be made (a note says why). Standard error carries the progress and the
estimated time left (one rewritten line on a terminal, a line per finished frame otherwise),
notes, and the line 'wrote PATH'. Unless the job keeps its frames, they are deleted once the
video exists.

Exit codes: 0 done; 1 the command line is wrong, or the job file holds a still picture;
2 the job file cannot be used (not a job file, newer format, invalid values, an HDR map that
is missing or changed, a folder of another job); 3 a file could not be read or written;
5 the render failed or was stopped.
";

/// The help page for `topic`.
#[must_use]
pub fn text(topic: Topic) -> String {
    let page = match topic {
        Topic::Root => ROOT,
        Topic::Info => INFO,
        Topic::Solve => SOLVE,
        Topic::Metrics => METRICS,
        Topic::Validate => VALIDATE,
        Topic::Optimize => OPTIMIZE,
        Topic::Retarget => RETARGET,
        Topic::Sweep => SWEEP,
        Topic::Export => EXPORT,
        Topic::Render => RENDER,
        Topic::TiltVideo => TILT_VIDEO,
    };
    // The UV lamps are offered only in a `physical-color` build, as in the desktop app.
    #[cfg(feature = "physical-color")]
    return page.replace(
        "illuminant-a or aset.",
        "illuminant-a, aset,\n                    uv365 or uv395.",
    );
    #[cfg(not(feature = "physical-color"))]
    page.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_TOPIC: [Topic; 11] = [
        Topic::Render,
        Topic::TiltVideo,
        Topic::Root,
        Topic::Info,
        Topic::Solve,
        Topic::Metrics,
        Topic::Validate,
        Topic::Optimize,
        Topic::Retarget,
        Topic::Sweep,
        Topic::Export,
    ];

    #[test]
    fn every_page_is_non_empty_and_ends_in_a_line_feed() {
        for topic in EVERY_TOPIC {
            let page = text(topic);
            assert!(page.len() > 100, "{topic:?}");
            assert!(page.ends_with('\n'), "{topic:?}");
        }
    }

    #[test]
    fn every_command_page_starts_with_its_usage_line() {
        for (topic, word) in [
            (Topic::Info, "info"),
            (Topic::Solve, "solve"),
            (Topic::Metrics, "metrics"),
            (Topic::Validate, "validate"),
            (Topic::Optimize, "optimize"),
            (Topic::Retarget, "retarget"),
            (Topic::Sweep, "sweep"),
            (Topic::Export, "export"),
            (Topic::Render, "render"),
            (Topic::TiltVideo, "tilt-video"),
        ] {
            let page = text(topic);
            assert!(
                page.starts_with(&format!("Usage: indicatrix-cli {word} ")),
                "{word}"
            );
            assert_eq!(Topic::of(word), topic);
        }
    }

    #[test]
    fn the_root_page_lists_every_command_and_exit_code() {
        let page = text(Topic::Root);
        for word in [
            "info",
            "solve",
            "metrics",
            "validate",
            "optimize",
            "retarget",
            "sweep",
            "export",
            "render",
            "tilt-video",
        ] {
            assert!(page.contains(&format!("  {word} ")), "{word}");
        }
        for code in ["  0  ", "  1  ", "  2  ", "  3  ", "  4  ", "  5  "] {
            assert!(page.contains(code), "{code:?}");
        }
    }

    #[test]
    fn the_pages_say_angles_are_positive_and_json_keeps_the_signed_ones() {
        let root = text(Topic::Root);
        assert!(root.contains("positive number"), "{root}");
        assert!(root.contains("JSON keeps the stored"), "{root}");
        assert!(root.contains("a pavilion angle is negative"), "{root}");

        let sweep = text(Topic::Sweep);
        assert!(sweep.contains("Angles are positive numbers"), "{sweep}");
        assert!(sweep.contains("--from 39 --to 43"), "{sweep}");
        assert!(sweep.contains("A sign in front is ignored"), "{sweep}");
        assert!(
            !sweep.contains("Angles are signed like the tier"),
            "the old rule is gone: {sweep}"
        );
        assert!(
            !sweep.contains("pavilion angles are negative"),
            "the old rule is gone: {sweep}"
        );

        let info = text(Topic::Info);
        assert!(info.contains("standard code"), "{info}");
        assert!(info.contains("positive number"), "{info}");
    }

    #[test]
    fn each_page_names_the_flags_its_command_takes() {
        let engine_flags: &[&str] = &[
            "--local",
            "--remote",
            "--cert-dir",
            "--compute",
            "--transfer",
            "--contribute-local",
            "--quiet",
        ];
        for (topic, own) in [
            (Topic::Render, "--out"),
            (Topic::TiltVideo, "--out-dir"),
            (Topic::TiltVideo, "--restart"),
        ] {
            let page = text(topic);
            assert!(page.contains(own), "{topic:?} page lacks {own}");
            for name in engine_flags {
                assert!(page.contains(name), "{topic:?} page lacks {name}");
            }
            assert!(page.contains("gpu feature"), "{topic:?}");
        }
        let flags: [(Topic, &[&str]); 8] = [
            (Topic::Info, &["--json", "--out", "--db"]),
            (Topic::Solve, &["--out", "--json", "--db"]),
            (
                Topic::Metrics,
                &[
                    "--material",
                    "--ri",
                    "--tilt",
                    "--json",
                    "--csv",
                    "--lighting",
                    "--out",
                ],
            ),
            (
                Topic::Validate,
                &["--json", "--material", "--ri", "--lighting", "--out"],
            ),
            (
                Topic::Optimize,
                &[
                    "--preset",
                    "--budget",
                    "--starts",
                    "--seed",
                    "--vary-anchored",
                    "--candidates",
                    "--out",
                ],
            ),
            (
                Topic::Retarget,
                &[
                    "--material",
                    "--ri",
                    "--mode",
                    "--crown-follow",
                    "--crown-fraction",
                    "--crown-ratio",
                    "--range",
                    "--no-keep-look",
                    "--out",
                ],
            ),
            (
                Topic::Sweep,
                &["--tier", "--from", "--to", "--step", "--tilt", "--csv"],
            ),
            (Topic::Export, &["--format", "--out"]),
        ];
        for (topic, names) in flags {
            let page = text(topic);
            for name in names {
                assert!(page.contains(name), "{topic:?} page lacks {name}");
            }
        }
    }
}
