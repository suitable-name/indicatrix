<#
.SYNOPSIS
  Profile-guided-optimization (PGO) release builds of indicatrix-cut and/or indicatrix-worker
  on Windows, over CPU axes.

    CPU:  avx512 -> -C target-cpu=x86-64-v4 (AVX-512; Zen 4 / Skylake-X or newer). Opt-in:
                    this host's native CPU support is probed before an avx512 build is
                    attempted (see Test-Avx512Supported below); an explicit `-Cpu avx512`
                    on an unsupported host refuses with a clear error instead of crashing
                    mid-training (0xC000001D, illegal instruction), and the default
                    `-Cpu all` silently drops avx512 from the tier list on such a host.
          avx2   -> -C target-cpu=x86-64-v3 (AVX2+FMA+BMI baseline; Haswell/Zen or newer)
          scalar -> -C target-cpu=x86-64-v2 (SSE4.2 baseline; standard non-AVX fallback)
          native -> -C target-cpu=native (host CPU instruction set)
    GPU:  gpu    -> indicatrix-cut/gpu + indicatrix-worker/gpu (wgpu megakernel, CPU fallback per frame)
          cpu    -> no gpu feature at all (pure CPU tracer binaries)

  The documented default is `-Cpu avx2` and `-Cpu scalar` (two separate invocations) --
  avx512 remains available but is opt-in and auto-skipped/refused where unsupported.

  A profile trained with `INDICATRIX_SIMD=avx2` (this script always caps training to the
  tier it is building) leaves `indicatrix`'s avx512 and scalar SIMD kernels
  (`simd::simd_level`) with zero profile counts, so LLVM treats them as cold/size-optimised
  in that binary even if the CPU it later runs on supports a wider tier. Ship the avx2
  build to AVX2-and-newer machines (including AVX-512 ones) and the scalar build only to
  machines without AVX2 -- do not expect an avx2-trained binary's avx512 path, or a
  scalar-trained binary's avx2 path, to be well-optimised.

