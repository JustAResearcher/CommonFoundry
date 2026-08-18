# CommonFoundry

CommonFoundry (CMFD) is a research-first cryptocurrency implementation built
around ForgeMatrix, a block-bound dense matrix proof of work.

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

`profile16gb` reports the disabled, seed-based v1 candidate; it is not the v2
profile. The v2 CLI/test paths are deliberately capped to tiny research shapes
so every vector can be recomputed quickly on an ordinary CPU.

## Private Devnet-0

`cmfd-node` is a private multi-node Devnet-0 runtime. RPC stays loopback-only;
P2P listeners and explicitly configured static peers are limited to loopback or
private addresses. From a Windows PowerShell prompt in the repository root:

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

This is not a public testnet. It has no peer discovery, peer identity
authentication, transport encryption, NAT traversal, wallet/key custody,
production miner protocol, or optimized miner. Extending a side branch
currently replays it from genesis, which is intentionally Devnet-only and does
not scale. See [docs/devnet-0.md](docs/devnet-0.md) for exact two- and
three-node commands, RPC examples, acceptance checks, and operating limits.

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
