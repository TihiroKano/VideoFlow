# VideoFlow 开发版启动脚本
#
# 用法（任选其一）：
#   start.cmd               普通启动
#   start.cmd preview       载入预览图填充态，用于与 docs\预览图.png 比对
#   npm run app             经由 npm 启动（需要 npm 可用）
#
# 为什么需要 -ExecutionPolicy Bypass：
#   本机 PowerShell 执行策略为默认的 Restricted，会拦截 npm.ps1 / npx.ps1 与
#   所有 .ps1 文件。start.cmd 以 Bypass 方式调用本脚本，从而绕过该限制。

param(
  [string]$Mode = ''
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $projectRoot

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "  VideoFlow 开发版启动器" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# ---- 工具链环境：全部指向 D 盘，避免写入 C 盘 ----
$rustRoot = 'D:\rust'
$mingwBin = Join-Path $rustRoot 'mingw-extract\mingw64\bin'
$cargoHome = Join-Path $rustRoot 'cargo'

$env:RUSTUP_HOME = Join-Path $rustRoot 'rustup'
$env:CARGO_HOME = $cargoHome
$env:CARGO_TARGET_DIR = Join-Path $rustRoot 'target'
$env:PATH = "$mingwBin;$cargoHome\bin;$env:PATH"

# ---- 前置检查 ----
$problems = @()
foreach ($tool in @('cargo.exe', 'gcc.exe', 'dlltool.exe', 'ar.exe', 'windres.exe')) {
  $found = if ($tool -like 'cargo*') { Test-Path (Join-Path $cargoHome "bin\$tool") }
           else { Test-Path (Join-Path $mingwBin $tool) }
  if (-not $found) { $problems += "缺少 $tool" }
}
foreach ($rel in @('node_modules\@tauri-apps\cli\tauri.js', 'node_modules\vite\bin\vite.js')) {
  if (-not (Test-Path (Join-Path $projectRoot $rel))) { $problems += "缺少 $rel（请先运行 npm install）" }
}

if ($problems.Count -gt 0) {
  Write-Host "环境不完整，已中止：" -ForegroundColor Red
  $problems | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
  exit 1
}

# ---- 可选：预览态夹具 ----
if ($Mode -ieq 'preview') {
  $env:VITE_VF_SEED = 'preview'
  Write-Host "[模式] 预览态夹具已启用" -ForegroundColor Yellow
}

# ---- 清理上一次残留：1420 端口被占用会导致启动失败 ----
$stale = netstat -ano | Select-String ':1420' | Select-String 'LISTENING'
foreach ($line in $stale) {
  $procId = ($line.ToString() -split '\s+')[-1]
  if ($procId -match '^\d+$' -and $procId -ne '0') {
    Write-Host "[清理] 结束占用 1420 端口的进程 PID=$procId" -ForegroundColor Yellow
    Stop-Process -Id ([int]$procId) -Force -ErrorAction SilentlyContinue
  }
}

Write-Host "[启动] 首次编译约需 30-60 秒，请耐心等待..." -ForegroundColor Green
Write-Host "       窗口出现后即可使用；关闭窗口或按 Ctrl+C 退出。" -ForegroundColor Green
Write-Host ""

# 直接用 node 调用 Tauri CLI，不经过 npx / npm，减少一层对执行策略的依赖
& node (Join-Path $projectRoot 'node_modules\@tauri-apps\cli\tauri.js') dev
$exitCode = $LASTEXITCODE

Write-Host ""
if ($exitCode -eq 0) {
  Write-Host "[退出] VideoFlow 已正常关闭。" -ForegroundColor Green
} else {
  Write-Host "[退出] 启动器返回错误码 $exitCode" -ForegroundColor Red
  Write-Host "       若提示端口 1420 被占用，请再运行一次本脚本。" -ForegroundColor Red
}

exit $exitCode