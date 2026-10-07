# 23. Render Jobs

## What you will do

Collect still pictures and tilt videos as render jobs, then let the program render them one after another while you keep working, or overnight. You can pause, resume, restart, reorder and cancel jobs at any time. You can also export the jobs as a script and render them on another computer with the command-line tool.

## Adding a job

You can add a job from two places:

- the export dialog (Chapter 9): set everything as you would for an export, then press **Add to Queue** instead of **Start Export**;
- the **Export tilt video** section of the Tilt Performance dialog (Chapter 14): press **Add to Queue** instead of **Start Video Export**.

**Add to Queue** sits next to the button that renders at once. The two do not replace each other. **Start Export** and **Start Video Export** still render immediately. **Add to Queue** saves a render job and renders nothing yet. A message tells you how many jobs were added and where to start them.

### What a job keeps

A job keeps a frozen copy of everything it needs: the finished stone, the material and its colour, the lighting, the camera, the size, the samples and the other export settings. If you change the design afterwards, the job still renders what you saw when you added it.

The job does not freeze the remote worker. Its address and certificates are read from Settings when the job runs (see Limitations). The job does remember your choices about where to render: local only, local and remote, or remote only, and the transfer.

### One job per picture

With presets ticked under **Also Render With These Presets**, each picture becomes its own job: one for the current view and one for every ticked preset.

### File names

The file name is worked out when you add the job, from the folder and the name template of the export dialog. If a file with that name exists by the time the job runs, or another job already uses the name, " (2)" is added to the name. A date or time in the template is the date and time of **Add to Queue**, not of the render.

## The Render Jobs window

Open it in one of two ways:

- **File → Render Jobs...**;
- the **Jobs** button at the top of the main window. It appears while a job is waiting or running, or while a started queue is running. It reads, for example, `Jobs: 3 waiting`, or `Rendering 2 of 5 · 42%` with a thin progress bar while a job renders. Click it to open the window.

Each row shows the job's number, its name, its state and what it renders, then its progress. The states are **Queued**, **Running**, **Paused**, **Done**, **Failed** and **Cancelled**. The list is kept in your library, so the jobs are still there the next time you start the program. The window has a round **?** button that opens this chapter, and **Close**. Closing the window does not stop a job that is rendering.

## Starting and pausing the queue

**Start Queue** renders the waiting jobs from the top of the list down, one job at a time. While the queue runs, the button reads **Pause Queue**. It stops the job that is rendering and keeps the rest waiting.

The queue never starts by itself, not even when the program starts. When the last waiting job is done, the queue stops and the program says so.

Only the buttons that make sense for a job's state are shown on its row.

| Button | What it does |
|---|---|
| Run Next | Moves the job to the top, so it is the next one to render. It does not interrupt the job that is rendering. |
| Up / Down | Moves the job one place. |
| Pause | Stops this job. The queue goes on with the next one. |
| Resume | Puts a paused or failed job back in line. A tilt video goes on with its first missing frame. |
| Restart | Renders the job again from the beginning. |
| Cancel | Stops the job for good. Restart can still bring it back. |
| Show | Opens the folder with the finished file. |
| Delete | Removes the job from the list. Files it already wrote stay on disk. |

Two more buttons sit at the top of the window.

| Button | What it does |
|---|---|
| Clear Finished | Removes every Done and Cancelled job from the list. Files stay on disk. |
| Export Script... | Opens the script section at the foot of the window (see below). |

**Delete** and **Clear Finished** never delete pictures, videos or frames. Only the list entry goes.

## What pausing keeps

Pausing works frame by frame.

- A **still picture** starts again from the beginning when you resume it. The picture is rendered as a whole, so nothing is kept.
- A **tilt video** keeps every frame it finished. When you resume it, it continues with the first missing frame, in the same folder. The video file is made once all frames are there.

A paused video keeps its frames on disk until it is finished or you delete the folder. At large sizes this can be many gigabytes.

**Restart** throws the finished frames of a video away and starts at frame one.

### Closing the program while a job renders

If you close the program while a job renders, the program asks first, in the same dialog it uses for unsaved work. If you go on, the job stops and waits as **Paused**. A tilt video keeps its finished frames. The queue is stopped, so nothing starts by itself when you open the program again.

If the program stops without asking (a crash or a power cut), the job is found as **Paused** the next time the program starts, with a note on its row that the program closed while it was rendering. At most one frame is rendered twice.

## Exporting a script

