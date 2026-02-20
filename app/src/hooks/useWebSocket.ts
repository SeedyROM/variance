import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useAppStore } from "../stores/appStore";
import { useMessagingStore } from "../stores/messagingStore";
import { variantWs } from "../api/websocket";
import type { WsEvent } from "../api/types";

/**
 * Connect the WebSocket when the node is running and dispatch incoming events.
 *
 * For DirectMessageReceived we bump a Zustand tick instead of calling
 * invalidateQueries with event.from. The tick is watched by MessageView, which
 * calls its own refetch() — avoiding any dependency on whether event.from
 * exactly matches the React Query key used by the mounted component.
 */
export function useWebSocket() {
  const nodeStatus = useAppStore((s) => s.nodeStatus);
  const queryClient = useQueryClient();
  const tickInboundMessage = useMessagingStore((s) => s.tickInboundMessage);

  useEffect(() => {
    if (nodeStatus !== "running") return;

    console.log("[WebSocket] Connecting...");
    void variantWs.connect();

    const off = variantWs.on((event: WsEvent) => {
      console.log("[WebSocket] Received event:", event.type, event);

      switch (event.type) {
        case "DirectMessageReceived": {
          console.log("[WebSocket] Processing DirectMessageReceived:", event.message_id);
          // Bump the tick — MessageView will call refetch() in response.
          tickInboundMessage();
          // Update the conversation list (timestamp, ordering).
          void queryClient.invalidateQueries({ queryKey: ["conversations"] });
          break;
        }

        case "DirectMessageSent": {
          console.log("[WebSocket] Processing DirectMessageSent:", event.message_id);
          // onSettled in MessageInput already invalidates ["messages", peerDid].
          // We only need to refresh the conversation list here.
          void queryClient.invalidateQueries({ queryKey: ["conversations"] });
          break;
        }

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
  }, [nodeStatus, queryClient, tickInboundMessage]);
}
