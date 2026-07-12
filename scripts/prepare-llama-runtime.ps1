$ErrorActionPreference = "Stop"

$runtimeTag = "b9964"
$archiveName = "llama-$runtimeTag-bin-win-cpu-x64.zip"
$archiveSha256 = "0898A593FACAFC314EAA7FB7F81A343B039F09A7BD133AD8FA884B994A1931C1"
$url = "https://github.com/ggml-org/llama.cpp/releases/download/$runtimeTag/$archiveName"
$repoRoot = Split-Path -Parent $PSScriptRoot
$targetDir = Join-Path $repoRoot "src-tauri\resources\engines\llama\$runtimeTag"
$versionFile = Join-Path $targetDir ".runtime-version"

if ((Test-Path (Join-Path $targetDir "llama-server.exe")) -and
    (Test-Path $versionFile) -and
    ((Get-Content -Raw $versionFile).Trim() -eq $runtimeTag)) {
  Write-Host "llama.cpp $runtimeTag CPU runtime is already prepared."
  exit 0
}

$archivePath = Join-Path $env:TEMP $archiveName
$extractDir = Join-Path $env:TEMP "hi-voicer-llama-$runtimeTag"

Invoke-WebRequest -Uri $url -OutFile $archivePath
$actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
if ($actualHash -ne $archiveSha256) {
  throw "llama.cpp archive checksum mismatch. Expected $archiveSha256, got $actualHash."
}

if (Test-Path $extractDir) {
  Remove-Item -LiteralPath $extractDir -Recurse -Force
}
New-Item -ItemType Directory -Path $extractDir -Force | Out-Null
Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDir -Force

$server = Get-ChildItem -Path $extractDir -Filter "llama-server.exe" -Recurse | Select-Object -First 1
if (-not $server) {
  throw "llama-server.exe was not found in the official CPU archive."
}

if (Test-Path $targetDir) {
  Remove-Item -LiteralPath $targetDir -Recurse -Force
}
New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
Copy-Item -Path (Join-Path $server.Directory.FullName "*") -Destination $targetDir -Force
Set-Content -LiteralPath $versionFile -Value $runtimeTag -NoNewline

$forbidden = Get-ChildItem -Path $targetDir -File | Where-Object {
  $_.Name -match "cuda|cudnn|tensorrt|vulkan"
}
if ($forbidden) {
  throw "The CPU runtime unexpectedly contains GPU files: $($forbidden.Name -join ', ')"
}

& (Join-Path $targetDir "llama-server.exe") --version
Write-Host "Prepared llama.cpp CPU runtime at $targetDir"
