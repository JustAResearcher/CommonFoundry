# CommonFoundry

CommonFoundry (CMFD) is a research-first cryptocurrency implementation built
around ForgeMatrix, a block-bound dense matrix proof of work.

This repository currently contains the consensus oracle, a seedless v2 model
format, the exact small-profile v2 arithmetic relation, a test-size matrix
sumcheck transcript, Rust/CUDA arithmetic smoke vectors, monetary policy, signed
UTXO validation, chain-derived difficulty, an inference payment-channel state
machine, and adversarial tests. It deliberately does **not** contain an enabled
mainnet. At minimum, a production transparent PCS, complete succinct proof,
model-link certificate, canonical bounded wire format, soundness analysis,
benchmarks, independent implementations, adversarial testnet, and two audits
are required before v2 can be connected to block acceptance; the full gates are
in [SECURITY.md](SECURITY.md).

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
cargo run -p cmfd-consensus -- economics --height 14716800 --fees 100000
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

See [docs/consensus/forgematrix-v1.md](docs/consensus/forgematrix-v1.md), the
[recommended v2 research specification](docs/consensus/forgematrix-v2.md), and
[SECURITY.md](SECURITY.md) before modifying consensus code. The inference
payment flow and its current integration boundary are documented in
[docs/marketplace/payment-channels.md](docs/marketplace/payment-channels.md).

## Economics

- 60-second target blocks and a 180-header difficulty window;
- each pre-tail block splits emission 70% to the miner, 25% as an immediately
  spendable steward award, and 5% to an immediately spendable community fund;
- a permanent 5 CMFD miner-only tail subsidy after block 14,716,800; and
- all transaction and channel-close fees burned rather than paid to miners.

Inference revenue is not a block subsidy. A customer authorizes cumulative
payments from a prepaid channel directly to the GPU provider as output is
streamed in bounded chunks. `cmfd-marketplace` implements that signed protocol,
and the consensus UTXO rules enforce exact settlement or a time-locked refund.
