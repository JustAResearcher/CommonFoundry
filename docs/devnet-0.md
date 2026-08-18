# Private Devnet-0

Devnet-0 is a private multi-node consensus, fork-choice, persistence, mempool,
and synchronization harness. It is not a public testnet and must not carry
value. RPC is restricted to loopback addresses. P2P listeners and explicitly
configured static peers may use loopback or private IP addresses.

The network uses the tiny ForgeMatrix v2 reference profile. Its proof is
compact on the wire, but every validator recomputes all four tiny-model layers.
This is not a succinct or production proof, and the production v2 profile
remains disabled.

## Consensus identity and data directories

Every peer handshake binds the full network ID and immutable consensus
fingerprint. That fingerprint includes the current emission schedule:

- the 500 CMFD launch subsidy decreases linearly for 2,628,000 blocks, which is
  nominally five 365-day years at the 60-second target;
- pre-tail blocks pay 70% to the miner, 25% as an immediately spendable
  steward award, and 5% as an immediately spendable community fund;
- scheduled pre-tail emission totals 657,000,249.98688 CMFD;
- the final declining reward is 0.00019025 CMFD at height 2,628,000;
- the permanent 5 CMFD miner-only tail begins at height 2,628,001, so the
  intentional boundary jumps from 0.00019025 CMFD to 5 CMFD; and
- every transaction and channel-close fee is burned.

See [consensus/emission.md](consensus/emission.md) for the exact integer formula
and rounding rules. The five-year duration is height-based; wall-clock duration
depends on achieved block cadence.

Data directories created with an earlier consensus fingerprint, including the
former emission schedule, are rejected by design. Do not delete or rewrite an
old directory to force it open. Keep it for inspection and give the current
binary a fresh empty path, such as `.\devnet-0-linear5y\node-a`. Each running
process also needs its own data directory.

A persistent `node.lock` file inside each directory is normal. The file records
the current PID for diagnostics, while a nonblocking OS-level exclusive lock on
its open handle is authoritative. Process exit, including hard termination,
releases that OS lock; an unlocked file is refreshed on the next open. Do not
delete `node.lock` merely because it remains on disk.

## Build and run one node

From Windows PowerShell in the repository root:

```powershell
Set-Location C:\Source\CommonFoundry
cargo build -p cmfd-node --locked

.\target\debug\cmfd-node.exe `
  --data-dir .\devnet-0-linear5y\node-a `
  run `
  --bind 127.0.0.1:18443 `
  --p2p-bind 127.0.0.1:18444
```

The defaults are data directory `commonfoundry-devnet0`, RPC
`127.0.0.1:18443`, and P2P `127.0.0.1:18444`. Explicit paths and ports are
recommended so that adding peers cannot accidentally reuse state or a socket.

The offline commands open, replay, and exclusively lock their selected data
directory:

```powershell
# Mine, validate, persist, and apply one block, then exit.
.\target\debug\cmfd-node.exe --data-dir .\devnet-0-linear5y\offline mine-once

# Replay the complete block log and print status, then exit.
.\target\debug\cmfd-node.exe --data-dir .\devnet-0-linear5y\offline status
```

`mine-once` uses a public, insecure development key unless a valid
64-character x-only Schnorr public key is supplied:

```powershell
.\target\debug\cmfd-node.exe `
  --data-dir .\devnet-0-linear5y\offline `
  mine-once `
  --miner <64-hex-x-only-public-key> `
  --attempts 1000000
```

Only one process may open a data directory. While `run` owns it, use RPC rather
than the offline `status` or `mine-once` commands.

## Run two or three local nodes

Build once, then run each command in a separate PowerShell window. The fully
connected static topology makes block and transaction propagation
bidirectional even though each node initiates its own pull sessions.

