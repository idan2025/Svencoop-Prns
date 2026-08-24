@echo off
REM sc-rns-bridge launcher for Windows.
REM Run: run.bat   (double-click works too)

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

REM Try common Steam install locations for the Sven Co-op dedicated server.
set SVENDS=
for %%P in (
    "%ProgramFiles(x86)%\Steam\steamapps\common\Sven Co-op\svends.exe"
    "%ProgramFiles%\Steam\steamapps\common\Sven Co-op\svends.exe"
    "C:\Program Files (x86)\Steam\steamapps\common\Sven Co-op\svends.exe"
    "C:\Program Files\Steam\steamapps\common\Sven Co-op\svends.exe"
    "%USERPROFILE%\Steam\steamapps\common\Sven Co-op\svends.exe"
) do (
    if exist %%P set SVENDS=%%~P
)

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
if "%SVENDS%"=="" (
    echo.
    echo Could not find svends.exe in the usual Steam install paths.
    echo If Sven Co-op is installed elsewhere, enter the full path.
    echo.
    set /p SVENDS=Full path to svends.exe:
)
if "%SVENDS%"=="" (
    echo No path given. Aborting.
    pause
    exit /b 1)
if not exist "%SVENDS%" (
    echo File not found: %SVENDS%
    pause
    exit /b 1
)
for %%I in ("%SVENDS%") do set SVENDS_DIR=%%~dpI
echo.
echo Found dedicated server: %SVENDS%
set /p sc_port=UDP port [27015]:
if "%sc_port%"=="" set sc_port=27015
set /p maxplayers=Max players [8]:
if "%maxplayers%"=="" set maxplayers=8
set /p map=Starting map [svencoop1]:
if "%map%"=="" set map=svencoop1
echo.
echo Starting Sven Co-op dedicated server on port %sc_port%...
echo Map: %map%   Max players: %maxplayers%
echo Press Ctrl-C to stop.
echo.
cd /d "%SVENDS_DIR%"
svends.exe -port %sc_port% +maxplayers %maxplayers% +map %map%
pause
exit /b

:server
set /p sc_host=Sven Co-op server host [127.0.0.1]:
if "%sc_host%"=="" set sc_host=127.0.0.1
set /p sc_port=Sven Co-op server UDP port [27015]:
if "%sc_port%"=="" set sc_port=27015
echo.
echo Interface: how should this node reach other nodes?
echo  a) TCP server (bind a public relay, e.g. 0.0.0.0:4234)
echo  b) Wi-Fi/LAN auto-discovery (no internet needed)
echo  c) Both
set /p iface=Choose [a/b/c]:
set tcp_flag=
set auto_flag=
if /i "%iface%"=="a" (
    set /p tcp=TCP bind address [0.0.0.0:4234]:
    if "%tcp%"=="" set tcp=0.0.0.0:4234
    set tcp_flag=--tcp %tcp%
)
if /i "%iface%"=="b" set auto_flag=--auto
if /i "%iface%"=="c" (
    set /p tcp=TCP bind address [0.0.0.0:4234]:
    if "%tcp%"=="" set tcp=0.0.0.0:4234
    set tcp_flag=--tcp %tcp%
    set auto_flag=--auto
)
set /p ann=Announce interval seconds [15]:
if "%ann%"=="" set ann=15
echo.
echo Starting bridge server. Press Ctrl-C to stop.
echo Players use the printed server_hash with --server-hash, or just run a client.
echo.
%BIN% server --sc-host %sc_host% --sc-port %sc_port% %tcp_flag% %auto_flag% --announce-interval %ann%
pause
exit /b

:client
set /p listen=Local UDP port for GoldSrc client to connect to [27015]:
if "%listen%"=="" set listen=27015
echo.
echo Interface: how should this node reach the bridge server?
echo  a) TCP client (connect to a public relay, e.g. example.com:4234)
echo  b) Wi-Fi/LAN auto-discovery (no internet needed)
set /p iface=Choose [a/b]:
set tcp_flag=
set auto_flag=
if /i "%iface%"=="a" (
    set /p tcp=Bridge server host:port (e.g. 1.2.3.4:4234):
    set tcp_flag=--tcp %tcp%
)
if /i "%iface%"=="b" set auto_flag=--auto
set /p hash=Server destination hash (32 hex chars, blank to auto-discover):
set hash_flag=
if not "%hash%"=="" set hash_flag=--server-hash %hash%
echo.
echo Starting bridge client. Point your Sven Co-op client at localhost:%listen%
echo Press Ctrl-C to stop.
echo.
%BIN% client --listen-port %listen% %tcp_flag% %auto_flag% %hash_flag%
pause
exit /b

:build
cargo build --release
pause
exit /b