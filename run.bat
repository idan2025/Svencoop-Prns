@echo off
REM sc-rns-bridge launcher for Windows.
REM Run: run.bat   (double-click works too)
setlocal enabledelayedexpansion

cd /d "%~dp0"

set BIN=target\release\sc-rns-bridge.exe
if not exist "%BIN%" (
    echo Release binary not found at %BIN%
    echo Building it now (first run takes a minute)...
    cargo build --release
    if errorlevel 1 (
        echo Build failed.
        pause
        exit /b 1
    )
)

REM Try to locate the Sven Co-op dedicated server.
REM Order: bundle-local .\svends, last-used path in .svends_path, Steam paths.
set SVENDS=
if exist ".\svends\svends.exe" set "SVENDS=.\svends\svends.exe"
if "!SVENDS!"=="" (
    if exist ".svends_path" (
        set PREV=
        set /p PREV=<.svends_path
        if exist "!PREV!\svends.exe" set "SVENDS=!PREV!\svends.exe"
    )
)
REM Checked as flat chained "if ... if exist ... set" lines rather than a
REM `for %%P in (...) do (...)` block: %ProgramFiles(x86)% and the literal
REM "(x86)" in the hardcoded fallback path both contain a raw ")" that,
REM inside a multi-line `(...)` block, breaks cmd's parenthesis-nesting count
REM and aborts the whole script with "... was unexpected at this time." A
REM single-line chained `if` has no block to miscount, so it's safe.
if "!SVENDS!"=="" if exist "%ProgramFiles(x86)%\Steam\steamapps\common\Sven Co-op\svends.exe" set "SVENDS=%ProgramFiles(x86)%\Steam\steamapps\common\Sven Co-op\svends.exe"
if "!SVENDS!"=="" if exist "%ProgramFiles%\Steam\steamapps\common\Sven Co-op\svends.exe" set "SVENDS=%ProgramFiles%\Steam\steamapps\common\Sven Co-op\svends.exe"
if "!SVENDS!"=="" if exist "C:\Program Files (x86)\Steam\steamapps\common\Sven Co-op\svends.exe" set "SVENDS=C:\Program Files (x86)\Steam\steamapps\common\Sven Co-op\svends.exe"
if "!SVENDS!"=="" if exist "C:\Program Files\Steam\steamapps\common\Sven Co-op\svends.exe" set "SVENDS=C:\Program Files\Steam\steamapps\common\Sven Co-op\svends.exe"
if "!SVENDS!"=="" if exist "%USERPROFILE%\Steam\steamapps\common\Sven Co-op\svends.exe" set "SVENDS=%USERPROFILE%\Steam\steamapps\common\Sven Co-op\svends.exe"

echo ==================================
echo  Sven Co-op over Reticulum
echo ==================================
echo  1) Start Sven Co-op dedicated server
echo  2) Bridge server  (relays SC server traffic over Reticulum)
echo  3) Bridge client  (you are a player; connects to a bridge server)
echo  4) Build only
echo.
set /p choice=Choose [1-4]:

if "%choice%"=="1" goto scserver
if "%choice%"=="2" goto server
if "%choice%"=="3" goto client
if "%choice%"=="4" goto build
echo Invalid choice
pause
exit /b 1

:scserver
REM Windows always has a Sven Co-op dedicated server build (app 276060).
if "!SVENDS!"=="" (
    echo.
    echo No Sven Co-op dedicated server found.
    set DL=
    set /p DL=Download it via steamcmd now? [y/N]:
    if /i not "!DL!"=="y" (
        echo Aborting.
        pause
        exit /b 1
    )
    call :ensure_steamcmd
    if errorlevel 1 (
        echo steamcmd setup failed.
        pause
        exit /b 1
    )
    set INSTALL_DIR=
    set /p INSTALL_DIR=Install path for the dedicated server [.\svends]:
    if "!INSTALL_DIR!"=="" set "INSTALL_DIR=.\svends"
    echo.
    echo Downloading Sven Co-op dedicated server (app 276060) into:
    echo   !INSTALL_DIR!
    echo This is ~2.7 GB. Please wait...
    echo.
    steamcmd\steamcmd.exe +force_install_dir "!INSTALL_DIR!" +login anonymous +app_update 276060 validate +quit
    if errorlevel 1 (
        echo.
        echo steamcmd exited with an error. The download may have failed.
        echo Check the output above and re-run.
        pause
        exit /b 1
    )
    set "SVENDS=!INSTALL_DIR!\svends.exe"
    if not exist "!SVENDS!" (
        echo Download finished but !SVENDS! was not found.
        pause
        exit /b 1
    )
    echo !INSTALL_DIR!>.svends_path
)
for %%I in ("!SVENDS!") do set "SVENDS_DIR=%%~dpI"
echo.
echo Found dedicated server: !SVENDS!
set sc_port=
set /p sc_port=UDP port [27015]:
if "!sc_port!"=="" set sc_port=27015
set maxplayers=
set /p maxplayers=Max players [8]:
if "!maxplayers!"=="" set maxplayers=8
set map=
set /p map=Starting map [svencoop1]:
if "!map!"=="" set map=svencoop1

