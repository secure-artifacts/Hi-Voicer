$ErrorActionPreference = "Stop"

$runtimeTag = "v1.13.2"
$runtimeName = "sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts"
$archiveName = "$runtimeName.tar.bz2"
$archiveSha256 = "15D10EC7AF9A8DDCE310BABC293307AEFDD25204A78A0F15684ECEBFA72DF132"
$url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/$runtimeTag/$archiveName"
$repoRoot = Split-Path -Parent $PSScriptRoot
$targetBin = Join-Path $repoRoot "src-tauri\resources\engines\sherpa\$runtimeTag\$runtimeName\bin"
$requiredFiles = @(
  "sherpa-onnx-offline.exe",
  "sherpa-onnx-offline-websocket-server.exe"
)

$ready = $true
foreach ($file in $requiredFiles) {
  if (-not (Test-Path -LiteralPath (Join-Path $targetBin $file))) {
    $ready = $false
  }
}
if ($ready) {
  Write-Host "Minimal Sherpa-ONNX $runtimeTag CPU runtime is already prepared."
  exit 0
}

$archivePath = Join-Path $env:TEMP $archiveName
$extractDir = Join-Path $env:TEMP "hi-voicer-sherpa-$runtimeTag"
Invoke-WebRequest -Uri $url -OutFile $archivePath
$actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
if ($actualHash -ne $archiveSha256) {
  throw "Sherpa-ONNX archive checksum mismatch. Expected $archiveSha256, got $actualHash."
}

if (Test-Path -LiteralPath $extractDir) {
  Remove-Item -LiteralPath $extractDir -Recurse -Force
}
New-Item -ItemType Directory -Path $extractDir -Force | Out-Null
& tar.exe -xjf $archivePath -C $extractDir
if ($LASTEXITCODE -ne 0) {
  throw "Failed to extract the Sherpa-ONNX CPU archive."
}

$sourceBin = Join-Path $extractDir "$runtimeName\bin"
New-Item -ItemType Directory -Path $targetBin -Force | Out-Null
foreach ($file in $requiredFiles) {
  $source = Join-Path $sourceBin $file
  if (-not (Test-Path -LiteralPath $source)) {
    throw "Required Sherpa-ONNX executable was not found: $file"
  }
  Copy-Item -LiteralPath $source -Destination (Join-Path $targetBin $file) -Force
}

$totalBytes = (Get-ChildItem -LiteralPath $targetBin -File | Measure-Object Length -Sum).Sum
Write-Host "Prepared minimal Sherpa-ONNX CPU runtime: $([math]::Round($totalBytes / 1MB, 1)) MiB"

