COMMON FOUNDRY STANDALONE CUDA MINER

1. Double-click LIST-GPUS.bat and confirm every NVIDIA GPU appears.
2. Right-click START-MINER.bat and choose Edit.
3. The default settings automatically use every supported GPU.
4. Save the file and double-click START-MINER.bat.
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

The miner downloads blocks from PEER. For other nodes to receive blocks mined
by this rig, one of them must also list this miner's reachable P2P address.

Devnet CMFD is test currency with no monetary value.
