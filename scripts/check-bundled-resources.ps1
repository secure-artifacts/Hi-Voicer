$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$resourceRoot = Join-Path $repoRoot "src-tauri\resources"
$requiredFiles = @(
  "engines\ffmpeg\bin\ffmpeg.exe",
  "engines\ffmpeg\bin\ffprobe.exe",
  "engines\llama\b9964\llama-server.exe",
  "engines\llama\b9964\llama-server-impl.dll",
  "engines\llama\b9964\llama-common.dll",
  "engines\llama\b9964\llama.dll",
  "engines\sherpa\v1.13.2\sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts\bin\sherpa-onnx-offline.exe",
  "engines\sherpa\v1.13.2\sherpa-onnx-v1.13.2-win-x64-static-MT-Release-no-tts\bin\sherpa-onnx-offline-websocket-server.exe"
)

foreach ($relativePath in $requiredFiles) {
  $path = Join-Path $resourceRoot $relativePath
  if (-not (Test-Path -LiteralPath $path)) {
    throw "Required bundled runtime file is missing: $relativePath"
  }
}

$forbidden = Get-ChildItem -Path $resourceRoot -Recurse -File | Where-Object {
  $_.Name -match "cuda|cudnn|tensorrt|vulkan"
}
if ($forbidden) {
  $paths = $forbidden.FullName | ForEach-Object { $_.Substring($resourceRoot.Length + 1) }
  throw "GPU runtime files must not enter the CPU installer: $($paths -join ', ')"
}

$totalBytes = (Get-ChildItem -Path $resourceRoot -Recurse -File | Measure-Object Length -Sum).Sum
$maxBytes = 270MB
if ($totalBytes -gt $maxBytes) {
  throw "Bundled resources are too large: $([math]::Round($totalBytes / 1MB, 1)) MiB (limit: 230 MiB)."
}

Write-Host "Bundled resources verified: $([math]::Round($totalBytes / 1MB, 1)) MiB"