**Export Script...** writes the jobs that are not finished, in queue order, to a new folder. The section at the foot of the window has these choices:

| Choice | Meaning |
|---|---|
| Script type: **Both**, **PowerShell (.ps1)**, **Shell (.sh)** | Which script files are written. |
| Render on: **This computer**, or **This computer and the remote worker** (with its address) | The second choice is dimmed with "No remote worker is set up in Settings." when there is none. |
| **Put the finished pictures and videos in the script folder** | Without it, each picture or video goes to the folder its job was added with. |

Press **Choose Folder and Export** and pick a folder. The program makes a new folder in it, named `render-jobs-` with the date and time, so nothing is ever overwritten. It holds:

```
render-jobs-2026-10-06-1403/
  run-render-jobs.ps1     the PowerShell script
  run-render-jobs.sh      the shell script
  jobs/                   one job file per job, and assets/ with any HDR maps
  renders/                the finished files, if you chose to put them there
```

The button is dimmed when no job is unfinished. Copy the whole folder to the computer that should render, then run:

- Windows: `powershell -ExecutionPolicy Bypass -File run-render-jobs.ps1`
- macOS and Linux: `sh run-render-jobs.sh`

The script runs `indicatrix-cli` once per job. If the tool is not on the PATH, set the environment variable `INDICATRIX_CLI` to its full path first. At the top of the script you can change the engines, the remote address and the certificate folder. A tilt video that stopped part way continues where it stopped when you run the script again.

## Running a job file by hand

The jobs of a script are job files (`*.job.json`). You can run one yourself:

```
indicatrix-cli render FILE.job.json
indicatrix-cli tilt-video FILE.job.json
```

`render` renders a still picture and `tilt-video` renders a tilt video. A job file of the other kind is refused. Both use the program's own render engine, so the picture is the one the program would have made.

| Option | Meaning |
|---|---|
| `--local cpu`, `gpu` or `cpu+gpu` | Which engines of this computer render. The default is `cpu+gpu`. Without GPU support in the build, everything renders on the processor and a note says so. |
| `--remote HOST:PORT` with `--cert-dir FOLDER` | Also use a remote worker. The two go together. |
| `--compute local`, `remote` or `both` | Replace the job's own choice. `remote` and `both` need `--remote`. |
| `--transfer full` or `final` | Replace the job's own transfer choice. |
| `--contribute-local` | With `--transfer final`: this computer renders a share too. |
| `--out FILE.png` | `render` only: write the picture somewhere else than the job says. |
| `--out-dir FOLDER` | `tilt-video` only: use another frame folder. |
| `--restart` | `tilt-video` only: start again from the first frame. Without it a video resumes. |
| `--quiet` | No progress lines. Notes and errors are still printed. |

The tool prints one line on standard output: the path of the finished file (for a video, the MP4 or GIF, or the frame folder when no video could be made). Progress, notes and the line `wrote PATH` go to standard error.

| Exit code | Meaning |
|---|---|
| 0 | Done. |
| 1 | The command line is wrong, or the job file holds the other kind of job. |
| 2 | The job file cannot be used: it is not a job file, it comes from a newer version, a value is invalid, an HDR map is missing or changed, or the frame folder belongs to another job. |
| 3 | A file could not be read or written. |
| 5 | The render failed or was stopped. |

## Same picture, or nearly the same

A render done only on the CPU, or only on the GPU of one computer, gives the same picture every time. A render that splits its work between the CPU and the GPU, or that uses a remote worker, is only the same statistically. Each run shares the samples out by measured speed, so a few samples land on a different engine, and the noise differs a little from run to run. Both pictures are equally correct.

## Limitations

- One job renders at a time. A picture you export directly from the export dialog can render at the same time as a queued job; both are then slower.
- The queue never starts by itself. After you open the program, press **Start Queue**.
- Jobs use the remote worker set up in Settings when they run, not the one set up when they were added. If it is gone, the job renders on this computer with a note.
- An HDR map must stay where it was and stay unchanged until the job has rendered, or the job fails with a message saying so. Script export copies the maps for you.
- A paused still starts again from the beginning. A paused video keeps its frames and uses disk space until it is done.
- A date or time in a file name template is the time of **Add to Queue**.
- Deleting a job never deletes files.
- The command-line tool needs the same kind of computer as the program. Without a GPU build it renders on the processor.

## Next steps

Chapter 9 explains every export setting, Chapter 10 sets up a remote worker, and Chapter 14 covers the tilt video. Chapter 12 lists what to do when a job fails.
