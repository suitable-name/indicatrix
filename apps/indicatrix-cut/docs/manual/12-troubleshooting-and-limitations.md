# 12. Troubleshooting and Limitations

## What You Will Do

This chapter is a quick-reference table of symptom, cause, and fix for the
problems you are most likely to run into, followed by a single consolidated
list of the app's current limitations gathered from earlier chapters into
one place.

## Solving Problems

| Symptom | Cause | Fix |
|---|---|---|
| Status strip reads `Not solved -- click Solve to compute masts and validate this design.` | The design is **stale**: some edit (Save Tier, Remove, Apply Preform, Undo/Redo, or loading in the first place) happened since the last Solve. This is normal, not an error. | Click **Solve** (Chapter 5). |
| `<Block> has no anchor: add a tier with an exact scale value.` | The named block (crown, pavilion, or girdle) has no tier of kind **Exact Scale Value**. Meet-point geometry alone can never fix a block's overall size. | Add an **Exact Scale Value** tier to that block (Chapter 4), or click the tier table's own **Add Anchor** button on one of that block's rows (Chapter 3), then Solve again. |
| `Degenerate: only N distinct vertex(es), volume <value or "non-finite"> -- check tier 5 (Girdle), tier 8.` | The facets bound a region, but it is not a valid solid -- usually two facets meeting somewhere they should not, or a wrong angle/constraint on a recently edited tier. The trailing "check ..." clause, when present, already names the likeliest tier(s). | Check the named tier(s) first, or otherwise whichever you edited most recently, especially ones sharing an anchor or a named-facet reference (Chapter 5). |
| `Unbounded: tier 3 (P1) never close the solid.` | One or more facet planes never meet enough neighbours to close the stone in some direction; the message names the tier(s) by number and name. | Check the named tier(s) for a missing meet partner, or an angle too shallow/steep to intersect its neighbours (Chapter 5). |
| MAST/SOLVE columns show `-`, `?`, or amber **not solved** / **no anchor yet** | Same staleness or missing-anchor causes as above, scoped to one tier. | Solve the whole design; if a single tier still will not resolve, check that its block has an anchor and that its Meets constraint names real, existing facets. |
| SOLVE column shows bold amber **Least-squares est.** or **FAILED (untrusted)** | The solver could not derive that tier's mast from real geometry and fell back to an estimate or a placeholder. | Treat the design as not actually finished. Review the tier's constraint and its neighbours; re-solve after changes. |
| Solve is slow on a large design | Solve is a full geometric solve plus a solid-closure check across every tier -- this can take a second or two once a design has over a hundred tiers. This is expected, not a hang. | Wait for it to finish; this is exactly why Solve is a deliberate button press rather than something that reruns on every keystroke (Chapter 5). |
| Deep Solve takes minutes | Deep Solve is a separate, much slower external verification pass -- a mean of roughly 68 solves per design on the app's own reference corpus. This is normal for Deep Solve specifically, not for ordinary Solve. | Let it finish, or click **Cancel** to stop waiting (the background computation keeps running to completion regardless; its result is simply discarded) -- Chapter 8. |
| Deep Solve button is greyed out: "...unavailable for a new or placeholder-reconstructed design" | This design has no printed proportions (Vol/W^3, L/W, C/W, P/W, H/W) recorded in the catalogue to verify against. | Nothing to fix -- Deep Solve genuinely has nothing to check a brand-new or placeholder-reconstructed design against (Chapter 8). |
| Deep Solve button is greyed out: "...has nothing to repair until you convert a tier to a meet constraint" | Every tier is still pinned exactly as imported -- there is nothing Deep Solve could move even if it found a discrepancy. | Adopt at least one tier first (Chapter 8), then try again. |
| Optimize button is greyed out: "Every tier is currently pinned as a scale reference..." | A freshly loaded catalogue design starts with zero free tiers -- this is expected, not broken. | Adopt at least one tier's real meet constraint, or author one by hand (Chapter 8). |
| Optimize will not move a particular tier | That tier is either pinned as an Exact Scale Value (including the girdle, which Optimize never moves), or its SOLVE strategy is still uncertain (Least-squares est. / FAILED / not solved / blocked / no anchor yet). | Adopt the tier if it is pinned and you want it free; otherwise fix the uncertain solve first -- see "Trusting the SOLVE Column" (Chapter 8). |
| Clicking Solve shows an **Abandon Solve** button instead of Cancel | A background solve (a design of more than 16 tiers, or an auto-solve) has no partial result to keep, so the button is named Abandon rather than Cancel. It stops the solver within a sweep or pipeline run, typically a few milliseconds, and discards the result (Chapter 5). | Nothing to fix; click it to stop waiting, then Solve again when you are ready. |
| A dialog titled "This Design Is Not a Closed Solid" appears when saving or exporting | The design does not currently solve to a closed solid, and you are about to write a file anyway. | Click **Cancel** and fix the design first, or **Save Anyway** (**Export Anyway**) to write it with a `NOT A CLOSED SOLID` header stamp so the file itself carries the warning (Chapter 11). |
| Save writes a **draft** even though every block has an anchor tier | The design has more facet planes than the solver can verify (currently more than 400 total planes) -- the masts it would otherwise produce are untrustworthy placeholders, so the app saves a draft instead, the same as a design with no anchor tiers yet (Chapter 11). | Reduce the design's total plane count (fewer indices per tier, or split the design) and save again once it solves within the limit. |

