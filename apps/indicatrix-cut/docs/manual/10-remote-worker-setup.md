# 10. Remote Worker Setup

## What you will do

This chapter covers connecting the app to a remote rendering machine (a
"worker"): getting certificates, adding the worker, testing the
connection, browsing its design library, and troubleshooting a connection
that has gone quiet.

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

1. Click **Remote** in the top toolbar to open the worker panel.
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

## Adding a worker

In the **Remote** panel, add a worker with:

- **Name** — a label for your own reference (e.g. "Office workstation").
- **Address (host:port)**.
- **Certificate bundle folder** (see above).
- **Transfer mode** — Live progressive (stream improving previews) or
  Final only (wait for the finished image).
- **Cadence (ms)** — how often the worker sends an updated preview during
  a live render (default 500ms; the worker may enforce a slower floor than
  you ask for).
- **Preview scale** — Full, Half, Quarter, or a custom percentage, for how
  large a preview image the worker streams back while you're actively
  moving the camera.

**Limitation.** Note that a newly added worker's Preview scale field shows
**Full** pre-selected in this form, even though a worker added through
some other path in the app defaults internally to **Quarter**. If you add
a worker through this dialog and don't touch the Preview scale field
yourself, it will be saved as Full — check this setting explicitly if you
want a smaller, faster preview stream.

## Testing the connection

Click **Test connection**. While it runs, the button shows "Testing...".
On success you'll see something like "Compatible -- CPU, 16 threads
(protocol v3)" (or a GPU description, or "library only (no render
capacity)" if that worker only serves the design library and cannot
render). On failure, the message explains why — a TLS/certificate problem,
a connection failure, an invalid address, or that the worker cannot render
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

## Browsing a worker's library, and mirroring it locally

Each configured worker has:

- A **Browse library** toggle — switches the catalogue panel (Chapter 2)
  to show that worker's design library instead of your own local one. A
  badge next to the status line always shows which library you're
  currently browsing.
- A **Mirror to local** button — pulls that worker's entire design library
  down into your own local database, with its own progress bar and a
  **Cancel** button while it runs.

While browsing a remote library, **Load Selected** (Chapter 3) fetches
that design's original `.asc` cutting-schedule file straight from the
worker and loads it into the editor, the same as a local design — you no
longer need to mirror it locally first just to open it for editing. This
only works for a design that actually has a `.asc` file attached on the
worker's side; one that doesn't (nothing was ever uploaded for it) shows
an error explaining there is nothing to load rather than silently loading
a placeholder. Mirroring to local first is still worthwhile if you want an
offline copy or plan to edit the same design repeatedly without a network
round trip each time.

## Denoise and sample budget

Two settings apply to remote rendering generally, not per-worker:

- **Denoise merged image** — a toggle for whether the combined
  local+remote image gets denoised (grain removed) before display.
- **Remote Render Samples** — a slider for how many samples per pixel a
  remote worker targets (128 up to 8192).

**Limitation.** The app always talks to the **first** configured worker
only. If you have several workers listed, there is no load-balancing or
per-job worker selection — only the first one in your list is ever used.

## When a worker "goes silent"

If the app sends work to your remote worker and gets no response at all —
not even a routine status update — for too long, you'll see a message
mentioning "worker silent for &lt;N&gt;s." The wait allowed is 8 seconds for an
ordinary render already underway, or a more generous 30 seconds for the
very first response of a brand-new job (to allow for a slow warm-up on a
busy or newly started machine).

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
