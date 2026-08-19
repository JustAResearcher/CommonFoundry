#!/usr/bin/env bash
set -euo pipefail

# Edit these values. Leave GPU_INDEXES empty to use every supported GPU.
PEER="107.214.187.2:18444"
P2P_BIND="127.0.0.1:19444"
DATA_DIR="$(cd -- "$(dirname -- "$0")" && pwd)/miner-data"
GPU_INDEXES=""
PAYOUT_ADDRESS=""
BATCH_SIZE="8192"

SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
ARGS=(mine --data-dir "$DATA_DIR" --p2p-bind "$P2P_BIND" --batch-size "$BATCH_SIZE")
if [[ -n "$PEER" ]]; then
  ARGS+=(--peer "$PEER" --allow-public-peers)
fi
if [[ -n "$GPU_INDEXES" ]]; then
  IFS=',' read -ra DEVICES <<< "$GPU_INDEXES"
  for DEVICE in "${DEVICES[@]}"; do
    ARGS+=(--device "$DEVICE")
  done
fi
if [[ -n "$PAYOUT_ADDRESS" ]]; then
  ARGS+=(--miner "$PAYOUT_ADDRESS")
fi

exec "$SCRIPT_DIR/cmfd-miner" "${ARGS[@]}"