.DESCRIPTION
  Per combination, in its own target directory (target\pgo-<cpu>-<gpu>):
    1. build the training example `pgo_train`, plus every selected app binary, all
       together in one `cargo build` instrumented with -C profile-generate. Building the
       app binaries (not just `pgo_train`) alongside it in the SAME invocation, with the
       SAME package set, target, target-dir and feature list as step 4 below, is required
       so every crate's `-C metadata` fingerprint (and therefore its mangled-symbol crate
       hash) is identical between this step and step 4: if the two builds select
       different package sets, every profiled crate gets a different hash, profile-use
       finds nothing to apply and the binary is optimised as if untrained.
    2. run the pgo_train.exe produced by that build (CPU tracer, every lighting model,
       denoiser, tone-map, meet-point solver against synthetic AND real designs,
       CAD-preview mesh extraction; GPU only under the `gpu` tier and only if an
       adapter exists)
    2b. build and run `indicatrix-cut-core`'s OWN `pgo_train` example (Design::solve,
       .asc export, native save/load, resolve_after_edit, optimize_design) -- in a
       SEPARATE, isolated `cargo build` invocation, never combined with step 1's `-p
       indicatrix ... --example pgo_train`: both crates ship an
       example named `pgo_train`, and Cargo's example-binary "pretty path" uplift
       (target\<profile>\examples\<name>.exe) is keyed by name alone, not by package
       -- selecting both in ONE invocation was measured (on this toolchain, via
       `cargo build --message-format=json`) to report the IDENTICAL `"executable"`
       path for both targets and non-deterministically pick one binary for that
       shared path, an unrecoverable ambiguity `cargo test --no-run`'s own
       hash-suffixed binaries don't have. `indicatrix-cut-core`'s LIBRARY code is
       still instrumented/optimised correctly regardless of this split --
       CARGO_ENCODED_RUSTFLAGS applies to the whole build graph, and
       `indicatrix-cut-core` is already a transitive dependency of
       `indicatrix-cut`/`indicatrix-worker` in step 1 -- this separate step exists
       only to actually EXECUTE its code so its already-instrumented functions
       collect real samples. See `Get-CutCoreFeatures`/`Invoke-CutCoreTraining`.
    2c. build and run `indicatrix-net`'s `scene_roundtrip` integration test under the same
       instrumentation (postcard wire-protocol encode/decode of `SceneState`), through
       `cargo test --no-run --message-format=json` so the hash-suffixed test binary can
       be located. Same training set as scripts/pgo-bolt-build.sh. See `Invoke-NetTraining`.
    3. merge the .profraw files with llvm-profdata
    4. rebuild the SAME packages, target, target-dir and feature list as step 1, with
       -C profile-use=<merged profile> instead of -C profile-generate (and
       -C llvm-args=-pgo-warn-missing-function, so a totally-unapplied profile shows up
       as a flood of "no profile data available" warnings in build.log instead of
       silently compiling as if untrained again)
    5. verify the profile actually applied (crate-hash check of BOTH `indicatrix` and
       `indicatrix-cut-core` against the merged .profdata, plus a build.log scan for "hash mismatch"/"no profile data available"
       over documented thresholds -- see Test-ProfileHashApplied/Test-ProfileWarnings)
    6. copy the executables to <OutDir> as <binary>-win-<cpu>-<gpu>.exe

  Training coverage is a known limitation, not a complete picture. The training binaries
  do NOT exercise: the radiance (HDR) image codecs, the solid rasterizer, `guide_pass`,
  the `indicatrix-vault` database, the desktop editor's event loops and GUI code, or the
  `GpuBackend` type. The `gpu` tier's `train_gpu` stage trains `render_hybrid`, which no
  app calls today, so it warms code the shipped binaries do not run. Functions in these
  areas have zero profile counts and are optimised as cold code. Two further limits:
  `indicatrix-cut-core` is trained by its own build with a narrower package and feature
  set than the optimised build (the two `pgo_train` examples cannot share one cargo
  invocation), and `indicatrix-net`'s test build selects only that package; the
  per-crate hash check in step 5 is what detects either producing a non-matching crate.

.PARAMETER Cpu
  avx512, avx2, scalar, native, or all (default all; see the CPU axis note above -- avx512
  is dropped automatically from `all` on a host without AVX-512 support, and refused
  outright if requested explicitly on such a host).
.PARAMETER Gpu
  gpu, cpu, or both (default both).
.PARAMETER Target
  worker, cut, or all (default all). Every combination always builds `indicatrix` itself
  alongside whichever of `indicatrix-cut`/`indicatrix-worker` this selects, all in one
  `cargo build` (see .DESCRIPTION) -- `indicatrix`'s `hdr` and `serde` features are always
  both enabled, regardless of -Target, because `indicatrix-cut` depends on `indicatrix`
  with `features = ["hdr"]` unconditionally and always enables `indicatrix-net/render`
  (which turns on `indicatrix/serde`), and `indicatrix-worker`'s own `worker` feature does
  the same. A `cut`-only or `worker`-only build's profile is kept in its own
  `pgo-profile\cut\` / `pgo-profile\worker\` subdirectory (its package set, and therefore
  its crate hashes, differ from the combined `all` build), so it never mixes with (or gets
  silently overwritten by) a profile trained for a different -Target. Building `all` (the
  default) keeps the original combined layout: one shared profile per combination, feeding
  both binaries, at `pgo-profile\windows-<cpu>-<gpu>.profdata`.
.PARAMETER OutDir
  Where the renamed executables are copied (default <repo>\bin).
.PARAMETER SkipTrain
  Reuse an existing windows-<cpu>-<gpu>.profdata from the pgo-profile directory.
.PARAMETER Parallel
  Execute all requested target combinations concurrently using background jobs.
.PARAMETER Native
  Build ONLY the host-CPU tier (-C target-cpu=native), replacing whatever -Cpu selected --
  a quick "build for this machine" run, without the generic avx2/scalar tiers. Same as
  scripts/pgo-bolt-build.sh's --native.
.PARAMETER MaxMissingFunctionWarnings
  Fallback threshold for the post-build "no profile data available for function" warning
  count (default 100000; see Test-ProfileWarnings) -- only consulted when
  Test-ProfileHashApplied could not verify the profile; once the crate-hash check has
  confirmed it applied, the count is reported and never fatal. indicatrix-cut measured
  ~66,000 with a fully applied profile on 2026-09-28. This warning is EXPECTED in bulk for
  `indicatrix-cut`/`indicatrix-worker`'s own GUI/server/CLI code -- `pgo_train` only
  exercises `indicatrix`'s tracer/solver/brep/tilt(/gpu) paths, never the desktop editor
  or the coordinator/worker networking code, so most of those two binaries' own functions
  legitimately have zero profile samples even with a correctly-applied profile. This is
  therefore a blunt tripwire for a total-parity-collapse (an instrumented/optimised
  crate-hash mismatch, where effectively EVERY function in a shared dependency crate like
  `indicatrix`/`wgpu`/`naga` goes missing), not a precision gate -- the default has not
  been calibrated against a known-good run, so tighten it once a real successful run
  establishes this host's actual baseline count.
#>
[CmdletBinding()]
param(
    [ValidateSet('avx512', 'avx2', 'scalar', 'native', 'all')]
    [string]$Cpu = 'all',
    [ValidateSet('gpu', 'cpu', 'both')]
    [string]$Gpu = 'both',
    [ValidateSet('worker', 'cut', 'all')]
    [string]$Target = 'all',
    [string]$OutDir = '',
    [switch]$SkipTrain,
    [switch]$Parallel,
    [switch]$Native,
    [int]$MaxMissingFunctionWarnings = 100000
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$hostTarget = 'x86_64-pc-windows-msvc'
if (-not $OutDir) { $OutDir = Join-Path $root 'bin' }
$pgoProfileDir = Join-Path $root 'pgo-profile'
# Unit separator: how CARGO_ENCODED_RUSTFLAGS joins individual rustc arguments, so a repo
# path containing a space (unlike this one today) can't split a flag in two the way
# space-joined RUSTFLAGS would.
$unitSep = [char]0x1F

$includeWorker = $Target -in @('all', 'worker')
$includeCut = $Target -in @('all', 'cut')
# `indicatrix` is always in the package set: it owns `pgo_train`, and the instrumented and
# optimised builds must select IDENTICAL packages so their crate hashes agree (see
# Get-PgoFeatures below and both build steps).
$pkgList = @()
if ($includeCut) { $pkgList += 'indicatrix-cut' }
if ($includeWorker) { $pkgList += 'indicatrix-worker' }
$trainPkgList = @('indicatrix') + $pkgList
$binList = $pkgList

# --- llvm-profdata / llvm-nm from the active toolchain -----------------------------
$sysroot = (& rustc --print sysroot).Trim()
$llvmBin = Join-Path $sysroot "lib\rustlib\$hostTarget\bin"
$profdata = Join-Path $llvmBin 'llvm-profdata.exe'
$llvmNm = Join-Path $llvmBin 'llvm-nm.exe'
# A self-compiled toolchain has no rustup components ("toolchain 'x' does not support
# components"), so rustup is only ever a best-effort convenience here: try it once for
# a rustup toolchain, then fall back to the tools in PATH, and only fail when neither
# place has them. (The host target's own std is always present; nothing to check.)
if (-not (Test-Path $profdata) -or -not (Test-Path $llvmNm)) {
    if (Get-Command rustup -ErrorAction SilentlyContinue) {
        Write-Host "llvm-tools-preview not in the sysroot; trying 'rustup component add llvm-tools-preview' (harmless if this is not a rustup toolchain)..."
        try { & rustup component add llvm-tools-preview 2>&1 | Out-Null } catch {}
    }
    if (-not (Test-Path $profdata)) {
        $fromPath = Get-Command llvm-profdata -ErrorAction SilentlyContinue
        if ($fromPath) { $profdata = $fromPath.Source } else { throw "llvm-profdata not found at $profdata nor in PATH. Install the toolchain's llvm-tools (rustup: 'rustup component add llvm-tools-preview'; self-compiled: build with llvm-tools enabled) or put an LLVM 'llvm-profdata' matching this rustc's LLVM in PATH." }
    }
    if (-not (Test-Path $llvmNm)) {
        $fromPath = Get-Command llvm-nm -ErrorAction SilentlyContinue
        if ($fromPath) { $llvmNm = $fromPath.Source } else { throw "llvm-nm not found at $llvmNm nor in PATH (same fix as for llvm-profdata)." }
    }
}

# --- AVX-512 host support probe ------------------------------------------------------
# `-Cpu avx512` (x86-64-v4) crashes a training run with 0xC000001D (illegal instruction)
# on any host whose CPU lacks AVX-512 -- there is no graceful runtime fallback for an
# instrumented binary built with a target-cpu baseline it can't execute. Probe once,
# up front, instead of discovering this mid-run.
function Test-Avx512Supported {
    $cfg = & rustc --print cfg -C target-cpu=native 2>$null
    return [bool]($cfg | Select-String -SimpleMatch 'target_feature="avx512f"')
}

$avx512Supported = $null
function Get-Avx512Supported {
    if ($null -eq $script:avx512Supported) { $script:avx512Supported = Test-Avx512Supported }
    return $script:avx512Supported
}

if ($Cpu -eq 'avx512' -and -not (Get-Avx512Supported)) {
    throw "-Cpu avx512 requested, but this host's native CPU has no AVX-512 support " +
    "(rustc --print cfg -C target-cpu=native has no target_feature=`"avx512f`"). " +
    "An instrumented avx512 binary would crash immediately with 0xC000001D (illegal " +
    "instruction) instead of training. Use -Cpu avx2 or -Cpu scalar (the documented " +
    "default pair), or run this on AVX-512-capable hardware."
}