```powershell
# Terminal A
Set-Location C:\Source\CommonFoundry
.\target\debug\cmfd-node.exe --data-dir .\devnet-0-linear5y\node-a run `
  --bind 127.0.0.1:18443 --p2p-bind 127.0.0.1:18444 `
  --peer 127.0.0.1:18454 --peer 127.0.0.1:18464

# Terminal B
Set-Location C:\Source\CommonFoundry
.\target\debug\cmfd-node.exe --data-dir .\devnet-0-linear5y\node-b run `
  --bind 127.0.0.1:18453 --p2p-bind 127.0.0.1:18454 `
  --peer 127.0.0.1:18444 --peer 127.0.0.1:18464

# Terminal C
Set-Location C:\Source\CommonFoundry
.\target\debug\cmfd-node.exe --data-dir .\devnet-0-linear5y\node-c run `
  --bind 127.0.0.1:18463 --p2p-bind 127.0.0.1:18464 `
  --peer 127.0.0.1:18444 --peer 127.0.0.1:18454
```

Peers that are not running yet do not prevent startup; the static poller tries
again every two seconds. For a two-node network, omit Terminal C and its
`--peer 127.0.0.1:18464` option from A and B. `--peer` is repeatable, and the
listener rejects public addresses, duplicate peers, and its own address.

Each two-second peer poll performs the compatibility handshake, pulls at most
16 blocks first, then requests the remote mempool inventory and at most 64
unknown transaction bodies. Every returned body must match its advertised ID
and pass the node's normal consensus and mempool admission rules. Relay is
pull-only: there is no unsolicited push broadcast, so each destination must be
configured to poll a source directly or through another polling node.

## Loopback RPC

The first node above uses `http://127.0.0.1:18443`.

| Method | Endpoint | Result |
|---|---|---|
| `GET` | `/health` | Storage health and Devnet identity |
| `GET` | `/v1/status` | Fingerprint, active tip, cumulative work, target, UTXO count, and mempool totals |
| `GET` | `/v1/mempool` | Ordered transaction IDs, encoded sizes, burned fees, and pool totals |
| `GET` | `/v1/template?miner=<64-hex-x-only-public-key>` | JSON template containing the current mempool transactions and exact burned fees |
| `POST` | `/v1/transaction` | Admit one canonical raw transaction to the volatile mempool |
| `POST` | `/v1/mine?miner=<64-hex-x-only-public-key>&attempts=<1..1000000>` | Build, mine, validate, persist, and apply one block; body must be empty |
| `POST` | `/v1/block` | Validate, persist, and index one canonical raw block |

Canonical transaction and block submissions require
`Content-Type: application/octet-stream`. Their body limits are 64 KiB and
1 MiB respectively. Template JSON is descriptive; `/v1/block` accepts the
canonical binary block frame, not that JSON.

PowerShell examples while `run` is active:

```powershell
$base = 'http://127.0.0.1:18443'
$miner = '1d16453b3ab3132acb0a5bc16cc49690d819a585267a15cd5a064e2a0ad40599'

Invoke-RestMethod -Uri "$base/health"
Invoke-RestMethod -Uri "$base/v1/status"
Invoke-RestMethod -Uri "$base/v1/mempool"
Invoke-RestMethod -Uri "$base/v1/template?miner=$miner"

$mineUri = "$base/v1/mine?miner=$miner&attempts=1000000"
$mined = curl.exe --silent --show-error --request POST `
  --header 'Content-Length: 0' $mineUri | ConvertFrom-Json
$mined

Invoke-RestMethod -Method Post `
  -Uri "$base/v1/transaction" `
  -ContentType application/octet-stream `
  -InFile .\transaction.cmfd

Invoke-RestMethod -Method Post `
  -Uri "$base/v1/block" `
  -ContentType application/octet-stream `
  -InFile .\block.cmfd
