<#
.SYNOPSIS
  Build the release binaries and package them into a per-user MSI (and a portable ZIP).

.DESCRIPTION
  Produces two artifacts in packaging/out/:
    * Nib-<version>-x64.msi  — per-user installer, no admin prompt
    * Nib-<version>-x64.zip  — portable, for people who would rather not run an installer

  Both contain the same runtime set: the two executables, the two DLLs actually required at
  runtime, and the licence/privacy documents. The ~460 MB speech model is deliberately NOT
  bundled — it is separately licensed and is fetched on first run against a pinned SHA-256.

  Requires the WiX CLI (`dotnet tool install --global wix`). If it is missing, the ZIP is still
  produced and the script says so, rather than failing outright.
#>
param(
    [string]$Version = "0.1.1"
)
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $root "target\release"
# Licence/doc sources differ by layout: the monorepo keeps the public-facing docs in oss/, while
# the exported public repo has them at its root. Support both so `build-installer.ps1` works for
# an outside contributor who only ever sees the public repo.
$docs = if (Test-Path (Join-Path $root "oss\LICENSE")) { Join-Path $root "oss" } else { $root }
$out = Join-Path $PSScriptRoot "out"

Write-Host "==> Building release binaries"
Push-Location $root
try {
    & cargo build --release -p nib-core -p nib-asr-sidecar
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally { Pop-Location }

# Exactly what the app needs at runtime. nib-asr-sidecar.exe imports sherpa-onnx-c-api.dll, which
# imports onnxruntime.dll; nothing links the cxx-api or the providers_shared DLL, so they stay out.
$payload = @(
    "nib-core.exe",
    "nib-asr-sidecar.exe",
    "sherpa-onnx-c-api.dll",
    "onnxruntime.dll"
) | ForEach-Object { Join-Path $bin $_ }

foreach ($f in $payload) {
    if (-not (Test-Path $f)) { throw "missing build output: $f" }
}

# Guard: never ship the espeak-ng (GPLv3) build of sherpa-onnx. Same check as fetch-sherpa.ps1,
# repeated here because this is the last point before bytes reach a user.
$dll = Join-Path $bin "sherpa-onnx-c-api.dll"
$text = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($dll))
foreach ($marker in @("espeak-ng-data", "ESPEAK_DATA_PATH", "phonemize_eSpeak")) {
    if ($text.Contains($marker)) {
        throw "REFUSING TO PACKAGE: '$marker' found in $dll - this build links espeak-ng (GPLv3)."
    }
}
Write-Host "==> GPL check passed (no espeak-ng in the shipped runtime)"

New-Item -ItemType Directory -Force -Path $out | Out-Null

# ---- portable ZIP -----------------------------------------------------------------------------
$stage = Join-Path $out "stage"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
$payload | ForEach-Object { Copy-Item $_ $stage }
foreach ($d in @("LICENSE", "THIRD-PARTY-NOTICES.md", "PRIVACY.md", "README.md")) {
    $p = Join-Path $docs $d
    if (Test-Path $p) { Copy-Item $p $stage }
}
$zip = Join-Path $out "HoldToSpeak-$Version-x64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip
Write-Host "==> ZIP:  $zip"

# ---- installer (Inno Setup) --------------------------------------------------------------------
# Inno Setup rather than WiX: free for any use including commercial, permanently, with no
# maintenance-fee model (WiX v6+ requires the paid OSMF; v5 is MS-RL). Same choice VS Code makes.
$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1

if (-not $iscc) {
    Write-Warning "Inno Setup not found - skipping the installer (the ZIP above is still valid)."
    Write-Warning "Install it with: winget install --id JRSoftware.InnoSetup --exact"
    Remove-Item $stage -Recurse -Force
    exit 0
}

& $iscc /Qp `
    "/DAppVersion=$Version" `
    "/DBinDir=$bin" `
    "/DDocsDir=$docs" `
    (Join-Path $PSScriptRoot "nib.iss")
if ($LASTEXITCODE -ne 0) { throw "Inno Setup build failed ($LASTEXITCODE)" }

Remove-Item $stage -Recurse -Force
Write-Host "==> Setup: $(Join-Path $out "HoldToSpeak-$Version-x64-setup.exe")"
Write-Host ""
Write-Host "Both artifacts are UNSIGNED. SmartScreen will warn users until the binaries are signed"
Write-Host "(see the launch checklist); signing is a prerequisite for a public release, not a nicety."