## Rendering and Remote-Worker Problems

Ray-Traced is a deterministic approximation. Tint may shift with path length, frost
uses outside-light quadrature, and paths beyond the bounce limit are discarded.
Polarization, double refraction, haze, zoning and edge rounding are
not shown.
Increasing AA taps smooths boundaries; it does not restore those optical models.
Ray-Traced needs no denoising. Metrics and tilt curves keep their existing model.

Remote Ray-Traced work requires matching protocol-29 peers. Update the viewer,
coordinator and joined workers together if the handshake refuses an old version.
The GPU path uses Vulkan, DX12 or Metal; OpenGL is not used on the desktop (a machine with
only OpenGL renders on the CPU; setting the environment variable `WGPU_BACKEND=gl` tries it
anyway, at your own risk, since the OpenGL driver layer can crash on exit).
An unavailable or declined local GPU falls back to the CPU; Remote Only retains its
usual refusal behavior when no remote lane can finish the work.

| Symptom | Cause | Fix |
|---|---|---|
| "Remote render failed: worker silent for &lt;N&gt;s" | The app sent work to the remote worker and got no response at all -- not even a routine status update -- for longer than the allowed wait: 8 seconds once a job is underway, or a more generous 30 seconds for a brand-new job's very first response (plus the time a large finished picture needs to cross a slow link, while one is in flight). | With **Local + Remote** selected, the app finishes the job locally automatically and tells you it happened -- no action needed beyond checking the worker later. With **Remote Only** selected, switch to **Local + Remote** or **Local Only** and retry (Chapters 9-10). |
| Worker keeps going silent | The worker machine may be asleep, unreachable, or its render-serving program may have stopped. | Check that the worker machine is on and awake, still reachable on your network (same network, VPN still connected, no firewall change), and that its serving process is still running (Chapter 10). |
| Test Connection fails with a message starting "TLS error: ..." | A problem with the mutual-TLS handshake itself -- often an expired or mismatched certificate bundle. | Re-generate or re-request the certificate bundle (Chapter 10) and re-enter it; confirm the worker's own certificates have not been reissued or revoked since you last connected. |
| Test Connection fails with "not a valid worker hostname: ..." | The **Address (host:port)** field is not a usable hostname or IP. | Re-check the address you entered against what the worker's operator gave you. |
| Test Connection fails with a message starting "connection error: ..." | An ordinary network problem -- the worker is unreachable at that address, or a firewall is blocking it. | Confirm the address and port, that the worker machine is on and reachable, and that no firewall is blocking the connection. |
| Test Connection succeeds but says "library only (no render capacity)" | The remote has nothing to render with: it was built without render support, or it is a coordinator started without `--render` that no worker has joined yet. | This is not a fault; use it for browsing/mirroring its library (Chapter 10). For remote rendering, ask its operator to join a worker to it or start it with `--render`. |
| An HDR-lit scene renders locally with the note "This remote doesn't support HDR environments; rendering locally" | The remote does not report HDR support -- typically its asset cache could not be opened. | Ask the remote's operator to check the asset cache (worker README, "HDR environment maps"). Studio-lit scenes are unaffected (Chapter 10). |
| "This HDR environment can't be sent to the remote; rendering locally" | The loaded map has no source file the app could send, or the file is larger than 256 MiB. | Load the map from a smaller `.hdr` file, or render that scene locally (Chapter 10). |
| "Security warning: &lt;address&gt; did not present the certificate authority this token was issued for..." during enrollment | The address you redeemed the token against is not the worker your token was actually issued for -- possibly a wrong address, or something intercepting the connection. | Stop. Do not retry against a different address on your own judgement -- confirm the correct address with whoever gave you the token first (Chapter 10). |
| "This token was not accepted -- it may be mistyped, already used, or expired..." | Enrollment tokens are single-use and expire 180 seconds after being issued. | Ask whoever manages the worker for a fresh token and redeem it promptly (Chapter 10). |
| Live viewport or export silently uses the CPU even though your build has GPU support | The current scene declined the GPU path for this frame or batch -- either this machine has no usable graphics adapter (or it stopped responding), or the scene uses an HDR environment map too large for the graphics card's buffer limits. (Concave tools, Chapter 16, are traced on the graphics card like any other facet.) | This is a normal, per-frame fallback, not an error -- nothing fails to render. If you expect GPU rendering and never see it, confirm your build was compiled with GPU support and that **Local Compute** is not set to plain **CPU** (Chapter 2). |
| High-resolution export fails outright partway through | You have **Compute: Remote Only** selected and a remote chunk timed out or failed, with no local fallback available to pick up the remainder. | Switch to **Local + Remote** and re-export, or fix the remote worker first (Chapter 9). |
| "Cannot export: &lt;reason&gt;" when clicking Export Edited .asc from the editor | The design is not currently in a solvable state. | Solve the design successfully first (Chapter 5), then export. |
| A render job is **Failed** with "The HDR map ... changed since the job was added." | The `.hdr` file the job's lighting uses was edited, replaced or moved after you added the job. | Put the original file back and press **Resume**, or add the job again from the export dialog (Chapter 23). |
| A render job is **Failed** with "The folder ... belongs to another render job." | The frame folder of a tilt video already holds frames, or a marker, from a different job or export. | Choose another folder when you add the job, or delete the frames in that folder, then press **Restart** (Chapter 23). |
| The exported script does nothing on Windows, or PowerShell says running scripts is disabled | Windows blocks unsigned scripts by default. | Run it as `powershell -ExecutionPolicy Bypass -File run-render-jobs.ps1` (Chapter 23). |

