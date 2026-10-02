# 9. Rendering and Export

## What you will do

This chapter covers exporting a still image of the rendered stone: sizes,
sample counts, filename templates, choosing local or remote computation,
and what the app's "preview then handoff" behaviour means while you work.

## Opening the export dialog

From the Live Render viewport (Chapter 2), open **Export High-Resolution
Render**. The dialog has these sections.

### Output Location

A read-only field shows the folder your image will be saved to (or
"Chosen on first export..." if you have never exported before), with a
**Change...** button that opens a normal folder picker. The app only asks
once; after that it remembers your choice.

Below it, a **Filename template** field lets you build the exported file's
name out of placeholders. Typing any of the following into the field
inserts that piece of information at that spot:

```
{design} {designer} {shape} {material} {ri} {width} {height} {spp}
{bounces} {colorspace} {preset} {lighting} {yaw} {pitch} {distance}
{exposure} {date} {time} {timestamp}
```

The default template is `gem_export_{material}_{width}x{height}_{spp}spp_{timestamp}`.
`.png` is added automatically if you don't type it yourself, and if a file
of that name already exists, the app adds `(2)`, `(3)`, and so on rather
than overwriting anything.

### Output Size

Choose **1080p**, **4K**, or **Custom** (16–8192 pixels per side, entered
directly).

### Colour Space

Choose **sRGB** (the default, and what almost every viewer expects),
**Display P3**, or **Rec.2020**. The latter two are wider-gamut options and
carry an embedded colour profile, so a viewer that understands it displays
the extra colour range correctly rather than looking washed out or overly
saturated.

### Compute

Choose **Local only**, **Remote only**, or **Local + Remote**. If a usable
remote (coordinator) is already configured and reachable, **Local + Remote**
is selected for you by default; otherwise the remote options are shown
greyed out with the reason underneath (see Chapter 10 for what those
reasons mean and how to fix them).

### Transfer

Shown once the remote is available and Compute includes it: **Full data**
(the remote's raw samples are merged with your own) or **Final picture
only** (the remote renders and tone-maps the whole image and sends one
finished PNG — much less data than Full data). With **Compute: Local +
Remote**, your own CPU/GPU is not left idle even on Final picture: a
Settings toggle, "Final-picture exports: this machine renders a share too"
(on by default), traces a share of the samples locally and uploads it for
the coordinator to fold in before tone-mapping — turn it off if you'd
rather this machine sit out the export. The starting choice for Transfer
itself is the remote's own default ("Export transfer (default)" in the
Remote Coordinator form); see Chapter 10, "Transfer: full data or final
picture". The tilt video's export section has the same row.

With **Full data**, the export does not hand the remote its whole share in one
go. It cuts the samples into requests and sizes each from the speed the
remote has shown so far: about 22 seconds of work per request for a single
worker, and up to about 90 seconds for a coordinator, which splits every
request across its own renderer and its joined workers and keeps them all
busy (one request never carries more than 65,536 samples, so a very fast
remote simply gets full-size requests back to back). The first request is a
short timing probe. For a coordinator the speed is measured over the whole
request, connection, upload and result transfer included, so the probe reads
low and the sizes settle on what the link and the remote really deliver
within a few requests.

### Max Ray Bounces

A separate rung ladder (4/8/12/24/64/128) just for this export — it starts
at whatever your live viewport is currently using, but is its own setting,
independent of the viewport's bounce count. At 64 or 128, a note reminds
you this is noticeably slower on GPU hardware than on CPU.

### Sample Count

A slider from 8 up to 32768 samples per pixel. Higher counts mean a
smoother, less noisy image at the cost of render time. At the very top of
the range (16384/32768), the app warns this "can take hours, especially at
large output sizes."

### Also Render With These Presets

If you have saved lighting presets marked as usable for export (Chapter 2),
you can tick any number of them here to render one extra image per ticked
preset alongside your current view. Export time multiplies with how many
you select. A preset that uses an HDR environment map is marked "HDR".

### While it renders

A live thumbnail updates roughly twice a second so you can judge
composition and how far along the image is — this preview is always shown
in ordinary sRGB regardless of which colour space you chose for the final
file, so do not judge final colour from it. A progress bar and percentage
track completion; **Cancel Export** stops the job early.

## What "preview then handoff" means while you work

While you are dragging the camera or the light, the app is not trying to
produce a polished picture — it draws a cheap, rough local preview so
movement stays responsive. The moment you stop moving (about six-tenths of
a second of no input), if a remote coordinator is set up, the app starts a
fresh settled image and brings the remote in — nothing from the rough local
sketch is mixed into it. With **Live Compute: Local + Remote** your own
computer and the remote both keep adding samples to that one image until
together they reach **Target Samples**; with **Remote only** the remote does
all of it. The remote sends back updates every "cadence" interval (half a
second by default), always sending at least a small update at least once
every two seconds even if there's nothing new worth showing. With **Live
Transfer: Final picture** it instead sends finished, denoised pictures of the
whole image and your own computer pauses (Chapter 10). If you grab the camera
again before it finishes, the app throws away the in-progress settled image
and drops straight back to the cheap local preview — the moving preview and
the settled image never blend.

An **HDR environment map** goes to the remote only if the remote reports that
it can render HDR scenes (Chapter 10); otherwise the live view and exports
render that scene on your own computer, with a one-time note.

**Local preview scale** (Off / Half / Quarter, in Settings) controls how
coarse that moving-camera preview is: a smaller fraction renders faster but
looks blockier while you are actively orbiting or panning.

## What the "worker silent" messages mean

If you see a message like "Remote render failed: worker silent for 8s," it
means the app sent your remote worker something to compute and heard
nothing back — not even a routine status update — for the stated number of
seconds. For an ordinary render this limit is 8 seconds; for the very
first response after starting a fresh job it is more generous, 30 seconds,
to allow for a slow warm-up on a busy or cold machine.

This is not necessarily a sign of anything broken. With **Local + Remote**
selected, the app automatically falls back to finishing the work locally
and tells you it happened — you do not lose the samples the remote worker
had already completed. With **Remote only** selected, there is no local
fallback, and the export or preview simply fails; switch to **Local +
Remote** or **Local only** and try again.

If this keeps happening, check that the worker machine is turned on,
reachable on your network, and that its render-serving program is still
running — see Chapter 10.

## Next steps

Continue to Chapter 10 to set up and troubleshoot a remote coordinator, or
Chapter 11 to understand the app's save formats.
