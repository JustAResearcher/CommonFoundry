# Security policy

ForgeMatrix and the CMFD monetary rules are consensus-critical research code.
Do not deploy this repository as a public-value mainnet.

The recommended ForgeMatrix v2 relation and its unresolved boundaries are
documented in [docs/consensus/forgematrix-v2.md](docs/consensus/forgematrix-v2.md).
Its production profile is a design target, not an activation notice.

The repository now has canonical, bounded, network-aware wire decoding for
transactions, tagged v1/v2 proof envelopes, and blocks. Devnet-0 can accept the
tiny v2 compact claim through network-parameter-bound `Block`/`ChainState`
validation. That claim is not a succinct argument: the verifier recomputes
every layer of the tiny pinned model. The production profile remains disabled.

Mainnet remains disabled until all of the following are complete:

1. A frozen production ForgeMatrix specification, a canonical bounded parser
   for the eventual production proof and public inputs, and concrete total
   soundness of at least 128 bits after all union bounds and Fiat-Shamir
   grinding assumptions.
2. A transparent-PCS succinct proof whose verifier binds every layer, matrix
   root, activation transition, block field, nonce, difficulty target, and
   final activation digest, plus a verified link between the raw model bytes
   and proof commitment.
3. A reproducible, publicly reviewed model-generation ceremony, including its
   entropy sources, resulting artifact, and structural analyses.
4. Independent cryptographic and GPU-kernel implementations with matching test
   vectors.
5. Parser fuzzing, allocation/CPU caps, malformed-proof and resource-exhaustion
   tests on every network-facing verification path.
6. End-to-end resident and low-VRAM/streaming benchmarks, including the full
   winner proof, on named 16 GiB and representative lower-memory cards.
7. Audits by at least two teams that did not design the algorithm.
8. A public incentivized adversarial testnet covering output elision,
   partial-layer execution, sparse/zero inputs, transcript substitution,
   replay, proof malleability, target confusion, and CPU/GPU arithmetic
   divergence.

Devnet-0 now has bounded private-address static-peer sessions, full block
validation before indexing, cumulative-chainwork fork choice and reorgs, a
checksummed append-only block log with consensus replay, touched-state atomic
active-chain validation, and a bounded volatile mempool with pull-only static
peer propagation. Those features make it a private multi-node test harness;
they do not make it safe for public peers or valuable funds.

Before any untrusted public testnet, the networking and storage design needs
explicit abuse, latency, and crash-recovery bounds. Peers are statically
configured and compatibility-checked by network ID and consensus fingerprint,
but are not identity-authenticated; transport is unencrypted, with no peer
discovery, NAT traversal, reputation/ban system, or demonstrated DDoS
resilience. Extending a side branch currently reconstructs that branch from
genesis, which is deliberately Devnet-only and not scalable. The mempool is
volatile and intentionally excludes unconfirmed-parent packages. There is no
production wallet/key custody, production miner protocol, or optimized miner.

The local Devnet GUI and wallet RPC use one fixed, source-visible signing key
shared by every checkout. They can display active-chain balances and history,
sign sends, mine development blocks, and consolidate mature unreserved outputs,
but they provide no key generation, encrypted storage, backup, or recovery.
They are private Devnet test tools for valueless funds, not a production wallet.
Never use the shared destination for real value.

A completed, sound proof can establish the committed function, not physical GPU
use or VRAM residency. No such hardware claim may be used to weaken the
arithmetic proof or any gate above.

No public vulnerability-reporting endpoint is configured yet. Until one is,
report potential vulnerabilities to the project owner through an established
private channel and do not place working consensus bypasses or mainnet exploit
instructions in public issues. Private vulnerability reporting with a named
security contact must be configured before this repository is publicly hosted.