## Catalogue Problems

| Symptom | Cause | Fix |
|---|---|---|
| A design's notes contain what looks like leftover web-page text, or only one tier (crown or pavilion) is stored where you would expect two | The design was imported from an external design-sharing source, and that import had a data-cleanup issue that has since been fixed. | Re-sync your catalogue against its original source to pick up the corrected import. If you do not manage the catalogue yourself, ask whoever does to re-run the sync. |
| Catalogue seems to have lost designs you had before | You most likely started the program from a different folder than usual -- the catalogue file is opened relative to the program's working directory, not a fixed location (Chapter 1). | Start the program the same way you did before. Your designs are not deleted, just not the ones this session is looking at. |

## Log File

An app started with `--log` (for example `indicatrix-cut --log`) writes a
plain-text log file, `indicatrix-cut.log`, next to the
app's executable — falling back to the system temp folder if that
location is not writable (Program Files, a read-only mount). By default
it logs at `warn` for everything, with this app's own two crates turned
up to `info`: the geometry/solver library (`indicatrix`) and the app
itself (`indicatrix_cut`). The same messages also print to stderr when a
console is attached — a debug build, or the app started from a terminal;
a normal release-build launch has no console, so without `--log` those
messages go nowhere.

To see more detail while reproducing a problem, set the `RUST_LOG`
environment variable before starting the app — for example
`RUST_LOG=indicatrix=debug,indicatrix_cut=debug` for both of this app's
own crates at debug level, or `RUST_LOG=trace` for the noisiest level
everywhere. `RUST_LOG`, when set, replaces the default filter above
entirely rather than adding to it.

If you are filing a bug report, restart with `--log`, reproduce the problem and
attach `indicatrix-cut.log` (from beside
the executable, or the temp folder if that is where it landed) — it is
the single most useful thing to include alongside a description of what
you were doing. A crash additionally writes `indicatrix-cut-panic.log` in
the same location, with the panic message, the thread it happened on, and
a full backtrace; attach that too if it exists.

## Known Limitations

- **The live margin estimate under an angle field ignores custom materials.**
  Export Edited .asc, Save, the cutting sheet, the Solid view's windowing-risk
  hatch and the MARGIN badge all resolve a custom catalogue material's own RI
  (Chapter 6), but the small estimate shown while you type an angle uses the
  built-in materials only. For a design whose Material names a custom catalogue
  material with no RI override, it can differ a little from the MARGIN badge until
  you commit the angle. Set an explicit RI override matching the custom
  material's own value if you notice this (Chapter 6).
