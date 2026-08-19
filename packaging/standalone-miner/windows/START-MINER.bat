@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Common Foundry Multi-GPU Miner

rem ================================================================
rem EDIT ONLY THIS SECTION
rem ================================================================
rem PEER is the node that receives your blocks and keeps this miner synced.
set "PEER=107.214.187.2:18444"
set "ALLOW_PUBLIC_PEER=1"
set "P2P_BIND=127.0.0.1:19444"
set "DATA_DIR=%~dp0miner-data"
set "GPU_INDEXES="
rem PAYOUT_ADDRESS is your wallet's 64-character receive address.
set "PAYOUT_ADDRESS="
set "BATCH_SIZE=8192"
rem ================================================================
rem GPU_INDEXES examples:
rem   blank   = use every supported NVIDIA GPU
rem   0       = use GPU 0 only
rem   0,1,2,3 = use GPUs 0 through 3
rem Run LIST-GPUS.bat to see the indexes on this rig.
rem Leaving PAYOUT_ADDRESS blank creates/reuses this miner's local key.
rem ================================================================

if not exist "%~dp0cmfd-miner.exe" (
  echo ERROR: cmfd-miner.exe is missing from this folder.
  pause
  exit /b 1
)

set "PEER_ARGS="
if defined PEER set "PEER_ARGS=--peer %PEER%"
if "%ALLOW_PUBLIC_PEER%"=="1" set "PEER_ARGS=!PEER_ARGS! --allow-public-peers"

set "DEVICE_ARGS="
if defined GPU_INDEXES (
  set "GPU_LIST=!GPU_INDEXES:,= !"
  for %%G in (!GPU_LIST!) do set "DEVICE_ARGS=!DEVICE_ARGS! --device %%G"
)

set "PAYOUT_ARGS="
if defined PAYOUT_ADDRESS set "PAYOUT_ARGS=--miner %PAYOUT_ADDRESS%"

echo Starting Common Foundry miner...
echo Peer: %PEER%
if defined GPU_INDEXES (
  echo GPUs: %GPU_INDEXES%
) else (
  echo GPUs: all supported devices
)
echo.

"%~dp0cmfd-miner.exe" mine ^
  --data-dir "%DATA_DIR%" ^
  --p2p-bind %P2P_BIND% ^
  !PEER_ARGS! ^
  !DEVICE_ARGS! ^
  !PAYOUT_ARGS! ^
  --batch-size %BATCH_SIZE%

echo.
echo Miner stopped with exit code %ERRORLEVEL%.
pause
