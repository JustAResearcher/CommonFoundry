import type {
  ConsolidationRequest,
  ConsolidationResult,
  MempoolSnapshot,
  MineResult,
  NodeStatus,
  WalletSendRequest,
  WalletSendResult,
  WalletSnapshot,
} from "../types";

const RPC_ROOT = "/rpc";

export class NodeApiError extends Error {
  readonly status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = "NodeApiError";
    this.status = status;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${RPC_ROOT}${path}`, {
    ...init,
    headers: {
      Accept: "application/json",
      ...init?.headers,
    },
  });

  const body = (await response.json().catch(() => null)) as
    | T
    | { error?: string }
    | null;
  if (!response.ok) {
    const message = body
      && typeof body === "object"
      && "error" in body
      && typeof body.error === "string"
      ? body.error
      : response.statusText;
    throw new NodeApiError(message || "Node request failed", response.status);
  }
  return body as T;
}

export function getNodeStatus(signal?: AbortSignal): Promise<NodeStatus> {
  return request<NodeStatus>("/v1/status", { signal });
}

export function getWalletSnapshot(signal?: AbortSignal): Promise<WalletSnapshot> {
  return request<WalletSnapshot>("/v1/wallet", { signal });
}

export function getMempool(signal?: AbortSignal): Promise<MempoolSnapshot> {
  return request<MempoolSnapshot>("/v1/mempool", { signal });
}

export function sendWalletTransaction(payload: WalletSendRequest): Promise<WalletSendResult> {
  return request<WalletSendResult>("/v1/wallet/send", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
}

export function consolidateWallet(payload: ConsolidationRequest): Promise<ConsolidationResult> {
  return request<ConsolidationResult>("/v1/wallet/consolidate", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
}

export function mineDevnetBlock(
  miner: string,
  attempts = 1_000_000,
): Promise<MineResult> {
  const query = new URLSearchParams({ miner, attempts: attempts.toString() });
  return request<MineResult>(`/v1/mine?${query.toString()}`, {
    method: "POST",
    headers: { "Content-Type": "application/octet-stream" },
    body: new Uint8Array(),
  });
}
