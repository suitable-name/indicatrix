# indicatrix-cut — settings file

The on-disk settings format: location, persistence guarantees, render quality,
and the remote endpoint. For everything else see [the README](../README.md).

## Settings file

Location, resolved by `settings::store::default_settings_path()`:

| Platform | Path |
|---|---|
| Windows | `%APPDATA%\indicatrix-cut\settings.toml` |
| macOS | `~/Library/Application Support/indicatrix-cut/settings.toml` |
| Linux/Unix | `$XDG_CONFIG_HOME/indicatrix-cut/settings.toml`, else `~/.config/indicatrix-cut/settings.toml` |
| (no env var found) | `./indicatrix-cut/settings.toml`, relative to the current directory |

Hand-rolled per-platform resolution (no `directories`/`dirs` crate), chosen to
avoid needing write access to the install directory (e.g. under `Program Files`
on Windows). Format is TOML, written via `toml::to_string_pretty`, and writes
are crash-safe: written to a `.toml.tmp` sibling first, then atomically renamed
over the real file. **Loading is deliberately infallible** — a missing or
unreadable settings file just logs a warning and falls back to defaults, and a
broken settings file can never block startup, since every field has a
`#[serde(default)]` so an old/partial file loads fine with the missing fields
defaulted. A **corrupt** (unparseable TOML) file is not silently discarded: it
is renamed aside first, to `settings.toml.corrupt-<unix-seconds>` next to it,
*before* falling back to defaults — the next save then writes a fresh
`settings.toml` rather than overwriting the corrupt one, so the broken file (and
whatever was in it) stays recoverable on disk instead of being lost outright. A
failed rename (e.g. no write permission on the directory) is logged and startup
continues with defaults anyway; the corrupt file is simply left at its original
path in that case.

**Legacy migration.** The first time this app runs with no settings file of its
own yet, it copies (never moves) a settings file from the old public viewer
app's config directory, `diagram-gui` — the app `indicatrix-cut` superseded,
before the 2026-09-07 suite-wide rename to Indicatrix removed it outright — so a
machine that already had that app configured doesn't start completely blank.
This only ever fires into a genuinely absent destination and never overwrites
an existing `indicatrix-cut` settings file; a failed copy is logged and falls
through to defaults, same as any other load failure.

Example of what the file looks like:

```toml
[settings]
target_samples = 256
max_bounces = 12
exposure = 1.0
light_yaw_deg = 48.0
light_pitch_deg = 54.0
lighting_rig = "Gem Studio Ring Lights"
camera_yaw = 0.6
camera_pitch = 0.45
camera_distance = 2.4
selected_material = "Diamond"
denoise_enabled = true
contribute_to_final_picture = true
remote_batch_lanes = 4
import_preview_choice = "ask"

[settings.remote]
export_transfer = "FullData"
live_transfer = "FullData"

[settings.remote.connection]
name = "Coordinator"
address = "10.0.0.5:7878"
cert_dir = "C:/Users/me/indicatrix-certs"
transfer_mode = "LiveProgressive"
cadence_ms = 500
preview_scale = "Quarter"

[[presets]]
name = "Studio Softbox"
built_in = true
light_yaw_deg = 48.0
light_pitch_deg = 54.0
exposure = 1.0
lighting_rig = "Gem Studio Ring Lights"
camera_distance = 2.4
```

### Target samples

Render quality is one setting, `target_samples`: how many samples per pixel the
progressive accumulation converges to before it stops. Default 256.

The dialog's slider drags an **exponent**, not the count — `2^3 = 8` up to
`2^10 = 1024` (`gui::render::sample_scale`). Noise falls as `1/sqrt(N)`, so a linear
8..1024 control would spend roughly 97% of its travel above 32 spp, where each
step barely changes anything visible. A `target_samples` value that is not an
exact power of two still loads: it resolves to the largest power of two not
exceeding it, so a hand-edited file can never fail to open.

An older settings file may still carry a `quality_preset = "High / Quality"`
string from the four-tier preset selector this replaced. Nothing rejects it —
there is no `#[serde(deny_unknown_fields)]`, so TOML drops the unknown key and
`target_samples` takes its default.

`target_samples` is also the one target for a live image rendered with a remote:
local and remote samples count toward it together. The separate
`remote_render_samples` key older files carry is ignored on load and dropped on the
next save.

