# indicatrix-cut — settings file

The on-disk settings format: location, persistence guarantees, render quality,
and remote-worker configuration. For everything else see [the README](../README.md).

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
over the real file. **Loading is deliberately infallible** — a missing,
unreadable, or corrupt settings file just logs a warning and falls back to
defaults; a broken settings file can never block startup, and every field has a
`#[serde(default)]` so an old/partial file loads fine with the missing fields
defaulted.

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

[[settings.remote_workers]]
name = "Workstation"
address = "10.0.0.5:9443"
cert_dir = "C:/Users/me/indicatrix-certs"
transfer_mode = "LiveProgressive"
cadence_ms = 500
preview_scale = "Full"

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
`2^10 = 1024` (`gui::sample_scale`). Noise falls as `1/sqrt(N)`, so a linear
8..1024 control would spend roughly 97% of its travel above 32 spp, where each
step barely changes anything visible. A `target_samples` value that is not an
exact power of two still loads: it resolves to the largest power of two not
exceeding it, so a hand-edited file can never fail to open.

An older settings file may still carry a `quality_preset = "High / Quality"`
string from the four-tier preset selector this replaced. Nothing rejects it —
there is no `#[serde(deny_unknown_fields)]`, so TOML drops the unknown key and
`target_samples` takes its default.

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

### Remote worker configuration (`WorkerSettings`)

```rust
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
`client.key` in one directory (see `indicatrix-worker`'s README). The app always
talks to the *first* configured worker; there is no load-balancing or
multi-worker selection UI. Render width/height is **not** per-worker — it's
session-wide, since every worker (and the local CPU path) must agree on it for
the summed samples to actually compose correctly.
