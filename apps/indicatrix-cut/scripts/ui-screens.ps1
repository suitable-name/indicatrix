#Requires -Version 7
<#
.SYNOPSIS
  Headless visual check of the indicatrix-cut desktop UI with slint-viewer.

.DESCRIPTION
  Finds every `export component X inherits Window` under ui/ and ui/components/, reads its
  declared min-width/min-height, runs `slint-viewer --check`, then renders each window at its
  declared minimum plus 1366x768, 1920x1080 and 2560x1440 (MainWindow also 900x900 and
  1024x640). A PNG larger than the requested size means the
  content tree enforces a minimum above the declared one (a finding). Rough Planner scenarios
  (results, loaded, cuts_mesh, running, save_rows) are rendered from patched ui/ copies at the
  declared minimum, 1366x768 and 1920x1080.

  Writes summary.md into the output folder. Exit code 1 when any check failed or any size
  finding exists, otherwise 0. Copies of ui/ go to <repo>/target/ui-screens/work/ and are
  removed at the end; the source tree is never modified.

.PARAMETER Only
  Render only this window (component name, e.g. RoughPlannerWindow). Rough Planner scenarios
  run only when this is empty or RoughPlannerWindow.

.PARAMETER OutDir
  Output folder. Default: <repo>/target/ui-screens/<yyyyMMdd-HHmmss>.
#>
[CmdletBinding()]
param(
    [string]$Only = '',
    [string]$OutDir = ''
)

$ErrorActionPreference = 'Stop'
$app = Split-Path -Parent $PSScriptRoot
$repo = Split-Path -Parent (Split-Path -Parent $app)
$uiSrc = Join-Path $app 'ui'
$fixtureScript = Join-Path $PSScriptRoot 'ui_fixtures/rough_planner.py'
$screensRoot = Join-Path $repo 'target/ui-screens'
$work = Join-Path $screensRoot 'work'
if (-not $OutDir) { $OutDir = Join-Path $screensRoot (Get-Date -Format 'yyyyMMdd-HHmmss') }

if (-not (Get-Command slint-viewer -ErrorAction SilentlyContinue)) {
    Write-Error 'slint-viewer is not on PATH (cargo install slint-viewer).'
    exit 2
}

$style = 'fluent-dark'
$commonSizes = @('1366x768', '1920x1080', '2560x1440')
$mainExtra = @('900x900', '1024x640')
$plannerSizes = @('1366x768', '1920x1080')
$scenarios = @('results', 'loaded', 'cuts_mesh', 'running', 'save_rows')

# Window discovery: component name, file (relative to ui/) and declared min size, from the .slint sources.
$windows = @()
$slintFiles = @(Get-ChildItem -Path $uiSrc -Filter '*.slint' -File) +
    @(Get-ChildItem -Path (Join-Path $uiSrc 'components') -Filter '*.slint' -File -ErrorAction SilentlyContinue)
foreach ($f in $slintFiles) {
    $lines = Get-Content -LiteralPath $f.FullName
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\s*export component (\w+) inherits Window\b') {
            $name = $Matches[1]
            $minW = $null; $minH = $null
            $end = [Math]::Min($lines.Count - 1, $i + 80)
            for ($j = $i + 1; $j -le $end; $j++) {
                # Direct children of the window sit one indent level (4 spaces) deep.
                if ($lines[$j] -match '^ {4}min-width:\s*(\d+(?:\.\d+)?)px;') { $minW = [int][Math]::Ceiling([double]$Matches[1]) }
                if ($lines[$j] -match '^ {4}min-height:\s*(\d+(?:\.\d+)?)px;') { $minH = [int][Math]::Ceiling([double]$Matches[1]) }
            }
            $rel = $f.FullName.Substring($uiSrc.Length).TrimStart('\', '/')
            $windows += [pscustomobject]@{ Name = $name; Rel = $rel; MinW = $minW; MinH = $minH }
        }
    }
}
if ($windows.Count -eq 0) { Write-Error "No 'inherits Window' components found under $uiSrc"; exit 2 }
if ($Only) {
    $windows = @($windows | Where-Object { $_.Name -eq $Only })
    if ($windows.Count -eq 0) { Write-Error "No window named '$Only'."; exit 2 }
}

# Fresh output and work folders.
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory -Force $work | Out-Null
New-Item -ItemType Directory -Force $OutDir | Out-Null

$checks = New-Object System.Collections.Generic.List[object]
$findings = New-Object System.Collections.Generic.List[string]
$failures = New-Object System.Collections.Generic.List[string]
$pngs = New-Object System.Collections.Generic.List[string]

function Get-PngSize([string]$path) {
    $b = [System.IO.File]::ReadAllBytes($path)
    if ($b.Length -lt 24) { return $null }
    $w = ([int]$b[16] -shl 24) -bor ([int]$b[17] -shl 16) -bor ([int]$b[18] -shl 8) -bor [int]$b[19]
    $h = ([int]$b[20] -shl 24) -bor ([int]$b[21] -shl 16) -bor ([int]$b[22] -shl 8) -bor [int]$b[23]
    return @($w, $h)
}

