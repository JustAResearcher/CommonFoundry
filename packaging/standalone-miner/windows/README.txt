COMMON FOUNDRY STANDALONE CUDA MINER

1. Double-click LIST-GPUS.bat and confirm every NVIDIA GPU appears.
2. The defaults automatically find a wallet on this PC, or use the community
   bootstrap when the wallet is elsewhere.
3. Double-click START-MINER.bat. No peer setting needs to be changed.
4. To change GPUs or the payout address, right-click START-MINER.bat and Edit.
5. Leave the miner window open. Press Ctrl+C to stop cleanly.

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

The miner first tries the local wallet at 127.0.0.1:18444, then the community
bootstrap. A status such as "node sync 1/2" means one peer is connected and is
working normally. Temporary relay failures retry automatically.
