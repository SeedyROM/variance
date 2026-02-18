import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useAppStore } from "../stores/appStore";
import { variantWs } from "../api/websocket";
import type { WsEvent } from "../api/types";

/**
 * Connect the WebSocket when the node is running and dispatch incoming events
 * to the React Query cache via invalidation.
 */
export function useWebSocket() {
  const nodeStatus = useAppStore((s) => s.nodeStatus);
  const queryClient = useQueryClient();

  useEffect(() => {
    if (nodeStatus !== "running") return;

    void variantWs.connect();

    const off = variantWs.on((event: WsEvent) => {
      switch (event.type) {
        case "DirectMessageReceived":
          void queryClient.invalidateQueries({
            queryKey: ["messages", event.from],
          });
          void queryClient.invalidateQueries({ queryKey: ["conversations"] });
          break;

        case "GroupMessageReceived":
          void queryClient.invalidateQueries({
            queryKey: ["messages", "group", event.group_id],
          });
          break;

        case "ReceiptDelivered":
        case "ReceiptRead":
          void queryClient.invalidateQueries({ queryKey: ["receipts"] });
          break;

        default:
          break;
      }
    });

    return () => {
      off();
      variantWs.disconnect();
    };
  }, [nodeStatus, queryClient]);
}
