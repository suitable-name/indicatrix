# indicatrix-cli

The Indicatrix design engines from a script: solve, check, score, optimize, retarget, sweep
and export a faceting design without a window and without a GPU. It runs the same engines as
the desktop editor (`indicatrix-cut-core` and `indicatrix-editor`), so a number printed here
is the number the editor shows.

```
cargo run -p indicatrix-cli -- <command> <design>... [options]
cargo run -p indicatrix-cli -- <command> --help
```

A `<design>` is a `.isdd`, `.asc`, `.gem` or `.gcs` file (an older `.indicatrix` file opens too, exactly like a `.isdd`). The kind is read from the
extension. Every command that takes a design takes several: see [Several designs](#several-designs).

## Commands

| Command | What it does |
|---|---|
| `info` | Name, gear, symmetry, preform (shape, size and offset), material and the tier list. Does not solve. |
| `solve` | Solves every tier, checks that the facets enclose a stone, lists the manufacturability warnings. `--out` saves the design as `.isdd` (this converts `.asc`, `.gem` and `.gcs`). |
| `metrics` | Table-up windowing, brilliance, extinction, fire and scintillation, the proportions and the yield. `--tilt` also averages the tilt performance. |
| `validate` | The warnings and the overall Good, Check or Problem verdict of the editor's status strip. |
| `optimize` | The Optimize tab's search on the design's own material. Lists ranked candidates; `--out` saves the best. |
| `retarget` | The Retarget dialog's engine and validity gate, for another material. |
| `sweep` | Sets one tier to every angle of a range and scores each one. |
| `export` | Writes `.asc` (GemCAD cutting instructions), `.isdd`, `.gcs` or the cutting sheet as `.html`. `--format isdd` also takes the older name `indicatrix`. |
| `census` | Read-only measurement of the meet-preserving gate over templates, files and a library. See [Census](#census). |
| `render` | Renders a still picture from a render job file (`*.job.json`) made by the desktop app. See [Render jobs](#render-jobs). |
| `tilt-video` | Renders a tilt performance video from a render job file, and resumes from the frames already written. |

### Examples

```
indicatrix-cli info round.asc
indicatrix-cli solve round.asc --out round.isdd
indicatrix-cli metrics round.isdd --ri 1.76 --json
indicatrix-cli metrics round.isdd --material Sapphire --tilt --csv
indicatrix-cli validate round.asc --json
indicatrix-cli optimize round.isdd --preset brilliance --budget 400 --starts 4 --seed 7 --out better.isdd
indicatrix-cli retarget round.isdd --material Sapphire --crown-fraction 0.33 --out sapphire.isdd
indicatrix-cli retarget round.isdd --ri 1.7681 --mode optimize --range 4 --out sapphire.isdd
indicatrix-cli sweep round.isdd --tier "Pavilion Main" --from 39 --to 43 --step 0.5 --csv sweep.csv
indicatrix-cli export round.isdd --format html --out sheet.html
indicatrix-cli export round.isdd --out round.asc
indicatrix-cli metrics *.isdd --csv
indicatrix-cli solve old/*.asc --out-dir converted
indicatrix-cli export *.isdd --format html --out-dir sheets
```

Angles are positive numbers, as the tier table shows them: the side of the girdle comes from
the tier, so a sweep over the pavilion is `--from 39 --to 43` (a sign in front is ignored, so
`--from -43 --to -39` does the same). A tier is named by its name (`P1`, `G1/G2`) or by `#N`,
the Nth row of the tier list.

**Angles in the output.** The text reports (`info`, `sweep`, `optimize`, `retarget`) and the
`sweep` CSV show every facet angle as a positive number, with the block (crown, pavilion,
girdle) naming the side, and `info` lists each tier's standard code (`P1`, `C1`, `Table`)
beside its own name. **`--json` keeps the stored signed convention**, the one the design files
use (GemCAD's format needs it): in JSON a pavilion angle is negative, and every `angle_deg`,
`from_deg`, `to_deg` and `current_deg` is signed. A difference (a change or a margin) is a
difference and keeps its sign in both.

`export --date TEXT` sets the date that the `.html` cutting sheet prints under its title (for
example `--date "October 2026"`). Without it no date is printed, so the page is the same every
time.

## Several designs

Every command that takes a design (`info`, `solve`, `metrics`, `validate`, `optimize`,
`retarget`, `sweep` and `export`) takes several, and a shell glob expands as usual. Each design
runs as the command would run on it alone, in the order given: its report, its notes and its
output file are the ones the same command gives that file by itself. With one design nothing
changes, byte for byte. (`render` and `tilt-video` take one job file, not designs.)

- **Standard output.** Text reports follow one another, each under a `==> FILE <==` line with
  a blank line between them, as `head` does. JSON reports follow one another as complete
  documents with nothing between them (`jq` reads such a stream as it is). `metrics --csv`
  prints its header line once and one row for each design (the first column names it). A design
  whose report went to its `--out-dir` file adds nothing.
- **Standard error.** What a design printed there (notes, `error: ...`) comes under the same
  `==> FILE <==` line. When any design failed, the last line reads
  `error: N of M design files failed`.
- **Output files.** `--out FILE` (and `--csv FILE` of `sweep`) names one file, so a line with
  several designs and one of them is refused (exit code 1). `--out-dir DIR` writes one file for
  each design instead, named after the design without its extension and given the extension of
  the output: `DIR/<name>.isdd` for `solve`, `optimize` and `retarget`; `.txt`, `.json` or `.csv`
  for a report written by `info`, `metrics` and `validate`; `.csv` for `sweep`; the chosen format
  for `export`, which then needs `--format` (`DIR/<name>.asc`, `.isdd`, `.gcs` or `.html`).
  `DIR` must exist (exit code 3 otherwise, before anything runs), and two designs that would get
  the same file name (`a/x.asc` and `b/x.gem`, or `x.asc` and `X.ASC`: names are compared
  without regard to case) are refused before anything runs, so one file never replaces another.
  `--out-dir` works with one design too. Give `--out` or `--out-dir`, not both.
- **Failures.** A design that fails prints its error under its name, and the others still run.
  The files of the designs that worked are written even so (a design that was refused never
  leaves one behind). The exit code is that of the first design that failed (1, 2, 3 or 5);
  when none failed it is 4 if a `validate` verdict is Problem, else 0.

```
indicatrix-cli validate stones/*.isdd --json --out-dir reports
indicatrix-cli retarget round.isdd cushion.isdd --material Sapphire --out-dir sapphire
```

## Material, lighting and the library

- `--material NAME` is a built-in material, or a custom one found in the library given with
  `--db`, or one the design file carries itself.
- `--ri N` is a bare refractive index (above 1, at most 5): a flat, non-dispersive material.
- With neither flag, a command that scores uses the design's own material. A design with none
  is scored with its refractive index as a flat material and the report says so (the Optimize
  tab does the same). An unknown `--material` is a command-line mistake and ends with exit
  code 1. A material the design itself names that neither the built-ins, `--db` nor the file's
  own snapshot hold is refused with exit code 2. Neither falls back silently.
- When a saved `.isdd` file names a custom material of the `--db` library (for example
  after `retarget --material`), the material's numbers are saved inside the file
  (`[material.custom]`), as the desktop's Save does, so the file opens without `--db`.
- `--db FILE` is a design library (`facet_diagrams.sqlite`). It is opened **read-only**; the
  CLI never writes to it.
- `--lighting NAME` is the light the score is taken under: `daylight`, `incandescent`, `ring`,
  `spotlight`, `iso` (or `grading`: the grading tray, the default), `tent`, `dome` (sky only, no sun),
  `sun` (sky plus a real 0.27° sun, about 82 % of the light), `tray` (the white tray lit from below, even walls), `shop`, `window`,
  `illuminant-a` (2856 K) or `aset` (contrast view: green 0-45°, red 45-75°, blue 75-90°, black below the horizon). A build with the `physical-color` feature
  adds `uv365` and `uv395`. Brilliance under a Studio rig (`daylight`, `incandescent`, `ring`,
  `spotlight`, `illuminant-a`) is measured against that rig's own lamps; the grading tray gives
  the standard figure.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Done. |
| 1 | The command line is wrong (unknown command or flag, missing value, bad number). |
| 2 | The design cannot be used (it does not solve or close), or a result was refused. Nothing is written. |
| 3 | A file could not be read or written. |
| 4 | `validate` only: the verdict is Problem. |
| 5 | `render` and `tilt-video` only: the render failed or was stopped. |

A file named by `--out` (or by `--csv` for `sweep`) is written only when the command ends with
code 0 (or 4 for `validate`). A command that refuses its result never leaves a file behind.
With several designs each design is its own command: see [Several designs](#several-designs).
Each file that is written is noted on standard error (`wrote FILE`). Reports go to standard
output; notes and errors go to standard error.

## Output

Text, `--json` and `--csv` output are deterministic: the same command on the same files prints
the same bytes. Numbers have a fixed number of decimals (never the shortest round-trip form), lists have a
fixed order, JSON objects come out with sorted keys, and nothing reads the clock or mints an id.
Only the thread count of a sweep depends on the machine: it sizes its worker pool from the
processor count, but every row is a pure function of its angle and the rows come back in angle
order (flattest first).

Before a command runs, the tool asks GitHub (at most 2 s) whether a newer Indicatrix release
exists; only then it prints one line to standard error, `info: Indicatrix 0.7.3 is available
(this is 0.7.2): <release page>`. Standard output is never touched, and up to date, offline or
any other failure prints nothing. `--no-update-check` (anywhere on the command line) or the
environment variable `INDICATRIX_NO_UPDATE_CHECK` (any value) skips the check.

`metrics --json` keys: `table_up_brilliance_pct`, `table_up_windowing_pct`,
`table_up_extinction_pct`, `table_up_fire_index`, `table_up_scintillation_pct`, with `tilt_*`
keys added by `--tilt`, then `table_pct_of_width`, `crown_pct_of_width`,
`pavilion_pct_of_width`, `girdle_pct_of_width`, `total_depth_pct_of_width`, `length_to_width`,
`volume_over_width_cubed`, `yield_pct`, `facets`, `tiers` and `warnings`. `--csv` is one header
line and one row with the same names.

## Optimize

- `--budget N` is the evaluation budget of the coordinate stage (default 800), shared by all
  starts.
- `--starts N` is how many starting arrangements the search tries, 1 to 32 (default 8). Your own
  design is always start 1. Each start gets at least four sweeps of the free tiers, so a budget
  too small for that runs fewer starts, and `--starts 1` is the plain single descent. The result
  depends only on the design, the flags and `--seed`, never on the number of processor cores.
- `--preset NAME` is `balanced` (default), `brilliance`, `low-windowing`, `low-extinction`,
  `keep-weight`, `lighten-dark` or `intensify-pale`. The last two also steer the face-up tone
  of a colored material.
- `--seed N` is the search seed (default 0): the same inputs give the same result.
- `--candidates N` is how many ranked candidates to keep, 1 to 5 (default 3).
- `--vary-anchored` turns pinned (scale-reference) tiers about their girdle edges too. It is
  on by default when every tier is pinned, as in the tab.
- Without `--lighting` the search scores under the grading tray (see above).

## Retarget

`retarget` turns every facet about its girdle corners by the shift of the critical angle,
exactly as the dialog does, and checks the result with the dialog's validity gate: a closed
stone, a girdle that survives, a table that stays flat and facets that do not vanish.

- `--mode shift` (default) is the critical-angle shift only. `--mode optimize` runs a search
  around it (`--range`, `--budget`, `--seed`, `--preset`). Every option is refitted so the
  table and culet keep their size, and by default scored with a penalty for drifting from
  the design's table size and crown-to-pavilion ratio; `--no-keep-look` drops the penalty.
  `--range DEG` is how far either side of each angle the search may move (above 0 and at
  most 45, default 6), `--budget N` its evaluations (default 300) and `--preset NAME` what it
  favours (default `balanced`, the names are those of `optimize`).
- The crown follows the pavilion's stretch by default (`--crown-follow`, accepted for symmetry):
  every crown angle's tangent is scaled by the same vertical stretch the pavilion gets, so the
  stone keeps its silhouette and table size, and the verdict line quotes the stone's depth.
  `--crown-fraction F` moves the crown by that share of the pavilion's shift instead (0 leaves
  it where it is). `--crown-ratio` scales the crown angle by the ratio of the two critical
  angles instead. Give at most one of the three.
- `--girdle-allowance` is the dialog's "Allow a Thicker Girdle (up to +10 %)": when the new
  angles would pinch the girdle at its corners, the band is thickened by 5 % and then 10 %
  (never thinner) before the result is refused. The plan view, the table and culet size and the
  crown-to-pavilion ratio are kept exactly; the verdict text says which step was used. It is on
  in the dialog and off here unless given, so that a command line gives the same result it
  always did; a result that is refused without it may be valid with it.
- A refused result prints the report and the reasons, ends with exit code 2 and writes nothing.
- Only a Valid verdict applies, as in the dialog. A change the check could not judge
  (**Unchecked**: the current design itself does not solve or close, so nothing could be
  compared and no retargeted stone exists) is refused with the dialog's own verdict line,
  `Could not check this change. The current design has a problem: ...`. There is no option to
  let one through, because the dialog has none any more either.
- The saved design has the target material set.

## Census

`census` measures what the meet-preserving gate does to many designs. It is read-only: it
changes no design, and the library (`--db`) is opened read-only. It is the evidence behind
the Retarget and Optimize constants and behind the decision whether support bindings are worth
building (plan `plan_meet_preserving_followups_2026-10-19.md`, parts 4.1 to 4.4).

```
indicatrix-cli census --templates --fixtures stone.asc --targets diamond,sapphire,quartz,cz,tourmaline --out census.csv
indicatrix-cli census --db facet_diagrams.sqlite --all --targets sapphire --limit 200 --out first200.csv
indicatrix-cli census --templates --targets sapphire --modes optimize --budget 100
```

- One row for each design, target material and mode. A target within 0.005 of the design's own
  refractive index gets no row. A design that cannot be read or captured is one row whose
  `capture` column says why (`no design file` for a library entry without one); a failing design
  never ends the run, and the exit code is 0 whenever the census ran.
- The columns, in order: identity (`source`, `key`, `title`, counts of tiers, planes, pinned,
  meet-derived and named tiers, relations, targets, concave tiers and placements, `gear`,
  `symmetry`, `ri_from`, `target`, `ri_to`, `mode`, `crown_mode`), the baseline (`capture`,
  `junctions`, `edges`, `live_facets`, `reach_fallbacks_upper_bound`), the verdict under the
  Retarget bounds (`status`, `strategy`, `girdle_step`, `reasons`, `preserved_meets`,
  `added_meets`, `max_residual_norm`), the raw deltas under a policy no stone reaches
  (`census_status`, `d_aspect`, `d_table_pts`, `d_crown`, `d_pavilion`, `d_girdle`,
  `junction_disp`, `travel_width`, `travel_scale`, `excess_depth`, `excess_girdle`,
  `excess_junction`), the branch diagnosis (`branch`, `branch_tiers`, `ideal_spread`), four
  concave columns that stay empty until the concave gate lands, and the brilliance percentage of
  the three Retarget optics columns (`score_before`, `score_unchanged_in_target`,
  `score_after`). `--timings` adds `t_capture_ms`, `t_projection_ms`, `t_solve_ms`,
  `t_validate_ms`, `t_fast_ms`, `t_full_ms`, `iterations` and `rejected`; they are the only
  columns that differ from run to run.
- `reach_fallbacks_upper_bound` counts every tier whose inward reach could not be read, turned
  or not, so it bounds the number of half-length fallbacks a retarget can use.
- `excess_*` is the change left after the turn's travel: the figure the Retarget constants
  bound. A negative excess means the turn alone explains the change.
- `branch` is `none`, `infeasible`, `branch_mismatch` or `unresolved`. It is diagnosed for a
  refused shift by projecting the check's anchored seed and diagnosing the projection's last
  refused probe; a refit attempt the check may have preferred is not reproduced.
- `gemcad_version` and `mast_decimals` describe the schedule (five decimals for GemCad 4.x,
  eight for GemCad 5; empty for a `.isdd` file). `faces`, `valence` (`2E/V`) and `split_ratio`
  (`V/(F-2)`: 1 when every junction is a four-facet meet, 2 when every meet is split into
  three-facet ones) describe the captured network, and `max_slack_ppm` is how far the design
  misses its own meets, in millionths of the stone (at most 50, the identification radius).
- The CSV starts with `# census_version = 2` and ends with the percentile tables as `#` comment
  lines (nearest-rank 50th, 90th, 95th and 100th percentile of each excess over the rows whose
  `census_status` is `valid`, with the current constant and the number of rows it alone would
  refuse), then the branch summary. With `--out FILE` the summary is also printed.
- `--json` prints one document (`census_version`, `columns`, `rows`, `summary`) instead.
- `--out-dir DIR` (the folder must exist) writes the retargeted `.isdd` of every Valid row as
  `DIR/<key>__<target>__<mode>.isdd`, so a row can be opened in the editor and the Compare
  window. A whole library keeps all those stones in memory until the run ends: shard it.
- Time: one retarget check is dozens of solves. A 2,881-design library at five targets takes
  hours, not minutes. Start with `--targets sapphire --limit 200`, shard with `--offset`, and
  take timings only on a quiet machine. Progress is one line per design on standard error.

## Render jobs

The desktop app collects still exports and tilt videos as render jobs (File > Render Jobs...).
A job is a frozen description of one picture or one video, stored as a `*.job.json` file. These
two commands render such a file with the **same engine the app runs**, so the result is the one
the app would have made, and a remote worker is used through the app's own client. Unlike the
other commands they write their pictures themselves.

```
indicatrix-cli render JOB.job.json [--out FILE.png] [ENGINE OPTIONS] [--quiet]
indicatrix-cli tilt-video JOB.job.json [--out-dir FOLDER] [--restart] [ENGINE OPTIONS] [--quiet]

ENGINE OPTIONS
  --engine path|ray           override the job renderer (Ray uses its AA tap count)
  --local cpu|gpu|cpu+gpu      engines of this computer (default cpu+gpu)
  --remote HOST:PORT           a remote coordinator; needs --cert-dir
  --cert-dir FOLDER            folder with ca.pem, client.pem and client.key; needs --remote
  --compute local|remote|both  replace the job's compute choice
  --transfer full|final        replace the job's transfer choice
  --contribute-local           with --transfer final: this computer renders a share too
```

- `--out` is for `render` only; `--out-dir` and `--restart` are for `tilt-video` only. Another
  command's flag is exit code 1 and names the command.
- A job file of the other kind is exit code 1: run a still with `render` and a video with
  `tilt-video`.
- The remote worker and the engines of this computer are machine settings, so they are not in
  the job: give them with the flags. `--compute remote` and `--compute both` need `--remote`.
- A tilt video queued with Compute "Remote only" (or run with `--compute remote`) traces nothing
  on this computer: it only receives and saves the frames and encodes the video. If the remote
  is missing or drops, the run stops with an error (exit code 5) rather than tracing locally;
  run it again and it continues from the frames already written.
- `--local gpu` and `--local cpu+gpu` need a build with the `gpu` feature
  (`cargo build -p indicatrix-cli --features gpu`). Without it every choice renders on the
  processor, and a note on standard error says so.
- Relative paths in the job (the output, the HDR map) are taken against the folder of the job
  file. A still never overwrites a file: a taken name gets ` (2)`.
- A video resumes by default. Frames that this job wrote to its folder are kept and only the
  missing ones are rendered, so a run that was interrupted (Ctrl+C is not trapped) continues
  where it stopped. `--restart` deletes this job's frames first.
- **Output.** Standard output is one line, the path written: the PNG, the MP4 or GIF, or the
  frame folder when no video could be made. Standard error carries the progress and the
  estimated time left, then `wrote PATH`. On a terminal it is one rewritten line; otherwise a
  still prints a line per 10 percent and a video a line per finished frame
  (`tilt-video: frame 37 of 181 done, about 12 min left`). `--quiet` turns the progress lines
  off, never the notes and errors.
- **Exit codes.** 0 done; 1 the command line is wrong; 2 the job file cannot be used (not a job
  file, a newer format, invalid values, an HDR map that is missing or changed, a frame folder
  that belongs to another job); 3 a file could not be read or written; 5 the render failed or
  was stopped (no reachable remote worker, a remote failure, a frame failure).
- **Scripts.** The app's Render Jobs window exports the unfinished jobs as `run-render-jobs.ps1`
  and `run-render-jobs.sh` beside a `jobs` folder of job files. The scripts call
  `indicatrix-cli render` and `indicatrix-cli tilt-video` once per job, with `--local`,
  `--remote` and `--cert-dir` set from variables at the top of the script. Set `INDICATRIX_CLI`
  to the program's full path when it is not on the `PATH`.

The binary links the desktop app's library, so it is larger than the other commands need, but
it never opens a window.

## Differences from the desktop editor

- `retarget` applies only a Valid result, and so does the dialog: an Invalid or an
  **Unchecked** change is refused, with the same verdict line. (Earlier versions of the dialog
  let an Unchecked change through; the meet-preserving gate has no stone to apply then, so it
  does not.) Two options are the dialog's and differ in their default: the girdle allowance is
  on in the dialog and `--girdle-allowance` turns it on here.
- A design with concave tiers (grooves and dimples) is scored as the stone with its tools, as
  the desktop's Compare window, Optimize, Retarget and Angle Sweep do: `metrics`, `validate`,
  `sweep`, `optimize` and `retarget` see the tools in the optics, and the volume (`metrics`'
  Volume over width cubed and yield, `sweep`'s yield, `optimize`'s yield loss) is the carved
  volume. The widths, heights and the table are the flat stone's. A design without concave tiers
  is measured exactly as before.
- `optimize --out` writes nothing, and still exits 0, when the search found no candidate better
  than the design as it is.
- `solve --out`, `export` and `metrics` refuse a design that does not close (exit code 2).
  `validate` instead reports a design that does not solve as a Problem (exit code 4).
- `--vary-anchored` is switched on automatically when every tier is pinned, as in the tab.
- `.asc` is written from the editor's export schedule with the formats writer, so it has the
  CRLF line ends the desktop's Export writes, and the cutting-sheet HTML by the shared document
  builder. The desktop's Export additionally stamps library-entry footnotes, which a headless
  run has no entry for.
- `sweep --csv` writes the editor's CSV, with its CRLF line ends.
- `.gcs` export is experimental, as in the editor. It follows the published Gem Cut Studio 1.1
  file description (manual pp. 58-61) and the files Gem Cut Studio writes, and this program
  reads it back to the same tiers, angles, names and index positions (the tests write and read
  back every time); it has not been opened in Gem Cut Studio itself. The one open question is
  a design whose facet positions are not mirrored about index 0 (a design with no mirror
  symmetry, or a chiral one): the side rule of the file puts a crown facet and a pavilion facet
  of the same index at mirrored positions, where GemCAD's own files put them at the same
  position, and only Gem Cut Studio can say which it expects. Check such a file there before
  relying on it.

## Library use

The binary is a thin shell around the `indicatrix_cli` library. `run_command_line(&[String])`
is pure and returns an `Outcome` (`stdout`, `stderr`, the files the command wants written, the
exit code); `run` is the same and then writes the files. Both are what the tests call.
`render` and `tilt-video` are the exception: they write their pictures themselves and stream
their progress (`run` to standard error, `run_command_line` into the start of `Outcome::stderr`).
A command line with several designs is one `Outcome` too: the reports together, the files of the
designs that worked, and the first failure's exit code. `census` takes its designs itself
(`--templates`, `--fixtures`, files, `--db`), so it is not a batch; it streams one progress line
per design to standard error through `dispatch_streaming`.

## Tests

```
cargo test -p indicatrix-cli
```

The tests run the commands in-process on built-in designs and on files in the system's
temporary folder. They need no GPU and no library file. The render job tests render 16 by 16
pictures on the processor.
