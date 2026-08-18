import { Blocks, Check, Clipboard, Hammer, RefreshCw, ServerCog, ShieldAlert } from "lucide-react";
import { useState } from "react";
import { mineDevnetBlock, usesEmbeddedNode } from "../api/nodeClient";
import { formatBytes, shortenHash } from "../lib/amount";
import type { MempoolSnapshot, NodeStatus, WalletSnapshot } from "../types";

interface NetworkViewProps {
  status: NodeStatus | null;
  wallet: WalletSnapshot | null;
  mempool: MempoolSnapshot | null;
  refreshing: boolean;
  onRefresh: () => Promise<void> | void;
  onNotice: (message: string) => void;
}

export function NetworkView({ status, wallet, mempool, refreshing, onRefresh, onNotice }: NetworkViewProps) {
  const [mining, setMining] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const copyDiagnostics = async () => {
    if (!status) return;
    await navigator.clipboard.writeText(JSON.stringify(status, null, 2));
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1_600);
  };

  const mine = async () => {
    if (!wallet) return;
    setMining(true);
    setError(null);
    try {
      const result = await mineDevnetBlock(wallet.destination);
      onNotice(`Block ${result.height} forged and accepted.`);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Mining attempt failed");
      setMining(false);
      return;
    }
    try {
      await onRefresh();
    } catch {
      // The accepted block remains successful even if the best-effort refresh fails.
    } finally {
      setMining(false);
    }
  };

  return (
    <div className="network-layout">
      <section className="node-details-card">
        <div className="section-heading">
          <div>
            <span>Local service</span>
            <h2>Node diagnostics</h2>
          </div>
          <div className="heading-actions">
            <button className="button-quiet" type="button" onClick={copyDiagnostics} disabled={!status}>
              {copied ? <Check aria-hidden="true" size={16} /> : <Clipboard aria-hidden="true" size={16} />}
              {copied ? "Copied" : "Copy diagnostics"}
            </button>
            <button className="icon-button bordered" type="button" onClick={() => void onRefresh()} aria-label="Refresh diagnostics">
              <RefreshCw aria-hidden="true" size={16} className={refreshing ? "spin" : ""} />
            </button>
          </div>
        </div>

        <div className="service-banner">
          <span className="service-icon"><ServerCog aria-hidden="true" size={21} /></span>
          <div>
            <strong>{status ? "Node connected" : "Node unavailable"}</strong>
            <span>{status
              ? (usesEmbeddedNode ? "Embedded Rust node is responding" : "Loopback RPC is responding")
              : (usesEmbeddedNode ? "Embedded node failed to start; retry or reopen the wallet" : "Start the local Devnet node, then refresh")}</span>
          </div>
          <span className={`service-state${status ? " is-online" : ""}`}>{status ? "Online" : "Offline"}</span>
        </div>

        <dl className="diagnostics-grid">
          <Diagnostic label="Network" value={status?.network ?? "—"} />
          <Diagnostic label="Proof of work" value={status?.proof_of_work ?? "—"} />
          <Diagnostic label="Block height" value={status?.accepted_height.toLocaleString() ?? "—"} />
          <Diagnostic label="Next height" value={status?.next_height.toLocaleString() ?? "—"} />
          <Diagnostic label="Chain tip" value={status ? shortenHash(status.tip, 10, 10) : "—"} mono title={status?.tip} />
          <Diagnostic label="Cumulative work" value={status ? shortenHash(status.cumulative_work, 10, 10) : "—"} mono title={status?.cumulative_work} />
          <Diagnostic label="Expected target" value={status ? shortenHash(status.expected_target, 10, 10) : "—"} mono title={status?.expected_target} />
          <Diagnostic label="UTXO count" value={status?.utxo_count.toLocaleString() ?? "—"} />
          <Diagnostic label="Mempool" value={mempool ? `${mempool.transactions} tx · ${formatBytes(mempool.bytes)}` : "—"} />
          <Diagnostic label="Storage" value={status ? (status.storage_healthy ? "Healthy" : "Faulted") : "—"} good={status?.storage_healthy} />
          <Diagnostic label="Network ID" value={status ? shortenHash(status.network_id, 10, 10) : "—"} mono title={status?.network_id} />
          <Diagnostic label="Consensus fingerprint" value={status ? shortenHash(status.consensus_fingerprint, 10, 10) : "—"} mono title={status?.consensus_fingerprint} />
        </dl>
      </section>

      <aside className="forge-card">
        <span className="forge-icon"><Hammer aria-hidden="true" size={23} /></span>
        <span className="card-eyebrow">Development mining</span>
        <h2>Forge a Devnet block</h2>
        <p>Run one bounded mining attempt with the tiny ForgeMatrix-v2 reference profile.</p>
        <div className="miner-destination">
          <span>Reward destination</span>
          <code title={wallet?.destination}>{wallet ? shortenHash(wallet.destination, 9, 9) : "Waiting for wallet"}</code>
        </div>
        <div className="warning-inline">
          <ShieldAlert aria-hidden="true" size={17} />
          <span>This unencrypted test key is only for valueless private Devnet testing.</span>
        </div>
        {error ? <p className="form-error" role="alert">{error}</p> : null}
        <button className="button-primary wide" type="button" disabled={!wallet || mining} onClick={() => void mine()}>
          {mining ? <RefreshCw className="spin" aria-hidden="true" size={18} /> : <Blocks aria-hidden="true" size={18} />}
          {mining ? "Forging block…" : "Forge one block"}
        </button>
        <small>Fees in included transactions are burned, not paid to the miner.</small>
      </aside>
    </div>
  );
}

function Diagnostic({ label, value, mono = false, good = false, title }: { label: string; value: string; mono?: boolean; good?: boolean; title?: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd className={`${mono ? "mono" : ""}${good ? " healthy" : ""}`} title={title}>{value}</dd>
    </div>
  );
}