REM Pre-create soundcache files for ALL maps in the maps directory.
REM The SC dedicated server fails to generate these on-the-fly, causing
REM "failed to transmit file" errors that disconnect clients. Creating
REM empty files for every .bsp means map changes mid-game won't break either.
set "SOUNDCACHE_DIR=%SVENDS_DIR%svencoop\maps\soundcache"
if not exist "%SOUNDCACHE_DIR%" mkdir "%SOUNDCACHE_DIR%"
set created=0
for %%F in ("%SVENDS_DIR%svencoop\maps\*.bsp") do (
    if not exist "%SOUNDCACHE_DIR%\%%~nF.txt" (
        type nul > "%SOUNDCACHE_DIR%\%%~nF.txt"
        set /a created+=1
    )
)
if !created! GTR 0 echo Pre-created !created! empty soundcache file(s) in %SOUNDCACHE_DIR%

echo.
echo Starting Sven Co-op dedicated server on port !sc_port!...
echo Map: !map!   Max players: !maxplayers!
echo Press Ctrl-C to stop.
echo.
cd /d "%SVENDS_DIR%"
svends.exe -port !sc_port! +maxplayers !maxplayers! +map !map!
pause
exit /b

:ensure_steamcmd
if exist "steamcmd\steamcmd.exe" exit /b 0
if not exist steamcmd mkdir steamcmd
echo Downloading steamcmd...
powershell -NoProfile -Command "try { Invoke-WebRequest -Uri 'https://steamcdn-a.akamaihd.net/client/installer/steamcmd.zip' -OutFile 'steamcmd.zip' -UseBasicParsing } catch { Invoke-WebRequest -Uri 'http://media.steampowered.com/installer/steamcmd.zip' -OutFile 'steamcmd.zip' -UseBasicParsing }"
if errorlevel 1 (
    echo Failed to download steamcmd.
    exit /b 1
)
powershell -NoProfile -Command "Expand-Archive -Path 'steamcmd.zip' -DestinationPath 'steamcmd' -Force"
if errorlevel 1 (
    echo Failed to extract steamcmd.
    if exist steamcmd.zip del steamcmd.zip
    exit /b 1
)
if exist steamcmd.zip del steamcmd.zip
if not exist "steamcmd\steamcmd.exe" (
    echo steamcmd.exe not found after extraction.
    exit /b 1
)
exit /b 0

:server
set sc_host=
set /p sc_host=Sven Co-op server host [127.0.0.1]:
if "!sc_host!"=="" set sc_host=127.0.0.1
set sc_port=
set /p sc_port=Sven Co-op server UDP port [27015]:
if "!sc_port!"=="" set sc_port=27015
echo.
echo Interface: how should this node reach other nodes?
echo  a) TCP server (bind a public relay, e.g. 0.0.0.0:4234)
echo  b) Wi-Fi/LAN auto-discovery (no internet needed)
echo  c) Both
set iface=
set /p iface=Choose [a/b/c]:
set tcp_flag=
set auto_flag=
if /i "!iface!"=="a" (
    set tcp=
    set /p tcp=TCP bind address [0.0.0.0:4234]:
    if "!tcp!"=="" set tcp=0.0.0.0:4234
    set tcp_flag=--tcp !tcp!
)
if /i "!iface!"=="b" set auto_flag=--auto
if /i "!iface!"=="c" (
    set tcp=
    set /p tcp=TCP bind address [0.0.0.0:4234]:
    if "!tcp!"=="" set tcp=0.0.0.0:4234
    set tcp_flag=--tcp !tcp!
    set auto_flag=--auto
)
set ann=
set /p ann=Announce interval seconds [15]:
if "!ann!"=="" set ann=15
echo.
echo Starting bridge server. Press Ctrl-C to stop.
echo Players use the printed server_hash with --server-hash, or just run a client.
echo.
%BIN% server --sc-host !sc_host! --sc-port !sc_port! !tcp_flag! !auto_flag! --announce-interval !ann!
pause
exit /b

:client
set listen=
set /p listen=Local UDP port for GoldSrc client to connect to [27015]:
if "!listen!"=="" set listen=27015
echo.
echo Interface: how should this node reach the bridge server?
echo  a) TCP client (connect to a public relay, e.g. example.com:4234)
echo  b) Wi-Fi/LAN auto-discovery (no internet needed)
set iface=
set /p iface=Choose [a/b]:
set tcp_flag=
set auto_flag=
if /i "!iface!"=="a" (
    set tcp=
    set /p tcp=Bridge server host:port (e.g. 1.2.3.4:4234):
    set tcp_flag=--tcp !tcp!
)
if /i "!iface!"=="b" set auto_flag=--auto
set hash=
set /p hash=Server destination hash (32 hex chars, blank to auto-discover):
set hash_flag=
if not "!hash!"=="" set hash_flag=--server-hash !hash!
echo.
echo Starting bridge client. Point your Sven Co-op client at localhost:!listen!
echo Press Ctrl-C to stop.
echo.
%BIN% client --listen-port !listen! !tcp_flag! !auto_flag! !hash_flag!
pause
exit /b

:build
cargo build --release
pause
exit /b