# ui-screens

Headless visual check of the desktop UI with `slint-viewer` (style `fluent-dark`, must be on PATH).

```
pwsh apps/indicatrix-cut/scripts/ui-screens.ps1 [-Only RoughPlannerWindow] [-OutDir <dir>]
```

- Windows and their declared minimum sizes are read from `ui/**/*.slint` (`inherits Window`, `min-width`/`min-height`).
- Each window is `--check`ed first (a failed check skips its renders), then rendered at its minimum, 1366x768, 1920x1080, 2560x1440 (MainWindow also 900x900, 1024x640).
- Rough Planner scenarios (results, loaded, cuts_mesh, running, save_rows) come from `ui_fixtures/rough_planner.py`, which patches `RoughPlanModel` defaults in copies of `ui/` and fails when a patched property no longer exists.
- A PNG larger than the requested size is a finding: the content minimum exceeds the declared minimum.
- Output: `target/ui-screens/<timestamp>/` with PNGs and `summary.md`. Work copies in `target/ui-screens/work/` are removed at the end. Exit code 1 on any failed check, render failure or size finding.
