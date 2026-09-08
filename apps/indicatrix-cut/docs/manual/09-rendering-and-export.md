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
remote worker is already configured and reachable, **Local + Remote** is
selected for you by default; otherwise the remote options are shown
greyed out with the reason underneath (see Chapter 10 for what those
reasons mean and how to fix them).

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
you select. A preset that uses an HDR environment map is marked "HDR ·
slower."

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
a second of no input), if a remote worker is set up, the app hands the
whole job over to it and starts fresh — nothing from the rough local sketch
is mixed into what comes back. From then on, the remote worker sends
back an improved image every "cadence" interval (half a second by
default), always sending at least a small update at least once every two
seconds even if there's nothing new worth showing. If you grab the camera
again before it finishes, the app throws away the in-progress remote image
and drops straight back to the cheap local preview — the two never blend.

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

Continue to Chapter 10 to set up and troubleshoot a remote worker, or
Chapter 11 to understand the app's save formats.
