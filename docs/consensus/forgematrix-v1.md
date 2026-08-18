# ForgeMatrix v1 consensus draft

Status: reference oracle implemented; production proving system not selected.

## Security objective

A valid proof must be computationally bound to:

- the complete committed matrix set;
- every sequential layer transition;
- the final activation bytes;
- the previous block hash and transaction Merkle root;
- the block height, timestamp, target, and nonce; and
- the ForgeMatrix algorithm and model versions.

Changing any bound value invalidates the proof. The production verifier must
never accept a plain claim, sampled output, pool share, or deferred certificate
in place of the complete consensus proof.

## Deterministic computation

The committed model contains `layers` row-major `dimension x dimension` dense
matrices of signed 8-bit values. Model bytes are generated during the model
ceremony and committed by a BLAKE3 root. Zero entries are remapped to one so
every weight participates when activations are nonzero.

For a block challenge, a nonzero signed 8-bit activation matrix `X_0` of shape
`batch x dimension` is derived from the canonical block fields. Each layer is:

```text
S_l = X_l * W_l                       (exact signed integer GEMM)
X_(l+1)[r,c] = Q(S_l[r,c] + mask_l[r,c])
```

`mask_l` is derived from the block challenge, layer, row, and column. `Q` is a
fully specified nonzero signed-8-bit reduction. The block-specific nonlinear
step prevents collapsing the fixed layer sequence into one precomputed linear
map.

The work digest is:

```text
BLAKE3("CMFD/FORGEMATRIX/WORK/V1" || challenge || model_root ||
       final_activation_digest)
```

and must be lexicographically less than or equal to the 256-bit big-endian
target.

## Candidate 16 GB profile

The unactivated candidate uses 384 matrices of `4096 x 4096` signed bytes:

- committed model bytes: 6 GiB;
- two `128 x 4096` activation buffers: 1 MiB total;
- exact work per nonce: 824,633,720,832 signed multiply-accumulates;
- arithmetic: int8 inputs, exact signed accumulation, consensus quantization;
- no floating point, TF32, stochastic rounding, or implementation-defined
  saturation in the consensus relation.

The profile is intentionally a candidate rather than a mainnet constant. GPU
benchmarks and proof-generation overhead must establish the final dimensions.

## What consensus cannot prove

Consensus proves the committed function, not physical hardware behavior. It
cannot prove that a vendor GPU was used or that all model bytes remained in
VRAM. Streaming committed weights from host memory is valid but expected to be
slower. Trusted hardware attestation is explicitly excluded.

## Known shortcut classes

| Shortcut | Required defense |
|---|---|
| Skip final output or post-processing | Final activation digest is a public input to the proof |
| Skip or replace a layer | Sequential layer claims and model root are proof inputs |
| Reuse work from another block | All mutable header fields and nonce enter the challenge |
| Use zero or sparse matrices | Fixed audited model root and deterministic nonzero encoding |
| Substitute an easier target | Chain state derives the target from the prior 180 headers and rejects a miner-selected value before proof verification |
| Accept a pool share as a block | Share verification API is separate and cannot reach block validation |
| CPU/GPU numeric disagreement | Exact integer arithmetic and cross-implementation vectors |
| Forge a model-residency claim | No residency claim exists in consensus |

## Production proof requirement

The reference verifier performs the full computation. This is the security
oracle but is too expensive for historical chain synchronization at production
dimensions. A mainnet implementation needs a transparent succinct argument,
most likely a sumcheck/GKR or STARK construction, with the reference evaluator
retained as the differential-test oracle.

No sampled-row or plain Freivalds proof is sufficient by itself. Fiat-Shamir
challenges, commitments, public inputs, field encoding, and soundness parameters
must receive independent cryptographic review.