$cpuList = if ($Cpu -in @('all', 'both')) { @('avx512', 'avx2', 'scalar') } else { @($Cpu) }
if ($cpuList -contains 'avx512' -and -not (Get-Avx512Supported)) {
    Write-Warning "Dropping avx512 from -Cpu all: this host's native CPU has no AVX-512 " +
    "support (see Test-Avx512Supported). Building avx2 and scalar only -- the documented " +
    "default pair. Pass -Cpu avx512 explicitly on AVX-512-capable hardware to opt back in."
    $cpuList = @($cpuList | Where-Object { $_ -ne 'avx512' })
}
# -Native replaces the tier list (it used to append to it).
if ($Native) {
    $cpuList = @('native')
}
$gpuList = if ($Gpu -in @('all', 'both')) { @('gpu', 'cpu') } else { @($Gpu) }

# One feature list, used VERBATIM by both the instrumented (profile-generate) and the
# optimised (profile-use) cargo invocation, so both produce the same crate hashes.
# `indicatrix/serde` and `indicatrix/hdr` are always included, regardless of -Target:
# `indicatrix-cut` depends
# on `indicatrix` with `features = ["hdr"]` unconditionally and always enables
# `indicatrix-net/render` (-> `indicatrix/serde`); `indicatrix-worker`'s own `worker`
# feature turns on both the same way (`indicatrix/hdr` explicitly, `indicatrix/serde` via
# `indicatrix-net/render`). Naming them explicitly here, rather than relying on that
# propagation, keeps the two build steps' `--features` string trivially comparable
# instead of depending on Cargo's feature-unification rules to line up identically both
# times. `indicatrix-net/compression` (the v14 zstd/LZ4/PNG payload codecs) is a DEFAULT
# feature of `indicatrix-net` that both apps inherit, so it is already on in every
# variant; it is named here too so a later change to that crate's default set can never
# silently drop it from a PGO build (the owner wants it active in every PGO variant).
function Get-PgoFeatures([bool]$IncludeCut, [bool]$IncludeWorker, [string]$GpuName) {
    $features = @('indicatrix/serde', 'indicatrix/hdr', 'indicatrix-net/compression')
    if ($IncludeWorker) { $features += 'indicatrix-worker/worker' }
    if ($GpuName -eq 'gpu') {
        $features += 'indicatrix/gpu'
        if ($IncludeCut) { $features += 'indicatrix-cut/gpu' }
        if ($IncludeWorker) { $features += 'indicatrix-worker/gpu' }
    }
    return ($features -join ',')
}

# The `--features` string for `indicatrix-cut-core`'s OWN, separate `pgo_train` build
# (see .DESCRIPTION's step 2b for why this is never merged into `Get-PgoFeatures`
# above / `$trainPkgList`). Deliberately narrower: `indicatrix-cut-core` has no Cargo
# features of its own, and a feature namespaced to a package NOT selected in this
# invocation (e.g. `indicatrix-cut/gpu`, which `Get-PgoFeatures` includes) is a hard
# `cargo` error under this workspace's resolver -- only the `indicatrix`-namespaced
# features `indicatrix-cut-core` actually depends on are named here, kept identical
# to `Get-PgoFeatures`'s own `indicatrix/*` subset so `indicatrix` itself builds with
# the SAME feature set (a cache hit, not a rebuild) in both invocations.
function Get-CutCoreFeatures([string]$GpuName) {
    $features = @('indicatrix/serde', 'indicatrix/hdr')
    if ($GpuName -eq 'gpu') { $features += 'indicatrix/gpu' }
    return ($features -join ',')
}

