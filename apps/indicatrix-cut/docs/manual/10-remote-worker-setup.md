# 10. Remote Worker Setup

## What you will do

This chapter covers connecting the app to its one remote: a **coordinator**
(`indicatrix-worker serve`, a server that other machines — "workers" — join,
and that renders on its own hardware too when started with `--render`). You
will get certificates, set up the remote, test the connection, choose how
results travel home (full data or final picture), choose how many pictures a
catalogue batch keeps in flight on it, browse its design library, and
troubleshoot a connection that has gone quiet.

The coordinator's operator sets it up with the worker software's own README
(`apps/indicatrix-worker/README.md`: "Setting up a coordinator with joined
workers"). The app connects to the coordinator's **viewer** port — 7878 unless
the operator chose another — never to the workers themselves.

The app talks to exactly **one** remote. If you have several render
machines, let them join a coordinator and point the app at the
coordinator; it spreads the work over them. (Settings saved by an older
version that listed several workers keep the **first** one — the one the
app was already rendering with — and drop the rest; the log names what was
dropped.)

## Why use a remote worker

Rendering — especially a high-resolution export, or a live view with many
samples per pixel — is faster on a more powerful machine. A remote worker
lets you point this app at another computer (or a render server) on your
network and have it do some or all of the rendering work, while you keep
working here.

## Certificates

A remote worker requires a certificate bundle: a folder containing exactly
three files — `ca.pem`, `client.pem`, `client.key`. This is exactly what
the worker software's own certificate-issuing command produces; you cannot
substitute other certificate files. There are two ways to get this folder
into the app.

### Option A: enter the folder manually

If you already have a bundle folder (someone gave it to you, or you
generated it yourself on the worker side), type or paste its path into the
**Certificate bundle folder** field, or click **Browse...** to pick it
with a folder dialog.

### Option B: redeem an enrollment token

If instead you have an **enrollment address** and a short-lived **token**
(starting `GW1-...`) from whoever manages the worker:

1. Click **Remote** in the top toolbar to open the **Remote Coordinator**
   panel, then **Set up remote coordinator** (or the edit icon of the one
   already configured).
2. Paste the enrollment address into **Enrollment address (host:port, from
   "cert issue-token")**.
3. Paste the token into **Token (GW1-...)**.
4. Click **Redeem**.

On success you'll see "Token redeemed -- certificate folder filled in,"
and the Certificate bundle folder field fills in automatically — you never
need to find or type that folder yourself. Tokens are single-use and
expire 180 seconds after being issued, so redeem one promptly; if you see
"This token was not accepted -- it may be mistyped, already used, or
expired... Ask for a fresh one and try again," that's what happened.

**If you ever see a security warning during enrollment** — text along the
lines of "did not present the certificate authority this token was issued
for... this is not an ordinary connection problem" — stop and do not
retry against a different address on your own judgement. This specific
message means the worker you reached is not the one your token was issued
for, which could mean a wrong address, or something intercepting the
connection. Confirm the correct address with whoever gave you the token
before trying again.

## Setting up the remote coordinator

In the **Remote Coordinator** panel, fill in the form:

- **Name** — a label for your own reference (e.g. "Office coordinator").
- **Address (host:port)** — the coordinator's viewer port, e.g.
  `render-server:7878`.
- **Certificate bundle folder** (see above).
- **Live stream** — Live progressive (stream improving previews) or
  Final only (wait for the finished image).
- **Cadence (ms)** — how often the remote sends an updated preview during
  a live render (default 500ms; the remote may enforce a slower floor than
  you ask for).
- **Preview scale** — Full, Half, Quarter (the default), or a custom
  percentage, for how large a preview image the remote streams back while
  you're actively moving the camera.
- **Export transfer (default)** — Full data or Final picture only; the
  starting choice of the export dialog's and the tilt video's **Transfer**
  row (see below).

Click **Save**. The panel then shows the configured remote with **Test
connection**, **Mirror library to local**, an edit icon and a remove
(**×**) button. Removing it makes every render local again.

## Transfer: full data or final picture

How the remote's result travels back to you is a separate choice from
*where* the rendering happens:

- **Full data** (the default) — the remote sends raw radiance, and your own
  CPU/GPU keep contributing samples to the same image (Local + Remote).
  This works with every remote.
- **Final picture only** — the remote renders the whole image, tone-maps
  and (live view) denoises it itself — with the same denoiser and tone curve
  this app uses — and sends finished pictures: far less data over a slow home
  link. For a still export or tilt video (not the live view), your own CPU/GPU
  can still pitch in: **Final-picture exports: this machine renders a share
  too**, in Rendering Settings next to Live Compute (on by default, greyed
  out without a remote configured), has your computer trace a reserved tail
  of the sample budget alongside the remote — but only when that export's
  Compute choice is Local + Remote — and upload its share for the coordinator
  to fold in before tone-mapping. If your share doesn't arrive in time, the
  coordinator quietly renders that tail itself instead, so a slow or
  interrupted local machine never stalls or breaks the export. Every current
  coordinator offers Final picture only, whether it renders itself or through
  joined workers. If a remote answers that it cannot, the app falls back to
  full data once, with a note, and does not ask that remote again until you
  reconnect or re-save it.

Either way the data is compressed losslessly on the wire when both sides
support it (they negotiate this themselves; there is nothing to set).

Where the choice lives:

- **Export dialog** and **tilt video** — a **Transfer** row (shown when a
  remote is available and the Compute choice includes it). Each exported
  PNG (or video frame) is written by this app, with the same color
  profile an all-local export would carry. If the final picture fails
  under Local + Remote, the image is rendered with full data instead; under
  Remote only it fails.
- **Live view** — **Live Transfer** in the Rendering Settings, next to Live
  Compute. With **Final picture** the settled image is the remote's alone:
  your own GPU/CPU pause after the handoff (as with Remote only), because
  finished 8-bit frames cannot be combined with your own samples.

## Testing the connection

Click **Test connection**. While it runs, the button shows "Testing...".
On success you'll see something like "Compatible -- coordinator (3
workers) (protocol v16)" (or "CPU, 16 threads" / a GPU description for a
coordinator that renders on its own with `--render` and has no workers
joined yet, or "library only (no render capacity)" if the remote only
serves the design library — a coordinator with no joined workers and no
`--render` lane). The "Last frame rendered by" line names
the coordinator the same way, and keeps up when workers join or leave it.
On failure, the message explains why — a TLS/certificate problem, a
connection failure, an invalid address, or that the remote cannot render
at all.

