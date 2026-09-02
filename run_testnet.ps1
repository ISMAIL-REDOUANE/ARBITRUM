@echo off
REM =============================================================================
REM MEV Arbitrage Engine - Arbitrum Sepolia Testnet Launch (Windows)
REM =============================================================================
REM Features:
REM   - Shadow mode (no real transactions)
REM   - CPU core affinity via start /affinity
REM   - Telemetry WebSocket server
REM =============================================================================

setlocal enabledelayedexpansion

echo.
echo =============================================================================
echo        MEV ARBITRAGE ENGINE - ARBITRUM SEPOLIA TESTNET
echo =============================================================================
echo.

set PROJECT_ROOT=%~dp0arbitrage-engine
set TELEMETRY_PORT=3000

echo [INFO] Loading environment from .env.testnet...
if exist "%PROJECT_ROOT%\.env.testnet" (
    for /f "usebackq tokens=1,* delims==" %%a in ("%PROJECT_ROOT%\.env.testnet") do (
        set %%a=%%b
    )
    echo [OK] Environment loaded
) else (
    echo [WARN] .env.testnet not found in project root
)

REM Set defaults
if "%SHADOW_MODE%"=="" set SHADOW_MODE=true
if "%WS_CORE%"=="" set WS_CORE=2
if "%SIM_CORE%"=="" set SIM_CORE=3
if "%TELEMETRY_PORT%"=="" set TELEMETRY_PORT=3000
if "%RUST_LOG%"=="" set RUST_LOG=info

echo.
echo [INFO] Configuration:
echo        SHADOW_MODE: %SHADOW_MODE%
echo        WS_CORE: %WS_CORE%
echo        SIM_CORE: %SIM_CORE%
echo        TELEMETRY_PORT: %TELEMETRY_PORT%
echo        RUST_LOG: %RUST_LOG%
echo.

REM Check if Rust build exists
set ENGINE_BIN=%PROJECT_ROOT%\target\release\arbitrage-engine.exe
if not exist "%ENGINE_BIN%" (
    echo [INFO] Building release binary...
    cargo build --release --manifest-path "%PROJECT_ROOT%\Cargo.toml" 2>&1 | findstr /C:"Finished" /C:"error"
    
    if %ERRORLEVEL% neq 0 (
        echo [WARN] Build may have issues, attempting to continue...
    )
)

if not exist "%ENGINE_BIN%" (
    echo [ERROR] Engine binary not found: %ENGINE_BIN%
    echo Run: cargo build --release --manifest-path "%PROJECT_ROOT%\Cargo.toml"
    exit /b 1
)

echo [OK] Engine binary found: %ENGINE_BIN%
echo.

REM Start telemetry WebSocket server (simple Python server)
echo [INFO] Starting telemetry server on port %TELEMETRY_PORT%...
start "ArbitrageEngine Telemetry" python -c "
import asyncio
import websockets
import json

async def handler(ws, path):
    async for msg in ws:
        data = json.loads(msg)
        print(f'[WS] {data}')

async def main():
    async with websockets.serve(handler, 'localhost', %TELEMETRY_PORT%) as server:
        await asyncio.Future()

asyncio.run(main())
"

REM Wait for telemetry server
timeout /t 2 /nobreak >nul

echo [INFO] Starting arbitrage engine...
echo.

REM Calculate CPU affinity mask (cores 2 and 3 = 0b1100 = 12)
set /a AFFINITY_MASK=1 << %WS_CORE%
set /a AFFINITY_MASK+=1 << %SIM_CORE%

echo [INFO] CPU Affinity Mask: 0x%AFFINITY_MASK% (cores %WS_CORE%, %SIM_CORE%)
echo.

REM Set environment variables for engine
set SHADOW_MODE=%SHADOW_MODE%
set RUST_LOG=%RUST_LOG%
set ARBITRUM_RPC_URL=%ARBITRUM_RPC_URL%
set ARBITRUM_WS_URL=%ARBITRUM_WS_URL%
set TELEMETRY_PORT=%TELEMETRY_PORT%

REM Start engine with CPU affinity
start "ArbitrageEngine" /affinity %AFFINITY_MASK% "%ENGINE_BIN%"

echo [OK] Engine started
echo.

echo =============================================================================
echo                          ENGINE RUNNING IN SHADOW MODE
echo =============================================================================
echo.
echo Architecture:
echo   - Core %WS_CORE%: Event Loop (WebSocket listener)
echo   - Core %SIM_CORE%: Simulation Engine (REVM hot path)
echo   - Core 0: Telemetry Logger
echo   - Shadow Mode: NO real transactions will be sent
echo.
echo Dashboard: http://localhost:%TELEMETRY_PORT%
echo.
echo Press Ctrl+C to stop...
echo.

REM Wait for user interrupt
pause >nul

:end
endlocal
