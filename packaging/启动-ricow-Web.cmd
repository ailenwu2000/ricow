@echo off
rem ===========================================================
rem  ricow - Windows Web UI entry point (025 FR-033)
rem
rem  Double-click this file to open the built-in Web UI. The
rem  process binds 127.0.0.1 only, prints the URL carrying a
rem  one-off token, and opens the default browser on it; the whole
rem  conversation then happens in the page. Close it by pressing
rem  Ctrl-C in this window (or just close the window).
rem
rem  "chcp 65001" switches the console to UTF-8 first, otherwise the
rem  Chinese output is mojibake on the default GBK/936 console code
rem  page.
rem
rem  The body is deliberately ASCII-only: cmd.exe re-reads a batch
rem  file byte-by-byte as it executes, so a UTF-8 body can be
rem  mis-parsed right after the code-page switch. All Chinese text
rem  comes from ricow itself.
rem
rem  The launch line is "ricow web" and NOT the bare entry: the bare
rem  entry (commands\chat.rs) is the terminal chat, which is exactly
rem  what this launcher is meant to replace. "ricow web" runs the same
rem  first-run wizard (if needed) -- provider, API key, Binance demo
rem  credentials -- saves them into ricow.toml itself, and only then
rem  serves the page. That is the whole point of this launcher: the
rem  user has to remember nothing.
rem
rem  Which binary to run, in order:
rem    1. ricow.exe next to this file  - a release archive. The .zip ships
rem       it here (see the "include" key in dist-workspace.toml); the msi
rem       installs the binary only, so this entry point belongs to the .zip.
rem    2. the build output in ..\target  - a source checkout. Both
rem       target\release\ricow.exe ("cargo build -p ricow --release") and
rem       target\debug\ricow.exe ("cargo build -p ricow") may be present, and
rem       they go stale independently, so we run whichever was built last.
rem    3. ricow.exe on PATH            - a normal install.
rem  With (2) we also cd to the repository root: ricow takes its data
rem  directory from the current directory when that directory holds
rem  ricow.db or strategies\, and a checkout keeps both at the root.
rem  Running from packaging\ instead would silently use (or create) a
rem  second, empty data directory.
rem
rem  FR-058: if the extracted ricow.exe still carries the
rem  "downloaded from the Internet" mark (the NTFS alternate data stream
rem  Zone.Identifier that Explorer propagates from the .zip), SmartScreen
rem  may refuse to start it. Say so and print the way out, instead of
rem  letting the user stare at a silent failure.
rem ===========================================================
chcp 65001 >nul

setlocal
rem Repository root, when this script sits in a source checkout (packaging\).
for %%I in ("%~dp0..") do set "RICOW_ROOT_DIR=%%~fI"

set "RICOW_BIN="
set "RICOW_CWD="

rem 1) Release archive: the binary sits right beside this file.
if exist "%~dp0ricow.exe" set "RICOW_BIN=%~dp0ricow.exe"

rem 2) Source checkout: use the build output, and run from the repo root.
if not defined RICOW_BIN if exist "%RICOW_ROOT_DIR%\Cargo.toml" set "RICOW_CWD=%RICOW_ROOT_DIR%"
if not defined RICOW_BIN if defined RICOW_CWD call :pick_from_target

rem 3) A normal install: whatever ricow.exe is on PATH.
if not defined RICOW_BIN for /f "delims=" %%I in ('where ricow.exe 2^>nul') do if not defined RICOW_BIN set "RICOW_BIN=%%I"

if not defined RICOW_BIN (
  echo [ricow] Error: no ricow.exe found.
  echo [ricow] Looked: next to this script, ..\target\release and ..\target\debug,
  echo [ricow] and every ricow.exe on PATH.
  echo [ricow] From a source checkout, build it once and run this file again:
  echo [ricow]   cargo build -p ricow
  echo.
  pause
  exit /b 1
)

if defined RICOW_CWD cd /d "%RICOW_CWD%"

dir /r "%RICOW_BIN%" 2>nul | findstr /i "Zone.Identifier" >nul 2>&1
if not errorlevel 1 (
  echo [ricow] Note: ricow.exe is still marked as "downloaded from the Internet".
  echo [ricow] If Windows says "Windows protected your PC", click "More info"
  echo [ricow] then "Run anyway"; or unblock every extracted file once with:
  echo [ricow]   powershell -Command "Get-ChildItem -Recurse . ^| Unblock-File"
  echo.
)

rem Web entry: "ricow web" -> first-run wizard (if needed) -> browser page.
"%RICOW_BIN%" web

echo.
echo [ricow] web ui stopped.
pause
exit /b 0

rem --------------------------------------------------------------------------
rem Set RICOW_BIN to the newer of the two build outputs, if either exists.
rem "%~tI" prints the last-write time in a fixed per-locale format, so a plain
rem string compare orders the two builds by age.
:pick_from_target
set "RICOW_REL=%RICOW_ROOT_DIR%\target\release\ricow.exe"
set "RICOW_DBG=%RICOW_ROOT_DIR%\target\debug\ricow.exe"
if exist "%RICOW_REL%" set "RICOW_BIN=%RICOW_REL%"
if exist "%RICOW_DBG%" if not defined RICOW_BIN set "RICOW_BIN=%RICOW_DBG%"
for %%A in ("%RICOW_REL%") do for %%B in ("%RICOW_DBG%") do if "%%~tB" GTR "%%~tA" set "RICOW_BIN=%RICOW_DBG%"
exit /b 0
