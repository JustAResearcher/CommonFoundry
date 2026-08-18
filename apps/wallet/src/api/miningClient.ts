import { invoke } from "@tauri-apps/api/core";
import type { MiningStartRequest, MiningStatus } from "../mining/types";
import { NodeApiError, usesEmbeddedNode, type NativeInvoke } from "./nodeClient";

interface NativeErrorEnvelope {
  code?: unknown;
  message?: unknown;
  status?: unknown;
  retryable?: unknown;
}

export interface MiningTransport {
  getMiningStatus(signal?: AbortSignal): Promise<MiningStatus>;
  startMining(request: MiningStartRequest): Promise<MiningStatus>;
  stopMining(): Promise<MiningStatus>;
}

function normalizeNativeError(cause: unknown): NodeApiError {
  if (cause instanceof NodeApiError) return cause;
  if (cause instanceof Error) {
    return new NodeApiError(cause.message || "Native mining request failed", 0);
  }
  if (cause && typeof cause === "object") {
    const envelope = cause as NativeErrorEnvelope;
    return new NodeApiError(
      typeof envelope.message === "string" ? envelope.message : "Native mining request failed",
      typeof envelope.status === "number" ? envelope.status : 0,
      typeof envelope.code === "string" ? envelope.code : "mining_request_failed",
      envelope.retryable === true,
    );
  }
  return new NodeApiError(
    typeof cause === "string" && cause ? cause : "Native mining request failed",
    0,
  );
}

async function nativeCall<T>(
  nativeInvoke: NativeInvoke,
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await nativeInvoke<T>(command, args);
  } catch (cause) {
    throw normalizeNativeError(cause);
  }
}

function abortError(): DOMException {
  return new DOMException("The mining status request was aborted", "AbortError");
}

function withAbort<T>(operation: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return operation;
  if (signal.aborted) return Promise.reject(abortError());

  return new Promise<T>((resolve, reject) => {
    let settled = false;
    const abort = () => {
      if (settled) return;
      settled = true;
      reject(abortError());
    };
    signal.addEventListener("abort", abort, { once: true });
    operation.then(
      (value) => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", abort);
        resolve(value);
      },
      (cause) => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", abort);
        reject(normalizeNativeError(cause));
      },
    );
  });
}

function desktopRequired(): Promise<never> {
  return Promise.reject(new NodeApiError(
    "Continuous mining requires the installed Common Foundry desktop wallet",
    501,
    "desktop_mining_required",
    false,
  ));
}

export const browserMiningTransport: MiningTransport = {
  getMiningStatus: () => desktopRequired(),
  startMining: () => desktopRequired(),
  stopMining: () => desktopRequired(),
};

export function createTauriMiningTransport(nativeInvoke: NativeInvoke): MiningTransport {
  return {
    getMiningStatus: (signal) => withAbort(
      nativeCall<MiningStatus>(nativeInvoke, "get_mining_status"),
      signal,
    ),
    startMining: (startRequest) => nativeCall<MiningStatus>(
      nativeInvoke,
      "start_mining",
      { request: startRequest },
    ),
    stopMining: () => nativeCall<MiningStatus>(nativeInvoke, "stop_mining"),
  };
}

const nativeInvoke: NativeInvoke = (command, args) => invoke(command, args);
const transport = usesEmbeddedNode
  ? createTauriMiningTransport(nativeInvoke)
  : browserMiningTransport;

export function getMiningStatus(signal?: AbortSignal): Promise<MiningStatus> {
  return transport.getMiningStatus(signal);
}

export function startMining(startRequest: MiningStartRequest): Promise<MiningStatus> {
  return transport.startMining(startRequest);
}

export function stopMining(): Promise<MiningStatus> {
  return transport.stopMining();
}
