<#
.SYNOPSIS
  Profile-guided-optimization (PGO) release builds of indicatrix-cut and/or indicatrix-worker
  on Windows, over CPU axes.

    CPU:  avx512 -> -C target-cpu=x86-64-v4 (AVX-512; Zen 4 / Skylake-X or newer)
          avx2   -> -C target-cpu=x86-64-v3 (AVX2+FMA+BMI baseline; Haswell/Zen or newer)
          scalar -> -C target-cpu=x86-64-v2 (SSE4.2 baseline; standard non-AVX fallback)
          native -> -C target-cpu=native (host CPU instruction set)
    GPU:  gpu    -> indicatrix-cut/gpu + indicatrix-worker/gpu (wgpu megakernel, CPU fallback per frame)
          cpu    -> no gpu feature at all (pure CPU tracer binaries)

.DESCRIPTION
  Per combination, in its own target directory (target\pgo-<cpu>-<gpu>):
    1. build the training example `pgo_train` instrumented with -C profile-generate
    2. run it (CPU tracer, denoiser, tone-map, meet-point solver; no GPU)
    3. merge the .profraw files with llvm-profdata
    4. rebuild the selected binaries (-Target) with -C profile-use=<merged profile>
    5. copy the executables to <OutDir> as <binary>-win-<cpu>-<gpu>.exe

.PARAMETER Cpu
  avx512, avx2, scalar, native, or all (default all).
.PARAMETER Gpu
  gpu, cpu, or both (default both).
