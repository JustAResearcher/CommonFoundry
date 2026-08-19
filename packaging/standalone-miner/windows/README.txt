COMMON FOUNDRY STANDALONE CUDA MINER

1. Double-click LIST-GPUS.bat and confirm every NVIDIA GPU appears.
2. The defaults automatically find a wallet on this PC, or use the community
   bootstrap when the wallet is elsewhere.
3. Double-click START-MINER.bat. No peer setting needs to be changed.
4. To change GPUs or the payout address, right-click START-MINER.bat and Edit.
5. Leave the miner window open. Press Ctrl+C to stop cleanly.

The miner downloads the node's current chain before starting GPU work. Seeing
"Syncing from ..." on the first start is normal. Chain data stays in your
Windows user application-data folder across miner upgrades.

To select specific cards, set GPU_INDEXES in START-MINER.bat:
  set "GPU_INDEXES=0,1,2,3"

Supported native CUDA targets:
  Volta sm_70: V100, Titan V, Quadro GV100
  Turing sm_75: RTX 20 series
  Ampere sm_86: RTX 30 series
  Ada sm_89: RTX 40 series
  Blackwell sm_120: RTX 50 series

The miner keeps one CUDA context and one worker on each selected GPU. Work
ranges are separated so GPUs in the same rig do not repeat one another.

Every five seconds the miner prints rig and per-GPU hashrate, power draw,
hashes per watt, temperature, fan, utilization, clocks, VRAM use, uptime,
attempts, accepted blocks, and stale jobs. One H/s means one complete
ForgeMatrix nonce evaluation per second. Sensors that a card does not expose
show N/A and do not interrupt mining. Edit STATS_SECONDS in START-MINER.bat to
change the reporting interval.

The miner first tries the local wallet at 127.0.0.1:18444, then the community
bootstrap. A status such as "node sync 1/2" means one peer is connected and is
working normally. Temporary relay failures retry automatically.
