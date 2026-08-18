import { useCallback, useEffect, useRef, useState } from "react";
import {
  getMiningStatus,
  startMining as requestMiningStart,
  stopMining as requestMiningStop,
} from "../api/miningClient";
import type { MiningStartRequest, MiningStatus } from "../mining/types";

type MiningAction = "starting" | "stopping" | null;

interface MiningDataState {
  status: MiningStatus | null;
  loading: boolean;
  action: MiningAction;
  error: string | null;
}

const INITIAL_STATE: MiningDataState = {
  status: null,
  loading: true,
  action: null,
  error: null,
};

export function useMiningData(refreshInterval = 1_000) {
  const [state, setState] = useState<MiningDataState>(INITIAL_STATE);
  const requestRef = useRef<AbortController | null>(null);
  const inFlightRef = useRef<Promise<void> | null>(null);
  const actionRef = useRef(false);

  const refresh = useCallback((): Promise<void> => {
    if (inFlightRef.current) return inFlightRef.current;
    if (actionRef.current) return Promise.resolve();

    const controller = new AbortController();
    requestRef.current = controller;
    const request = (async () => {
      try {
        const status = await getMiningStatus(controller.signal);
        if (controller.signal.aborted) return;
        setState((current) => ({
          ...current,
          status,
          loading: false,
          error: null,
        }));
      } catch (cause) {
        if (controller.signal.aborted) return;
        setState((current) => ({
          ...current,
          loading: false,
          error: cause instanceof Error ? cause.message : "Unable to read miner status",
        }));
      } finally {
        if (requestRef.current === controller) {
          requestRef.current = null;
          inFlightRef.current = null;
        }
      }
    })();

    inFlightRef.current = request;
    return request;
  }, []);

  const start = useCallback(async (startRequest: MiningStartRequest) => {
    if (actionRef.current) return;
    actionRef.current = true;
    setState((current) => ({ ...current, action: "starting", error: null }));
    try {
      const status = await requestMiningStart(startRequest);
      setState((current) => ({ ...current, status, loading: false, error: null }));
    } catch (cause) {
      setState((current) => ({
        ...current,
        error: cause instanceof Error ? cause.message : "Unable to start mining",
      }));
    } finally {
      actionRef.current = false;
      setState((current) => ({ ...current, action: null }));
    }
  }, []);

  const stop = useCallback(async () => {
    if (actionRef.current) return;
    actionRef.current = true;
    setState((current) => ({ ...current, action: "stopping", error: null }));
    try {
      const status = await requestMiningStop();
      setState((current) => ({ ...current, status, loading: false, error: null }));
    } catch (cause) {
      setState((current) => ({
        ...current,
        error: cause instanceof Error ? cause.message : "Unable to stop mining",
      }));
    } finally {
      actionRef.current = false;
      setState((current) => ({ ...current, action: null }));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), refreshInterval);
    return () => window.clearInterval(timer);
  }, [refresh, refreshInterval]);

  useEffect(() => () => {
    requestRef.current?.abort();
    requestRef.current = null;
    inFlightRef.current = null;
  }, []);

  return { ...state, refresh, start, stop };
}
