# Desktop node peer configuration

Common Foundry Wallet normally starts by double-clicking its shortcut. With no
arguments, the embedded Devnet-0 node listens on `127.0.0.1:18444` and does not
make outbound peer connections.

Operators can start the packaged executable from a terminal and add one or more
static private peers. `--peer` is repeatable. `--p2p-bind` changes the address
used for inbound P2P connections; it is optional and defaults to
`127.0.0.1:18444`. Close any running wallet first: a second launch only focuses
the existing single-instance window and cannot change that process's peer list.

Windows PowerShell (replace the executable path with the installed location):

```powershell
& 'C:\path\to\common-foundry-wallet.exe' `
  --p2p-bind 192.168.50.10:18444 `
  --peer 192.168.50.11:18444 `
  --peer 192.168.50.12:18444
```

Linux AppImage:

```bash
./Common-Foundry-Wallet.AppImage \
  --p2p-bind 192.168.50.10:18444 \
  --peer 192.168.50.11:18444 \
  --peer 192.168.50.12:18444
```

Every value must be a numeric `IP:port`. IPv6 addresses use bracket notation,
for example `--peer [fd12:3456::12]:18444`. Only loopback, RFC1918 IPv4,
IPv6 unique-local, and IPv6 link-local addresses with nonzero ports are
accepted. Public addresses, unspecified addresses such as `0.0.0.0`, duplicate
peers, and the wallet's own listener address are rejected before the embedded
node starts.

The Devnet-0 peer transport is neither authenticated nor encrypted. Keep these
connections on a trusted private network; do not expose the listener to the
public Internet.

## Pool configuration

Pool mining is configured inside **Mining -> Pool**, not with a wallet process
argument. The URL format is exact:

```text
cmfd+tls://127.0.0.1:18445?pin=<64-hex-certificate-sha256>
```

The host must be a numeric loopback, RFC1918 IPv4, or IPv6 unique-local
address; use brackets around IPv6. DNS names, public addresses, paths, extra
query parameters, zero ports, and malformed pins are rejected. Worker names
must match `[A-Za-z0-9._-]{1,32}`.

This is the CMFD Devnet pool v1 protocol over TLS 1.3, not Stratum. The pin is
the SHA-256 digest of the exact DER leaf certificate printed by
`cmfd-node pool-certificate` and `cmfd-node pool-serve`; verify it through a
trusted path before connecting. Miners need only that public pin; they must
never receive the pool's private-key DER. TLS authenticates the pinned server,
not the worker or payout claim, so the session counters are not identity-secure.
Pool share, block, and credited-atom values in the wallet are bounded,
volatile, session-only, valueless, nonwithdrawable test counters. They are not
funds or an on-chain payout balance. See
[../../../docs/devnet-0.md](../../../docs/devnet-0.md) for pool operator
commands and the complete security boundary.
