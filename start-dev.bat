@echo off
title DeepAgent Studio - Dev Server
cd /d "%~dp0apps\desktop"

rem ---- 1. Install dependencies if missing (node_modules may be wiped or partial) ----
if not exist "node_modules\.bin\vite.cmd" goto :need_install
if not exist "node_modules\@tauri-apps\cli\tauri.js" goto :need_install
echo [1/2] node_modules OK
goto :start

:need_install
echo [1/2] Dependencies missing or incomplete, running pnpm install...
call pnpm install
if errorlevel 1 (
    echo.
    echo [ERROR] pnpm install failed. Exiting.
    pause
    exit /b 1
)

:start

rem ---- 2. Start Tauri dev server (vite + cargo) ----
echo [2/2] Starting dev server, please wait...
call pnpm tauri dev

echo.
echo Dev server stopped.
pause
