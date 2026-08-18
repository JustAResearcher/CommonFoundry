import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MiningStatus } from "../mining/types";
import { useMiningData } from "./useMiningData";

const apiMocks = vi.hoisted(() => ({
  getMiningStatus: vi.fn(),
  startMining: vi.fn(),
  stopMining: vi.fn(),
}));

vi.mock("../api/miningClient", () => apiMocks);

const stoppedStatus: MiningStatus = {
  lifecycle: "stopped",
  mode: null,
  payout: null,
  pool_url: null,
  worker_name: null,
  matrix_attempts_per_second: 0,
  session_attempts: 0,
  blocks_found: 0,
  shares_accepted: 0,
  shares_rejected: 0,
  credited_atoms: "0",
  pool_connected: false,
  current_height: 12,
  last_block: null,
  last_error: null,
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

describe("useMiningData", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    apiMocks.getMiningStatus.mockReset();
    apiMocks.startMining.mockReset();
    apiMocks.stopMining.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("joins scheduled refreshes while a miner status request is active", async () => {
    const pending = deferred<MiningStatus>();
    apiMocks.getMiningStatus.mockReturnValueOnce(pending.promise).mockResolvedValue(stoppedStatus);

    const { result } = renderHook(() => useMiningData(1_000));
    expect(apiMocks.getMiningStatus).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(4_000);
    });
    expect(apiMocks.getMiningStatus).toHaveBeenCalledTimes(1);

    await act(async () => {
      pending.resolve(stoppedStatus);
      await result.current.refresh();
    });
    expect(result.current.loading).toBe(false);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(apiMocks.getMiningStatus).toHaveBeenCalledTimes(2);
  });

  it("keeps start mutations single-flight", async () => {
    apiMocks.getMiningStatus.mockResolvedValue(stoppedStatus);
    const pending = deferred<MiningStatus>();
    apiMocks.startMining.mockReturnValue(pending.promise);
    const request = { mode: "solo" as const, payout: "11".repeat(32) };
    const { result } = renderHook(() => useMiningData(60_000));

    await act(async () => {
      await Promise.resolve();
    });

    let first!: Promise<void>;
    let second!: Promise<void>;
    await act(async () => {
      first = result.current.start(request);
      second = result.current.start(request);
      await second;
    });
    expect(apiMocks.startMining).toHaveBeenCalledTimes(1);

    await act(async () => {
      pending.resolve(stoppedStatus);
      await first;
    });
  });

  it("keeps stop mutations single-flight", async () => {
    apiMocks.getMiningStatus.mockResolvedValue(stoppedStatus);
    const pending = deferred<MiningStatus>();
    apiMocks.stopMining.mockReturnValue(pending.promise);
    const { result } = renderHook(() => useMiningData(60_000));

    await act(async () => {
      await Promise.resolve();
    });

    let first!: Promise<void>;
    let second!: Promise<void>;
    await act(async () => {
      first = result.current.stop();
      second = result.current.stop();
      await second;
    });
    expect(apiMocks.stopMining).toHaveBeenCalledTimes(1);

    await act(async () => {
      pending.resolve(stoppedStatus);
      await first;
    });
  });
});
