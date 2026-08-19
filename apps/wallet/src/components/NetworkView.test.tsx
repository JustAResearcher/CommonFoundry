import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { NodeStatus } from "../types";
import { NetworkView } from "./NetworkView";

const status: NodeStatus = {
  network: "CommonFoundry Devnet-0",
  network_id: "11".repeat(32),
  consensus_fingerprint: "22".repeat(32),
  proof_of_work: "ForgeMatrix-v2 tiny full-recompute reference",
  tip: "33".repeat(32),
  cumulative_work: "00".repeat(64),
  accepted_height: 0,
  next_height: 1,
  expected_target: "00".repeat(32),
  utxo_count: 0,
  mempool_transactions: 0,
  mempool_bytes: 0,
  storage_healthy: true,
  public_peer_mode: true,
};

describe("NetworkView", () => {
  it("shows a prominent warning while public P2P is enabled", () => {
    render(
      <NetworkView
        status={status}
        wallet={null}
        mempool={null}
        refreshing={false}
        onRefresh={vi.fn()}
        onNotice={vi.fn()}
      />,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Public Devnet P2P is enabled",
    );
  });
});
