# VideoFlow 开发环境变量
# 用途：把 Rust / MinGW 工具链固定在 D 盘，避免写入 C 盘。
# 用法：在仓库根目录执行  . .\scripts\env.ps1

$ErrorActionPreference = 'Stop'

$RustRoot = 'D:\rust'
$MinGwBin = Join-Path $RustRoot 'mingw-extract\mingw64\bin'

$env:RUSTUP_HOME      = Join-Path $RustRoot 'rustup'
$env:CARGO_HOME       = Join-Path $RustRoot 'cargo'
$env:CARGO_TARGET_DIR = Join-Path $RustRoot 'target'

if (Test-Path $MinGwBin) {
  $env:PATH = "$MinGwBin;" + $env:PATH
}
$CargoBin = Join-Path $env:CARGO_HOME 'bin'
if (Test-Path $CargoBin) {
  $env:PATH = "$CargoBin;" + $env:PATH
}

Write-Host "RUSTUP_HOME      = $env:RUSTUP_HOME"
Write-Host "CARGO_HOME       = $env:CARGO_HOME"
Write-Host "CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR"