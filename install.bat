@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

echo ==^> franime_dl - installation (Windows)

REM --- Rust / cargo ---
where cargo >nul 2>nul
if errorlevel 1 (
  echo ==^> Rust manquant. Installation via winget...
  winget install -e --id Rustlang.Rustup --accept-source-agreements --accept-package-agreements
  echo !! Ferme et rouvre ce terminal pour avoir cargo dans le PATH, puis relance install.bat
  exit /b 1
)

REM --- ffmpeg ---
where ffmpeg >nul 2>nul
if errorlevel 1 (
  echo ==^> ffmpeg manquant. Installation via winget...
  winget install -e --id Gyan.FFmpeg --accept-source-agreements --accept-package-agreements
  echo    ^(si winget echoue: choco install ffmpeg, ou ajoute ffmpeg au PATH^)
)

REM --- Python ---
where python >nul 2>nul
if errorlevel 1 (
  echo !! Python 3 introuvable. Installe-le ^(winget install -e --id Python.Python.3.12^) puis relance.
  exit /b 1
)

echo ==^> Environnement Python (.venv)...
python -m venv .venv
".venv\Scripts\python.exe" -m pip install --upgrade pip >nul
".venv\Scripts\pip.exe" install -r python\requirements.txt yt-dlp

echo ==^> Compilation (cargo build --release)... (quelques minutes)
cargo build --release
if errorlevel 1 (
  echo !! La compilation a echoue.
  exit /b 1
)

echo.
echo ==^> Termine.
echo     Lancer :  target\release\franime_dl.exe
echo     Note   :  Chrome ou Chromium doit etre installe (sidecar Cloudflare).
endlocal
