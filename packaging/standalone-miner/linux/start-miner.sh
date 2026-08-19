#!/usr/bin/env bash
set -euo pipefail

# These defaults work with a wallet on this computer or the community bootstrap.
# Leave GPU_INDEXES empty to use every supported GPU.
LOCAL_PEER="127.0.0.1:18444"
BOOTSTRAP_PEER="107.214.187.2:18444"
P2P_BIND="127.0.0.1:19444"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/common-foundry-miner/devnet-0"
GPU_INDEXES=""
# PAYOUT_ADDRESS is your wallet's 64-character receive address.
PAYOUT_ADDRESS=""
BATCH_SIZE="8192"

SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
ARGS=(mine --data-dir "$DATA_DIR" --p2p-bind "$P2P_BIND" --batch-size "$BATCH_SIZE")
if [[ -n "$LOCAL_PEER" ]]; then
  ARGS+=(--peer "$LOCAL_PEER")
fi
if [[ -n "$BOOTSTRAP_PEER" ]]; then
  ARGS+=(--peer "$BOOTSTRAP_PEER" --allow-public-peers)
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
