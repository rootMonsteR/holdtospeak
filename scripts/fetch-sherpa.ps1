<#
.SYNOPSIS
  Fetch the GPL-free sherpa-onnx prebuilt libraries the ASR sidecar links against.

.DESCRIPTION
  IMPORTANT — this is a LICENSING control, not just a convenience.

  `sherpa-onnx-sys`'s build.rs downloads the DEFAULT prebuilt archive, whose
  `sherpa-onnx-c-api.dll` statically links **espeak-ng (GPLv3)** for text-to-speech (verified:
  the binary contains `ESPEAK_DATA_PATH`, `phonemize_eSpeak`, an `mbrola.dll` import). Nib ships
  closed-source paid tiers and requires permissive-only dependencies
  (docs/licenses/COMMERCIAL-USE-REGISTER.md), so that binary must never ship.

  Upstream also publishes a `-no-tts-` archive with espeak-ng excluded. This script downloads it
  and extracts it to vendor/, where .cargo/config.toml points SHERPA_ONNX_LIB_DIR — which makes
  build.rs use these libs and skip its own download entirely.

  Run once after cloning (and whenever SHERPA_VERSION changes).
#>
$ErrorActionPreference = "Stop"

$SherpaVersion = "1.13.5"   # must match the `sherpa-onnx` crate version in nib-asr-sidecar
$Name = "sherpa-onnx-v$SherpaVersion-win-x64-shared-MT-Release-no-tts-lib"
$Url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v$SherpaVersion/$Name.tar.bz2"

$root = Split-Path -Parent $PSScriptRoot
$vendor = Join-Path $root "vendor"
$dest = Join-Path $vendor $Name

if (Test-Path (Join-Path $dest "lib\sherpa-onnx-c-api.lib")) {
    Write-Host "sherpa-onnx $SherpaVersion (no-tts) already vendored at $dest"
    exit 0
}

New-Item -ItemType Directory -Force -Path $vendor | Out-Null
$archive = Join-Path $vendor "$Name.tar.bz2"
Write-Host "Downloading $Url ..."
Invoke-WebRequest -Uri $Url -OutFile $archive
Write-Host "Extracting ..."
# Use Windows' own bsdtar explicitly. `tar` on PATH may resolve to GNU tar (shipped with Git for
# Windows), which parses "C:\..." as a remote host:path spec and fails with
# "Cannot connect to C: resolve failed". bsdtar has handled drive letters since Win10 1803.
$sysTar = Join-Path $env:SystemRoot "System32\tar.exe"
$tarExe = if (Test-Path $sysTar) { $sysTar } else { "tar" }
& $tarExe -xjf $archive -C $vendor
if ($LASTEXITCODE -ne 0) { throw "extract failed ($tarExe exited $LASTEXITCODE)" }
Remove-Item $archive -Force

# Guard: fail loudly if a future archive ever regains espeak-ng, rather than silently shipping GPL.
$dll = Join-Path $dest "lib\sherpa-onnx-c-api.dll"
$text = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($dll))
foreach ($marker in @("espeak-ng-data", "ESPEAK_DATA_PATH", "phonemize_eSpeak")) {
    if ($text.Contains($marker)) {
        throw "GPL check FAILED: '$marker' found in $dll — this build links espeak-ng (GPLv3) and must not ship."
    }
}
Write-Host "OK: vendored $Name (GPL-free: no espeak-ng markers)"
