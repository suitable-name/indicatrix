# 12. Troubleshooting and Limitations

## What you will do

This chapter is a quick-reference table of symptom, cause, and fix for the
problems you are most likely to run into, followed by a single consolidated
list of the app's current limitations gathered from earlier chapters into
one place.

## Solving problems

| Symptom | Cause | Fix |
|---|---|---|
| Status banner reads `Not solved -- click Solve to compute masts and validate this design.` | The design is **stale**: some edit (Save Tier, Remove, Apply Preform, Undo/Redo, or loading in the first place) happened since the last Solve. This is normal, not an error. | Click **Solve** (Chapter 5). |
| `Cannot solve: no scale-reference tier for: <block>` | The named block (crown, pavilion, or girdle) has no tier of kind **Exact scale value**. Meet-point geometry alone can never fix a block's overall size. | Add an **Exact scale value** tier to that block (Chapter 4), then Solve again. |
| `Degenerate: only N distinct vertex(es), volume <value or "non-finite">` | The facets bound a region, but it is not a valid solid -- usually two facets meeting somewhere they should not, or a wrong angle/constraint on a recently edited tier. | Check the tier(s) you edited most recently, especially ones sharing an anchor or a named-facet reference (Chapter 5). |
| `Unbounded: plane(s) [<indices>] never close the solid.` | One or more facet planes never meet enough neighbours to close the stone in some direction. | Check the facet(s) at the listed plane index/indices for a missing meet partner, or an angle too shallow/steep to intersect its neighbours (Chapter 5). |
| MAST/SOLVE columns show `-`, `?`, or amber **not solved** / **no anchor yet** | Same staleness or missing-anchor causes as above, scoped to one tier. | Solve the whole design; if a single tier still will not resolve, check that its block has an anchor and that its Meets constraint names real, existing facets. |
| SOLVE column shows bold amber **Least-squares est.** or **FAILED (untrusted)** | The solver could not derive that tier's mast from real geometry and fell back to an estimate or a placeholder. | Treat the design as not actually finished. Review the tier's constraint and its neighbours; re-solve after changes. |
| Solve is slow on a large design | Solve is a full geometric solve plus a solid-closure check across every tier -- this can take a second or two once a design has over a hundred tiers. This is expected, not a hang. | Wait for it to finish; this is exactly why Solve is a deliberate button press rather than something that reruns on every keystroke (Chapter 5). |
| Deep Solve takes minutes | Deep Solve is a separate, much slower external verification pass -- a mean of roughly 68 solves per design on the app's own reference corpus. This is normal for Deep Solve specifically, not for ordinary Solve. | Let it finish, or click **Cancel** to stop waiting (the background computation keeps running to completion regardless; its result is simply discarded) -- Chapter 8. |

## Rendering and remote-worker problems

| Symptom | Cause | Fix |
|---|---|---|
| "Remote render failed: worker silent for &lt;N&gt;s" | The app sent work to the remote worker and got no response at all -- not even a routine status update -- for longer than the allowed wait: 8 seconds once a job is underway, or a more generous 30 seconds for a brand-new job's very first response. | With **Local + Remote** selected, the app finishes the job locally automatically and tells you it happened -- no action needed beyond checking the worker later. With **Remote only** selected, switch to **Local + Remote** or **Local only** and retry (Chapters 9-10). |
| Worker keeps going silent | The worker machine may be asleep, unreachable, or its render-serving program may have stopped. | Check that the worker machine is on and awake, still reachable on your network (same network, VPN still connected, no firewall change), and that its serving process is still running (Chapter 10). |
| Test connection fails with a message starting "TLS error: ..." | A problem with the mutual-TLS handshake itself -- often an expired or mismatched certificate bundle. | Re-generate or re-request the certificate bundle (Chapter 10) and re-enter it; confirm the worker's own certificates have not been reissued or revoked since you last connected. |
| Test connection fails with "not a valid worker hostname: ..." | The **Address (host:port)** field is not a usable hostname or IP. | Re-check the address you entered against what the worker's operator gave you. |
| Test connection fails with a message starting "connection error: ..." | An ordinary network problem -- the worker is unreachable at that address, or a firewall is blocking it. | Confirm the address and port, that the worker machine is on and reachable, and that no firewall is blocking the connection. |
| Test connection succeeds but says "library only (no render capacity)" | The worker was built without render support -- it only serves its design library. | This is not a fault; use it for browsing/mirroring its library (Chapter 10), and point at a different worker if you need remote rendering. |
| "Security warning: &lt;address&gt; did not present the certificate authority this token was issued for..." during enrollment | The address you redeemed the token against is not the worker your token was actually issued for -- possibly a wrong address, or something intercepting the connection. | Stop. Do not retry against a different address on your own judgement -- confirm the correct address with whoever gave you the token first (Chapter 10). |
| "This token was not accepted -- it may be mistyped, already used, or expired..." | Enrollment tokens are single-use and expire 180 seconds after being issued. | Ask whoever manages the worker for a fresh token and redeem it promptly (Chapter 10). |
| Live viewport or export silently uses the CPU even though your build has GPU support | The current scene declined the GPU path for this frame or batch -- either this machine has no usable graphics adapter, or the scene uses a loaded HDR environment map (the GPU path has no equivalent for that yet). | This is a normal, per-frame fallback, not an error -- nothing fails to render. If you expect GPU rendering and never see it, confirm your build was compiled with GPU support and that **Local Compute** is not set to plain **CPU** (Chapter 2). |
| High-resolution export fails outright partway through | You have **Compute: Remote only** selected and a remote chunk timed out or failed, with no local fallback available to pick up the remainder. | Switch to **Local + Remote** and re-export, or fix the remote worker first (Chapter 9). |
| "Cannot export: &lt;reason&gt;" when clicking Export Edited .asc from the editor | The design is not currently in a solvable state. | Solve the design successfully first (Chapter 5), then export. |

