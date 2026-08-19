# Common Foundry standalone CUDA miner

`cmfd-miner` is a separate command-line miner for dedicated NVIDIA mining
rigs. It embeds a Devnet node, maintains its own data directory, and runs one
independent CUDA worker on every selected GPU.

## Supported GPUs

The packaged CUDA 12.9 fat library contains these native images:

| Architecture | CUDA target | Representative cards |
| --- | --- | --- |
| Volta | `sm_70` | Tesla V100, Titan V, Quadro GV100 |
| Turing | `sm_75` | GeForce RTX 20 series |
| Ampere | `sm_86` | GeForce RTX 30 series |
| Ada | `sm_89` | GeForce RTX 40 series |
| Blackwell | `sm_120` | GeForce RTX 50 series |

A `compute_70` PTX image is included for forward compatibility. CUDA Toolkit
12.9 is intentional: it can compile both Volta and Blackwell, whereas CUDA 13
removed offline compilation support for Volta. The miner package uses the
static CUDA runtime, so testers need a compatible NVIDIA driver but do not need
to install the CUDA Toolkit.

## Multi-GPU behavior

- With no `--device` options, every detected GPU with compute capability 7.0
  or newer is selected.
- Repeat `--device` to choose a subset, for example `--device 0 --device 2`.
- Every GPU owns a separate CUDA context, model allocation, and worker thread.
- GPU `i` begins at batch `i`; later batches advance by `GPU count × batch
  size`. This keeps nonce ranges disjoint across the rig.
- The displayed rig rate is the sum of complete ForgeMatrix nonce evaluations
  from every worker.
- CUDA is a candidate generator. Rust independently recomputes a below-target
  candidate before it can be submitted as a block.

## Windows quick start

The Windows ZIP includes:

- `cmfd-miner.exe`
- `cmfd-forgematrix-v2-miner.dll`
- `LIST-GPUS.bat`
- `START-MINER.bat`

Run `LIST-GPUS.bat`, then launch `START-MINER.bat`. Its defaults try a GUI
wallet on the same computer first and the community bootstrap second, so the
peer settings normally need no editing. Leave `GPU_INDEXES` blank to use every
supported card automatically.

## Direct commands

List devices:

```text
cmfd-miner devices
```

Mine with every supported GPU and the community bootstrap:

```text
cmfd-miner mine --peer 127.0.0.1:18444 --peer 107.214.187.2:18444 --allow-public-peers
```

Select GPUs 0, 1, and 3:

```text
cmfd-miner mine --device 0 --device 1 --device 3
```

The standalone miner currently performs continuous solo mining. Its embedded
node owns the selected data directory, so do not open the GUI wallet against
that same directory while the miner is running. The existing wallet remains
the Pool-mode client for this Devnet iteration.

The miner synchronizes in both directions over each configured peer session. It
downloads newer blocks, immediately submits every locally accepted block to the
configured node, and retries missing locally active blocks on the regular peer
poll. A temporary disconnect therefore shows `node sync pending` rather than a
false remote acceptance; relay catches up automatically after reconnection.
The packaged launchers configure two peers. `node sync 1/2` means one of them is
reachable and block relay is working; `node sync pending 0/2` means neither is
currently reachable.

`--miner` (or `PAYOUT_ADDRESS` in the packaged launch file) selects the coinbase
payout destination. It does not identify or locate a node. `--peer` selects the
node that receives blocks. One outbound miner-to-node peer configuration is
enough; the wallet/node does not also need to list the miner as a peer.
