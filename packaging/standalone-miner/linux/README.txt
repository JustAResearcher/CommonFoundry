COMMON FOUNDRY STANDALONE CUDA MINER

1. Open start-miner.sh in a text editor.
2. Edit PEER and any other values at the top.
3. Leave GPU_INDEXES empty to use every supported NVIDIA GPU.
4. Run: chmod +x start-miner.sh && ./start-miner.sh
5. Leave the terminal open. Press Ctrl+C to stop cleanly.

To select specific cards, use a comma-separated list:
  GPU_INDEXES="0,1,2,3"

Supported native CUDA targets:
  Volta sm_70: V100, Titan V, Quadro GV100
  Turing sm_75: RTX 20 series
  Ampere sm_86: RTX 30 series
  Ada sm_89: RTX 40 series
  Blackwell sm_120: RTX 50 series

The miner keeps one CUDA context and one worker on each selected GPU. Work
ranges are separated so GPUs in the same rig do not repeat one another.

The miner downloads from PEER and submits locally accepted blocks back to it.
Temporary relay failures retry automatically; no reciprocal peer is required.