**A worker at capacity.** Every `indicatrix-worker serve` process caps how many
connections it will handle at once (`--max-connections`, default 64 — an
operator setting, not something this app exposes). A connection past that
cap is not left to hang: the worker still accepts it and immediately sends
back a clear refusal, which shows up here as a failure message naming that
the worker is already at capacity. This is unrelated to a worker "going
silent" (see below) — a capacity refusal is immediate and definitive, not a
timeout. If you see it, either wait for one of the worker's existing
connections to finish, or ask whoever manages the worker to raise
`--max-connections`.

A coordinator also caps the memory its in-progress jobs may use
(`--max-job-memory-mib`). A render refused for that reason reports
"coordinator busy" and is handled like any other failed remote request:
with Local + Remote it finishes on your own computer.

## Browsing the remote's library, and mirroring it locally

The panel's **Library** row switches between **Local** and **Remote**:

- **Remote** switches the catalogue panel (Chapter 2) to show the remote's
  design library instead of your own local one. A badge next to the status
  line always shows which library you're currently browsing.
- **Mirror library to local** pulls the remote's entire design library
  down into your own local database, with its own progress bar and a
  **Cancel mirror** button while it runs.

While browsing a remote library, **Load Selected** (Chapter 3) fetches
that design's original `.asc` cutting-instructions file straight from the
worker and loads it into the editor, the same as a local design — you no
longer need to mirror it locally first just to open it for editing. This
only works for a design that actually has a `.asc` file attached on the
worker's side; one that doesn't (nothing was ever uploaded for it) shows
an error explaining there is nothing to load rather than silently loading
a placeholder. Mirroring to local first is still worthwhile if you want an
offline copy or plan to edit the same design repeatedly without a network
round trip each time.

