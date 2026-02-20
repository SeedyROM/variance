import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useAppStore } from "../stores/appStore";
import { useIdentityStore } from "../stores/identityStore";
import { variantWs } from "../api/websocket";
import type { WsEvent } from "../api/types";
import type { DirectMessage } from "../api/types";

/**
 * Connect the WebSocket when the node is running and dispatch incoming events
 * to the React Query cache via invalidation.
 */
export function useWebSocket() {
  const nodeStatus = useAppStore((s) => s.nodeStatus);
  const localDid = useIdentityStore((s) => s.did);
  const queryClient = useQueryClient();

  useEffect(() => {
    if (nodeStatus !== "running") return;

    console.log("[WebSocket] Connecting...");
    void variantWs.connect();

    const off = variantWs.on((event: WsEvent) => {
      console.log("[WebSocket] Received event:", event.type, event);

      switch (event.type) {
        case "DirectMessageReceived": {
          console.log("[WebSocket] Processing DirectMessageReceived:", event.message_id);
          // Add message directly to cache
          const message: DirectMessage = {
            id: event.message_id,
            sender_did: event.from,
            recipient_did: localDid || "",
            text: event.text,
            timestamp: event.timestamp,
            reply_to: event.reply_to,
          };

          queryClient.setQueryData<DirectMessage[]>(["messages", event.from], (old = []) => {
            console.log("[WebSocket] Current messages for", event.from, ":", old?.length || 0);
            // Check if message already exists
            if (old.some((m) => m.id === message.id)) {
              console.log("[WebSocket] Message already exists, skipping");
              return old;
            }
            console.log("[WebSocket] Adding new message to cache");
            return [...old, message];
          });

          void queryClient.invalidateQueries({
            queryKey: ["conversations"],
          });
          break;
        }

        case "DirectMessageSent": {
          console.log("[WebSocket] Processing DirectMessageSent:", event.message_id);
          // Add sent message directly to cache
          const message: DirectMessage = {
            id: event.message_id,
            sender_did: localDid || "",
            recipient_did: event.recipient,
            text: event.text,
            timestamp: event.timestamp,
            reply_to: event.reply_to,
            status: "sent",
          };

          queryClient.setQueryData<DirectMessage[]>(["messages", event.recipient], (old = []) => {
            console.log("[WebSocket] Current messages for", event.recipient, ":", old?.length || 0);
            // Remove any optimistic version and add real message
            const withoutOptimistic = old.filter((m) => !m.id.startsWith("temp-"));
            // Check if message already exists
            if (withoutOptimistic.some((m) => m.id === message.id)) {
              console.log("[WebSocket] Message already exists, skipping");
              return old;
            }
            console.log("[WebSocket] Adding sent message to cache");
            return [...withoutOptimistic, message];
          });

          void queryClient.invalidateQueries({
            queryKey: ["conversations"],
          });
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
  }, [nodeStatus, queryClient]);
}