- **Deep Solve never modifies the design.** It is a read-only diagnostic
  that reports and suggests; nothing it finds is written back to the tier
  list unless you separately make the same change yourself (Chapter 8).
- **A catalogue row's stone effects are not remembered.** The Stone tab's effects
  (Inclusion Haze, Crystal Axis Orientation, Frosted Girdle, Edge Rounding) are saved
  with a design you open in the Edit tab. A catalogue row you are only browsing shows
  them view-only: they start from the defaults for every row and are lost when you
  open another (Chapter 2).
- **Material effects and the stone width come from the design only.** The old
  app-wide values for them are applied once, to the first design you open after
  updating, and are ignored afterwards; a design file saved by an older version has no
  effects and opens with all of them off (Chapter 11).
- **The Solid and Live Render viewports show a generic placeholder for a
  brand-new, empty design.** Both read "Solve to preview" / "Select a
  design to preview" rather than a message that names New Design.../Load
  Selected/Open as the next step -- only the tier table itself got
  that friendlier empty-state text (Chapter 3).
- **One well-known gem species is not in the render material list.**
  Sphene (titanite) is deliberately left out of the built-in render
  materials: it needs birefringence and dispersion far outside the range
  this renderer's anisotropic optics have been verified against, so adding
  it without that verification would mean shipping unverified numbers
  rather than a measured material (Appendix C). Rutile is included and is
  reachable from the Stone tab's material lists like every other built-in.
- **The two render paths agree on the daylight spectrum and on Quartz.** The
  "D65 Daylight" lighting presets use the tabulated measured daylight
  spectrum on the graphics card as well as on the CPU, and the one built-in
  material with a genuinely wavelength-dependent birefringence curve
  (Quartz, and by extension Amethyst and Citrine, which share its optics)
  traces that curve for the extraordinary ray on both paths. A picture
  routed to the graphics card therefore differs from the CPU one only by
  sampling noise (Appendix C).
- **The app talks to exactly one remote.** Spreading work over several
  machines is the coordinator's job: let them join it and point the app at
  the coordinator (Chapter 10).
- **"Final Picture" transfer does not combine with your own samples.** The
  remote renders the whole image; your own CPU/GPU sit that image out. If a
  remote ever answers that it cannot send finished pictures, the app falls
  back to full data once, with a note (Chapter 10).
- **Loading a remote design into the editor needs a real attached `.asc`
  file on the worker's side.** "Load Selected" fetches a remote design's
  original cutting-instructions file over the network and loads it exactly
  like a local one (Chapter 10). A design with no `.asc` ever attached on
  the worker has nothing genuine to fetch, so this fails with a clear
  error message; a *local* design in the same situation instead loads a
  reconstructed placeholder schedule (every mast at `0.0`, with its own
  warning) since the angle table alone is available for that reconstruction
  without a network round trip.
- **Render jobs run one at a time, and only start when you press Start Queue.**
  A picture or video you start directly from a dialog can render beside a
  queued job, and both are then slower (Chapter 23).
- **A render job uses the remote worker set up in the Remote Coordinator dialog when it runs,** not
  the one that was set up when you added it (Chapter 23).
- **A paused still picture starts again from the beginning.** A paused tilt
  video keeps its frames, which can take many gigabytes at large sizes, until
  it finishes or you delete the folder (Chapter 23).
- **Hybrid and remote renders of the same job are only statistically the same.**
  A fixed CPU-only or GPU-only split gives the same picture every time
  (Chapter 23).
- **The Rough Planner hands the editor a convex piece, without the inclusions.**
  **Open in Editor** gives the design the planned stone's sawn piece as its
  preform. That piece is convex: for a scanned rough it follows the rough's
  outline, not its true surface, so a notch in the scan is not in the piece.
  Inclusions you modelled in the planner are not drawn or checked in the editor.
  A design file saved with a planned piece needs this build or a newer one; a
  build from before 2026-10-12 refuses it as an unsupported version. To hand
  such a design to someone with an older build, press **Apply Preform** first, which
  replaces the piece with a Block or Cylinder (Chapter 15).

## Next Steps

Appendix A collects the faceting and optics terms used throughout this
manual; Appendix B lists every keyboard shortcut this app actually has;
Appendix C tables every built-in render material's optical properties.