## Catalogue batches on the remote

The catalogue's batch tools — **Generate Previews** (one design, a filtered
set or the whole library) and **Compute Tilt Curves** (Chapters 2 and 14) —
send their work to the remote too, whenever Live Compute is **Remote only** or
**Local + Remote**. A preview is one small picture per view and a tilt sweep
is one request per design, and a remote renders one of them in a fraction of
the time the request needs to travel there and back. So the app does not send
one and wait for the answer before the next: it keeps **several pictures in
flight on the remote at once**, each rendered whole by one of the
coordinator's machines, and writes each as it arrives.

How many is the **Remote lanes for batches** setting in the Remote
Coordinator panel (1 to 32, default 4). It is saved the moment you change it
and read when a batch starts, so a batch already running keeps the count it
began with. A coordinator with several joined workers wants more lanes than
one with a single machine; beyond roughly one lane per worker slot a higher
number only queues pictures on the coordinator. If the remote fails pictures
repeatedly, each lane backs off on its own (and with **Local + Remote** your
own computer renders the pictures the remote could not, exactly as before).
With **Local only**, no lane is used.

## Denoise and sample budget

One setting applies to remote rendering generally, not per-worker:

- **Denoise merged image** — a toggle for whether the combined
  local+remote image gets denoised (grain removed) before display. With
  **Live Transfer: Final picture** the remote denoises its pictures itself,
  the same way.

There is no separate remote sample budget any more: the live view's
**Target Samples** (Chapter 9) is the one target for the whole settled
image. With **Local + Remote**, your computer and the worker split that
budget between them as they go — each takes the next free batch of
samples, so a fast worker simply does more of the work — and the image
is finished once the two together reach the target.

**HDR environments.** A scene lit by an HDR environment map renders
remotely when the remote says it supports HDR scenes: the app sends the
map's file once (the remote keeps a copy on disk and asks again only if it
has lost it), and the remote lights the stone with exactly the same map. A
map that was not loaded from a file, or is larger than 256 MiB, is never
sent. If the remote does not support HDR — for example because its operator
has no working asset cache (see the worker README's "HDR environment maps")
— the live view renders that scene on your own computer (a one-time note
says so), and exports do the same, so the whole image is always lit by one
environment.

**One remote.** Load-balancing over several machines is the
coordinator's job: add workers to the coordinator, not to this app. By
default the coordinator spreads exports over all its idle workers but keeps
the live view on a single lane for the quickest response; its operator can
change that.

## When a worker "goes silent"

If the app sends work to your remote worker and gets no response at all —
not even a routine status update — for too long, you'll see a message
mentioning "worker silent for &lt;N&gt;s." The wait allowed is 8 seconds for an
ordinary render already underway, or a more generous 30 seconds for the
very first response of a brand-new job (to allow for a slow warm-up on a
busy or newly started machine).

A coordinator that loses every worker able to finish a job reports "All
remote workers were lost"; the app treats that exactly like a silent
remote.

This does not necessarily mean anything is broken:

- With **Local + Remote** selected (Chapter 9), the app automatically
  finishes the job locally and tells you it happened, keeping every sample
  the remote worker had already completed.
- With **Remote only** selected, there is no local fallback — the render
  or export simply fails, and you'll need to either fix the worker or
  switch to a mode that includes local computation.

**What to check** if this keeps happening:

1. Is the worker machine turned on and awake (not asleep/hibernating)?
2. Is it still reachable on your network (same network, VPN still
   connected, no firewall change)?
3. Is the worker's serving process still running there?

Once you've confirmed the worker is reachable again, just retry — with
Local + Remote selected, the app will pick it back up on the next attempt
without any further configuration.

## Next steps

Continue to Chapter 11 for how the app saves your work and the file
formats it uses, or Chapter 12 for a consolidated troubleshooting list.