```

The example miner destination is the source-visible, insecure Devnet key. It is
appropriate only for this valueless local network. The repository does not yet
provide wallet/key custody or a transaction-building CLI, so
`transaction.cmfd` must already be a correctly signed canonical transaction.

## Multi-node acceptance checks

With all three nodes running, mine on A, wait for at least one two-second poll
round, then compare their live status:

```powershell
$miner = '1d16453b3ab3132acb0a5bc16cc49690d819a585267a15cd5a064e2a0ad40599'
$mineUri = "http://127.0.0.1:18443/v1/mine?miner=$miner&attempts=1000000"
curl.exe --silent --show-error --request POST `
  --header 'Content-Length: 0' $mineUri | ConvertFrom-Json

Start-Sleep -Seconds 5
$rpcPorts = 18443, 18453, 18463
$statuses = foreach ($port in $rpcPorts) {
  Invoke-RestMethod -Uri "http://127.0.0.1:$port/v1/status"
}
$statuses | Format-Table accepted_height, tip, cumulative_work, mempool_transactions

if (@($statuses.consensus_fingerprint | Sort-Object -Unique).Count -ne 1) {
  throw 'consensus fingerprints differ'
}
if (@($statuses.accepted_height | Sort-Object -Unique).Count -ne 1 -or
    @($statuses.tip | Sort-Object -Unique).Count -ne 1 -or
    @($statuses.cumulative_work | Sort-Object -Unique).Count -ne 1) {
  throw 'nodes have not converged'
}
```

Mine the next block through B's RPC port (`18453`) to check the reverse path.
For restart/replay, stop C, mine additional blocks on A or B, restart C with the
same command and data directory, and confirm the three fields converge again.

If a signed `transaction.cmfd` is available, submit it to A with the RPC example
above. After a poll round, all three `/v1/mempool` responses should contain the
same transaction ID. Mine through B, wait for block convergence, and confirm
that the confirmed transaction is absent from every volatile pool.

For an isolated fork-choice exercise, start A and B without `--peer`, mine a
different branch into each directory, and give one branch strictly more
cumulative work. Restart both with their reciprocal peer options. Both must
retain a local tip on equal work and switch only when the competing branch has
strictly greater cumulative work.

## Mempool, fork choice, and persistence

The active-chain mempool is deliberately small and deterministic:

- at most 1,024 transactions and 512 KiB of canonical transaction bytes;
- transaction IDs order the block template;
- every input must already be confirmed on the active chain, so unconfirmed
  parents and package relay are unsupported;
- first admission wins an input conflict;
- the minimum relay fee is one atom per started KiB, and all admitted fees are
  burned when mined; and
- the pool is revalidated after an active-tip change and is not persisted
  across restart.

Static peers advertise mempool transaction IDs in deterministic order. A
polling node skips IDs it already has, downloads no more than 64 unknown bodies
per peer poll, verifies the ID/body binding, and runs each transaction through
the same admission path used by RPC. A malformed or missing response ends only
that peer session; later polling rounds and the inbound listener continue.

Every received block is fully consensus-validated before indexing. Valid side
branches are retained, and the active tip changes only for strictly greater
cumulative work; an equal-work branch does not replace the current tip. The
node stores the immutable consensus fingerprint in `network.meta` and accepted
blocks, including side branches, in an append-only checksummed `blocks.log`.
Startup verifies canonical encodings, checksums, and consensus rules before
reconstructing forks, cumulative work, and the active tip.

Active-chain block validation commits only a touched-state delta rather than
cloning the full UTXO set. Extending a side branch still reconstructs that
branch from genesis. This is simple and auditable for Devnet-0, but is not a
scalable public-network design.

## Current boundary

Devnet-0 has no public-peer discovery, peer identity authentication, transport
encryption, NAT traversal, reputation/ban system, demonstrated DDoS maturity,
wallet/key custody, production mining protocol, or optimized miner. Peer
compatibility checks do not establish who operates the remote process, and
peers never supply the local block-acceptance time. All network-facing use must
remain on an isolated, valueless private network.

The compact v2 proof is also still a full-recomputation reference path. A
production succinct proof, raw-model commitment ceremony/link, independent
implementations, benchmarks, adversarial public testnet, and audits remain
mandatory gates in [../SECURITY.md](../SECURITY.md).
