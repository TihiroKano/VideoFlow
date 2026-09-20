# VideoFlow 测试总入口
#
# 用法：powershell -ExecutionPolicy Bypass -File test\run-all.ps1 [-IncludeNetwork]
#
# 组成：
#   1. 前端纯函数单测（node --test，Node 24 原生支持 TypeScript）
#   2. Rust 单元测试（cargo test）
#   3. 可选：真实网络下载测试（-IncludeNetwork），验证双引擎之一的原生 HTTP 引擎

param(
  [switch]$IncludeNetwork
)

$ErrorActionPreference = 'Continue'
$root = Join-Path $PSScriptRoot '..'
Set-Location $root

. (Join-Path $root 'scripts\env.ps1')

$failed = @()

Write-Host "`n=== 1/3 前端纯函数单测 ===" -ForegroundColor Cyan
$tsTests = Get-ChildItem -Path (Join-Path $PSScriptRoot 'unit') -Filter '*.test.ts' -Recurse -ErrorAction SilentlyContinue
if ($tsTests.Count -eq 0) {
  Write-Host "未找到前端测试文件，跳过"
} else {
  & node --test @($tsTests.FullName | ForEach-Object { $_ })
  if ($LASTEXITCODE -ne 0) { $failed += 'node --test' }
}

Write-Host "`n=== 2/3 Rust 单元测试 ===" -ForegroundColor Cyan
Push-Location (Join-Path $root 'src-tauri')
& cargo test -j 2 --lib 2>&1 | Select-Object -Last 30
if ($LASTEXITCODE -ne 0) { $failed += 'cargo test' }
Pop-Location

if ($IncludeNetwork) {
  Write-Host "`n=== 3/3 真实网络下载测试（原生 HTTP 引擎）===" -ForegroundColor Cyan
  Push-Location (Join-Path $root 'src-tauri')
  & cargo test -j 2 --lib -- --ignored --nocapture 2>&1 | Select-Object -Last 40
  if ($LASTEXITCODE -ne 0) { $failed += 'cargo test --ignored' }
  Pop-Location
} else {
  Write-Host "`n=== 3/3 跳过网络测试（加 -IncludeNetwork 启用）===" -ForegroundColor Yellow
}

Write-Host ""
if ($failed.Count -gt 0) {
  Write-Host "失败项：$($failed -join ', ')" -ForegroundColor Red
  exit 1
}
Write-Host "全部测试通过" -ForegroundColor Green