## Catalogue problems

| Symptom | Cause | Fix |
|---|---|---|
| A design's notes contain what looks like leftover web-page text, or only one tier (crown or pavilion) is stored where you would expect two | The design was imported from an external design-sharing source, and that import had a data-cleanup issue that has since been fixed. | Re-sync your catalogue against its original source to pick up the corrected import. If you do not manage the catalogue yourself, ask whoever does to re-run the sync. |
| Catalogue seems to have lost designs you had before | You most likely started the program from a different folder than usual -- the catalogue file is opened relative to the program's working directory, not a fixed location (Chapter 1). | Start the program the same way you did before. Your designs are not deleted, just not the ones this session is looking at. |

## Known limitations

- **A custom catalogue material's own RI does not flow to the exported
  schedule automatically.** The Design Settings panel's Effective RI (and
  what "Export .asc"/"Save Native" actually writes) only ever derives from a
  built-in preset's name or an explicit RI override -- never from a custom
  catalogue material's own number, even though that same custom material's
  real RI/dispersion IS what the optimizer, tilt curve, and (when linked)
  the viewport use for it. Set an explicit RI override matching the custom
  material's own value if you need the exported schedule to agree (Chapter
  6).
- **No automated "retarget for a new material" proposal yet.** Adapting a
  design's pavilion angles for a different RI is still a manual, tier-by-
  tier process guided by the tier list's MARGIN column (Chapter 6) -- gear,
  symmetry order, mirror and material ARE now editable design properties
  (Chapter 6), just not yet a one-click retarget.
- **Deep Solve never modifies the design.** It is a read-only diagnostic
  that reports and suggests; nothing it finds is written back to the tier
  list unless you separately make the same change yourself (Chapter 8).
- **No 3D solid preview while editing.** The Edit sub-tab is a form and a
  table over the tier list only -- it does not show a render of the stone.
  To see the current geometry you switch to the Live Render sub-tab, which
  always shows the last successfully *solved* state, not a live preview of
  unsolved edits (Chapters 2-3).
- **One well-known gem species is not in the render material list.**
  Sphene (titanite) is deliberately left out of the built-in render
  materials: it needs birefringence and dispersion far outside the range
  this renderer's anisotropic optics have been verified against, so adding
  it without that verification would mean shipping unverified numbers
  rather than a measured material (Appendix C). Rutile, previously
  excluded for the same reason, has since been added and is reachable from
  the render material drop-down like every other built-in.
- **The GPU render path has two known gaps against the CPU path.** First,
  the "D65 Daylight" lighting preset renders with the real measured
  daylight spectrum on the CPU but still falls back to a simplified 6500K
  blackbody curve when routed to the GPU. Second, the one built-in material
  with a genuinely wavelength-dependent birefringence curve (Quartz, and by
  extension Amethyst and Citrine, which share its optics) renders that
  curve accurately only on the CPU; the GPU path always uses a simpler
  constant-offset approximation for the extraordinary ray. Both are flagged
  follow-up work rather than permanent limits, and neither affects a
  CPU-only build at all.
- **A remote worker connection always uses the first configured worker.**
  There is no load-balancing or per-job worker selection if you have
  several configured (Chapter 10).
- **Loading a remote design into the editor needs a real attached `.asc`
  file on the worker's side.** "Load Selected" fetches a remote design's
  original cutting-schedule file over the network and loads it exactly
  like a local one (Chapter 10). A design with no `.asc` ever attached on
  the worker has nothing genuine to fetch, so this fails with a clear
  error message; a *local* design in the same situation instead loads a
  reconstructed placeholder schedule (every mast at `0.0`, with its own
  warning) since the angle table alone is available for that reconstruction
  without a network round trip.

## Next steps

Appendix A collects the faceting and optics terms used throughout this
manual; Appendix B lists keyboard shortcuts (there are none beyond standard
text-field editing); Appendix C tables every built-in render material's
optical properties.