# Builds and runs `indicatrix-cut-core`'s own `pgo_train` example as a separate,
# isolated `cargo build` invocation -- see .DESCRIPTION's step 2b. `$Tdir`/`$CpuFlag`
# must match the SAME target-dir/target-cpu the caller's own instrumented build just
# used; `$env:CARGO_ENCODED_RUSTFLAGS` must already carry that build's
# `-C profile-generate=...` (this function does not set it itself, so the caller's
# own `Remove-Item Env:...`/env setup around its call stays the single source of
# truth for what "instrumented" means here). Called immediately after the caller
# training-runs `indicatrix`'s own pgo_train.exe, so the shared
# target\...\examples\pgo_train.exe path this overwrites has already served its
# purpose for that binary by the time it does.
function Invoke-CutCoreTraining([string]$Tdir, [string]$Name, [string]$GpuName, [string]$LogFile) {
    $cutCoreFeatures = Get-CutCoreFeatures $GpuName
    "==> [$Name] instrumented build (indicatrix-cut-core, separate invocation; features: $cutCoreFeatures)" |
        Out-File -FilePath $LogFile -Append -Encoding utf8
    cargo build --release --target $hostTarget --target-dir $Tdir `
        -p indicatrix-cut-core --example pgo_train --features $cutCoreFeatures 2>&1 |
        Out-File -FilePath $LogFile -Append -Encoding utf8
    if ($LASTEXITCODE -ne 0) { throw "[$Name] indicatrix-cut-core instrumented build failed (exit $LASTEXITCODE)" }

    "==> [$Name] indicatrix-cut-core training run" | Out-File -FilePath $LogFile -Append -Encoding utf8
    $cutCoreExe = Join-Path $Tdir "$hostTarget\release\examples\pgo_train.exe"
    & $cutCoreExe 2>&1 | Out-File -FilePath $LogFile -Append -Encoding utf8
    if ($LASTEXITCODE -ne 0) { throw "[$Name] indicatrix-cut-core training run failed (exit $LASTEXITCODE)" }
}

# Builds and runs `indicatrix-net`'s `scene_roundtrip` integration test under the same
# instrumentation, covering `SceneState`'s postcard wire-protocol encode/decode -- the
# hot path `indicatrix`'s `pgo_train` cannot reach (an `indicatrix` example cannot
# depend on `indicatrix-net`). Same arguments as scripts/pgo-bolt-build.sh: built with
# `cargo test --no-run --message-format=json` so the hash-suffixed test binary's path
# comes from the JSON `executable` field. `$env:CARGO_ENCODED_RUSTFLAGS` and
# `$env:LLVM_PROFILE_FILE` must already carry the instrumented build's settings.
function Invoke-NetTraining([string]$Tdir, [string]$Name, [string]$LogFile) {
    "==> [$Name] instrumented build (indicatrix-net scene_roundtrip test)" |
        Out-File -FilePath $LogFile -Append -Encoding utf8
    $jsonPath = Join-Path $Tdir 'net-test-build.json'
    cargo test --release --target $hostTarget --target-dir $Tdir `
        -p indicatrix-net --features render --test scene_roundtrip --no-run `
        --message-format=json 2>> $LogFile > $jsonPath
    if ($LASTEXITCODE -ne 0) { throw "[$Name] indicatrix-net instrumented test build failed (exit $LASTEXITCODE)" }
    $netExe = Get-NetTestExecutable $jsonPath
    if (-not $netExe) { throw "[$Name] could not locate the instrumented scene_roundtrip test binary in $jsonPath" }

    "==> [$Name] indicatrix-net wire-protocol training run" | Out-File -FilePath $LogFile -Append -Encoding utf8
    $env:LLVM_PROFILE_FILE = Join-Path (Split-Path -Parent $env:LLVM_PROFILE_FILE) 'net-%p-%m.profraw'
    & $netExe --test-threads=1 2>&1 | Out-File -FilePath $LogFile -Append -Encoding utf8
    if ($LASTEXITCODE -ne 0) { throw "[$Name] indicatrix-net training run failed (exit $LASTEXITCODE)" }
}

# The last `"executable"` path in a `cargo --message-format=json` log, JSON-unescaped
# (Windows paths carry doubled backslashes there); `$null` when none is present or the
# file does not exist.
function Get-NetTestExecutable([string]$JsonPath) {
    if (-not (Test-Path $JsonPath)) { return $null }
    $m = [regex]::Matches((Get-Content -Raw -LiteralPath $JsonPath), '"executable":"([^"]+)"')
    if ($m.Count -eq 0) { return $null }
    $path = $m[$m.Count - 1].Groups[1].Value -replace '\\\\', '\'
    if (Test-Path $path) { return $path }
    return $null
}

# Keyed on the actual -Target value (not just "does this include worker"): an `all`
# build and a `worker`-only build both "include worker", but they select different
# package sets (`all` also builds `indicatrix-cut`), so they'd otherwise collide on the
# same `windows-<cpu>-<gpu>.profdata` path despite having different crate hashes --
# the same crate-hash mismatch an instrumented/optimised package-set split would cause,
# just for -Target. Only the combined `all` layout keeps the original,
# un-prefixed path (back-compat with already-collected profiles); `cut`-only and
# `worker`-only each get their own subdirectory.
function Get-SharedProfilePath([string]$PgoProfileDir, [string]$TargetKey, [string]$Name) {
    if ($TargetKey -eq 'all') {
        return Join-Path $PgoProfileDir "windows-$Name.profdata"
    }
    return Join-Path (Join-Path $PgoProfileDir $TargetKey) "windows-$Name.profdata"
}

# --- Profile-applied verification: the merged profile must cover the crate hash the
#     optimised build just produced, i.e. the instrumented and optimised builds must
#     produce the same crate hash or profile-use silently finds nothing to apply.
#     Best-effort: a parsing miss (this LLVM/rustc version's exact
#     symbol text not matching the regex below) prints a warning and returns rather than
#     failing the whole build over a diagnostic script's own regex, but a CONFIRMED
#     mismatch between the two crate-hash tokens is a hard failure.
# Returns $true when the crate-hash match was CONFIRMED for EVERY checked crate
# (`indicatrix` and `indicatrix-cut-core`), $false when any could not be verified (a
# parsing miss -- a warning is printed); throws on a confirmed mismatch in either.
# `indicatrix-cut-core` is trained by its own `pgo_train` build with a narrower package
# and feature set (see Get-CutCoreFeatures), so its crate hash is checked separately: a
# feature-unification difference there would otherwise apply nothing to it while the
# missing-function tolerance hid the loss.
function Test-ProfileHashApplied([string]$Profdata, [string]$Tdir, [string]$Name) {
    $coreOk = Test-CrateHashToken -Profdata $Profdata -Tdir $Tdir -Name $Name `
        -FuncName 'trace_spectral_ray_with_finish_soa' -TokenPattern 'Cs[0-9a-zA-Z]+_10indicatrix' `
        -PackageDir 'indicatrix' -RlibStem 'indicatrix'
    $cutCoreOk = Test-CrateHashToken -Profdata $Profdata -Tdir $Tdir -Name $Name `
        -FuncName 'resolve_after_edit' -TokenPattern 'Cs[0-9a-zA-Z]+_19indicatrix_cut_core' `
        -PackageDir 'indicatrix-cut-core' -RlibStem 'indicatrix_cut_core'
    return ($coreOk -and $cutCoreOk)
}

# Compares one crate's hash token between the merged profile and the newest optimised
# rlib of that crate. `$PackageDir` is the package's directory name under the cargo
# build-dir layout, `$RlibStem` the rlib's crate-name stem (underscores).
function Test-CrateHashToken([string]$Profdata, [string]$Tdir, [string]$Name, [string]$FuncName, [string]$TokenPattern, [string]$PackageDir, [string]$RlibStem) {
    $profLine = & $profdata show --all-functions $Profdata 2>&1 | Select-String -SimpleMatch $FuncName | Select-Object -First 1
    if (-not $profLine) {
        Write-Warning "[$Name] '$FuncName' not found in $Profdata -- cannot verify the $PackageDir crate-hash match (training may not have recorded it; check the training run's own output)."
        return $false
    }
    $profToken = [regex]::Match($profLine.Line, $TokenPattern).Value

    # The compiled library is `lib<stem>-<hash>.rlib` in one of two layouts: the new
    # cargo build-dir layout (`build\<package>\<hash>\out\`, current nightly) or the
    # classic one (`deps\`, older/stable cargo). More than one may exist (the
    # instrumented build's, or a different feature set); take the newest across both --
    # the optimised build that just finished.
    $releaseRoot = Join-Path $Tdir "$hostTargetelease"
    $rlib = @(
        Get-ChildItem -Path (Join-Path $releaseRoot "build\$PackageDir\*\out\lib$RlibStem-*.rlib") -ErrorAction SilentlyContinue
        Get-ChildItem -Path (Join-Path $releaseRoot "deps\lib$RlibStem-*.rlib") -ErrorAction SilentlyContinue
    ) | Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not $rlib) {
        Write-Warning "[$Name] no lib$RlibStem-*.rlib found under $releaseRootuild\$PackageDir\*\out or $releaseRoot\deps -- cannot verify the $PackageDir crate-hash match."
        return $false
    }
    $nmLine = & $llvmNm $rlib.FullName 2>&1 | Select-String -SimpleMatch $FuncName | Select-Object -First 1
    if (-not $nmLine) {
        Write-Warning "[$Name] '$FuncName' not found in $($rlib.FullName) -- cannot verify the $PackageDir crate-hash match."
        return $false
    }
    $nmToken = [regex]::Match($nmLine.Line, $TokenPattern).Value

    if (-not $profToken -or -not $nmToken) {
        Write-Warning "[$Name] could not extract a '$TokenPattern' crate-hash token from one or both symbols -- cannot verify the $PackageDir crate-hash match. profile: '$($profLine.Line)' rlib: '$($nmLine.Line)'"
        return $false
    }
    if ($profToken -ne $nmToken) {
        throw "[$Name] profile did NOT apply to ${PackageDir}: the merged profile's crate-hash token ($profToken) differs from the optimised build's rlib ($nmToken) for '$FuncName'. The instrumented and optimised builds resolved '$FuncName' to two different compiled crates (their package set, target, target-dir or feature list differ), so profile-use silently found nothing to apply."
    }
    Write-Host "  [$Name] crate-hash check OK ($PackageDir): $FuncName -> $profToken"
    return $true
}

# Scans an optimised build's log for the two warning classes -pgo-warn-missing-function
# and profile staleness produce. See -MaxMissingFunctionWarnings' doc comment for why
# "no profile data available" is thresholded rather than required to be zero, and why
# "hash mismatch" is not.
function Test-ProfileWarnings([string]$LogFile, [string]$Name, [int]$MaxMissing, [bool]$HashVerified) {
    if (-not (Test-Path $LogFile)) { return }
    $lines = Get-Content -LiteralPath $LogFile
    $hashMismatch = @($lines | Select-String -SimpleMatch 'hash mismatch')
    $noProfile = @($lines | Select-String -SimpleMatch 'no profile data available')

    if ($hashMismatch.Count -gt 0) {
        $sample = ($hashMismatch | Select-Object -First 5 | ForEach-Object { $_.Line }) -join "`n"
        throw "[$Name] $($hashMismatch.Count) 'hash mismatch' warning(s) in $LogFile. With identical --target/--target-dir/package-set/--features between the instrumented and optimised builds and no source edits between the two steps, this should be zero -- a function's compiled code differs from what was profiled. First few:`n$sample"
    }
    if ($HashVerified) {
        # The crate-hash check already proved the profile applied; the count is only
        # code the training run never executes (the whole GUI for indicatrix-cut).
        Write-Host "  [$Name] $($noProfile.Count) function(s) without profile data (untrained code; the crate-hash check confirmed the profile applied)"
        return
    }
    if ($noProfile.Count -gt $MaxMissing) {
        throw "[$Name] $($noProfile.Count) 'no profile data available' warning(s) in $LogFile, over the configured threshold ($MaxMissing, -MaxMissingFunctionWarnings), and the crate-hash check could not verify the profile. Expected in bulk for indicatrix-cut/indicatrix-worker's own GUI/server code that pgo_train never runs (indicatrix-cut measured ~66,000 with a fully applied profile) -- but without the crate-hash confirmation a count this high may mean the profile silently stopped applying again. Check the crate-hash warning printed just above."
    }
    Write-Host "  [$Name] $($noProfile.Count) function(s) without profile data (crate-hash check could not verify; under the $MaxMissing threshold)"
}