function Invoke-Check([string]$label, [string]$component, [string]$file) {
    $out = & slint-viewer --style $style --component $component --check $file 2>&1
    $ok = ($LASTEXITCODE -eq 0) -and (-not $out)
    $text = ($out | Select-Object -First 30 | ForEach-Object { "$_" }) -join "`n"
    $checks.Add([pscustomobject]@{ Label = $label; Ok = $ok; Text = $text })
    if (-not $ok) { Write-Host "CHECK FAILED: $label" -ForegroundColor Red }
    return $ok
}

function Invoke-Render([string]$label, [string]$component, [string]$file, [string]$size, [string]$png) {
    $out = & slint-viewer --style $style --component $component --size $size --screenshot $png $file 2>&1
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $png)) {
        $failures.Add("render failed: $label at $size " + (($out | Select-Object -First 3 | ForEach-Object { "$_" }) -join ' '))
        Write-Host "RENDER FAILED: $label $size" -ForegroundColor Red
        return
    }
    $pngs.Add((Split-Path -Leaf $png))
    $req = $size -split 'x'
    $got = Get-PngSize $png
    if ($got -and ($got[0] -gt [int]$req[0] -or $got[1] -gt [int]$req[1])) {
        $findings.Add("$label at ${size}: rendered $($got[0])x$($got[1]) > requested; content minimum exceeds declared minimum")
    }
}

foreach ($w in $windows) {
    $sizes = @()
    if ($w.MinW -and $w.MinH) { $sizes += "$($w.MinW)x$($w.MinH)" }
    else { $findings.Add("$($w.Name): no min-width/min-height found on the window; declared minimum not rendered") }
    if ($w.Name -eq 'MainWindow') { $sizes += $mainExtra }
    $sizes += $commonSizes
    $sizes = @($sizes | Select-Object -Unique)
    $label = $w.Name
    $file = Join-Path $uiSrc $w.Rel
    if (-not (Invoke-Check $label $w.Name $file)) { continue }
    foreach ($size in $sizes) {
        Invoke-Render $label $w.Name $file $size (Join-Path $OutDir "${label}_$size.png")
    }
}

# Rough Planner scenarios from the fixture generator.
$planner = $windows | Where-Object { $_.Name -eq 'RoughPlannerWindow' }
if ($planner -and (-not $Only -or $Only -eq 'RoughPlannerWindow')) {
    $fix = Join-Path $work 'planner'
    & python -I $fixtureScript $app $fix
    if ($LASTEXITCODE -ne 0) {
        $failures.Add('fixture generator failed (rough_planner.py); Rough Planner scenarios not rendered')
    }
    else {
        $psizes = @()
        if ($planner.MinW -and $planner.MinH) { $psizes += "$($planner.MinW)x$($planner.MinH)" }
        $psizes += $plannerSizes
        foreach ($sc in $scenarios) {
            $label = "RoughPlanner_$sc"
            $file = Join-Path $fix "$sc/ui/$($planner.Rel)"
            if (-not (Invoke-Check $label 'RoughPlannerWindow' $file)) { continue }
            foreach ($size in $psizes) {
                Invoke-Render $label 'RoughPlannerWindow' $file $size (Join-Path $OutDir "${label}_$size.png")
            }
        }
    }
}

# Work copies never stay behind.
if (Test-Path $work) { Remove-Item -Recurse -Force $work }

# summary.md
$failedChecks = @($checks | Where-Object { -not $_.Ok })
$sb = New-Object System.Text.StringBuilder
[void]$sb.AppendLine('# UI screens summary')
[void]$sb.AppendLine()
[void]$sb.AppendLine("Style: $style. Output: $OutDir")
[void]$sb.AppendLine()
[void]$sb.AppendLine("## Checks ($($checks.Count) run, $($failedChecks.Count) failed)")
foreach ($c in $checks) {
    [void]$sb.AppendLine("- $(if ($c.Ok) { 'OK  ' } else { 'FAIL' }) $($c.Label)")
    if (-not $c.Ok -and $c.Text) {
        foreach ($l in ($c.Text -split "`n")) { [void]$sb.AppendLine("    $l") }
    }
}
[void]$sb.AppendLine()
[void]$sb.AppendLine("## Render failures ($($failures.Count))")
foreach ($f in $failures) { [void]$sb.AppendLine("- $f") }
[void]$sb.AppendLine()
[void]$sb.AppendLine("## Size findings ($($findings.Count))")
foreach ($f in $findings) { [void]$sb.AppendLine("- $f") }
[void]$sb.AppendLine()
[void]$sb.AppendLine("## PNGs ($($pngs.Count))")
foreach ($p in ($pngs | Sort-Object)) { [void]$sb.AppendLine("- $p") }
Set-Content -LiteralPath (Join-Path $OutDir 'summary.md') -Value $sb.ToString()

Write-Host "Checks: $($checks.Count) run, $($failedChecks.Count) failed. Render failures: $($failures.Count). Size findings: $($findings.Count). PNGs: $($pngs.Count)."
Write-Host "Summary: $(Join-Path $OutDir 'summary.md')"
if ($failedChecks.Count -gt 0 -or $failures.Count -gt 0 -or $findings.Count -gt 0) { exit 1 }
exit 0
