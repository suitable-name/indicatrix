# indicatrix-cli

The Indicatrix design engines from a script: solve, check, score, optimize, retarget, sweep
and export a faceting design without a window and without a GPU. It runs the same engines as
the desktop editor (`indicatrix-cut-core` and `indicatrix-editor`), so a number printed here
is the number the editor shows.

```
cargo run -p indicatrix-cli -- <command> <design> [options]
cargo run -p indicatrix-cli -- <command> --help
```

A `<design>` is a `.indicatrix`, `.asc`, `.gem` or `.gcs` file. The kind is read from the
extension.

## Commands

| Command | What it does |
|---|---|
| `info` | Name, gear, symmetry, preform (shape, size and offset), material and the tier list. Does not solve. |
| `solve` | Solves every tier, checks that the facets enclose a stone, lists the manufacturability warnings. `--out` saves the design as `.indicatrix` (this converts `.asc`, `.gem` and `.gcs`). |
| `metrics` | Table-up windowing, brilliance, extinction, fire and scintillation, the proportions and the yield. `--tilt` also averages the tilt performance. |
| `validate` | The warnings and the overall Good, Check or Problem verdict of the editor's status strip. |
| `optimize` | The Optimize tab's search on the design's own material. Lists ranked candidates; `--out` saves the best. |
| `retarget` | The Retarget dialog's engine and validity gate, for another material. |
| `sweep` | Sets one tier to every angle of a range and scores each one. |
| `export` | Writes `.asc` (GemCAD cutting instructions), `.indicatrix`, `.gcs` or the cutting sheet as `.html`. |
| `render` | Renders a still picture from a render job file (`*.job.json`) made by the desktop app. See [Render jobs](#render-jobs). |
| `tilt-video` | Renders a tilt performance video from a render job file, and resumes from the frames already written. |

### Examples

```
indicatrix-cli info round.asc
indicatrix-cli solve round.asc --out round.indicatrix
indicatrix-cli metrics round.indicatrix --ri 1.76 --json
indicatrix-cli metrics round.indicatrix --material Sapphire --tilt --csv
indicatrix-cli validate round.asc --json
indicatrix-cli optimize round.indicatrix --preset brilliance --budget 400 --starts 4 --seed 7 --out better.indicatrix
indicatrix-cli retarget round.indicatrix --material Sapphire --crown-fraction 0.33 --out sapphire.indicatrix
indicatrix-cli retarget round.indicatrix --ri 1.7681 --mode optimize --range 4 --out sapphire.indicatrix
indicatrix-cli sweep round.indicatrix --tier "Pavilion Main" --from 39 --to 43 --step 0.5 --csv sweep.csv
indicatrix-cli export round.indicatrix --format html --out sheet.html
indicatrix-cli export round.indicatrix --out round.asc
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

## Material, lighting and the library

- `--material NAME` is a built-in material, or a custom one found in the library given with
  `--db`, or one the design file carries itself.
- `--ri N` is a bare refractive index: a flat, non-dispersive material.
- With neither flag, a command that scores uses the design's own material. A design with none
  is scored with its refractive index as a flat material and the report says so (the Optimize
  tab does the same). An unknown `--material` is a command-line mistake and ends with exit
  code 1. A material the design itself names that neither the built-ins, `--db` nor the file's
  own snapshot hold is refused with exit code 2. Neither falls back silently.
- When a saved `.indicatrix` file names a custom material of the `--db` library (for example
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
Each file that is written is noted on standard error (`wrote FILE`). Reports go to standard
output; notes and errors go to standard error.

## Output

Text, `--json` and `--csv` output are deterministic: the same command on the same files prints
the same bytes. Numbers have a fixed number of decimals (never the shortest round-trip form), lists have a
fixed order, JSON objects come out with sorted keys, and nothing reads the clock or mints an id.
Only the thread count of a sweep depends on the machine: it sizes its worker pool from the
processor count, but every row is a pure function of its angle and the rows come back in angle
order (flattest first).

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
- Without `--lighting` the search scores under the grading tray (see above).

## Retarget

`retarget` turns every facet about its girdle-side edge by the shift of the critical angle,
exactly as the dialog does, and checks the result with the dialog's validity gate: a closed
stone, a girdle that survives, a table that stays flat and facets that do not vanish.

- `--mode shift` (default) is the critical-angle shift only. `--mode optimize` runs a search
  around it (`--range`, `--budget`, `--seed`, `--preset`). Every option is refitted so the
  table and culet keep their size, and by default scored with a penalty for drifting from
  the design's table size and crown-to-pavilion ratio; `--no-keep-look` drops the penalty.
- The crown follows the pavilion's stretch by default (`--crown-follow`, accepted for symmetry):
  every crown angle's tangent is scaled by the same vertical stretch the pavilion gets, so the
  stone keeps its silhouette and table size, and the verdict line quotes the stone's depth.
  `--crown-fraction F` moves the crown by that share of the pavilion's shift instead (0 leaves
  it where it is). `--crown-ratio` scales the crown angle by the ratio of the two critical
  angles instead. Give at most one of the three.
- A refused result prints the report and the reasons, ends with exit code 2 and writes nothing.
- The saved design has the target material set.

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

- `retarget` refuses an **Unchecked** result as well as an Invalid one. The dialog also lets an
  Unchecked change through (the current design itself could not be analysed, so nothing could
  be compared); a command line has nobody to look at the stone, so only Valid is accepted.
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
- `.gcs` export is experimental, as in the editor.

## Library use

The binary is a thin shell around the `indicatrix_cli` library. `run_command_line(&[String])`
is pure and returns an `Outcome` (`stdout`, `stderr`, the files the command wants written, the
exit code); `run` is the same and then writes the files. Both are what the tests call.
`render` and `tilt-video` are the exception: they write their pictures themselves and stream
their progress (`run` to standard error, `run_command_line` into the start of `Outcome::stderr`).

## Tests

```
cargo test -p indicatrix-cli
```

The tests run the commands in-process on built-in designs and on files in the system's
temporary folder. They need no GPU and no library file. The render job tests render 16 by 16
pictures on the processor.
