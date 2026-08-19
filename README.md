# CommonFoundry

CommonFoundry (CMFD) is a research-first cryptocurrency implementation built
around ForgeMatrix, a block-bound proof of work whose proposed production
kernel is dominated by signed INT8 x INT8 matrix multiplication with exact
INT32 accumulation.

The [technical white paper](docs/whitepaper.md) explains the motivation,
consensus architecture, monetary policy, ForgeMatrix v2 relation, proposed
succinct proof system, inference-market protocol, threat model, and activation
requirements in detail.

This repository contains the consensus oracle, a seedless v2 model format, the
exact small-profile v2 arithmetic relation, a test-size matrix sumcheck
transcript, Rust/CUDA arithmetic smoke vectors, monetary policy, signed UTXO
validation, chain-derived difficulty, an inference payment-channel state
machine, and adversarial tests. Canonical bounded wire encodings now cover
transactions, tagged v1/v2 proofs, and blocks. Block validation and `ChainState`
are bound to immutable network parameters.

The tagged v2 path accepted by Devnet-0 is deliberately tiny. Its proof is a
compact serialized claim, but every verifier recomputes every model layer. It
is **not** a succinct proof or a production verifier, and the production-sized
v2 profile remains disabled. A production transparent PCS, complete succinct
proof, model-link certificate, soundness analysis, benchmarks, independent
implementations, adversarial public testnet, and two audits are still required;
the full gates are in [SECURITY.md](SECURITY.md).

## What ForgeMatrix proves

ForgeMatrix v1 evaluates a fixed, committed sequence of dense integer matrices.
Every layer consumes the prior layer, applies a block-specific nonlinear mask,
and feeds the next layer. The final activation digest is bound to the previous
block, transaction Merkle root, height, timestamp, target, model root, and
nonce.

The reference verifier recomputes the complete function. It does not trust a
miner-supplied claim that a matrix, output, model, or GPU operation happened.

ForgeMatrix validates the output of the committed matrix relation. It cannot
prove the physical location of those bytes. A miner may keep the model in VRAM,
stream it from host memory, or implement equivalent arithmetic on different
hardware. Resident execution being faster is an unverified design hypothesis,
not a consensus claim.

There is no consensus VRAM minimum. Lower-memory cards can in principle tile or
stream and still mine valid blocks; their speed and complete winner-proof
memory requirements have not been measured. Mining compatibility is separate
from whether a paid inference job fits on the same card.

## Build and test

```powershell
cargo test --workspace
cargo run -p cmfd-consensus -- vector
cargo run -p cmfd-consensus -- economics --height 2628001 --fees 100000
cargo run -p cmfd-consensus -- profile16gb
```

To run the separate CUDA arithmetic smoke tests:

```powershell
.\scripts\test-cuda.ps1 -Version v1
.\scripts\test-cuda.ps1 -Version v2
```

The optional wallet miner backend is a different, batched CUDA path. Build its
20/30/40/50-series fat library and run the CPU/CUDA differential canary with:

```powershell
.\scripts\build-cuda-miner.ps1
```

See [ForgeMatrix v2 CUDA miner](docs/cuda-miner.md) for the exact trust
boundary, supported architectures, wallet packaging, and tester procedure.

`profile16gb` reports the disabled, seed-based v1 candidate; it is not the v2
profile. The v2 CLI/test paths are deliberately capped to tiny research shapes
so every vector can be recomputed quickly on an ordinary CPU.

## Devnet-0

`cmfd-node` is a bounded multi-node Devnet-0 runtime. RPC stays loopback-only.
P2P defaults to loopback or private addresses; public numeric addresses require
the explicit `--allow-public-peers` test-only flag. From a Windows PowerShell
prompt in the repository root:

```powershell
cargo run -p cmfd-node -- mine-once
cargo run -p cmfd-node -- status
cargo run -p cmfd-node -- run --p2p-bind 127.0.0.1:18444
```

The defaults are RPC `127.0.0.1:18443`, P2P `127.0.0.1:18444`, and data
directory `commonfoundry-devnet0`. The loopback RPC exposes health/status,
templates, a bounded volatile mempool, canonical transaction/block submission,
and development mining. Static peers exchange canonical blocks in bounded
batches, then pull unknown mempool transactions through the same admission
rules; transaction propagation is pull-only rather than a push broadcast. Each
block is fully validated before it is indexed, the active branch is selected by
strictly greater cumulative work, and the checksummed block log is replayed on
startup to reconstruct forks and the active tip.

For a small direct-IP test network, forward TCP `18444` on one router to the
node's private LAN address, start that hub with `--allow-public-peers`, and have
testers add `--allow-public-peers --peer PUBLIC_IP:18444`. Never forward RPC
port `18443`. Public P2P remains unauthenticated and unencrypted, with no peer
discovery, ban system, or demonstrated DDoS resistance.

### Local Devnet wallet

The native wallet runs its own embedded Devnet-0 node. Do not leave a separate
node on the default P2P port while starting it. From PowerShell:

```powershell
Set-Location C:\Source\CommonFoundry\apps\wallet
npm ci
npm run desktop:dev
```

The desktop application stores its chain data under the operating system's
application-data directory, opens P2P on `127.0.0.1:18444`, and communicates
with the React interface through a narrow Tauri command allowlist. It does not
open the node HTTP RPC port. A production-style Windows installer can be built
with `npm run desktop:build`; Linux builds use the same command and the
platform-specific Tauri bundle configuration.

The browser-only developer mode remains available by running a separate
`cmfd-node`, then `npm run dev`. Vite listens only on loopback and proxies
requests under `/rpc` to `http://127.0.0.1:18443`; the node RPC is not exposed
directly to the browser or public network.

