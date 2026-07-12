$ErrorActionPreference = "Stop"

Write-Host "Hi-Voicer Windows environment setup" -ForegroundColor Cyan

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
  $installer = Join-Path $env:TEMP "rustup-init.exe"
  Write-Host "Downloading official Rust installer..."
  Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $installer
  & $installer -y --default-toolchain stable-x86_64-pc-windows-msvc --profile minimal
}

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if ($env:Path -notlike "*$cargoBin*") {
  $env:Path = "$cargoBin;$env:Path"
}

rustup toolchain install 1.96.0-x86_64-pc-windows-msvc --profile minimal
rustup default 1.96.0-x86_64-pc-windows-msvc
rustup component add --toolchain 1.96.0-x86_64-pc-windows-msvc rustfmt

npm ci
npm run prepare:llama
npm run prepare:sherpa
npm test
npm run build
cargo check --manifest-path src-tauri\Cargo.toml

Write-Host "Hi-Voicer Windows environment setup completed" -ForegroundColor Green
