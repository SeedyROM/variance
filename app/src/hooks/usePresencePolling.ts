import { useEffect } from "react";
import { useAppStore } from "../stores/appStore";
import { useIdentityStore } from "../stores/identityStore";
import { useMessagingStore } from "../stores/messagingStore";
import { presenceApi } from "../api/client";

const POLL_INTERVAL_MS = 15_000; // 15 seconds

/**
 * Periodically polls the /presence endpoint to reconcile online status.
 *
 * WS events (PresenceUpdated) are the primary mechanism and fire immediately
 * on connect/disconnect. This polling loop is a fallback that catches any
 * missed events (e.g. if the WS reconnected after a brief network blip).
 */
export function usePresencePolling() {
  const nodeStatus = useAppStore((s) => s.nodeStatus);
  const localDid = useIdentityStore((s) => s.did);
  const syncPresence = useMessagingStore((s) => s.syncPresence);

  useEffect(() => {
    if (nodeStatus !== "running") return;

    let cancelled = false;

    const poll = async () => {
      try {
        const { online } = await presenceApi.get();
        if (!cancelled) {
          // Always include the local DID so self-conversations stay online
          const dids = localDid ? [...online, localDid] : online;
          syncPresence(dids);
        }
      } catch {
        // Non-fatal — we'll retry on the next interval
      }
    };

    // Initial poll immediately on mount
    void poll();

    const id = setInterval(() => void poll(), POLL_INTERVAL_MS);

    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [nodeStatus, localDid, syncPresence]);
}
