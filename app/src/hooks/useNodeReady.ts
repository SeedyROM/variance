import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../stores/appStore";
import { resetApiBase } from "../api/client";
import type { NodeStatus } from "../api/types";

const POLL_INTERVAL_MS = 500;

/**
 * Poll `get_node_status` until the node is running.
 * Updates the app store and resets the API base URL when the node comes up.
 */
export function useNodeReady() {
  const setNodeStatus = useAppStore((s) => s.setNodeStatus);
  const setApiPort = useAppStore((s) => s.setApiPort);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    timerRef.current = setInterval(async () => {
      try {
        const status = await invoke<NodeStatus>("get_node_status");
        if (status.running && status.api_port) {
          setNodeStatus("running");
          setApiPort(status.api_port);
          resetApiBase();
          if (timerRef.current) clearInterval(timerRef.current);
        }
      } catch {
        // Node not available yet
      }
    }, POLL_INTERVAL_MS);

    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
  }, [setNodeStatus, setApiPort]);
}
