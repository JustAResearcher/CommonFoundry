# Security policy

ForgeMatrix and the CMFD monetary rules are consensus-critical research code.
Do not deploy this repository as a public-value mainnet.

The recommended ForgeMatrix v2 relation and its unresolved boundaries are
documented in [docs/consensus/forgematrix-v2.md](docs/consensus/forgematrix-v2.md).
That document is a design target, not an implementation or activation notice.

Mainnet remains disabled until all of the following are complete:

1. A frozen ForgeMatrix specification, canonical bounded proof parser, and
   concrete total soundness of at least 128 bits after all union bounds and
   Fiat-Shamir grinding assumptions.
2. A transparent-PCS succinct proof whose verifier binds every layer, matrix root, activation
   transition, block field, nonce, difficulty target, and final activation
   digest, plus a verified link between the raw model bytes and proof
   commitment.
3. Independent cryptographic and GPU-kernel implementations with matching test
   vectors.
4. Parser fuzzing, allocation/CPU caps, malformed-proof and resource-exhaustion
   tests on every network-facing verification path.
5. End-to-end resident and low-VRAM/streaming benchmarks, including the full
   winner proof, on named 16 GiB and representative lower-memory cards.
6. Audits by at least two teams that did not design the algorithm.
7. A public incentivized adversarial testnet covering output elision, partial-layer execution,
   sparse/zero inputs, transcript substitution, replay, proof malleability,
   target confusion, and CPU/GPU arithmetic divergence.

A completed, sound proof can establish the committed function, not physical GPU
use or VRAM residency. No such hardware claim may be used to weaken the
arithmetic proof or any gate above.

No public vulnerability-reporting endpoint is configured yet. Until one is,
report potential vulnerabilities to the project owner through an established
private channel and do not place working consensus bypasses or mainnet exploit
instructions in public issues. Private vulnerability reporting with a named
security contact must be configured before this repository is publicly hosted.