if ($Parallel) {
    Write-Host "`n==> Prefetching crate dependencies to avoid concurrent registry locks..." -ForegroundColor Cyan
    & cargo fetch --target $hostTarget

    $jobs = @()

    # NOTE: this scriptblock runs in a separate process (Start-Job), so it has no access
    # to the outer script's functions/variables beyond what -ArgumentList passes in --
    # everything it needs (the feature string, package list, shared-profile path) is
    # therefore computed in the DISPATCH loop below (which does have access to
    # Get-PgoFeatures/Get-SharedProfilePath) and handed in ready-made. Verification
    # (Test-ProfileWarnings/Test-ProfileHashApplied) likewise runs after Wait-Job, back
    # in this process, rather than inside the job.
    $jobScript = {
        param($root, $hostTarget, $OutDir, $SkipTrain, $CpuName, $TargetCpu, $SimdCap, $GpuName, $logfile, $profdata, $unitSep, $features, $trainPkgList, $binList, $sharedMerged, $cutCoreFeatures)

        $ErrorActionPreference = 'Stop'
        $null > $logfile # Truncate/create log file

        function Invoke-Checked {
            param([string]$Description, [scriptblock]$Command)
            "==> $Description" | Out-File -FilePath $logfile -Append -Encoding utf8
            try {
                & $Command 2>&1 | Out-File -FilePath $logfile -Append -Encoding utf8
                if ($LASTEXITCODE -ne 0) { throw "$Description failed (exit $LASTEXITCODE)" }
            }
            catch {
                "ERROR: $_" | Out-File -FilePath $logfile -Append -Encoding utf8
                throw
            }
        }

        $name = "$CpuName-$GpuName"
        $tdir = Join-Path $root "target\pgo-$name"
        $pdir = Join-Path $tdir 'profiles'
        $localMerged = Join-Path $tdir 'merged.profdata'
        $cpuFlag = @('-C', "target-cpu=$TargetCpu")
        $pkgArgs = $trainPkgList | ForEach-Object { '-p', $_ }

        Set-Location $root
        Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue

        if (-not $SkipTrain -or -not (Test-Path $sharedMerged)) {
            if (Test-Path $pdir) { Remove-Item -Recurse -Force $pdir }
            New-Item -ItemType Directory -Force $pdir | Out-Null

            # This build must select the IDENTICAL packages/target/target-dir/features as
            # the profile-use build below (only RUSTFLAGS differs) so both produce the
            # same crate hashes.
            $env:CARGO_ENCODED_RUSTFLAGS = (@($cpuFlag) + @('-C', "profile-generate=$pdir")) -join $unitSep
            Invoke-Checked "[$name] instrumented build (packages: $($trainPkgList -join ', '); features: $features)" {
                cargo build --release --target $hostTarget --target-dir $tdir @pkgArgs --bins --example pgo_train --features $features
            }

            $env:LLVM_PROFILE_FILE = Join-Path $pdir 'train-%p-%m.profraw'
            if ($SimdCap) { $env:INDICATRIX_SIMD = $SimdCap } else { Remove-Item Env:INDICATRIX_SIMD -ErrorAction SilentlyContinue }
            $exe = Join-Path $tdir "$hostTarget\release\examples\pgo_train.exe"
            Invoke-Checked "[$name] training run (INDICATRIX_SIMD='$SimdCap')" { & $exe }

            # indicatrix-cut-core's own training binary -- a separate, isolated
            # `cargo build` invocation (see .DESCRIPTION's step 2b for why this can
            # never share `indicatrix`'s own `-p ... --example pgo_train`
            # invocation above). Runs immediately after, so the shared
            # examples\pgo_train.exe path it is about to overwrite has already
            # served its purpose for indicatrix's own binary above.
            Invoke-Checked "[$name] instrumented build (indicatrix-cut-core, separate invocation; features: $cutCoreFeatures)" {
                cargo build --release --target $hostTarget --target-dir $tdir -p indicatrix-cut-core --example pgo_train --features $cutCoreFeatures
            }
            $cutCoreExe = Join-Path $tdir "$hostTarget\release\examples\pgo_train.exe"
            Invoke-Checked "[$name] indicatrix-cut-core training run" { & $cutCoreExe }

            # indicatrix-net's wire-protocol test, the third training binary (same set
            # as scripts/pgo-bolt-build.sh; see Invoke-NetTraining, which a job cannot
            # call). The JSON build log goes to a file so the test binary path survives.
            $netJson = Join-Path $tdir 'net-test-build.json'
            Invoke-Checked "[$name] instrumented build (indicatrix-net scene_roundtrip test)" {
                cargo test --release --target $hostTarget --target-dir $tdir -p indicatrix-net --features render --test scene_roundtrip --no-run --message-format=json > $netJson
            }
            $netMatches = [regex]::Matches((Get-Content -Raw -LiteralPath $netJson), '"executable":"([^"]+)"')
            if ($netMatches.Count -eq 0) { throw "[$name] could not locate the instrumented scene_roundtrip test binary in $netJson" }
            $netExe = $netMatches[$netMatches.Count - 1].Groups[1].Value -replace '\\\\', '\'
            $env:LLVM_PROFILE_FILE = Join-Path $pdir 'net-%p-%m.profraw'
            Invoke-Checked "[$name] indicatrix-net wire-protocol training run" { & $netExe --test-threads=1 }

            $raw = @(Get-ChildItem -Path $pdir -Filter '*.profraw')
            if ($raw.Count -eq 0) { throw "no .profraw files were written to $pdir" }
            Invoke-Checked "[$name] merging $($raw.Count) profile(s)" {
                & $profdata merge -o $localMerged $pdir
            }

            $sharedMergedDir = Split-Path -Parent $sharedMerged
            if (-not (Test-Path $sharedMergedDir)) { New-Item -ItemType Directory -Force $sharedMergedDir | Out-Null }
            Copy-Item -Force $localMerged $sharedMerged
            $activeProfile = $sharedMerged
        }
        else {
            "[$name] reusing shared profile $sharedMerged" | Out-File -FilePath $logfile -Append -Encoding utf8
            $activeProfile = $sharedMerged
        }

        Remove-Item Env:LLVM_PROFILE_FILE -ErrorAction SilentlyContinue
        Remove-Item Env:INDICATRIX_SIMD -ErrorAction SilentlyContinue
        # Same package/target/target-dir/features as the instrumented build above,
        # byte-for-byte, so both builds produce the same crate hashes.
        # `-pgo-warn-missing-function` surfaces every function profile-use can't find
        # data for, in $logfile, for Test-ProfileWarnings to scan after this job completes.
        $env:CARGO_ENCODED_RUSTFLAGS = (@($cpuFlag) + @('-C', "profile-use=$activeProfile", '-C', 'llvm-args=-pgo-warn-missing-function')) -join $unitSep

        Invoke-Checked "[$name] PGO release build (packages: $($trainPkgList -join ', '); features: $features)" {
            cargo build --release --target $hostTarget --target-dir $tdir @pkgArgs --bins --example pgo_train --features $features
        }

        if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Force $OutDir | Out-Null }
        $releaseDir = Join-Path $tdir "$hostTarget\release"
        foreach ($bin in $binList) {
            $src = Join-Path $releaseDir "$bin.exe"
            if (-not (Test-Path $src)) { throw "expected binary missing: $src" }
            $dst = Join-Path $OutDir "$bin-win-$name.exe"
            Copy-Item -Force $src $dst
            $size = ((Get-Item $dst).Length / 1MB).ToString("N1")
            "   $dst ($size MB)" | Out-File -FilePath $logfile -Append -Encoding utf8
        }
    }

    foreach ($c in $cpuList) {
        $targetCpu = switch ($c) {
            'avx512' { 'x86-64-v4' }
            'avx2' { 'x86-64-v3' }
            'scalar' { 'x86-64-v2' }
            'native' { 'native' }
        }
        $simdCap = switch ($c) {
            'avx512' { 'avx512' }
            'avx2' { 'avx2' }
            'scalar' { 'scalar' }
            'native' { '' }
        }
        foreach ($g in $gpuList) {
            $name = "$c-$g"
            $tdir = Join-Path $root "target\pgo-$name"
            if (-not (Test-Path $tdir)) { New-Item -ItemType Directory -Force $tdir | Out-Null }
            $logfile = Join-Path $tdir "build.log"
            $features = Get-PgoFeatures $includeCut $includeWorker $g
            $cutCoreFeatures = Get-CutCoreFeatures $g
            $sharedMerged = Get-SharedProfilePath $pgoProfileDir $Target $name

            Write-Host "==> [$name] Dispatched build in background (log: $logfile)"

            $jobArgs = @(
                $root, $hostTarget, $OutDir, $SkipTrain.IsPresent,
                $c, $targetCpu, $simdCap, $g, $logfile, $profdata, $unitSep, $features, $trainPkgList, $binList, $sharedMerged, $cutCoreFeatures
            )
            $job = Start-Job -Name $name -ScriptBlock $jobScript -ArgumentList $jobArgs
            $jobs += @{ Job = $job; Name = $name; Log = $logfile; Tdir = $tdir; Profdata = $sharedMerged }
        }
    }

    Write-Host "`n==> Waiting for $($jobs.Count) background builds to complete..." -ForegroundColor Cyan

    $failed = $false
    foreach ($j in $jobs) {
        $completed = Wait-Job $j.Job
        if ($completed.State -eq 'Completed') {
            Write-Host "  [SUCCESS] $($j.Name)" -ForegroundColor Green
            try {
                # Crate-hash check FIRST: it is the real proof the profile applied;
                # the missing-function count is only fatal when it could not verify.
                $verified = Test-ProfileHashApplied -Profdata $j.Profdata -Tdir $j.Tdir -Name $j.Name
                Test-ProfileWarnings -LogFile $j.Log -Name $j.Name -MaxMissing $MaxMissingFunctionWarnings -HashVerified ([bool]$verified)
            }
            catch {
                Write-Host "  [FAILED]  $($j.Name) profile verification: $_" -ForegroundColor Red
                $failed = $true
            }
        }
        else {
            Write-Host "  [FAILED]  $($j.Name) (see tail of $($j.Log))" -ForegroundColor Red
            $failed = $true
            if (Test-Path $j.Log) {
                Get-Content $j.Log -Tail 20 | ForEach-Object { Write-Host "    $_" -ForegroundColor DarkGray }
            }
        }
        Receive-Job $completed | Out-Null
        Remove-Job $completed
    }

    if ($failed) { throw "One or more parallel builds failed." }

}
else {
    # -------------------------------------------------------------------------
    # Sequential Execution
    # -------------------------------------------------------------------------
    function Build-PgoCombination {
        param([string]$CpuName, [string]$TargetCpu, [string]$SimdCap, [string]$GpuName)

        $name = "$CpuName-$GpuName"
        $tdir = Join-Path $root "target\pgo-$name"
        $pdir = Join-Path $tdir 'profiles'
        $localMerged = Join-Path $tdir 'merged.profdata'
        $sharedMerged = Get-SharedProfilePath $pgoProfileDir $Target $name
        $cpuFlag = @('-C', "target-cpu=$TargetCpu")
        $logfile = Join-Path $tdir 'build.log'
        $null > $logfile # Truncate/create log file, so Test-ProfileWarnings has something to scan

        # Same feature string, same package set (indicatrix + whichever of
        # indicatrix-cut/indicatrix-worker -Target selected) for BOTH cargo invocations
        # below -- see Get-PgoFeatures' doc comment.
        $features = Get-PgoFeatures $includeCut $includeWorker $GpuName
        $pkgArgs = $trainPkgList | ForEach-Object { '-p', $_ }

        $savedEncodedRustflags = $env:CARGO_ENCODED_RUSTFLAGS
        $savedRustflags = $env:RUSTFLAGS
        $savedSimd = $env:INDICATRIX_SIMD
        $savedProfileFile = $env:LLVM_PROFILE_FILE

        function Invoke-Checked {
            param([string]$Description, [scriptblock]$Command, [string]$LogFile)
            Write-Host "==> $Description" -ForegroundColor Cyan
            "==> $Description" | Out-File -FilePath $LogFile -Append -Encoding utf8
            & $Command *>&1 | Tee-Object -FilePath $LogFile -Append
            if ($LASTEXITCODE -ne 0) { throw "$Description failed (exit $LASTEXITCODE)" }
        }

        try {
            Push-Location $root
            Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue

            if (-not $SkipTrain -or -not (Test-Path $sharedMerged)) {
                if (Test-Path $pdir) { Remove-Item -Recurse -Force $pdir }
                New-Item -ItemType Directory -Force $pdir | Out-Null

                $env:CARGO_ENCODED_RUSTFLAGS = (@($cpuFlag) + @('-C', "profile-generate=$pdir")) -join $unitSep
                Invoke-Checked "[$name] instrumented build (packages: $($trainPkgList -join ', '); features: $features)" {
                    cargo build --release --target $hostTarget --target-dir $tdir @pkgArgs --bins --example pgo_train --features $features
                } $logfile

                $env:LLVM_PROFILE_FILE = Join-Path $pdir 'train-%p-%m.profraw'
                if ($SimdCap) { $env:INDICATRIX_SIMD = $SimdCap } else { Remove-Item Env:INDICATRIX_SIMD -ErrorAction SilentlyContinue }
                $exe = Join-Path $tdir "$hostTarget\release\examples\pgo_train.exe"
                Invoke-Checked "[$name] training run (INDICATRIX_SIMD='$SimdCap')" { & $exe } $logfile

                # indicatrix-cut-core's own training binary -- see .DESCRIPTION's
                # step 2b and `Invoke-CutCoreTraining`'s own doc comment for why
                # this is always a separate, isolated `cargo build` invocation.
                Invoke-CutCoreTraining $tdir $name $GpuName $logfile

                # indicatrix-net's wire-protocol test, the third training binary
                # (same set as scripts/pgo-bolt-build.sh).
                Invoke-NetTraining $tdir $name $logfile

                $raw = @(Get-ChildItem -Path $pdir -Filter '*.profraw')
                if ($raw.Count -eq 0) { throw "no .profraw files were written to $pdir" }
                Invoke-Checked "[$name] merging $($raw.Count) profile(s)" {
                    & $profdata merge -o $localMerged $pdir
                } $logfile

                $sharedMergedDir = Split-Path -Parent $sharedMerged
                if (-not (Test-Path $sharedMergedDir)) { New-Item -ItemType Directory -Force $sharedMergedDir | Out-Null }
                Copy-Item -Force $localMerged $sharedMerged
                $activeProfile = $sharedMerged
            }
            else {
                Write-Host "[$name] reusing shared profile $sharedMerged"
                $activeProfile = $sharedMerged
            }

            Remove-Item Env:LLVM_PROFILE_FILE -ErrorAction SilentlyContinue
            Remove-Item Env:INDICATRIX_SIMD -ErrorAction SilentlyContinue
            # Same package/target/target-dir/features as the instrumented build above,
            # byte-for-byte, so both builds produce the same crate hashes.
            # `-pgo-warn-missing-function` surfaces every function profile-use can't find
            # data for, in $logfile, for Test-ProfileWarnings to scan below.
            $env:CARGO_ENCODED_RUSTFLAGS = (@($cpuFlag) + @('-C', "profile-use=$activeProfile", '-C', 'llvm-args=-pgo-warn-missing-function')) -join $unitSep

            Invoke-Checked "[$name] PGO release build (packages: $($trainPkgList -join ', '); features: $features)" {
                cargo build --release --target $hostTarget --target-dir $tdir @pkgArgs --bins --example pgo_train --features $features
            } $logfile

            # Crate-hash check FIRST -- see the parallel branch's identical note.
            $verified = Test-ProfileHashApplied -Profdata $activeProfile -Tdir $tdir -Name $name
            Test-ProfileWarnings -LogFile $logfile -Name $name -MaxMissing $MaxMissingFunctionWarnings -HashVerified ([bool]$verified)

            if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Force $OutDir | Out-Null }
            $releaseDir = Join-Path $tdir "$hostTarget\release"
            foreach ($bin in $binList) {
                $src = Join-Path $releaseDir "$bin.exe"
                if (-not (Test-Path $src)) { throw "expected binary missing: $src" }
                $dst = Join-Path $OutDir "$bin-win-$name.exe"
                Copy-Item -Force $src $dst
                Write-Host ("   {0}  ({1:N1} MB)" -f $dst, ((Get-Item $dst).Length / 1MB)) -ForegroundColor Green
            }
            Write-Host ""
        }
        finally {
            Pop-Location
            if ($null -ne $savedEncodedRustflags) { $env:CARGO_ENCODED_RUSTFLAGS = $savedEncodedRustflags } else { Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue }
            if ($null -ne $savedRustflags) { $env:RUSTFLAGS = $savedRustflags } else { Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue }
            if ($null -ne $savedSimd) { $env:INDICATRIX_SIMD = $savedSimd } else { Remove-Item Env:INDICATRIX_SIMD -ErrorAction SilentlyContinue }
            if ($null -ne $savedProfileFile) { $env:LLVM_PROFILE_FILE = $savedProfileFile } else { Remove-Item Env:LLVM_PROFILE_FILE -ErrorAction SilentlyContinue }
        }
    }

    foreach ($c in $cpuList) {
        $targetCpu = switch ($c) {
            'avx512' { 'x86-64-v4' }
            'avx2' { 'x86-64-v3' }
            'scalar' { 'x86-64-v2' }
            'native' { 'native' }
        }
        $simdCap = switch ($c) {
            'avx512' { 'avx512' }
            'avx2' { 'avx2' }
            'scalar' { 'scalar' }
            'native' { '' }
        }
        foreach ($g in $gpuList) {
            Build-PgoCombination -CpuName $c -TargetCpu $targetCpu -SimdCap $simdCap -GpuName $g
        }
    }
}

Write-Host "`nAll requested PGO builds are in $OutDir" -ForegroundColor Green
