import { describe, expect, it, vi } from "vitest";
import type { NativeInvoke } from "./nodeClient";
import { browserMiningTransport, createTauriMiningTransport } from "./miningClient";

describe("mining transport", () => {
  it("maps the desktop mining commands without retrying mutations", async () => {
    const invokeMock = vi.fn().mockResolvedValue({ lifecycle: "stopped" });
    const invoke: NativeInvoke = (command, args) => invokeMock(command, args);
    const transport = createTauriMiningTransport(invoke);
    const request = {
      mode: "pool" as const,
      payout: "11".repeat(32),
      pool_url: `cmfd+tls://127.0.0.1:443?pin=${"ab".repeat(32)}`,
      worker_name: "foundry.worker-1",
    };

    await transport.getMiningStatus();
    await transport.startMining(request);
    await transport.stopMining();

    expect(invokeMock.mock.calls).toEqual([
      ["get_mining_status", undefined],
      ["start_mining", { request }],
      ["stop_mining", undefined],
    ]);
  });

  it("does not pretend browser RPC supports continuous mining", async () => {
    await expect(browserMiningTransport.getMiningStatus()).rejects.toMatchObject({
      code: "desktop_mining_required",
      status: 501,
      retryable: false,
    });
  });
});