### Bounce cap

`max_bounces` is independent of the sample count. Default 12; the dialog offers
4 / 8 / 12 / 24 / 64 / 128, raised from an earlier 4/8/12/16/24 ladder on the
strength of `crates/indicatrix/examples/bounce_cost.rs`: going from a cap of 12 to
1024 costs only 1.3-1.4x wall time on CPU (the path population is short-tailed —
median 4 bounces, and only 0.03% of paths ever reach 128), while the hardest
material measured was still just 95.6% converged at the old 24 ceiling, reaching
99.5% at 64 and 99.99% at 128. Nothing above 128 is offered because nothing
measurably changes past it.

A persisted value that is no longer a rung (16, say) is still honoured exactly
as written for rendering; only the highlighted pill snaps to the nearest rung.

Render resolution is its own setting (`render_width`/`render_height`), not part
of either control.

### Remote endpoint (`RemoteEndpoint`)

The viewer has exactly one remote: `remote: Option<RemoteEndpoint>`, an
`indicatrix-worker serve` coordinator (rendering through joined workers, its own
`--render` lane, or both). `address` is the coordinator's viewer port (7878 by
default), never a joined worker's.

```rust
pub struct RemoteEndpoint {
    pub connection: WorkerSettings,
    pub export_transfer: ExportTransfer, // FullData (default) | FinalPicture
    pub live_transfer: LiveTransfer,     // FullData (default) | FinalPicture
}

pub struct WorkerSettings {
    pub name: String,
    pub address: String,             // "host:port"
    pub cert_dir: String,            // directory holding ca.pem / client.pem / client.key
    pub transfer_mode: TransferMode, // LiveProgressive | FinalOnly
    pub cadence_ms: u32,             // default 500; clamped UP to the worker's advertised floor
    pub preview_scale: PreviewScale, // Full | Half | Quarter | Custom(1..=100)
}
```

`cert_dir` should point at exactly what `indicatrix-worker cert issue-client --dir
<pki-dir> --name <label> --out <bundle-dir>` writes — `ca.pem`, `client.pem`,
`client.key` in one directory (see `indicatrix-worker`'s README). Load-balancing
over several machines is the coordinator's job, not the viewer's. Render
width/height is **not** part of the endpoint — it's session-wide, since the remote
(and the local CPU path) must agree on it for the summed samples to actually
compose correctly.

`export_transfer` is the default of the export dialog's and the tilt video's
"Transfer" choice; `live_transfer` is the settings dialog's "Live Transfer" -- see
[remote-rendering.md](remote-rendering.md) for what "final picture" does.

`contribute_to_final_picture` (default `true`, top-level, not on `RemoteEndpoint`)
is the settings dialog's "Final-picture exports: this machine renders a share too"
switch (v16): with a remote configured and `export_transfer`/the tilt video's own
"Transfer" pill at `FinalPicture`, the viewer's own idle CPU/GPU traces a share of
the sample budget alongside the remote and uploads it as one `CONTRIBUTION`, which
the coordinator folds in before tone-mapping. Only takes effect when the export's
"Compute" choice is `Both`; a scene mismatch or an HDR map the local tracer can't
resolve identically silently falls back to a remote-only picture instead of
refusing the export.

`import_preview_choice` (default `"ask"`, top-level; `"full"`, `"solid"` or `"skip"`) is
the remembered answer to the question asked after an import about generating catalogue
previews, stored by the question's "Remember my choice" box. Set it back to `"ask"` to be
asked again.

`remote_batch_lanes` (default `4`, top-level, limited to 1..=32 on load and on set) is
the remote worker dialog's "Remote lanes for batches" spin box: how many pictures
(preview batch) or designs (tilt batch) a catalogue batch keeps in flight on the remote
at once, one remote dispatcher per lane. Read when a batch starts. Against a
coordinator, keep it at or below the coordinator's `--jobs-per-viewer` (default 8):
requests beyond that wait in its per-viewer queue without progress, and one that waits
there longer than 60 s is given up by the desktop (rendered locally under
"Compute: Both", counted as failed under "Remote only").

**Migration.** A file written before the single-endpoint model carries a
`[[settings.remote_workers]]` list. It still loads: the FIRST entry becomes `remote` (it was the only one any
feature used), the rest are dropped and logged by address, and the next save writes
only `[settings.remote]`.