.PARAMETER Target
  worker, cut, or all (default all). `indicatrix-worker`'s training always enables
  `indicatrix`'s `hdr` feature (its own render path always has it on); a `cut`-only
  build trains without it, and its profiles are kept in a separate `pgo-profile\cut\`
  subdirectory so they never mix with (or overwrite) worker-trained profiles at
  `pgo-profile\windows-<cpu>-<gpu>.profdata`. Building `all` (the default) keeps the
  original combined layout: one shared profile per combination, feeding both binaries.
.PARAMETER OutDir
  Where the renamed executables are copied (default <repo>\bin).
.PARAMETER SkipTrain
  Reuse an existing windows-<cpu>-<gpu>.profdata from the pgo-profile directory.
.PARAMETER Parallel
  Execute all requested target combinations concurrently using background jobs.
.PARAMETER Native
  Appends an additional build combination targeting the host CPU (-C target-cpu=native).
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
    [switch]$Native
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$hostTarget = 'x86_64-pc-windows-msvc'
if (-not $OutDir) { $OutDir = Join-Path $root 'bin' }
$pgoProfileDir = Join-Path $root 'pgo-profile'

$includeWorker = $Target -in @('all', 'worker')
$includeCut = $Target -in @('all', 'cut')
$pkgList = @()
if ($includeCut) { $pkgList += 'indicatrix-cut' }
if ($includeWorker) { $pkgList += 'indicatrix-worker' }
$binList = $pkgList

# --- llvm-profdata from the active toolchain --------------------------------------
$sysroot = (& rustc --print sysroot).Trim()
$profdata = Join-Path $sysroot "lib\rustlib\$hostTarget\bin\llvm-profdata.exe"
if (-not (Test-Path $profdata)) {
    Write-Host "llvm-tools-preview not installed; adding it via rustup..."
    & rustup component add llvm-tools-preview
    if (-not (Test-Path $profdata)) { throw "llvm-profdata.exe not found at $profdata" }
}
$installedTargets = & rustup target list --installed
if (-not ($installedTargets -contains $hostTarget)) { & rustup target add $hostTarget }

$cpuList = if ($Cpu -in @('all', 'both')) { @('avx512', 'avx2', 'scalar') } else { @($Cpu) }
if ($Native -and -not ($cpuList -contains 'native')) {
    $cpuList += 'native'
}
$gpuList = if ($Gpu -in @('all', 'both')) { @('gpu', 'cpu') } else { @($Gpu) }

# Feature lists depend on which binaries are selected: `indicatrix-cut/gpu` only
# matters when `cut` is being built, `indicatrix-worker/gpu`/`indicatrix-worker/worker`
# only when `worker` is. `hdr` (an `indicatrix` training feature, not an app feature)
# is included whenever `worker` is selected, since its render path always has it on;
# a `cut`-only build trains without it, matching indicatrix-cut's own feature set.
function Get-TrainFeatures([bool]$IncludeWorker, [string]$GpuName) {
    $features = @('serde')
    if ($IncludeWorker) { $features += 'hdr' }
    if ($GpuName -eq 'gpu') { $features += 'gpu' }
    return ($features -join ',')
}

function Get-AppFeatures([bool]$IncludeCut, [bool]$IncludeWorker, [string]$GpuName) {
    $features = @()
    if ($IncludeCut -and $GpuName -eq 'gpu') { $features += 'indicatrix-cut/gpu' }
    if ($IncludeWorker) {
        if ($GpuName -eq 'gpu') { $features += 'indicatrix-worker/gpu' }
        $features += 'indicatrix-worker/worker'
    }
    if ($features.Count -eq 0) { return @() }
    return @('--features', ($features -join ','))
}

function Get-SharedProfilePath([string]$PgoProfileDir, [bool]$IncludeWorker, [string]$Name) {
    if ($IncludeWorker) {
        return Join-Path $PgoProfileDir "windows-$Name.profdata"
    }
    $cutDir = Join-Path $PgoProfileDir 'cut'
    return Join-Path $cutDir "windows-$Name.profdata"
}

if ($Parallel) {
    Write-Host "`n==> Prefetching crate dependencies to avoid concurrent registry locks..." -ForegroundColor Cyan
    & cargo fetch --target $hostTarget

    $jobs = @()

    $jobScript = {
        param($root, $hostTarget, $OutDir, $pgoProfileDir, $SkipTrain, $CpuName, $TargetCpu, $SimdCap, $GpuName, $logfile, $profdata, $includeWorker, $includeCut, $pkgList, $binList)

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
        $sharedMerged = if ($includeWorker) { Join-Path $pgoProfileDir "windows-$name.profdata" } else { Join-Path (Join-Path $pgoProfileDir 'cut') "windows-$name.profdata" }
        $cpuFlag = "-C target-cpu=$TargetCpu"

        $trainFeaturesList = @('serde')
        if ($includeWorker) { $trainFeaturesList += 'hdr' }
        if ($GpuName -eq 'gpu') { $trainFeaturesList += 'gpu' }
        $trainFeatures = ($trainFeaturesList -join ',')

        $appFeaturesList = @()
        if ($includeCut -and $GpuName -eq 'gpu') { $appFeaturesList += 'indicatrix-cut/gpu' }
        if ($includeWorker) {
            if ($GpuName -eq 'gpu') { $appFeaturesList += 'indicatrix-worker/gpu' }
            $appFeaturesList += 'indicatrix-worker/worker'
        }
        $appFeatures = if ($appFeaturesList.Count -gt 0) { @('--features', ($appFeaturesList -join ',')) } else { @() }

        Set-Location $root

        if (-not $SkipTrain -or -not (Test-Path $sharedMerged)) {
            if (Test-Path $pdir) { Remove-Item -Recurse -Force $pdir }
            New-Item -ItemType Directory -Force $pdir | Out-Null

            $env:RUSTFLAGS = "$cpuFlag -C profile-generate=$pdir"
            Invoke-Checked "[$name] instrumented build of pgo_train (indicatrix features: $trainFeatures)" {
                cargo build --release --target $hostTarget --target-dir $tdir `
                    -p indicatrix --features $trainFeatures --example pgo_train
            }

            $env:LLVM_PROFILE_FILE = Join-Path $pdir 'train-%p-%m.profraw'
            if ($SimdCap) { $env:INDICATRIX_SIMD = $SimdCap } else { Remove-Item Env:INDICATRIX_SIMD -ErrorAction SilentlyContinue }
            $exe = Join-Path $tdir "$hostTarget\release\examples\pgo_train.exe"
            Invoke-Checked "[$name] training run (INDICATRIX_SIMD='$SimdCap')" { & $exe }

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

        $env:RUSTFLAGS = "$cpuFlag -C profile-use=$activeProfile"
        Remove-Item Env:LLVM_PROFILE_FILE -ErrorAction SilentlyContinue
        Remove-Item Env:INDICATRIX_SIMD -ErrorAction SilentlyContinue

        $pkgArgs = $pkgList | ForEach-Object { '-p', $_ }
        $featureDesc = if ($appFeatures.Count -gt 1) { $appFeatures[1] } else { '(none)' }
        Invoke-Checked "[$name] PGO release build of $($pkgList -join ', ') ($featureDesc)" {
            cargo build --release --target $hostTarget --target-dir $tdir @pkgArgs @appFeatures
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

            Write-Host "==> [$name] Dispatched build in background (log: $logfile)"

            $jobArgs = @(
                $root, $hostTarget, $OutDir, $pgoProfileDir, $SkipTrain.IsPresent,
                $c, $targetCpu, $simdCap, $g, $logfile, $profdata, $includeWorker, $includeCut, $pkgList, $binList
            )
            $job = Start-Job -Name $name -ScriptBlock $jobScript -ArgumentList $jobArgs
            $jobs += @{ Job = $job; Name = $name; Log = $logfile }
        }
    }

    Write-Host "`n==> Waiting for $($jobs.Count) background builds to complete..." -ForegroundColor Cyan

    $failed = $false
    foreach ($j in $jobs) {
        $completed = Wait-Job $j.Job
        if ($completed.State -eq 'Completed') {
            Write-Host "  [SUCCESS] $($j.Name)" -ForegroundColor Green
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
        $sharedMerged = Get-SharedProfilePath $pgoProfileDir $includeWorker $name
        $cpuFlag = "-C target-cpu=$TargetCpu"

        $trainFeatures = Get-TrainFeatures $includeWorker $GpuName
        $appFeatures = Get-AppFeatures $includeCut $includeWorker $GpuName

        $savedRustflags = $env:RUSTFLAGS
        $savedSimd = $env:INDICATRIX_SIMD
        $savedProfileFile = $env:LLVM_PROFILE_FILE

        try {
            Push-Location $root
            if (-not $SkipTrain -or -not (Test-Path $sharedMerged)) {
                if (Test-Path $pdir) { Remove-Item -Recurse -Force $pdir }
                New-Item -ItemType Directory -Force $pdir | Out-Null

                function Invoke-Checked {
                    param([string]$Description, [scriptblock]$Command)
                    Write-Host "==> $Description" -ForegroundColor Cyan
                    & $Command
                    if ($LASTEXITCODE -ne 0) { throw "$Description failed (exit $LASTEXITCODE)" }
                }

                $env:RUSTFLAGS = "$cpuFlag -C profile-generate=$pdir"
                Invoke-Checked "[$name] instrumented build of pgo_train (indicatrix features: $trainFeatures)" {
                    cargo build --release --target $hostTarget --target-dir $tdir `
                        -p indicatrix --features $trainFeatures --example pgo_train
                }

                $env:LLVM_PROFILE_FILE = Join-Path $pdir 'train-%p-%m.profraw'
                if ($SimdCap) { $env:INDICATRIX_SIMD = $SimdCap } else { Remove-Item Env:INDICATRIX_SIMD -ErrorAction SilentlyContinue }
                $exe = Join-Path $tdir "$hostTarget\release\examples\pgo_train.exe"
                Invoke-Checked "[$name] training run (INDICATRIX_SIMD='$SimdCap')" { & $exe }

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
                Write-Host "[$name] reusing shared profile $sharedMerged"
                $activeProfile = $sharedMerged
            }

            $env:RUSTFLAGS = "$cpuFlag -C profile-use=$activeProfile"
            Remove-Item Env:LLVM_PROFILE_FILE -ErrorAction SilentlyContinue
            Remove-Item Env:INDICATRIX_SIMD -ErrorAction SilentlyContinue

            function Invoke-Checked {
                param([string]$Description, [scriptblock]$Command)
                Write-Host "==> $Description" -ForegroundColor Cyan
                & $Command
                if ($LASTEXITCODE -ne 0) { throw "$Description failed (exit $LASTEXITCODE)" }
            }

            $pkgArgs = $pkgList | ForEach-Object { '-p', $_ }
            $featureDesc = if ($appFeatures.Count -gt 1) { $appFeatures[1] } else { '(none)' }
            Invoke-Checked "[$name] PGO release build of $($pkgList -join ', ') ($featureDesc)" {
                cargo build --release --target $hostTarget --target-dir $tdir @pkgArgs @appFeatures
            }

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