The wallet reads real active-chain balances and history, displays the receive
destination, creates signed sends, runs cancellable continuous Solo or Pool
mining, and consolidates mining outputs through the embedded node. With the
optional CUDA library present, the wallet accelerates the INT8 matrix stage on
one NVIDIA GPU; otherwise it uses the CPU reference evaluator. Every CUDA
candidate is fully recomputed in Rust before submission. Mining reports
complete ForgeMatrix nonce attempts per second, not raw GPU TOPS. Mined rewards
require 100 confirmations before they are spendable. Consolidation selects mature, unreserved outputs in smallest-first
order, accepts 2 through 128 inputs, creates one wallet output, and burns the
chosen transaction fee.

The Mining page supports both continuous solo mining and the CMFD Devnet pool
v1 protocol. Pool mode uses TLS 1.3 with an exact SHA-256 pin of the server's
leaf certificate and accepts only numeric loopback or private-network
endpoints in this form:

```text
cmfd+tls://127.0.0.1:18445?pin=<64-hex-certificate-sha256>
```

This protocol is not Stratum compatible. The pool sends the immutable block
challenge and a separate easier share target. Workers submit only the issued
job ID and nonce; the server independently recomputes the committed
ForgeMatrix relation, compares its work digest with the share target, and
submits a block only when the same digest also meets the unchanged chain
target. Pool counters are bounded, volatile, session-only, valueless, and
nonwithdrawable. They are neither funds nor an on-chain payout ledger.

Generate a test certificate and start a local pool from the repository root:

```powershell
cargo build -p cmfd-node --locked
New-Item -ItemType Directory -Force .\devnet-0-linear5y\pool-tls | Out-Null

.\target\debug\cmfd-node.exe pool-certificate `
  --certificate .\devnet-0-linear5y\pool-tls\pool-cert.der `
  --private-key .\devnet-0-linear5y\pool-tls\pool-key.der

.\target\debug\cmfd-node.exe `
  --data-dir .\devnet-0-linear5y\pool `
  pool-serve `
  --bind 127.0.0.1:18445 `
  --p2p-bind 127.0.0.1:18454 `
  --certificate .\devnet-0-linear5y\pool-tls\pool-cert.der `
  --private-key .\devnet-0-linear5y\pool-tls\pool-key.der `
  --share-leading-zero-bits 7
```

Copy the `certificate_sha256` printed by either command into the wallet URL.
Publish only the certificate and its pin, never `pool-key.der`. The generator
creates the key with mode `0600` on Unix; on Windows, restrict the key and pool
data directories with operator-only ACLs. TLS authenticates the pinned server,
not worker or payout claims, so the volatile counters are not identity-secure.
The explicit `18454` P2P bind avoids the wallet's default `18444`; configure
reciprocal static peers if the wallet must follow blocks found by the pool.
See [docs/devnet-0.md](docs/devnet-0.md) for the full pool boundary and
[apps/wallet/src-tauri/README.md](apps/wallet/src-tauri/README.md) for desktop
static-peer command-line options.

Each new data directory creates a distinct unencrypted Schnorr test key in
`wallet.key`; the node never returns its private bytes through RPC or desktop
IPC. Stop the node before backing up that file and never share it. An existing
nonempty Devnet-2 directory retains the old public demonstration key during
migration so an upgrade cannot strand its test outputs. The interface and
funds remain private, valueless Devnet-0 test tools: there is no encryption,
mnemonic recovery, hardware-wallet support, or production custody, and the
wallet must never receive real value.

This is not a public testnet. It has no peer discovery, peer identity
authentication, encrypted P2P transport, NAT traversal, production wallet/key
custody, durable pool payout system, or optimized GPU miner. Extending a side
branch currently replays it from genesis, which is intentionally Devnet-only
and does not scale. See [docs/devnet-0.md](docs/devnet-0.md) for exact node and
pool commands, RPC examples, acceptance checks, and operating limits.

See [docs/consensus/forgematrix-v1.md](docs/consensus/forgematrix-v1.md), the
[recommended v2 research specification](docs/consensus/forgematrix-v2.md), and
[SECURITY.md](SECURITY.md) before modifying consensus code. The inference
payment flow and its current integration boundary are documented in
[docs/marketplace/payment-channels.md](docs/marketplace/payment-channels.md).

## Economics

- 60-second target blocks and a 180-header difficulty window;
- a 500 CMFD launch subsidy that decreases linearly over 2,628,000 blocks,
  exactly five 365-day years at the target rate;
- each pre-tail block splits emission 70% to the miner, 25% as an immediately
  spendable steward award, and 5% to an immediately spendable community fund;
- 657,000,249.98688 CMFD of scheduled pre-tail emission in total;
- a permanent 5 CMFD miner-only tail subsidy beginning at block 2,628,001; and
- all transaction and channel-close fees burned rather than paid to miners.

The declining component reaches zero at the five-year boundary and is then
replaced by the tail. Because the tail remains 5 CMFD, the final declining
reward (0.00019025 CMFD) is followed by the 5 CMFD tail reward; this boundary is
intentional and consensus-tested. The exact height and rounding rules are in
[docs/consensus/emission.md](docs/consensus/emission.md).

Inference revenue is not a block subsidy. A customer authorizes cumulative
payments from a prepaid channel directly to the GPU provider as output is
streamed in bounded chunks. `cmfd-marketplace` implements that signed protocol,
and the consensus UTXO rules enforce exact settlement or a time-locked refund.
