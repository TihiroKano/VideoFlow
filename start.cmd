@echo off
REM ============================================================
REM  VideoFlow launcher (ASCII-only by design)
REM
REM  Why this file contains no Chinese characters:
REM  cmd.exe parses batch files using the OEM code page, so UTF-8
REM  Chinese text gets mis-read and fragments of it are executed as
REM  commands. All localized text lives in scripts\dev.ps1 instead.
REM
REM  Usage:
REM    start.cmd            -> normal launch
REM    start.cmd preview    -> launch with the preview-state fixture
REM ============================================================

setlocal
set "PROJECT_DIR=%~dp0"
set "PS1=%PROJECT_DIR%scripts\dev.ps1"

if not exist "%PS1%" (
  echo [ERROR] scripts\dev.ps1 not found:
  echo         %PS1%
  pause
  exit /b 1
)

REM -ExecutionPolicy Bypass is required because this machine has the
REM default Restricted policy, which blocks npm.ps1 / npx.ps1 / *.ps1.
powershell -NoProfile -ExecutionPolicy Bypass -File "%PS1%" %*
set "EXITCODE=%ERRORLEVEL%"

if not "%EXITCODE%"=="0" (
  echo.
  echo [EXIT] launcher returned error code %EXITCODE%
  pause
)

exit /b %EXITCODE%